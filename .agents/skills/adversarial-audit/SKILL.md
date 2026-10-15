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

## 1. Anti-Rubber-Stamping Protocol & 3-Axis Threat Modeling

1. **Guilty Until Proven Innocent**: Never conclude "0 P0, 0 P1" just because `cargo test` and `cargo clippy` pass. Happy-path unit tests rarely catch cancellation leaks, parser differentials, or missing-file security bypasses.
2. **Mandatory 3-Axis Threat Modeling (For Finding Unknown Bugs)**:
   Do not merely check off known past bugs. For every touched function, handler, or state transition, explicitly interrogate three orthogonal axes:
   - **Axis A — Lifecycle & Concurrency**:
     - *What if the caller's Future is cancelled (HTTP disconnect, timeout, `SIGINT`) while `spawn_blocking` or an `.await` is in flight?*
     - *What if Request B (`approve`, `discard`, `deactivate`) runs while Request A (`exec`, `spawn`, `sync`) is suspended at `.await`?*
     - *What if the process crashes (`SIGKILL`) right between a disk mutation and an in-memory/DB update, and restarts?*
   - **Axis B — Input, Path & Parser Boundaries**:
     - *What if a path does not exist yet (`ENOENT`), is already deleted, or traverses a symlinked root (`/var` vs `/private/var`)?*
     - *What if a user/agent supplies a ref or path starting with `-` (Git CLI option injection), backslash line continuations (`\\\n`), tabs (`\t`), quotes, or non-ASCII/control characters?*
   - **Axis C — Resource Exhaustion & Fail-Closed Cap Behavior**:
     - *When a bounded map/queue hits its cap (`MAX_RATE_LIMIT_TRACKED_KEYS`, `MAX_TRACKED_WORKTREES`), does it fail-closed (evict/reject) or accidentally fail-open (`return;` bypassing security)?*
     - *Can an unauthenticated caller or noisy neighbor lock out another user's account, exhaust quota slots via cancelled requests, or trigger unbounded memory/disk growth?*
3. **Concrete Trace or Probe Required**: When auditing sandbox profiles, git commands, or parsers, run a real probe (in a temporary directory under `mktemp -d`) whenever feasible.
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

### Check 3: Post-`.await` State TOCTOU & Lock Coordination
- **Stale State Overwrite Across `.await`**: Mutually exclusive operations on the same entity (`exec`, `approve`, `discard`) must coordinate via per-entity `exec_lock` (`try_lock_owned`), and the lock guard must be held until **after** both the on-disk operation and the in-memory/DB state removal/transition complete.
- **Terminal State Monotonicity**: State transitions after `.await` must re-verify that the entity has not transitioned to a terminal state (`Merged`, `Discarded`) while locks were dropped.
- **No Blocking I/O Inside Locks**: Never hold a `RwLock`/`Mutex` across `git` subprocesses, disk I/O, or Argon2 hashing.

### Check 4: Config & Credential Parser Differentials
- **`.git/config` Credential Bypass**: Before exposing `<common_git_dir>` read-only to a sandbox, fold backslash-escaped line continuations (`\\\r\n`, `\\\n`), strip quotes, handle arbitrary whitespace/tabs in section headers (`[include\t]`, `[includeIf...]`), block `credential.helper` using `store` or inline shell `!`, block `[include]` / `[includeIf]` / `include.path`, and inspect all URL/header fields case-insensitively.

### Check 5: Crash Recovery & Detached-HEAD Worktrees
- **Porcelain Parser Blind Spot**: `git worktree list --porcelain` omits `branch refs/heads/...` when a worktree is in `detached HEAD` state. Identify managed agent worktrees by their canonical parent directory (`worktrees_base`) and `WorktreeId` leaf name so detached worktrees remain visible to `list_agent_worktrees` and `recover_agent_worktrees`.

### Check 6: Sandbox Isolation & Environment Hygiene
- **Fail-Closed Execution**: Never fall back to unsandboxed host execution when `sandbox-exec` (macOS) or `bwrap` (Linux) is unavailable.
- **Strict Environment Allowlist**: Strip host secrets (`HOME`, `SSH_AUTH_SOCK`, `GIT_ASKPASS`, `GIT_SSH_COMMAND`, `GIT_CONFIG_*`, `GITHUB_TOKEN`, `AWS_*`, `DYLD_*`, `LD_*`) and enforce `GIT_CONFIG_NOSYSTEM=1` + isolated `HOME`/`TMPDIR`.

### Check 7: Auth Rate-Limiting DoS, Fail-Closed Caps & Unbounded Growth
- **Client-Scoped Rate Limiting**: Scope login rate-limit keys by unforgeable caller scope (`ConnectInfo<SocketAddr>`) so remote attackers cannot globally lock out known usernames (`"siteadmin"`) from other IPs, and never trust spoofable headers (`X-Forwarded-For`) unless an explicit trusted-proxy configuration is verified.
- **Fail-Closed Cap Eviction**: When `failed_logins` reaches `MAX_RATE_LIMIT_TRACKED_KEYS`, never `return;` early (which fails open and disables rate limiting after an attacker floods fake usernames). Evict an older/non-account entry so new failed attempts are always recorded.
- **Bounded State**: Prune expired/revoked sessions (preserving suspended-user revoked sessions until expiry for `403 AccountDeactivated` semantics) and stale rate-limit windows on snapshot writes.
- **Wire-Verified SSH Fingerprints**: Decode OpenSSH Base64 blobs, verify the length-prefixed wire algorithm header matches the outer algorithm token, and compute `SHA256:<unpadded-base64>`.

### Check 8: Single Source of Truth (Zero Split-Brain State)
- Domain policies and entities (`EnterpriseSecurityPolicy`, worktree metadata) must have a single authoritative source rather than duplicated fields across `AppState` and `AuthStore`.

### Check 9: BOLA / IDOR & Deactivated Principal Enforcement
- Every repository/worktree/sync route must enforce `check_repo_access` (fail-closed on unknown `RepoId`), memoize per-`RepoId` checks in list endpoints, and immediately reject deactivated principals (`403 Forbidden`).

### Check 10: Zero Fabricated Telemetry or Fake Stubs (`AGENTS.md` Principle 5)
- Never hardcode fake latency numbers, no-op timers, or fake stub responses. All performance claims must come from reproducible benchmarks with hardware/OS context.

### Check 11: Git Subprocess, Hook/Filter & Ref CAS Integrity
- **Argument Injection (`--`)**: Whenever user/agent-supplied branch names, revisions, or paths are passed to `git` CLI commands, validate them and separate positional paths/refs with `--` where supported so values starting with `-` cannot be interpreted as flags (e.g. `--upload-pack`).
- **Untrusted Repo Hook/Filter Neutralization**: When running `git` commands on repositories or worktrees that may contain untrusted commits/configs, ensure malicious `.git/hooks`, `core.hooksPath`, `core.fsmonitor`, or `.gitattributes` `filter` commands cannot execute arbitrary host binaries.
- **Atomic Ref CAS**: Use compare-and-swap (verifying expected old OID or `PreviousValue::MustNotExist`) when creating/updating refs so concurrent pushes or merges never silently clobber commits.
- **Outbound Sync Noise Guard**: Verify outbound GitHub sync strictly rejects `refs/heads/agent/*` and unapproved refs.

### Check 12: MCP, Streaming & Request Bounds
- Enforce explicit byte/line limits on HTTP request bodies, MCP JSON-RPC frames, and captured sandbox stdout/stderr buffers so a runaway process or client cannot trigger OOM.
- Verify MCP sessions cannot access or mutate worktrees belonging to unauthorized repositories or other principals.

### Check 13: DB / Disk Crash Consistency & N+1 Prevention
- Never issue DB or lock-heavy repository lookups inside loops (`AGENTS.md` §7.1).
- Ensure disk mutations (worktree creation/removal) and state mutations (`worktrees` / DB) are ordered and reconciled on startup so neither crash nor cancellation leaves permanent zombie entries.

### Check 14: Frontend, Design System & Optimistic UI Rollback (`packages/*`)
- When auditing frontend changes (`packages/design-system`, `packages/sync-client`, `packages/web`):
  - **0ms Optimistic UI + Rollback**: Mutations must update UI state immediately without spinners, and deterministically roll back if the server returns an error.
  - **Strict Design Tokens & Sprite Icons**: Zero magic numbers (use `--ds-*` CSS variables only) and zero Unicode emojis in production UI (use `<Icon name="..." />` only).
  - **XSS Hygiene**: Never inject unsanitized commit messages, branch names, diffs, or agent logs into raw HTML sinks.

---

## 3. Execution Steps & Output Format

1. **Inspect Scope & Interacting Context**:
   - Inspect both uncommitted changes (`git status`, `git diff`) and surrounding callers/callees in touched crates.
   - **Never restrict findings solely to `+`/`-` diff lines** if the change exposes or interacts with a bug in adjacent code.
2. **Walk the 3-Axis Threat Model & 14-Point Checklist**: Evaluate every applicable check against the actual code paths.
3. **Run Verification Gates**:
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
