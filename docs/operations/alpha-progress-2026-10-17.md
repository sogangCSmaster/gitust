# Alpha acceptance progress — 2026-10-17

The subsequent repository-name editing slice passed local full gates and the real
two-repository import/rename/ACL/restart rehearsal. Browser checks saved names on
both primary and secondary repositories, retained the name after a remembered-session
reload, and found no page overflow at 320/768/1440px. Matching dialog buttons measure
44px on the mobile viewport and 32px on the larger viewports; physical devices were
not tested. The [initial static JavaScript closure](evidence/repository-rename-ui-size-2026-10-17.json)
is 69,794 bytes gzip and remains above the 50 kB target.

Adjacent original-credential omissions in the summary, four legacy object-read routes
and inbound synchronization were reproduced and fixed. Permanent session/PAT regressions
failed before the fixes and passed afterward, including checks before actual Git
publication phases. Independent local pre-fix sync probes observed publication after
revocation; the additional independent native post-fix probe was not completed because
the reviewing agent was interrupted by tool security policy. This is not a completed
external review or Gate B. The first full run also exposed a cold-module test deadline;
waiting for the actual import after the pending-modal assertions made the full rerun
pass without preloading the feature or extending timeouts. Exact-source hosted CI for
this later slice remains pending.

The [offline repository-import checkpoint](repository-import.md)
`2a0a81bbecc151a743265c870fe20cbc59730bd7` passed
[exact-source Linux/macOS CI](evidence/alpha-ci-2a0a81b-2026-10-17.json).
Both platforms completed all mechanical gates, the ten operating tasks, crash
recovery and the actual two-repository import rehearsal, including a requested
default branch distinct from upstream HEAD, annotated tags, fresh identity,
exact retry and source preservation. Independent local probes also passed interruption
during both Git initialization and fetch, refusal while an orphan child could
remain, explicit stopped-process recovery and administrator deactivation during
fetch. These are process-interruption checks, not Git-payload power-loss evidence.
This fixed checkpoint excludes the subsequent repository-name editing work.

The multiple-repository browsing checkpoint `689cdb58ac5067987af0a25efd3af2195c6c442f`
passed [exact-source Linux/macOS CI](evidence/alpha-ci-689cdb5-2026-10-17.json).
Both platforms completed all mechanical gates, the ten operating tasks, crash recovery
and the new actual two-repository routing/ACL/startup-rollback rehearsal. This fixed
checkpoint does not include the subsequent repository-import work. Intended IDE-agent
sessions and the user-deferred GitHub product-publication evidence remain open.

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

## Name-edit checkpoint and inherited lock descriptors

The [public run metadata](evidence/alpha-ci-ecf79b5-2026-10-17.json) records
[run 39037210088](https://github.com/sogangCSmaster/gitust/actions/runs/39037210088),
attempt 1, for `ecf79b54a9c0352409918dd6c07d4692011b74e9`. Ubuntu passed every
mechanical gate, the ten-task operating rehearsal, the two-repository rehearsal and
execution-crash recovery. macOS failed during the CLI import tests; subsequent
operating rehearsals were skipped. Local verification success does not change this
hosted failure into a passed checkpoint.

The failing test immediately reopened the destination claim after an intentionally
failed fetch. A local deterministic regression retained a duplicate of the same
open-file description, released its owner and attempted a fresh acquisition. Both
the import claim and auth DB lock failed before explicit unlock-on-drop guards and
passed afterward. The tests also require exclusion while the new owner remains
alive, even after the previous duplicate closes. This proves the descriptor-lifetime
defect; inheritance by a concurrent fork is the likely CI trigger, not a separately
observed process trace. The existing repository service guard already releases its
lock explicitly.

The fix preserves private persistent lock files, bounded auth-lock waits, fail-closed
errors, import recovery acknowledgements, durable execution fences and write ordering.
The import claim is released after its stage and repository-service handles. No
lock file is unlinked and no sleep/retry is added to conceal the failing test.
Hosted verification of the fixed revision must be recorded separately; this failed
run does not establish macOS crash-recovery coverage for `ecf79b5`.

The separate [fixed-revision CI record](evidence/alpha-ci-9de8a33-2026-10-17.json)
records [run 39038225699](https://github.com/sogangCSmaster/gitust/actions/runs/39038225699),
attempt 1, for `9de8a33483954db9dcd3fe96061bb829a34547df`. Both Ubuntu 22.04
and macOS 15 passed all mechanical gates, ten operating tasks, the two-repository
and startup-rollback rehearsal, and execution-crash recovery. These results cover
the lock-release fix at that exact source revision; later unborn-HEAD changes
require their own validation. Product GitHub publication remains deferred.

The [unborn-HEAD checkpoint record](evidence/alpha-ci-181f67e-2026-10-17.json)
records [run 39039984783](https://github.com/sogangCSmaster/gitust/actions/runs/39039984783),
attempt 1, for `181f67e67427a7b7d326947297ad629cf1bbb2cb`. Both hosts passed
mechanical gates, the ten-task operating rehearsal, the two-repository/startup
rollback rehearsal and execution-crash recovery. This covers nullable default HEAD,
the existing first ordinary push path and its committed regressions. It excludes
the subsequent repository-create CLI, Clone dialog and new empty-creation rehearsal.

## Offline creation and Clone candidate

The [local candidate record](evidence/repository-create-local-2026-10-17.json)
covers the subsequent offline `repository create` implementation and compact Clone
dialog. Its final backend/Clone source manifests are recorded separately; the
Darwin operating run used a dirty candidate based on `181f67e`, not a clean hosted
revision. Two real CLI-created repositories retained their UUID/registration/inode
on empty retry. An actual relocated empty backup served its identity and accepted
login while both original Git and auth paths were unavailable. Real HTTP clone,
alternate/default push and fetch, explicit alternate-commit reads before default
HEAD existed, restart and strict server/client Git `fsck` passed. An obsolete create
retry after the first commit was refused without changing refs or anchors.

Independent review found that an invalid display name could seal a journal before
DB registration rejected it, preventing a corrected request at the same destination.
The shared import/create path now applies the existing DB name validator before
login or a destination claim. A before/after regression checks unchanged auth and
no journal on rejection, followed by successful corrected creation. Existing raw
request bytes remain in the import-v1 digest; journals written by the previous CLI,
including padded names and interrupted stages, resumed with the same identity.
Existing malformed journals are preserved for operator inspection.

The operator rehearsal itself also caught an unsupported `env` argument on a Git
wrapper and an unproven alternate-branch result. The corrected calls use the
existing environment-cleaning helper, and the result now requires an actual HTTP
blob read against an independent Git oracle while the default HEAD is still absent.
Restoring the old calls reproduces the TypeError and produces no success report.
The new required CI step exercises this operating path on both platforms.

Browser checks used the same frozen UI/dist before the backend-only name correction:
actual login, primary Clone URL, secondary absence, Escape/focus and widths of
320/768/1440 pixels. Dialog buttons match in height, including 44px mobile targets,
and no page overflow was observed. The real host clipboard was not exercised;
clipboard success/failure and stale-response behavior use mocked unit tests.
The [measured initial JavaScript closure](evidence/repository-create-clone-ui-size-2026-10-17.json)
is 71,359 bytes at gzip level 9, still above the 50,000-byte target. This candidate
does not add HTTP creation, live inventory changes, secondary writes, standalone
installation or native IDE evidence. Hosted verification must be recorded at its
own source revision. Product GitHub publication remains deferred.

The subsequent [fixed-source creation checkpoint](evidence/alpha-ci-5b7801c-2026-10-17.json)
records [run 39041940368](https://github.com/sogangCSmaster/gitust/actions/runs/39041940368),
attempt 1, for `5b7801c32647271b86b14e80f599707ae67e25e0`. Ubuntu 22.04 and
macOS 15 both passed all required gates, ten operating tasks, two-repository and
startup-rollback checks, the new actual empty-create/first-push/independent-restore
rehearsal, and execution-crash recovery. This exact-source result covers the
creation and Clone implementation; it excludes subsequent branch-list API/UI
changes and the enhanced branch-page assertions in that rehearsal. It does not
establish product GitHub publication, standalone installation or native IDE proof.

## Branch browsing candidate, 2026-10-18

The [local branch candidate record](evidence/branch-browsing-local-2026-10-18.json)
binds the frozen backend10/UI17 source manifests to the final Darwin binary.
The dirty candidate is based on `5b7801c`; that revision's hosted creation result
does not establish hosted branch coverage. The final local mechanical gate passed
CLI33, DB37, Git118 (two existing benchmarks ignored), server145, sandbox30 and
frontend251 across 33 files, alongside Python regressions, formatting, Clippy,
typechecks, bundle freshness and line/fail-open checks.

The actual operating rehearsal checks one-entry UUID branch pages against Git
before any refs, after an alternate first push while the default is unborn,
after the first default push and after restart. Empty secondary reads remain
independent. A canonical maximum 43,709-byte cursor also round-trips through the
real HTTP listener; code/URI-construction checks alone were not used as this
transport evidence. The previous binary fails the same operator at the missing
branch route without producing a success report. An earlier draft used `after`
instead of the finalized `cursor` parameter and failed on page two; the actual
pagination path now remains required in CI.

Independent review caught mixed commit-list/tree identity: checking only the tree
could publish another commit's history. The helper now requires the requested SHA
in the commits envelope and its nonempty first entry too. Three independent wrong
envelope/first-entry/empty-history cases fail before the correction and pass after
it. The touched shared server test Git helpers also now clear inherited selectors
on all three entry paths; isolated child regressions preserve the separate
synthetic repository's HEAD, refs and working state.

The final binary's browser flow covered actual login and 320/768/1440px layouts.
An alternate branch was readable before the default existed, a later real push
and list refresh preserved the reviewed commit, selecting the new tip updated
Files/Changes, and default-HEAD refresh preserved the explicit branch. Choosing
the default or a secondary branch read that repository's identified content.
Mobile branch changes previously cleared the file path while leaving the file
list hidden; actual before/after visibility and focus checks close that defect.
The fix uses the existing path-reset signal and adds no test-only state marker.

The [initial JavaScript closure measurement](evidence/branch-browsing-ui-size-2026-10-18.json)
is 72,956 bytes at gzip level 9. The selector and its existing Select dependency
load when Files/Changes opens; the 50,000-byte initial target remains unmet.
Browser evidence does not claim UI pagination over more than 50 branches: its
controls have behavioral tests and real HTTP keyset pagination has the operating
rehearsal. Branch mutation, secondary writes, standalone installation, native IDE
integration, actual GitHub product publication and deployment remain outside this
candidate. A later hosted run must be recorded at its exact tested source.

The subsequent [fixed-source branch checkpoint](evidence/alpha-ci-caf4161-2026-10-18.json)
records [run 39045033914](https://github.com/sogangCSmaster/gitust/actions/runs/39045033914),
attempt 1, for `caf4161108891be5283eb33f32df0f63c6b44054`. Ubuntu 22.04 and macOS 15
both passed all required gates, ten operating tasks, two-repository startup checks,
empty-create and branch-page checks including the maximum cursor, and execution-crash
recovery. This exact-source result does not cover subsequent multi-repository backup
changes or establish product GitHub publication, standalone installation or native IDE
integration.

## Multi-repository stopped backup candidate, 2026-10-18

The [local checkpoint](evidence/multi-backup-local-2026-10-18.json) records the dirty
candidate based on `caf4161`. A repeated `--read-only-repo` includes up to 32 explicit
repositories in a version 2 snapshot under all service locks and one shared auth lock.
Fixed ordinal payload paths preserve each repository's Git data and managed worktrees;
AuthDB rows do not infer service paths. Single-repository writes retain version 1.
Independent old-writer/new-reader and new-writer/old-reader round trips passed with
the original repository, managed-worktree base and auth file unavailable.

The actual Darwin/Seatbelt rehearsal made the entire original installation unavailable,
restored both repositories and the auth checkpoint, and started only the restored paths.
It checked stable UUIDs and registration, renamed catalog entries, current team access,
outsider denial and secondary write/transport denial against independent Git refs,
objects and strict `fsck`. Origin/owner/approval/history bytes and modes, an annotated tag
and dirty files in both repositories survived. Durable approval recovered `Merged`;
an unmerged task with passing history recovered `Pending`, null verification fields,
and an approval rejection without changing refs. All owned services and fixtures were
removed before the report was published. The same final operator reaches the old
tool's unsupported multi-repository flag and fails without a success report.

Review caught a version 2 inventory mismatch that could ignore an unclaimed third
repository while declaring a two-repository restore complete. Fixed-slot inventory
validation now refuses that inconsistent snapshot before creating output. A valid
uppercase UUID path also exposed a writer/reader mismatch; both now use the existing
UUID validity rule while retaining the original spelling. Both actual before cases are
covered by the final consolidated regressions. The touched backup test Git helpers
also strip inherited repository selectors, including the direct bare-clone path.

The integrated mechanical gate passed Python19, CLI33, DB37, Git118, server145,
sandbox30 and frontend251, with formatting, Clippy, typechecks and bundle/line/fail-open
checks. The new operating rehearsal is required on both CI platforms, but hosted
evidence for this later checkpoint is still separate from `caf4161`'s successful run.
No Rust or UI behavior changed in this slice. This is a stopped cooperative snapshot,
without an online or power-loss guarantee. Product GitHub publication remains deferred;
native IDE integration, standalone installation and deployment acceptance remain open.

The [fixed-source multi-backup checkpoint](evidence/alpha-ci-dd8c0c5-2026-10-18.json)
records [run 39046465958](https://github.com/sogangCSmaster/gitust/actions/runs/39046465958),
attempt 1, for `dd8c0c595d2ff5cf0e0057d7e72c0a77c613c657`. Ubuntu 22.04 and macOS 15
both passed every required step, including the new all-repository stopped restoration
and execution-crash recovery. This is exact-source hosted evidence for the backup
slice; it excludes subsequent default-branch management changes and does not establish
product GitHub publication or deployment.

## Lock-time ref comparisons candidate, 2026-10-18

The [local ref-integrity checkpoint](evidence/ref-cas-local-2026-10-18.json) binds
the frozen candidate based on `dd8c0c5` to its rebuilt Darwin binary. Ordinary
repository regressions added to otherwise unchanged `dd8c0c5` production source
reproduced five failures: stale approval, host commit, create-only branch, mirror
batch and shared deletion expectations. The deletion regression retains a fixed
expected SHA so a delayed test thread cannot legitimately delete a later tip.
The same final cases pass with the native prepared-transaction adapter. The
baseline filter also selected one existing passing test; it did not fail to build.

Expected object refs are now checked under Git's actual locks rather than a
snapshot read before lock waiting. Packed refs, raw annotated tags and origin
reflogs remain compatible. Symbolic aliases, denied publication gates and oversized
batches are refused without a partial batch result. Prepared refusal aborts normally
before forced-cleanup fallback; process-group cleanup is performed once and pipe
workers are collected. Mirror batch failures propagate instead of spawning an
unbounded single-ref fallback or turning operational errors into skipped success.
See the [exact transaction and recovery boundary](../architecture/ref-transactions.md).

The approval path now rechecks the original session/PAT, current role and actual
repository access after slow validation, using the prepared-ref publication gate
for bare refs and the final pre-command gate for a checked-out-main merge. Core
regressions prove denial preserves refs, index and approval absence in both layouts,
then permits an authorized retry. They do not claim a newly executed network
revocation-race experiment. An uncertain ref outcome retains an inspection/retry
error; it is not described as an aborted merge.

The final mechanical gate passed Python19, CLI33, DB37, Git128 (two existing ignored
benchmarks), server145, sandbox30 and frontend251, together with formatting, Clippy,
typechecks and bundle/line/fail-open checks. The rebuilt binary then passed all ten
actual local operating tasks: HTTP MCP commits, Checks, exact approval, local-bare
publication retry, relocated restoration, and a real Smart HTTP clone/commit/push/
fetch round trip with strict server/upstream/client `fsck`. Owned scratch services
and data were removed before the success report was written. Independent reviews
cover the implementation, complementary test slices and final evidence boundaries.

This is local dirty-candidate evidence; `dd8c0c5`'s green hosted run covers the
earlier backup checkpoint. Hosted Linux/macOS evidence for these later changes must
be recorded at their own source revision. The prior standalone review interrupted
by tool policy has not been resumed or reclassified as complete. Gate B remains
disabled, product GitHub publication remains deferred, and this checkpoint does not
establish new UI, deployment, standalone installation or native IDE acceptance.

The [fixed-source ref-integrity checkpoint](evidence/alpha-ci-6c67809-2026-10-18.json)
records [run 39050388214](https://github.com/sogangCSmaster/gitust/actions/runs/39050388214),
attempt 1, for `6c67809b2543f153971519c9805d5cf09bed9863`. Ubuntu 22.04 and macOS 15
both passed all required gates, ten operating tasks, two-repository startup checks,
empty-create/branch-page checks, stopped multi-repository restoration and execution-crash
recovery. That exact-source result covers the common ref fix and approval gate; it
does not cover the subsequent default-branch feature or establish product publication.

## Default branch management candidate, 2026-10-18

The [local default-branch checkpoint](evidence/default-branch-local-2026-10-18.json)
binds the frozen candidate based on `6c67809` to its rebuilt embedded-UI binary.
A SiteAdmin can change an existing committed branch into the default of a configured
registered bare primary, through the compact Overview branch line. The symbolic
HEAD comparison occurs under `HEAD.lock`; original session/PAT and current role/ACL
are rechecked after slow validation and lock waiting. Existing worktrees retain
their original origin, Checks and approval target. Omitted HTTP/MCP bases resolve
the current committed symbolic default without a detached/unborn `main` fallback.
The [contract](default-branch.md) distinguishes HEAD CAS from target-tip validation
under the server mutation guard; no external-writer target-tip atomicity is claimed.

The final mechanical gate passed Python19, CLI33, DB37, Git133 (two existing ignored
benchmarks), server152, sandbox30 and frontend258 across 34 files, plus formatting,
Clippy, typechecks and bundle/line/fail-open checks. The actual operating rehearsal
checks both repositories' refs/HEAD after denied, stale and permitted requests,
stable identity/registration, unchanged annotated tags and verified task records,
old-task approval to `main`, and a new omitted-base checkout at `develop`. Restart,
Smart HTTP HEAD advertisement, fresh clone contents, existing-clone fetch and strict
`fsck` pass. The same operator against the prior `6c67809` binary reaches the absent
route's 404 and creates no success report; that is feature-absence evidence, not an
authentication defect. The operator is now required in both CI jobs.

Actual SiteAdmin login and native browser controls were exercised at 320/768/1440px.
The mobile dialog saved the change with matching 44px action buttons; tablet/desktop
buttons match at 32px. Ten observed layouts have no page overflow. Escape returns
focus to the trigger, keyboard selection works, an explicit `main` file snapshot
survives the default change, and choosing Default reads `develop`. The existing
task's comparison still identifies its original `main` base; the read-only repository
exposes no change trigger. Independent pixel review covers six of the ten saved
screens. Owned browser services, private fixtures and tabs were removed and the
viewport override reset. Physical devices and an already-open cross-tab comparison
were not separately exercised; stale/session/ABA cases use permanent tests.

The [remeasured initial static-import closure](evidence/default-branch-ui-size-2026-10-18.json)
is 73,656 bytes at gzip level 9, 700 bytes above the byte-identical parent UI. Opening
the default dialog brings the cumulative emitted closure to 98,272 gzip bytes.
The 50,000-byte initial target remains unmet, and no latency improvement is claimed.
This candidate preserves the prior ref fix and adds no secondary writes, branch
creation/deletion or online service inventory. Its own hosted result remains separate
from `6c67809`'s successful run. Product GitHub publication remains deferred; standalone
installation, native IDE and deployment acceptance remain open.

The [fixed-source default-branch checkpoint](evidence/alpha-ci-ada787a-2026-10-18.json)
records [run 39052238635](https://github.com/sogangCSmaster/gitust/actions/runs/39052238635),
attempt 1, for `ada787a8ab49b0b6493815b3088c7fa3a7f4498c`. Ubuntu 22.04 and macOS 15
both completed successfully, including the actual default-branch operator and
execution-crash recovery. macOS intentionally skipped Linux-only step 6; every other
required step succeeded. This result covers that source revision, rather than the
later JavaScript import changes.

## Initial JavaScript import boundary candidate, 2026-10-18

The [local bundle checkpoint](evidence/initial-js-local-2026-10-18.json) records a
candidate based on `ada787a`. Direct runtime primitive imports and an independent
sample-data module keep an optional viewer out of the initial graph. Repository
Overview is now loaded on repository entry while its root controllers and session
epochs retain their lifetime. The existing deferred boundary exposes loading and
failure without taking away navigation or granting hidden mutation authority.

The [complete emitted-closure measurement](../benchmarks/web-bundle.md) falls from
73,656 to 70,031 gzip9 bytes (-4.92%); first Overview cumulative bytes instead rise
by 232 to 73,888. CSS and public package exports are unchanged. The 50,000-byte target
remains unmet. A repository command reproduces the measurement using private build
output and cache directories. Static graphs are supplemented by a real authenticated
fresh-page asset trace; Access-link startup remains a distinct optional cohort.

The integrated mechanical gate passed frontend260 across 36 files, Python19,
CLI33, core2, DB37, Git133 (two existing ignored benchmarks), sandbox30 and server152,
plus formatting, Clippy, typechecks and bundle/line/fail-open checks. Two new route
boundary regressions fail against the prior eager implementation and pass afterward.
Native browser checks on the rebuilt binary cover 320/768/1440px, Clone, display-name
and default-branch saves, actual file contents and a browse-only repository. Delayed
module navigation A→catalog→B resolves B; an actual chunk 404 shows a visible error,
permits catalog navigation and recovers through application reload. Native Git
confirms the saved symbolic default and unchanged branch/task tips and original task
base. Owned fixture processes, temporary repositories and browser tab were removed,
and the viewport override was reset.

This checkpoint claims no latency gain, final release acceptance, physical-device
coverage, new IDE integration or deployment. Product GitHub publication is deferred
at the user's request; local bare-remote evidence is not relabelled as GitHub proof.

The [fixed-source initial-JavaScript checkpoint](evidence/alpha-ci-1fee2f7-2026-10-18.json)
records [run 39054125386](https://github.com/sogangCSmaster/gitust/actions/runs/39054125386),
attempt 1, for `1fee2f7d77dba8e57de30022cdf256b3edc6aef1`. Both Ubuntu 22.04 and macOS 15
completed successfully. macOS intentionally skipped Linux-only step 6; every other
required step succeeded. It verifies the import-boundary checkpoint and does not
establish hosted success for the later access-scope implementation.

## Registered repository access-scope candidate, 2026-10-18

The [local access-scope checkpoint](evidence/repository-access-local-2026-10-18.json)
binds a candidate based on `1fee2f7` to a rebuilt combined API/UI binary. The
[contract](repository-access.md) changes registered ACL privacy metadata only.
SiteAdmin authority, the original login/PAT, current account and role, registration
binding and expected privacy are checked under the same auth write transaction.
MCP-purpose credentials cannot perform the mutation. The two scopes retain existing
role/ownership rules and anonymous denial; they are not a new read-only sharing
mode. Later authorization checks use the new value, while existing copies and
already-authorized responses are not revoked retroactively.

Prepublication failure preserves the earlier snapshot; a rename/synchronization
outcome that cannot be confirmed returns a distinct sanitized 503 and requires
reading the current value. Strict expected-value conflicts remain 409, and matching
no-ops still confirm persistence. The existing session/PAT policy and persistence
error API remain compatible with earlier callers. Optional catalog hints do not
replace mutation authorization, and legacy/older-server UI invents no privacy state.

The combined mechanical gate passed frontend268 across 37 files, Python19,
CLI33, core2, DB40, Git133 (two existing ignored benchmarks), sandbox30 and server156,
plus formatting, Clippy, typechecks and bundle/line/fail-open checks. Permanent
regressions cover current two-team access, strict concurrent comparisons, original
credential revocation and persistence uncertainty. The new feature's prior-route
404 and UI assertion failures are recorded as feature-absence evidence, not as a
previous authentication vulnerability.

The actual operator grants and revokes active outside-team/no-team reads, rejects
anonymous and nonadministrator writes, and checks real outside-team Git clone,
fetch and revoked fetch. Both repositories' refs, identity/binding, owner/origin,
approval and completed-run records remain unchanged. A stopped two-repository
snapshot restores with every original installation path unavailable; the changed
privacy and actual per-team reads survive. Human approval and history survive,
while a pending task still needs fresh verification before approval. This operator
is added to both CI jobs before crash recovery.

Native SiteAdmin controls exercised primary and browse-only access saves at
320/768/1440px, matching action heights of 44/32/32px, six observed layouts without
page overflow, keyboard selection and Escape focus restoration. An explicit main
file snapshot and its actual content survive the metadata change. Independent Git
and authenticated catalog reads confirm the observed values and unchanged refs.
Owned services, fixtures and browser tab were removed and the viewport reset.
Session/ABA/conflict/failure cases use permanent tests; physical devices and every
dialog-failure variant were not separately exercised in the browser.

The measured initial JavaScript is now 70,284 gzip9 bytes (+253), first Overview
74,405 and first access dialog 98,204 cumulative bytes. The 50,000-byte initial
target remains unmet, with no latency claim. This local candidate's hosted result
must be observed at its own revision. Native IDE, separate-installation and
deployment acceptance remain open, and actual GitHub product publication remains
deferred at the user's request. Gate B remains stopped.

The [fixed-source access-scope CI record](evidence/alpha-ci-691fdd2-2026-10-18.json)
observes [run 39055998585](https://github.com/sogangCSmaster/gitust/actions/runs/39055998585),
attempt 1, for `691fdd24e73f7ddfcce91904a5b1492df5cc2a0b`. Ubuntu 22.04 and macOS 15
both completed successfully. Required steps 1–18 succeeded except the intentional
macOS Linux-prerequisite skip at step 6. Subsequent development was not part of
that tested source revision.

## Read-only secondary Git candidate, 2026-10-18

The [local checkpoint](evidence/read-only-git-local-2026-10-18.json) binds a
candidate based on `691fdd2` to a final combined Rust/API/UI binary. Configured
secondary UUIDs now support authenticated upload-pack clone/fetch. Original
session/PAT credentials, the original user and current pinned registration/ACL
are checked before and after Git; service ownership survives cancellation until
blocking work finishes. Existing primary aliases and receive-pack behavior remain
unchanged. Secondary receive-pack stays disabled for every role.

Full final mechanical gates passed frontend270/37 files, Python19, CLI33, core2,
DB40, Git133 with two existing ignored benchmarks, sandbox30, server161 and sync1,
plus fmt, Clippy, typechecks, bundle freshness, fail-closed and line checks.
Independent source reviews cover backend8, frontend5 and operator/CI5 separately;
final combined Gate A remains mandatory before checkpointing. The permanent
native regressions cover distinct repository objects, raw annotated tags,
secondary push denial, current credentials/ACLs across body and Git waits, and
service ownership after cancellation. The prior binary's secondary404 and old
reader assertions demonstrate feature absence rather than an old security bypass.

The final operator performed real session/Basic-PAT clones, tag-identity checks,
denied secondary pushes, private/shared-scope changes, revoked outside-team fetch,
an offline secondary update followed by restart/fetch, PAT revocation and account
deactivation. It preserved primary refs and registration anchors and passed strict
server/client `fsck`. Updated catalog, empty-creation and stopped-relocation
operators also passed against the same reviewed Rust source, with their earlier
embedded UI explicitly distinguished from final browser evidence.

Native SiteAdmin checks at 320/768/1440px verified the compact Clone dialog, no
document/dialog overflow, matching action heights of 44/32/32px, keyboard open,
URL focus and Escape restoration. The list and Overview now say **Read only**;
the dialog explicitly limits secondary operation to clone/fetch. Native Git used
the actual displayed UUID URL and confirmed distinct content, raw tags, fetch,
push denial and strict `fsck`. No actual clipboard mutation was tested. Owned
tabs/services/fixtures were removed and the viewport restored.

The maintained measurement reports 70,350 initial gzip9 bytes (+66) and 74,532
cumulative bytes for Overview and Clone. The 50,000-byte target remains unmet;
no latency claim is made. The new read-only Git operator is CI step 18 and crash
recovery moves to 19. This candidate's hosted Linux/macOS result must be observed
after its exact source push. Physical devices, intended IDE-client sessions,
separate installation and final alpha/deployment acceptance remain open. Actual
GitHub product publication remains deferred, and Gate B remains stopped.
