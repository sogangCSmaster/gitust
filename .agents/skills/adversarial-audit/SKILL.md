---
name: adversarial-audit
description: >-
  Mandatory pre-commit adversarial code audit and bug-hunting runbook for Gitust.
  Use this skill whenever finishing a coding step, reviewing uncommitted changes
  or commit history, or running the AGENTS.md Section 7.5 Review Gate (both for
  self-review and when spawning or executing a review subagent).
---

# Adversarial Code Audit & Bug-Hunting Skill (`adversarial-audit`)

This skill enforces a ruthless, non-rubber-stamping verification standard before any code is committed or pushed to `main`. Every rule below is derived from real P0/P1 bugs caught in Gitust's commit history (`926c024`, `4953544` → `cb38d94`, `526d940`) and Gitust's 4-Pillar architecture (`AGENTS.md`).

---

## 1. Anti-Rubber-Stamping Protocol & 4-Axis Audit Model

1. **Guilty Until Proven Innocent**: Never conclude "0 P0, 0 P1" just because `cargo test` and `cargo clippy` pass. Happy-path unit tests rarely catch cancellation leaks, parser differentials, missing-file security bypasses, or $O(N)$ lock-contention regressions.
2. **Mandatory 4-Axis Audit Model (Security + Correctness + Blazing Speed)**:
   Do not merely check off known past bugs. For every touched function, handler, or state transition, explicitly interrogate four orthogonal axes:
   - **Axis A — Blazing Speed & Hot-Path Latency (`AGENTS.md` §3.1 & §6)**:
     - *Did a security or stability fix accidentally introduce an $O(N)$ full-map scan on every write under a lock, unconditional `String` allocations on the fast path, N+1 lookups, or blocking I/O on Tokio worker threads?*
     - *Can fast-path checks use `Cow::Borrowed`, O(1) index lookups, or amortized pruning instead of per-call full scans?*
   - **Axis B — Lifecycle & Concurrency**:
     - *What if the caller's Future is cancelled (HTTP disconnect, timeout, `SIGINT`) while `spawn_blocking` or an `.await` is in flight?*
     - *What if Request B (`approve`, `discard`, `deactivate`) runs while Request A (`exec`, `spawn`, `sync`) is suspended at `.await`?*
     - *What if the process crashes (`SIGKILL`) right between a disk mutation and an in-memory/DB update, and restarts?*
   - **Axis C — Input, Path & Parser Boundaries**:
     - *What if a path does not exist yet (`ENOENT`), is already deleted, or traverses a symlinked root (`/var` vs `/private/var`)?*
     - *What if a user/agent supplies a ref or path starting with `-` (Git CLI option injection), backslash line continuations (`\\\n`), tabs (`\t`), quotes, or non-ASCII/control characters?*
   - **Axis D — Resource Exhaustion & Fail-Closed Cap Behavior**:
     - *When a bounded map/queue hits its cap (`MAX_RATE_LIMIT_TRACKED_KEYS`, `MAX_TRACKED_WORKTREES`), does it fail-closed (evict/reject) or accidentally fail-open (`return;` bypassing security)?*
     - *Can an unauthenticated caller or noisy neighbor lock out another user's account, exhaust quota slots via cancelled requests, or trigger unbounded memory/disk growth?*
3. **Concrete Trace, Independent Oracle & Anti-Tautological Testing**:
   - When auditing sandbox profiles, git commands, or parsers, run a real probe (in a temporary directory under `mktemp -d`) whenever feasible.
   - **Anti-Tautological Test Rule**: Never verify an implementation (`apply_inbound_mirror_refs`) using a test helper (`list_non_agent_refs`) that applies the exact same lossy transformation (e.g., calling `r.peel_to_id_in_place()` on both upstream and mirror, which hides the bug where annotated `Tag` objects were peeled into lightweight `Commit` refs!). Always assert against an independent oracle (`git cat-file -t`, `git rev-parse`, `git status`) and include non-trivial fixtures (annotated tags `git tag -a`, filenames with spaces/quotes/globs/non-ASCII, lines starting with `--` / `++`).
4. **Separate Verified vs. Suspected**: Explicitly label each finding as `[Verified]` (reproduced via test/probe or deterministic execution trace) or `[Suspected]`, with exact `file:line` references and ready-to-apply diff fixes.

---

## 2. The 14-Point Adversarial Bug Checklist

### Check 1: Filesystem & `fs::canonicalize` Traps
- **Missing-Leaf Security Bypass**: When building sandbox/access rules (e.g., `PathAccess::Hidden` for `.git/refs/stash`, `COMMIT_EDITMSG`, `logs`, `index`, `FETCH_HEAD`), never call `fs::canonicalize(&rule.path)` and silently skip (`let Ok(p) = fs::canonicalize(...) else { continue; }`) paths that do not exist at startup.
  - *Required pattern*: Canonicalize the deepest existing non-root ancestor directory and append the remaining lexical components so `deny` rules are always emitted even for not-yet-created files.
- **Idempotent Cleanup on Symlinked Roots (`/var` vs `/private/var`)**: When removing a managed directory (`remove_agent_worktree`), if `worktree_dir` was already deleted on disk while `base` still exists, `fs::canonicalize(worktree_dir)` fails with `ENOENT` while `fs::canonicalize(base)` returns `/private/var/...`.
  - *Required pattern*: Canonicalize the deepest existing ancestor of both paths before comparing `target.parent() != Some(base)`.

### Check 2: Async Cancellation Safety & RAII Guard Ordering
- **Guard Created After `.await`**: Never construct a cleanup guard (`SandboxCleanupGuard`, `SpawnReservation`, `CliWorktreeGuard`) *after* `tokio::task::spawn_blocking(...).await` without cancellation rollback.
  - *Why it's a bug*: Once `spawn_blocking` starts, Tokio cannot abort the OS thread. If the outer Future is cancelled while `spawn_blocking` is in flight (or in the scheduling gap before the outer Future resumes), the blocking closure still creates the directory/worktree on disk and leaks it forever.
  - *Required pattern*: Coordinate via a shared cancellation state (`Arc<AtomicBool>` or `Arc<Mutex<...>>`) between the RAII guard's `Drop` and the `spawn_blocking` closure so that if the Future is dropped before commit, the blocking task (or a cleanup task) rolls back the on-disk worktree/directory and releases quota.
- **No Blocking Subprocesses on Async Worker Threads**: Use `tokio::task::spawn_blocking` for normal and `SIGINT` cleanup paths, keeping synchronous `Drop` cleanup only as a last-resort fallback.

### Check 3: Post-`.await` / Post-Unlock State TOCTOU & Lock Coordination
- **Stale State Overwrite Across `.await`**: Mutually exclusive operations on the same entity (`exec`, `approve`, `discard`) must coordinate via per-entity `exec_lock` (`try_lock_owned`), and the lock guard must be held until **after** both the on-disk operation and the in-memory/DB state removal/transition complete.
- **Re-Authenticate After Slow-Loris Request Body `.await`**: When a handler authenticates headers before awaiting an attacker-controlled request body stream (`axum::body::to_bytes(...).await` or `FromRequestParts` + `Json<T>`), a client can hold the body stream open while its account is deactivated or token is revoked, then release the body bytes to execute a mutation. Always re-authenticate (`extract_principal` or `auth_store.verify_active_principal`) **after** the request body `.await` completes before dispatching mutations.
- **Password-Hash CAS Across Argon2 Unlock**: When `login` drops the state lock to run CPU-intensive Argon2 verification against `phc_to_verify`, re-acquiring the lock to mint a session MUST re-verify `user.password_hash == phc_to_verify` (and `user.status == Active`) so a concurrent password reset cannot be bypassed by an in-flight login using the old password.
- **Terminal State Monotonicity**: State transitions after `.await` must re-verify that the entity has not transitioned to a terminal state (`Merged`, `Discarded`) while locks were dropped.
- **No Blocking I/O Inside Locks**: Never hold a `RwLock`/`Mutex` across `git` subprocesses, disk I/O, or Argon2 hashing (e.g., run network `git fetch` outside `repo_mutation_lock` and acquire `repo_mutation_lock` + `lock_worktree_service` only for the fast local CAS ref phase).

### Check 4: Config & Credential Parser Differentials
- **`.git/config` & `config.worktree` Credential Bypass**:
  1. Inspect **all** config files exposed inside `<common_git_dir>` (`config` and `config.worktree`), not just `<common_git_dir>/config`.
  2. Git's config parser supports mid-value quoting (`url = https://"user:tok@host"/r.git`, `helper = "st""ore"`) and backslash escapes (`\"`, `\\`, `\n`, `\t`). Never rely solely on `.trim_matches('"')`; parse via `gix::config::File` (failing closed on unparseable configs) in addition to lexical checks before inspecting keys, subsections, URLs, `extraHeader`, `credential.helper` (`store` or `!`), and `[include]` / `[includeIf]`.

### Check 5: Crash Recovery & Detached-HEAD Worktrees
- **Porcelain Parser Blind Spot**: `git worktree list --porcelain` omits `branch refs/heads/...` when a worktree is in `detached HEAD` state. Identify managed agent worktrees by their canonical parent directory (`worktrees_base`) and `WorktreeId` leaf name so detached worktrees remain visible to `list_agent_worktrees` and `recover_agent_worktrees`.

### Check 6: Sandbox Isolation & Environment Hygiene
- **Fail-Closed Execution**: Never fall back to unsandboxed host execution when `sandbox-exec` (macOS) or `bwrap` (Linux) is unavailable.
- **Strict Environment Allowlist**: Strip host secrets (`HOME`, `SSH_AUTH_SOCK`, `GIT_ASKPASS`, `GIT_SSH_COMMAND`, `GIT_CONFIG_*`, `GITHUB_TOKEN`, `AWS_*`, `DYLD_*`, `LD_*`) and enforce `GIT_CONFIG_NOSYSTEM=1` + isolated `HOME`/`TMPDIR`.

### Check 7: Auth Rate-Limiting DoS, Per-Key Memory Bounds & Unbounded Growth
- **Input Length Validation & Fixed-Size Hash Keys**: Bounding map entry *count* (`MAX_RATE_LIMIT_TRACKED_KEYS`) is not enough if an attacker can send 1.9 MiB `identifier` strings that get stored verbatim as map keys (10,000 × 1.9 MiB = 19 GiB OOM). Always reject oversized inputs (`identifier.len() > 254`, `password.len() > 256`) *before* allocating/lowercasing, and hash variable-length identifiers (SHA-256) when constructing rate-limit keys.
- **Client-Scoped Rate Limiting**: Scope login rate-limit keys by unforgeable caller scope (`ConnectInfo<SocketAddr>`) so remote attackers cannot globally lock out known usernames (`"siteadmin"`) from other IPs, and never trust spoofable headers (`X-Forwarded-For`) unless an explicit trusted-proxy configuration is verified.
- **Fail-Closed Cap Eviction**: When `failed_logins` reaches `MAX_RATE_LIMIT_TRACKED_KEYS`, never `return;` early (which fails open and disables rate limiting after an attacker floods fake usernames). Evict an older/non-account entry so new failed attempts are always recorded.
- **Bounded State**: Prune expired/revoked sessions (preserving suspended-user revoked sessions until expiry for `403 AccountDeactivated` semantics) and stale rate-limit windows on snapshot writes.
- **Wire-Verified SSH Fingerprints**: Decode OpenSSH Base64 blobs, verify the length-prefixed wire algorithm header matches the outer algorithm token, and compute `SHA256:<unpadded-base64>`.

### Check 8: Clean Architecture, Hard `<600` Line Gate & State/Lock Encapsulation
- **Single Source of Truth & Cross-Process Repository Ownership**: Domain policies and entities (`EnterpriseSecurityPolicy`, worktree metadata) must have a single authoritative source rather than duplicated fields across `AppState` and `AuthStore`. Furthermore, independent OS processes (`gitust serve`, `gitust mcp`, `gitust sync`) targeting the same repository must acquire and retain an exclusive OS file lock (`<common_git_dir>/gitust-worktree-service.lock` via `File::try_lock()`, never unlinked on release to avoid inode replacement races) so two processes cannot construct split-brain state or clobber refs concurrently.
- **Hard `<600` Line Per-File Ceiling (P1 Gate)**: Every `.rs`, `.ts`, and `.tsx` file across `crates/` and `packages/` (including test modules) MUST stay strictly under **600 lines** (`wc -l < 600`). Any file reaching 600 lines is an automatic **P1** finding and must be split into single-responsibility submodules immediately.
- **State & Lock Encapsulation**: Callers in HTTP/MCP route handlers must never manually coordinate lock acquisition/ordering across multiple internal maps (`worktrees`, `worktree_runtime`, `spawn_quota`). Encapsulate multi-map state transitions behind cohesive methods on `AppState` / `AuthStore`. In frontend packages, strictly separate live backend HTTP API clients from in-memory showcase/demo state.

### Check 9: BOLA / IDOR & Deactivated Principal Enforcement
- Every repository/worktree/sync/browse route must enforce `check_repo_access` (fail-closed on unknown `RepoId`), memoize per-`RepoId` checks in list endpoints, and immediately reject deactivated principals (`403 Forbidden`).

### Check 10: Zero Fabricated Telemetry or Fake Stubs (`AGENTS.md` Principle 5)
- Never hardcode fake latency numbers, no-op timers, or fake stub responses that return `200 OK` claiming an action (`Merged`, `synced_at`) happened when no underlying Git ref or remote operation was performed. Either execute the real Git operation or return an explicit error status (`409` / `501` / `502`).
- All performance claims must come from reproducible benchmarks with hardware/OS context.

### Check 11: Git Subprocess, Mirror Sync, Diff/Blob Parser & Ref CAS Integrity
- **Argument Injection (`--`) & Literal Pathspecs (`--literal-pathspecs`)**:
  - Separate positional paths/refs with `--` so values starting with `-` cannot be interpreted as flags (`--upload-pack`).
  - Whenever passing a file path filter to `git diff` or `git log`, always pass `--literal-pathspecs` before `-C` so characters like `[ab].txt`, `*`, `?`, or `:(` are never interpreted as Git pathspec globs.
- **Inbound Mirror Sync Safety (`gitust sync`)**:
  - Require explicit `--repo` on destructive/sync CLI commands (never default `--repo` to `.`). Only run `git init --bare` when the target path does not exist or is an empty directory.
  - Preserve local-only branches and ahead branches (including approved merges on `main`) using in-process `gix` ancestry classification (`classify_branch_relation`) and batched CAS (`repo.edit_references`).
  - Never abort the entire branch loop when a single branch diverges (`non-fast-forward`), is indeterminate, is checked out in a linked worktree, or is dirty in the main worktree; record it in `skipped_branches: Vec<(name, reason)>` and continue syncing all remaining branches.
  - Resolve reference targets with `r.follow_to_object()` (symbolic refs only), **never** `r.peel_to_id_in_place()`, so annotated `Tag` objects (messages and signatures) are preserved 1:1.
  - **Safe Topological Ancestry & 4-State Classification (`FastForward`, `LocalAhead`, `Diverged`, `Indeterminate(String)`)**:
    - **NEVER cut off ancestry search by commit timestamp** (`parent.time > child.time` clock skew in real-world Git histories causes false negatives if timestamp cutoffs are used).
    - Use safe topological ordering from `.git/objects/info/commit-graph` (`repo.commit_graph_if_enabled()`, written via `-c fetch.writeCommitGraph=true`, where `gen(parent) < gen(child)` is a strict DAG invariant) combined with a 2-color DAG merge-base walk (`FROM_LOCAL`, `FROM_UPSTREAM`, `BOTH`) that terminates as soon as single-color frontier nodes merge into `BOTH`.
    - Strictly separate **4 states**: `FastForward`, `LocalAhead`, `Diverged` (proven non-ancestor), and `Indeterminate(String)` (when `MAX_ANCESTRY_WALK_COMMITS` is reached before finding the target or when a commit object read/decode fails—never conflate `Indeterminate` with `Diverged` or `false`).
  - Pass negative refspec `^refs/heads/agent/*` to `git fetch` and configure Git stall detection (`http.lowSpeedLimit`, `http.lowSpeedTime`, SSH `ServerAliveInterval`) instead of a short fixed wall-clock timeout.
- **Unified Diff & Git C-Quoted Path Parsing**:
  - Track remaining `(rem_old, rem_new)` hunk line counts from `@@ -a,b +c,d @@` so deleted lines starting with `--` (`--- old sql comment`) or added lines starting with `++` (`+++ evil()`) inside a hunk are never misparsed as file headers.
  - Decode Git's C-quoted paths (`"b/quoted\"name.txt"`) even when `-c core.quotePath=false` is set, because Git still C-quotes paths containing `"`, `\`, `\t`, or `\n`.
- **Untrusted Repo Hook/Filter Neutralization**: Enforce `-c core.hooksPath=/dev/null -c core.fsmonitor=false`, `--no-ext-diff --no-textconv`, and `GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null`.

### Check 12: MCP, Streaming, Pre-Allocation Object Size Checks & `SIGPIPE`-Safe Draining
- **Pre-Allocation Object Header Check**: Before calling `repo.find_object(oid)` to read a Git blob, check `repo.objects.find_header(oid)` so multi-hundred-MiB blobs do not allocate in memory before `MAX_BLOB_VIEW_BYTES` is enforced.
- **Drain After Cap (`SIGPIPE` Prevention)**: When capping captured child `stdout`/`stderr`, never drop the pipe reader immediately after `.take(cap).read_to_end(...)` while the child process is still running—always drain remaining output into `std::io::sink()` / `tokio::io::sink()` until EOF.
- Enforce explicit byte/line limits on HTTP request bodies and MCP JSON-RPC frames so a runaway client cannot trigger OOM.

### Check 13: DB / Disk Crash Consistency, `.gitignore` Hygiene & N+1 Prevention
- Never issue DB or lock-heavy repository lookups inside loops (`AGENTS.md` §7.1).
- Ensure disk mutations (worktree creation/removal) and state mutations (`worktrees` / DB) are ordered and reconciled on startup so neither crash nor cancellation leaves permanent zombie entries.
- **Secret File `.gitignore` Hygiene**: Ensure default local state paths (e.g., `.gitust/`, `.*.gitust-worktrees/`) are ignored in `.gitignore` so credentials or `auth-db.json` snapshots cannot be accidentally committed.

### Check 14: Frontend, Design System, Async Request Ordering & Viewer State (`packages/*`)
- When auditing frontend changes (`packages/design-system`, `packages/sync-client`, `packages/web`):
  - **0ms Optimistic UI + Rollback**: Mutations must update UI state immediately without spinners, and deterministically roll back if the server returns an error.
  - **Monotonic Async Request Ordering (`latestViewReqId`)**: File/commit/diff viewers that fire async `fetch()` calls on user selection must share a monotonic request sequence counter and discard stale responses (`if (reqId !== latestViewReqId) return;`) so out-of-order network responses (`A -> B` clicked, `B -> A` resolved) never cause a mismatched file title and body.
  - **Explicit Empty / Binary / Truncated / Error States (No Stale Content or Silent Fake Data)**: Clear stale viewer lines immediately when selection changes, display explicit status lines for empty diffs (`lines: []`), binary blobs (`is_binary: true`), and truncated blobs (`truncated: true`), surface actual `catch (err)` messages, and never fall back to fake showcase data when a live backend token is active.
  - **Strict Design Tokens & Sprite Icons**: Zero magic numbers (use `--ds-*` CSS variables only) and zero Unicode emojis in production UI (use `<Icon name="..." />` only).
  - **Virtualization & Keyboard Viewport Sync**:
    - Never combine `left: 0; right: 0` with `contain: paint` on virtualized code/diff rows inside an `overflow-x: auto` viewport (it clips long lines and breaks horizontal scrolling; use `min-width: 100%; width: max-content; contain: layout style`).
    - Keyboard navigation (`ArrowUp`/`ArrowDown`, `j`/`k`, `Ctrl+N`/`Ctrl+P`) in scrollable lists/palettes must call `scrollIntoView({ block: "nearest" })` so the active selection never moves off-screen.
  - **XSS Hygiene**: Never inject unsanitized commit messages, branch names, diffs, or agent logs into raw HTML sinks.

---

## 3. Execution Steps & Output Format

1. **Inspect Scope & Interacting Context**:
   - Inspect both uncommitted changes (`git status`, `git diff`) and surrounding callers/callees in touched crates.
   - **Never restrict findings solely to `+`/`-` diff lines** if the change exposes or interacts with a bug in adjacent code.
2. **Walk the 4-Axis Audit Model & 14-Point Checklist**: Evaluate every applicable check against the actual code paths.
3. **Run Verification Gates**:
   - `find crates packages -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' \) -not -path '*/node_modules/*' -not -path '*/dist/*' -exec wc -l {} + | awk '$2 != "total" && $1 >= 600 { print "OVER_600:", $0; exit 1 }'`
   - `cargo fmt --all -- --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --workspace`
4. **Structured Finding Output Format**:
   Format every finding with exact file/line, severity (`P0` / `P1` / `P2`), verification status (`[Verified]` / `[Suspected]`), failure trace, and a ready-to-apply code diff:

   ```markdown
   ## File: crates/<crate>/src/lib.rs
   ### L<LINE_NUMBER>: [P0|P1|P2] [Verified|Suspected] One-sentence summary of the bug.
   - **Trigger & Impact**: Exact sequence of inputs/events that triggers the bug and its security/correctness impact.
   - **Suggested fix**:
   ```diff
      unchanged context
   -  buggy line
   +  fixed line
   ```
   ```

5. **Explicit Verdict**: End with `Verdict: CLEAR TO COMMIT` or `Verdict: BLOCKED (list blocking P0/P1 items)`. Never commit or push while any P0 or P1 item remains unresolved.

---

## 4. Continuous Post-Mortem Evolution Rule

Whenever a new class of P0 or P1 bug is uncovered during development, testing, or subagent review that was not already explicitly covered by the checklist above, **update `.agents/skills/adversarial-audit/SKILL.md` in the same step** so that every bug permanently strengthens the repository's audit runbook.
