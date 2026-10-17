# Alpha acceptance progress — 2026-10-17

The alpha goal remains **open**, with [nine acceptance conditions](alpha-acceptance.md).
The user changed A8's review policy on 2026-10-17 as recorded below. This report records
local operational evidence, not release approval. Base
revision: `adb4b6e2e09b99ff8bb3ebf268dad11199c6f718`, plus uncommitted command-output,
review-fix, operations and MCP commit changes in the working tree. The observations below
belong to the binaries/scripts identified by their reports. The latest local checks and
architecture verdict are recorded below; the independent HTTP verdict remains incomplete.

## Observed evidence

| Area | Evidence and scope |
| --- | --- |
| Clean source/output installation | macOS arm64, offline locked dependency installation, production UI and release binary build with an empty target directory. Existing host toolchains/caches were used; this was not a newly provisioned machine. |
| Execution and Git transport | 20 actual Seatbelt tests and the Smart HTTP clone/commit/push/fetch/RBAC regression passed, with server/client `git fsck`. |
| Standalone binary | The release binary was copied away from its build tree, source assets were unavailable, and PATH contained Git without Node/pnpm. Eight embedded responses matched and fresh administrator login succeeded. |
| Ten operational tasks | [Recorded JSON](evidence/alpha-rehearsal-macos-2026-10-17.json): two concurrent HTTP MCP protocol clients, actual task source edits/commits, failed Checks, stale/dirty SHA rejection, approval, explicit publication/retry, discard, restart and relocated restore. All ten passed in 15.2 seconds on this host. This is an observation, not a performance guarantee. |
| MCP-only task commits, earlier dirty binary | [Schema-2 JSON](evidence/alpha-rehearsal-mcp-commit-macos-2026-10-17.json): tasks 1–9 made 15 commits through `gitust_worktree_commit`, including concurrent clients, repair and a commit after relocated restore with the original PAT; task 10 used a separate real Git HTTP client. All ten tasks and server/upstream/client `git fsck` passed in 11.928 seconds. Binary SHA-256 `5335b5bc588d93860f100715f4d93c073a403f2122adf5d93ae37cf2d3ef5724` identifies that earlier dirty build, not the final candidate. |
| Recovery | The original repository path was made unavailable. The new location retained auth/login, owners, seven prior approvals/publications, main SHA and two completed run records. Unmerged work recovered Pending and required fresh verification. |
| Backup regressions | Nine tests cover normal/bare layouts, dirty files and links, original immutability, busy locks, integrity/mode damage, external Git storage, partial clones and writer/reader size parity. |
| Operational isolation regressions | Three tests prove that inherited Git repository selectors cannot redirect fixture commands/snapshots, and ignored symlink parents cannot copy outside files into the source snapshot. |

See [platform validation](platform-validation.md) for exact tools, commands and the source
snapshot hashes. The full platform run after inherited-Git-environment isolation used
224 source files, SHA-256
`b60914b351bebc0b0fdb78df9d9dddcb8ab4d0d7e780f03961cdae53d7657dc7`.
Its existing-cache install, web build and release build took 1.451, 3.233 and 27.643 seconds.
The later source-parent-symlink hardening was checked with its failing-before/passing-after
regression; it does not change the Rust binary or sandbox implementation.

## Independent review and fixes

Gate A found five issues, each reproduced and fixed before its final clear verdict:

1. Symlinked Git object/metadata storage could leave a restored snapshot dependent on the
   original disk. Unsupported metadata symlinks, alternates and partial clones are refused.
2. File-mode damage was invisible to content hashes. File/directory modes now participate
   in snapshot integrity checks, including executable and private-directory modes.
3. A writer could create a manifest larger than the restore reader accepted. Both paths
   now use the same 64 MiB cap and an oversize create fails without publishing a backup.
4. Inherited `GIT_DIR`/`GIT_WORK_TREE` could redirect a rehearsal command into an original
   checkout. Operational subprocesses discard inherited `GIT_*` before explicit settings.
5. A tracked file below an ignored symlink parent could read outside the source tree.
   Every source path component is checked before copying and rehashing.

Gate B independently ran 16 backup probe groups, including real Rust/Python lock
cooperation, missing/damaged inputs, metadata-link and traversal rejection, failed-copy
cleanup, original immutability and normal restoration. Its scope was the new backup
utility; it does not claim a fresh audit of unrelated OAuth/network functionality.
The independent operations reviews ended clear with no remaining P0/P1/P2 in this scope.
The architecture audit checklist now records the new backup and operational invariants.

The earlier operations change's `scripts/verify.sh` exited 0: the 12 operations regressions, 99 frontend tests, Rust
workspace tests, formatting, Clippy, type checks, fail-open audit, file-size gate and embedded
bundle freshness all passed. One earlier run while a separate release build was running
hit the existing lazy-command-palette test's one-second wait; the later serial full run
passed without changing the test or weakening its assertion. The initial outer-sandbox
Seatbelt denial was also retained as a failed run before approved host-level verification.

## Current MCP commit implementation boundary

Gitust implements [host-side MCP commits](../architecture/mcp-commits.md) while keeping
shared Git metadata read-only in the command sandbox. HTTP commits require trusted mode,
SiteAdmin authority and the persisted task owner; local stdio uses operator authority
without changing ownership. Requests name an exact parent and explicit files. Starting
an authorized attempt clears old verification; success leaves Pending and requires new
Checks before human approval. Builtin Git normalization is supported, and external filters,
hooks and signing programs are excluded from this path. Errors after branch creation
identify the SHA requiring index recovery; an uncatchable kill can leave a stale index
lock requiring operator inspection with all writers stopped.

Repository tests cover selected content and independent Git SHA/tree checks, failure and
ownership boundaries, hidden inputs, stdio restart, staged/hidden-index state, path aliases,
builtin normalization, forbidden executable filters and index recovery. These are the
implemented test cases. The frozen-source `scripts/verify.sh` exited 0 with 12 operations
tests, 113 frontend tests and all Rust workspace checks, including 78 Git tests (two
benchmark cases ignored), 20 sandbox tests and 68 server tests. Independent final Gate A
ended CLEAR with no remaining verified P0/P1/P2 in its scope, including stale human
approval, mode/attribute combinations and replacement refusal. The [latest rehearsal report](evidence/alpha-rehearsal-sha-bound-macos-2026-10-17.json)
records ten successful tasks and 15 MCP commits, including a stale human approval after
newer successful verification, in an observed 13.517 seconds. Binary SHA-256:
`f5899e6269836fbad5b19c3063b568c0e3a618c686ae1fb1b21e2f248b7477bc`.

The independent HTTP reviewer reported 59 completed checks on the preceding binary,
including body/final-CAS credential revocation, owner/role boundaries, disconnect
completion, input limits and private auth-file denial. Further checks were interrupted
by tool policy before a final verdict; those results do not clear Gate B for the latest
normalization changes. See the [MCP review record](../security/mcp-commit-review-2026-10-17.md).

## Worktree review follow-up after the source checkpoint

Source checkpoint `11143a4cb6f5d65a495ca84cf9cf9a43d24d9d12` was committed and pushed
with the user's explicit Gate B exception. The following step received a separate Gate B
exception on 2026-10-17. The user also ended automatic Gate B reviews going forward;
Gate A, project checks and runtime regressions remain mandatory. Historical incomplete
Gate B results are not recorded as passed.

The workbench now opens committed worktree changes directly from the task row. It
resolves head/base once, validates those immutable SHAs on file switches, refuses an
unknown merge target and binds tree/blob previews to the tree's own commit. The shared
Git resolver also prevents same-named refs from replacing a full OID or an explicitly
qualified ref. Independent Git regressions failed before both fixes and pass afterward.

[Browser evidence](evidence/alpha-web-review-macos-2026-10-17.json) records a real local
preview task with two MCP-committed files, branch and verified-SHA comparisons, file
switching, fresh Checks after server restart and approval of that same SHA. Independent
Git queries confirmed the task/main SHA, clean checkout and repository/upstream `fsck`.
The final browser review/verification/approval used binary SHA-256
`0837660b38b3cb7946a0b6ed2df265d02565a691268834a649675bc85ab7415a` and the Vite
development UI. This does not establish a standalone release installation or GitHub
publication. Desktop 1280px and mobile 390px layouts were inspected; the mobile page
had no page-level horizontal overflow, and temporary viewport overrides were reset.

The follow-up's `scripts/verify.sh` exited 0 with 136 frontend tests, 81 Git tests
(two benchmark cases ignored), 20 sandbox tests, 68 server tests and the remaining
workspace/operations checks. Production UI build and CLI build passed. Independent
Gate A ended CLEAR with no remaining verified P0/P1/P2 in this scope; all 183 Rust/TS/TSX
source/test files remained below 600 lines. Gate B was not completed; the user approved
the exception and removed it as a standing requirement. The
production entry bundle is 80.73 kB gzip, above the 50 kB goal. Task labels also remain
generic after restart; branch/owner/approval identity is preserved.

## Remaining exit conditions

- Execute the acceptance suite on an actual Linux host with functioning Bubblewrap
  namespaces. No usable Linux VM/container runtime was available locally.
- Verify real intended agent-client integration. The two Python clients prove protocol
  concurrency; the rehearsal does not represent IDE/LLM sessions.
- Validate authorized GitHub authentication/publication and a separately provisioned
  installation. Local bare upstreams do not prove either external integration.
- Complete the live browser workflow evidence against the eventual release candidate,
  then run the full project checks and independent reviews for that candidate.

Full Phase 3/4 capabilities remain outside this milestone. No deployment, actual upstream
GitHub workflow publication or production-data restore is established by this evidence.
Source-repository commits and pushes are separate from the application publication test.
