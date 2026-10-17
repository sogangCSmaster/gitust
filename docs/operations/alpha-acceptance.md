# Self-hosted alpha acceptance

## Objective and boundary

Keep GitHub as the source of truth while one operator runs multiple coding agents on a
single self-hosted Gitust server. Each task receives an independent worktree; verification
binds to an exact clean commit; a person reviews, approves and explicitly publishes that
commit. The installation must be recoverable from backup.

This is a trusted single-user alpha. The current OS sandbox is not a multi-tenant boundary.
Agent/model configuration independently determines what a model provider receives.
Support claims require execution evidence on the named platform, not merely compilation.

## Exit conditions

| ID | Acceptance condition | Required evidence |
| --- | --- | --- |
| A1 | A fresh environment can build/install from README, bootstrap the administrator, log in and connect a repository. | Clean source/build log; embedded UI and authenticated API used without Node at runtime; explicit prerequisites. |
| A2 | At least two MCP clients can work concurrently in separate managed worktrees. | Real HTTP MCP calls, independent commits/files, no cross-worktree state mixing, actual client integration configuration. |
| A3 | Only the exact clean commit that passed all required Checks can be approved. | Failed Checks, dirty/stale SHA, execution failure and restart cases deny approval; successful fresh verification enables it. |
| A4 | Human approval and upstream publication are distinct; retry preserves the approved SHA. | Actual Git clone/commit/push/fetch, upstream SHA oracle, private `agent/*` ref exclusion, client/server `git fsck --full --strict`; authorized GitHub integration evidence. |
| A5 | The web workflow completes task creation, diff review, verification, history, approval and publication. | Browser evidence for the live server; failure feedback, keyboard/modal boundaries, compact desktop and mobile layouts; demos visibly identified. |
| A6 | Failure, concurrency and restart do not silently damage work or grant authority. | Network failure/retry, duplicate operations, process restart, durable owner/approval/history checks; pending verification authority is not restored. |
| A7 | A stopped installation can be backed up and restored to a different location. | Restored auth, refs, dirty worktrees, owner/origin records, approvals/publications and history; original remains unchanged; reverify pending tasks before approval. |
| A8 | A fixed release candidate passes all project checks and an independent Gate A review. | `scripts/verify.sh` exit 0; architecture/correctness review with P0/P1/P2 resolved; regression and relevant runtime tests for reproduced defects. |
| A9 | Ten actual task rehearsals complete with recorded results. | Ten distinct managed tasks modifying committed Gitust source, successful and failed/retried/discarded cases, exact commit/result records, explicit local-vs-GitHub evidence boundary. |

## Evidence ledger

On 2026-10-17 the user approved the worktree-review follow-up's Gate B exception and
ended automatic Gate B reviews. A8 now requires Gate A and the existing project checks;
historical incomplete Gate B results remain incomplete evidence, not passed reviews.
The other acceptance conditions and functional/security regressions remain in force.

Record the source revision **and dirty working-tree scope**, OS/toolchain, commands, exit
codes, and artifact paths. Do not store credentials, model inputs or private output in a
public report. A local bare upstream proves Git transport and publication behavior, but
does not prove GitHub credentials, SSH host verification or GitHub deployment integration.
Two test protocol clients prove concurrent MCP handling; separately record use from the
actual agent clients intended for the alpha.

The initial acceptance audit found existing local workflow tests and independent review
reports, but no complete clean-install, relocated backup/restore, Linux runtime or ten-task
operational evidence. Those conditions remain open until current execution proves them.

Current procedures:

- [Offline backup and restore](backup-restore.md).
- [Ten-task operational rehearsal](alpha-rehearsal.md).
- [Clean installation and platform validation](platform-validation.md).
- [Acceptance progress and remaining evidence, 2026-10-17](alpha-progress-2026-10-17.md).

## Development rules and exclusions

Keep changes cohesive, measure rather than assume speed, and explicitly exercise edge
cases. Bound resource use and keep blocking work off async workers. Apply the repository's
under-600-line source limit and shared-state encapsulation rules. Do not weaken correctness
or security checks to improve latency. Record performance gaps rather than declaring the
1ms/16ms ambitions achieved.

Full PR/issue collaboration, automatic Checks on every push, SSH Git serving, GitHub
Actions compatibility, SSO, billing, multi-node scheduling and microVM isolation are later
milestones. A newly discovered defect in the accepted workflow belongs to this milestone;
a new product capability does not silently expand it.

The goal is complete only when every exit condition above has current evidence. Unavailable
platform or external integration evidence stays open, even when all local tests pass.
