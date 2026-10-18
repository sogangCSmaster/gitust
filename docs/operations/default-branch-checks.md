# Automatic default-branch Checks

An active primary can run committed Checks after an accepted Git HTTP update to
its current ordinary default branch. This requires
`--trusted-single-user-sandbox`, a current SiteAdmin with repository access and
`"auto_run": true` in the **previously committed** default-branch policy. It uses
the same temporary checkout, OS sandbox and bounded completed history as
[explicit Local Checks](local-checks.md). Success is diagnostic and never grants
verification, merge approval or publication authority.

## Enabling and updating the policy

Commit `.gitust-checks.json` with `version: 1`, required `commands` and
`auto_run: true`. The push that first enables it does not run automatically:
there was no prior committed consent. A subsequent eligible default-branch push
runs the policy from the **accepted new commit**, including changed commands.
Both old consent and accepted policy identities are retained independently.
Uncommitted files and policy edits are not executed.

An accepted new policy that disables automatic execution, removes the file or
has invalid configuration produces a rejected attempt without running commands.
Reenabling the policy takes effect on the following eligible push. A missing or
disabled old policy creates no attempt. An unreadable or invalid old policy leaves
consent unproven: scheduling is skipped with a fixed diagnostic while the existing
Git authorization and transport result remain authoritative.

Only a non-deletion, non-no-op update to the active primary's current default
ordinary ref is eligible. Other branches, tags, secondary repositories,
Maintainer pushes, local filesystem commits and MCP operations do not trigger
this flow. Managed-worktree automatic Checks retain their existing separate
[contract](../architecture/automatic-checks.md).

## Accepted updates and execution

Before receive-pack, a proven opt-in creates a private `prepared` intent. Failure
to persist that intent prevents receive-pack from starting. The original Git
session or PAT, its account, role and actual repository access are checked again
after preparation. No credential is saved with the intent.

After receive-pack, Gitust observes the actual direct default ref under its
existing mutation guard and reads the accepted commit's policy before saving
`queued`. HTTP 200 alone is not acceptance evidence. A scheduling failure after
Git accepts an update cannot undo that push or replace its actual Git response.
This is not a durable per-request Git receipt or coordination with out-of-band
host Git writers.

There is at most one running attempt and one latest queued attempt. A newer
accepted revision replaces only the queued attempt and records the older one as
superseded. Manual and automatic Local Checks share one operation lock and the
existing executor. Current account activity, SiteAdmin role, repository access,
identity, default ref, exact SHA and policy are checked at dispatch and after
executor admission, before the first command. A new push does not cancel an
already admitted immutable execution snapshot.

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

`GET /api/v1/repos/{id}/local-check-automatic-runs` requires current primary
SiteAdmin access and returns exactly `version: 1`, `repo_id` and `runs`, bounded
to 3 MiB with `Cache-Control: no-store`. Prepared intents are private. Runs contain
`run_id`, `source_ref`, `expected_commit`, `checks_blob_oid`, `phase`, `reason`,
`history`, `created_at` and `updated_at`; no command output, credentials or host
paths are included. The order is running, queued, then up to five terminal
attempts in newest insertion order. The optional catalog hint
`can_view_local_checks_automatic` is independent of execution enablement.

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
