# MCP worktree commit review — 2026-10-17

Base source revision: `adb4b6e2e09b99ff8bb3ebf268dad11199c6f718`, plus the reviewed
command-output, full-review remediation, operations and MCP commit working-tree changes.
This record describes local implementation evidence, not alpha release approval.

## Contract and fixes

The new host-side MCP tool commits explicit paths against an expected parent without
making shared Git administration writable inside command sandboxes. HTTP requires
trusted single-user mode, a live SiteAdmin and persisted worktree ownership. Actual stdio
uses explicit local-operator authority; it can resume work without rewriting ownership.
The operation clears old verification and never grants approval, merges or publishes.

A blocking worker retains worktree/service/index ownership through final credential and
repository checks, ref compare-and-swap, index publication and state transition, including
when the requesting HTTP connection closes. A failure after the ref update identifies the
created SHA and required index recovery; it does not pretend the commit was rolled back.

Review and implementation checks reproduced and fixed these boundaries:

- Host source reads could bypass a task's hidden auth-file policy. Source and implicit
  attribute/configuration inputs now enforce that policy, including private directory
  aliases and hardlinked files. The regression failed before the fix and passes with
  forbidden HTTP tool results and unchanged HEAD; permitted sources still commit.
- Ordinary CRLF/text/encoding/ident conversions require Git's builtin rules. Whitelisted
  configuration, attributes and captured regular source bytes are copied to private
  metadata/workspace storage; executable custom filters, hooks and signing are excluded.
- Existing CRLF blobs under `text=auto` or `core.autocrlf` need parent-index-aware staging.
  A single batched Git add preserves this behavior. Attribute files selected for deletion
  are removed from both private-index fallback and copied attributes. Both regressions
  failed before the change; exact tree IDs now match an independent ordinary Git index,
  with clean status, clean-commit verification and fsck.
- Filesystem case/Unicode aliases cannot introduce a second Git spelling for one file.
  Distinct actual names on case-sensitive filesystems remain supported by the implementation;
  this macOS execution does not establish Linux behavior.
- Git can ignore reserved names while returning success. Every requested result is checked
  for exact mode, OID or deletion before publishing, preventing a successful partial commit.
  Executable modes follow Git's owner-execute-bit rule.
- Active linked-worktree configuration is checked before host Git validation/execution.
  Config reads are bounded, no-follow, nonblocking regular-file reads. A regression proves
  an ordinary Git filter executes in its synthetic fixture while Gitust refuses it first.
- Selected paths, derived ancestor candidates, source bytes, metadata, output and indexes
  are bounded. Traversal, staged/hidden-index state, special files, stale HEAD and foreign
  index locks fail closed.

Interrupted-commit recovery is documented in [the MCP contract](../architecture/mcp-commits.md).
An earlier independent SIGKILL check preserved edits and HEAD but left index.lock. Recovery
requires all writers stopped, a private backup, verified metadata and explicit stale-lock
handling; `reset --mixed --no-refresh HEAD` preserves working files without refreshing them.

## Index-aware normalization checkpoint

- Current `scripts/verify.sh`: exit 0. Operations regressions 12, frontend cases 99, Rust
  workspace tests, fmt/Clippy, types, file-size gate, fail-open scanner and embedded bundle
  freshness passed. Git reported 70 passed / 2 ignored benchmark cases, sandbox 20 passed,
  server 65 passed.
- Independent final Gate A: **CLEAR TO COMMIT**, with no remaining verified P0/P1/P2 in
  its reviewed scope. It reran 26 host-commit cases, verified both failing-before logs and
  the corrected staging path, and checked 176 Rust/TS/TSX files below 600 lines (maximum 585).
- The [latest local rehearsal](../operations/evidence/alpha-rehearsal-mcp-index-aware-macos-2026-10-17.json)
  passed all ten tasks using the rebuilt binary: 15 MCP commits, failed Checks and repair,
  fresh-main retry, exact verification/approval/publication, remote-failure retry, discard,
  restart, relocated restoration and the original credential's subsequent commit.
  Server, local bare upstream and HTTP Git client passed fsck. Duration 12.385 seconds is
  one observation, not a latency guarantee. Binary SHA-256:
  `a92e237d0ef42e712e022ff3ef56c580f903ee4114d97feaf2d98e4af963f191`.

Earlier rehearsal reports are retained with their own binary and script hashes. They do
not establish the behavior of a later binary.

## Independent HTTP review limitation

The independent reviewer reported 59 completed local fixture checks on the preceding
binary (`3280943be1511de604f5ecb14e034d9e856b9285b7ddf8bb44cbcd8c99e3b030`).
Reported observations include own OAuth commit success, other-admin/developer refusal,
revocation/deactivation while a body is incomplete, revocation during preparation before
CAS, disconnect completion and concurrent discard refusal, body/source/path limits,
nonblocking FIFO refusal, discarded-task refusal, process-service ownership, restart with
private auth DB/lock/hardlink refusal, permitted-source success and independent Git fsck.

Further review was interrupted by the tool service's policy before a final Gate B verdict.
An evidence-only follow-up was also interrupted. No final CLEAR verdict was obtained, and
those earlier checks do not clear the later index-aware normalization change. The partial
report is not represented as a completed seven-surface review. The parent confirmed the
reported extra process handle was absent, no matching probe/server processes remained,
and no isolated HTTP fixture directories remained. No further blocked probes were retried.

The user explicitly approved the AGENTS.md 7.3 exception for committing and pushing
with this incomplete Gate B, provided the limitation is recorded here and in the commit
message. This permits source publication only; it does not complete alpha acceptance or
retroactively turn an unexecuted review into a passed review.

The subsequent compatibility review reproduced a blob-replacement discrepancy; managed
Git workflows now explicitly reject replacement views, as described below.
Actual agent-client integration, GitHub publication, Linux runtime execution, a separately
provisioned installation and final browser acceptance remain open in the
[acceptance progress report](../operations/alpha-progress-2026-10-17.md).


## Follow-up hardening before source publication

A normal two-client API/MCP/Git regression proved a P1 approval-intent bug: client A's
screen retained verified SHA A, client B committed and verified B, and the old ID-only
approval merged B. The new bounded request requires `expected_commit` and compares it
with the verified SHA under the worktree guard. A stale request gets 409 while B's valid
verification stays intact; a new explicit B request can merge B. Missing, malformed,
unknown-field and oversized preconditions are refused. The original credential is checked
after body reading and again before the merge operation. Both original regressions failed
before the implementation and pass after it; a body-read session-revocation regression
also denies approval without changing main or writing an approval receipt.

Execution responses now include the actual `checked_commit`. The web correlates it with
current verification, sends the observed SHA for approval and checks the returned merge
SHA. Missing/mismatched evidence requires refresh; a new SHA clears older output. A
successful discard no longer claims that the preserved private branch was deleted. New
frontend regressions failed 13/14 before the fixes and all pass afterward.

The normal Git compatibility review also reproduced executable-bit/type changes under
`core.fileMode=false` and `core.symlinks=false`, ignored legacy `crlf` attributes, and a
blob replacement discrepancy. The private staging path now uses effective mode/symlink
settings and stage modes as well as OIDs, preserves captured executable permissions for
conversion, and supports `crlf`, `crlf=input` and `-crlf`. Independent Git index/tree, clean
status and fsck oracles cover the mode/conversion combinations. The extra executable
conversion defect found in final Gate A was fixed before publication.

Replacement refs are refused in managed worktree/verification/approval/publication safety
checks, including loose/packed/malformed refs and retained gix replacement mappings. The
server cannot validate a logical replacement view and publish a different physical SHA.
No replacement ref is silently deleted. A retained mapping requires service restart after
the operator removes the replacement and reviews the stored objects.

The remaining product gap is a worktree-row entry into that worktree's diff review. It is
tracked for the next implementation step; the current Changes tab browses repository
commit history. This source publication does not claim the complete browser acceptance.


## Frozen source publication candidate

After all follow-up fixes, `scripts/verify.sh` exited 0: 12 operations tests, 113
frontend tests, 78 Git tests (two benchmark cases ignored), 20 sandbox tests, 68 server
tests and the remaining Rust workspace cases, fmt/Clippy, types, fail-open and embedded
bundle/file-size gates passed. Final independent Gate A remained CLEAR after its cached
replacement and approval-body revocation checks, with all 181 source/test files below
600 lines. No further source edits followed this verification.

The [SHA-bound rehearsal report](../operations/evidence/alpha-rehearsal-sha-bound-macos-2026-10-17.json) passed all ten tasks and
15 MCP commits, including stale approval after newer verification, in 13.517 seconds.
Binary SHA-256: `f5899e6269836fbad5b19c3063b568c0e3a618c686ae1fb1b21e2f248b7477bc`. Real Git HTTP and final server/upstream/client fsck passed.
The production build measured initial JavaScript at 79.59 kB gzip; the 50 kB target remains
unmet. The incomplete Gate B and broader alpha acceptance limits above remain explicit.
