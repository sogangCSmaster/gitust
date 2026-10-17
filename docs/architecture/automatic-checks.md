# Durable automatic Checks

Automatic Checks are repository-policy jobs created by accepted managed-worktree
MCP commits and Git HTTP pushes. They run only in the explicitly enabled trusted
single-user sandbox. Success is diagnostic: it never restores verification,
approval or publication authority. Non-managed branch triggers and the broader
Actions workflow screen remain separate work.

Before publishing a tracked ref, Gitust writes a private `Prepared` intent bound
to its stable repository ID, worktree, branch, expected commit and committed
merge-target Checks blob. MCP revalidates the original credential immediately
before its ref CAS. Git HTTP prepares all eligible ref intents before receive-pack
and revalidates the original credential, account, role and repository access
again after that disk I/O. Failed preparation or authorization prevents the Git
mutation and rejects earlier intents without deleting an older queued job.
An already accepted commit cannot be rolled back by a later scheduling failure;
MCP retains its exact SHA and explicit `unavailable` outcome, and Git HTTP retains
the Git protocol's actual per-ref response.

A completed ref mutation becomes `Queued` only after its actual ref and clean
checkout match the intent. New accepted revisions coalesce the latest queued job;
rejected candidates do not replace it. Each run has a stable logical run ID.
`Running` is synchronized to disk before sandbox execution. Completed output stays
in the existing bounded run history; matching run IDs make history writes
idempotent. The lifecycle endpoint is SiteAdmin-only:
`GET /api/v1/swarm/worktrees/{id}/automatic-runs`.

The selected worktree's Activity dialog combines those lifecycle records with the
latest five completed results from `/runs`. It shows at most thirteen distinct
run IDs and joins output only when the automatic-run marker, actor, commit and Checks
blob agree. A lifecycle-only completion is neutral, and queued, interrupted or
cancelled attempts never imply a passing command. Both reads belong to the same
session and selection; either failure clears the combined view. This is a
worktree diagnostic viewer, not the repository-wide Actions screen.

On restart, prepared intents are reconciled against actual Git state. Never-started
queued jobs may resume after current account existence, active SiteAdmin role,
ownership, repository access, trusted execution, clean exact SHA and committed
policy OID are checked again. Losing a condition records an explicit rejection,
including when execution is disabled; enabling it later does not resurrect that
job. A fixed reason enum describes failures without recording raw errors,
credentials or arbitrary command strings in scheduling metadata.

An accepted job is not an authenticated request resumed from disk. Its stored
actor contains only a user ID, trigger origin and the `automatic_checks` purpose;
raw credentials and cached principal/role objects are never serialized. Revoking
the originating session or PAT after Git acceptance does not cancel this existing
repository-policy job. Account deactivation, role loss or the checks above still
prevent dispatch. The job cannot become a login, manual verification, approval or
publication capability. Live local stdio jobs use an in-memory context for that
run only; queued local-operator jobs are cancelled on restart and require a new
explicit local operation.

A `Running` intent with a matching completed history result is reconciled without
rerunning its command. Otherwise it becomes `Interrupted`, leaves the worktree
failed and is never replayed automatically: a command may already have had side
effects. Timeout and service cancellation retain lifecycle records. Existing
execution fences remain authoritative until cleanup is confirmed or the operator
uses the documented explicit recovery procedure. Recovering a fence does not
requeue an uncertain run, and scheduling never kills a stored PID.

Scheduling metadata lives below `<common_git_dir>/gitust-automatic-runs`, hidden
from execution sandboxes. It uses private 0700/0600 directories/files, bounded
versioned documents, no-follow file access and atomic synchronized replacement.
There are at most 1,024 tracked worktree documents, each at most 64 KiB, containing
one prepared candidate, one latest queued job, one running job and five recent
terminal records. Capacity and storage errors are explicit failures; active jobs
are not evicted to make room. Stopped backups preserve these files and the stable
repository identity on relocation; restored jobs follow the same recovery rules.
This is conservative execution recovery, not an exactly-once side-effect promise.
