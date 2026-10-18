# Automatic ordinary-branch Checks

An active primary or writable secondary (`--git-write-repo`) repository can run
committed Checks after an accepted Git Smart HTTP or SSH update to an ordinary
branch (`refs/heads/*`, excluding `refs/heads/agent/*`). This requires
`--trusted-single-user-sandbox`, a current SiteAdmin with repository access and
`"auto_run": true` in the **previously committed** default-branch policy. It uses
the same temporary checkout, OS sandbox and bounded completed history as
[explicit Local Checks](local-checks.md). Success is diagnostic and never grants
verification, merge approval or publication authority.

## Enabling and updating the policy

Commit `.gitust-checks.json` with `version: 1`, required `commands` (or named
`steps` with `name` and `command`), optional `branches` (exact branch short names
or `refs/heads/...` refs eligible for automatic triggers) and `auto_run: true`.
The push that first enables it on the default branch does not run automatically:
there was no prior committed consent on the default branch before the push. Once
the default branch has prior committed `auto_run: true` consent (and allows the
target branch when `branches` is specified), a subsequent eligible ordinary-branch
push runs the policy from the **accepted new commit** on that pushed branch,
including changed commands or steps. Both old default-branch consent and accepted
branch policy identities are retained independently. Uncommitted files and policy
edits are not executed.

An accepted new policy on the pushed branch that disables automatic execution,
excludes the branch via `branches`, removes the file or has invalid configuration
produces a rejected attempt without running commands. Reenabling the policy takes
effect on the following eligible push. A missing or disabled prior default-branch
policy creates no attempt. An unreadable or invalid prior default-branch policy
leaves consent unproven: scheduling is skipped with a fixed diagnostic while the
existing Git authorization and transport result remain authoritative.

Only a non-deletion, non-no-op update to an ordinary branch (`refs/heads/*`,
excluding `refs/heads/agent/*`) on the active primary or a writable secondary
(`--git-write-repo`) repository is eligible. Read-only secondary repositories
(`--read-only-repo`), tags, Maintainer pushes, local filesystem commits and MCP
operations do not trigger this flow. Managed-worktree automatic Checks retain
their existing separate [contract](../architecture/automatic-checks.md).

## Accepted updates and execution

Before receive-pack, a proven opt-in on the repository's current default branch
creates a private `prepared` intent for each eligible ordinary branch in the push.
Failure to persist that intent prevents receive-pack from starting. The original
Git session, PAT or SSH key, its account, role and actual repository access are
checked again after preparation. No credential is saved with the intent.

After receive-pack, Gitust observes each target direct ordinary ref under its
existing mutation guard and reads the accepted commit's policy before saving
`queued`. HTTP 200 alone is not acceptance evidence. A scheduling failure after
Git accepts an update cannot undo that push or replace its actual Git response.
This is not a durable per-request Git receipt or coordination with out-of-band
host Git writers.

There is at most one running attempt and one latest queued attempt per repository.
A newer accepted revision replaces only the queued attempt and records the older
one as superseded. Manual and automatic Local Checks share a per-repository
operation lock and the server's executor. Current account activity, SiteAdmin role,
repository access, identity, target ordinary ref, exact SHA and policy are checked
at dispatch and after executor admission, before the first command. A new push does
not cancel an already admitted immutable execution snapshot.

Accepted jobs store a user ID and fixed purpose, not a resumed authenticated
request. Later expiration or revocation of the originating credential does not
by itself cancel the job. Current account, role and repository authority still
must permit dispatch. Mid-command revocation and exactly-once side effects are
not guaranteed.

## Diagnostics in the workbench

**Actions → Local Checks** shows running and queued work plus the latest terminal
attempt. Older retained attempts are collapsed. Entry and explicit Refresh read
state once; there is no periodic polling or retry/cancel control. An exact
retained result can be opened without replacing the selected result automatically.

`GET /api/v1/repos/{id}/local-check-automatic-runs` requires current SiteAdmin
access to the primary or writable secondary repository and returns exactly
`version: 1`, `repo_id` and `runs`, bounded to 3 MiB with `Cache-Control: no-store`.
Prepared intents are private. Runs contain `run_id`, `source_ref`, `expected_commit`,
`checks_blob_oid`, `phase`, `reason`, `history`, `created_at` and `updated_at`; no
command output, credentials or host paths are included. The order is running,
queued, then up to five terminal attempts in newest insertion order. The optional
catalog hint `can_view_local_checks_automatic` is independent of execution enablement.

A completed or failed command outcome is separate from history saving. `history`
is `saved`, `failed` (saving unconfirmed) or `not_attempted`. Output is linked only
when the retained completed record matches the exact run ID, ref, commit and
policy blob. The five retained lifecycle records and five completed results have
independent retention; expired or unsaved output is not replaced with another
run. Reading diagnostics remains available with execution disabled.

## Restart and offline restoration

Metadata lives under `<common_git_dir>/gitust-local-check-automatic-runs` in a
bounded, private, versioned document, hidden from command sandboxes. Stopped
[backups](backup-restore.md) preserve it with repository identity and auth state;
no absolute installation path is stored in the document.

On restart, `prepared` becomes `interrupted/acceptance_unconfirmed`. Present ref
equality does not recreate a lost acceptance receipt, so a crash between the
push and durable queue can deliberately miss an automatic run. Only durable
queued work can resume after current authority and target checks. Starting with
execution disabled rejects queued work; enabling it later does not replay it.

A running intent with an exactly matching retained result can be finalized
without execution. Otherwise it becomes interrupted and is never automatically
replayed. Unresolved execution fences still require the existing explicit
[operator recovery](execution-recovery.md). Lifecycle recovery does not clear a
fence, terminate a stored PID or restore verification authority.
