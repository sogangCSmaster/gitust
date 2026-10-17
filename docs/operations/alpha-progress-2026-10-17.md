# Alpha acceptance progress — 2026-10-17

The explicit repository-registration checkpoint `d92b8fe0b6bf30ad8c4965a8ea39421f43052b16`
passed [exact-source Linux/macOS CI](evidence/alpha-ci-d92b8fe-2026-10-17.json).
Both platforms completed the mechanical gates, all ten operating tasks and crash
recovery; tasks 9 and 10 exercised registered ACLs through relocated restoration
and actual Git HTTP operations. Tasks 1 through 8 retained legacy sharing.

The earlier repository Actions checkpoint `89a2062dda66a25356ab54c178afe3b20e01fae9`
passed [exact-source Linux/macOS CI](evidence/alpha-ci-89a2062-2026-10-17.json),
including all mechanical gates, six repository-page regressions, ten operating tasks
and actual execution-crash recovery. The same automatic attempt was visible through
the repository API as Running and then Interrupted after explicit recovery, without
replay or restored verification authority. GitHub product publication remains deferred
by the user; this evidence does not cover later changes or intended IDE-agent sessions.

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

## Task labels follow-up after `9cb3852`

The worktree-review and review-policy checkpoint was committed and pushed as
`9cb385259f94d7bc6c175fab4b0760229a368bbc`; its pre-push verification passed and the
remote main SHA matched. The next uncommitted step preserves a new task's display
label in its immutable creation record. Existing unlabeled records retain the generic
recovered label. Display metadata does not restore verification or change owner/target
identity; CLI tasks store a fixed label rather than their command and arguments.

Core recovery regressions failed on the old implementation, and the two HTTP/MCP
restart regressions also failed against an isolated `9cb3852` source tree. They passed
after the fix. The final `scripts/verify.sh` exited 0 with 136 frontend, 85 Git
(two benchmark cases ignored), six CLI, 70 server, 20 sandbox and 12 operations tests,
plus the remaining checks. Independent Gate A ran eight focused cases and ended CLEAR
with no remaining verified P0/P1/P2; all 185 Rust/TS/TSX source/test files were below
600 lines. Gate B was not run, as instructed by the user.

[Live recovery evidence](evidence/task-label-recovery-macos-2026-10-17.json) identifies
the binary and source hashes. A named MCP task was created on the actual local preview
server, the server process was restarted, and its label remained visible in the browser.
Its owner record, branch and base were unchanged; it remained Pending with no restored
verification or approval authority. The preview checkout stayed clean and `git fsck`
passed. This remains macOS development-server evidence, not release acceptance or
GitHub publication. New labeled origin records require a compatible newer binary;
older binaries reject that optional field, so downgrading them is unsupported.

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

## Fixed source checkpoint `a178330`

The [release checkpoint evidence](evidence/alpha-release-macos-a178330-2026-10-17.json)
records a clean, pushed source revision and one matching release binary for installation,
all ten operating tasks and execution-crash recovery. A separate 321-file source/output
directory used offline dependency caches, passed 29 real Seatbelt sandbox tests and the
Smart HTTP permission/`git fsck` roundtrip. The copied binary served 20 exact embedded
responses and accepted fresh administrator login while its source assets were unavailable
and runtime PATH contained Git alone. The ten-task rehearsal and interrupted-command
recovery both passed with source `a178330a995c150b3a07e72bbcbf1729cf435c19` and binary
SHA-256 `56952424c15273d70409d8d4c51c9d4eecc19cab5665cde961624cb8b4142b52`.

These remain local macOS observations using existing host tools, protocol MCP clients
and a local bare upstream. Intended IDE sessions, authorized GitHub publication, a
separately provisioned machine and complete Linux acceptance remain open. The hosted
[run for this checkpoint](https://github.com/sogangCSmaster/gitust/actions/runs/39008388426)
passed all mechanical gates, ten operating tasks and crash recovery on macOS 15.
Linux stopped in a cold-loading frontend test before Rust sandbox tests ran; its
result cannot establish the new Bubblewrap policy's behavior. The overall run failed.

## Hosted Linux and macOS checkpoint `42bae69`

The [allowlisted CI evidence](evidence/alpha-ci-42bae69-2026-10-17.json) records
[run 39009448871](https://github.com/sogangCSmaster/gitust/actions/runs/39009448871),
attempt 1, for source `42bae69103127dd2263ad1fed6cc620b6f9cb161`. Both Ubuntu 22.04
and macOS 15 passed every mechanical gate, all 29 real OS sandbox tests, the late-host
metadata/symlink regression, all ten operating tasks and execution-crash recovery.
The report assertions confirmed the exact clean source and required Bubblewrap/Seatbelt
backend. The operating and crash rehearsals also completed their required strict Git
`fsck` checks.

This closes the missing hosted Linux runtime evidence for this fixed checkpoint.
Earlier failed runs retain their recorded outcomes. These results do not cover later
uncommitted repository-identity work or establish a standalone installation, an intended
IDE session or actual GitHub publication. Crash recovery proves durable rejection and
explicit recovery; it does not assert automatic termination of every orphaned command.

## Repository identity checkpoint `80c1540`

The [identity CI evidence](evidence/alpha-ci-80c1540-2026-10-17.json) records
[run 39010142717](https://github.com/sogangCSmaster/gitust/actions/runs/39010142717),
attempt 1, for source `80c15402144c1debe4e2a369ec796dce019c0809`. Both Ubuntu 22.04
and macOS 15 passed all mechanical gates, 105 Git tests (two existing comparative
benchmarks ignored), 82 server tests and 29 sandbox tests. Real OS regressions covered
atomic identity publication, process restart, concurrent initialization, unsafe metadata
rejection and relocation; Linux exercised `renameat2` with `RENAME_NOREPLACE`.

Both hosts completed all ten operating tasks, including the new repository-ID check
across restart and relocated restore, and execution-crash recovery with strict Git
`fsck`. Source, clean-runtime and expected-backend report assertions all passed. The
prior `42bae69` evidence remains unchanged. These results cover only `80c1540`, exclude
later uncommitted scheduler work and do not establish standalone installation, intended
IDE sessions or actual GitHub publication.

The user explicitly deferred actual GitHub publication testing on 2026-10-17 while
requesting continued development elsewhere in the roadmap. Local fixture upstreams
remain the publication evidence boundary; this deferral does not close acceptance A4.

The [durable automatic-run evidence](evidence/alpha-ci-f424969-2026-10-17.json) records
[CI run 39013527741](https://github.com/sogangCSmaster/gitust/actions/runs/39013527741),
attempt 1, for `f424969f021682647918f0cada818325b36d4e9b`. Ubuntu 22.04 and macOS 15
both passed all mechanical gates, 105 Git tests (two comparative benchmarks ignored),
29 DB tests, 106 server tests, 30 sandbox tests, all ten operating tasks and the updated
crash rehearsal. Same-run-ID Interrupted retention and no-replay booleans follow the
fixed script's successful `require` -> report path; raw private report JSON was not
collected. Exact-source/backend checks and strict Git `fsck` passed. This fixed-checkpoint
record excludes later activity UI changes. Actual GitHub product publication remains
deferred by the user, and native agent-client proof remains incomplete.

The selected-worktree Activity viewer at `b2433779b6f28624eb2c9384d01380b9f99ee1b4`
also passed [CI run 39015002517](https://github.com/sogangCSmaster/gitust/actions/runs/39015002517)
on both operating systems. Each job passed 197 frontend tests, all mechanical gates,
the ten-task rehearsal and the actual crash-recovery rehearsal. The [fixed-source
record](evidence/alpha-ci-b243377-2026-10-17.json) excludes subsequent repository-wide
Actions changes; it does not close the deferred publication or native-client criteria.
