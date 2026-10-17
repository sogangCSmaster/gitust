# Explicit local Repository Checks

`gitust sandbox checks` runs the current ordinary branch's committed
`.gitust-checks.json` in a temporary managed checkout. It is an explicit local
operator command for a stopped repository, including bare repositories. An active
Gitust service retains exclusive ownership; stop it before using this command.

```sh
gitust sandbox checks --repo /absolute/path/to/repository \
  --expected-commit FULL_COMMIT_SHA --timeout 300

# Optionally retain the completed result for later inspection.
gitust sandbox checks --repo /absolute/path/to/repository \
  --expected-commit FULL_COMMIT_SHA --record
gitust sandbox checks-history --repo /absolute/path/to/repository
```

The repository path and full commit ID are required. The timeout defaults to 600
seconds and accepts 1–600 seconds. HEAD must name an ordinary
local branch and match that commit. Detached, unborn and managed-agent HEADs
are unsupported. The configuration and checkout are pinned to the same commit;
uncommitted files and local configuration edits stay in the original checkout.
Missing or invalid configuration is rejected before running any command.

Use the existing [Checks configuration](../architecture/worktree-verification.md):
version 1, one to eight nonempty commands, at most 4,096 bytes per command and
16 KiB for the committed configuration. The optional `auto_run` setting controls
automatic triggers; it does not disable this explicitly requested run. Commands
execute in order under the OS sandbox and shared timeout. A failing step stops
later commands, which are reported as skipped. Every configured command is a
required step; no additional placeholder command is inserted.

## Result contract

After CLI arguments parse successfully, standard output contains one JSON object
with version 1 and these fields. Syntax errors such as missing required arguments
or a nonnumeric timeout use the usual CLI usage error on standard error and exit 2.

| Field | Meaning |
| --- | --- |
| `status` | `passed`, `failed`, `rejected`, `interrupted` or `unconfirmed` |
| `reason` | Stable failure category, or `null` for a completed execution |
| `source_ref` | The selected full `refs/heads/...` name |
| `checked_commit` | The exact commit used for checkout and configuration |
| `checks_blob_oid` | The committed configuration's Git blob ID |
| `result` | Actual sandbox result and per-command output, or `null` when no execution result is available |
| `cleanup` | `not_created`, `confirmed` or `unconfirmed` |

Admission failures can leave the three Git identity fields `null`. Exit 0 means
all commands passed and cleanup was confirmed. A completed failed run returns
the command's exit status in the range 0–255 (otherwise 1). Admission rejection returns 2; interruption
returns 130; an unconfirmed execution or cleanup returns 1. Inspect the JSON status
as well as the exit code, since a check itself can return 1, 2 or 130.

Normal completion removes the temporary checkout and branch. An uncertain
execution retains its fence and requires the existing
[explicit recovery procedure](execution-recovery.md); it is never automatically
replayed. Without `--record`, the result is printed and no history is saved. This command
does not grant verification, approve a merge, publish refs, enable HTTP execution
for secondary repositories, or add an automatic ordinary-branch trigger.

## Opt-in completed-run history

`--record` retains the newest five completed runs in the selected repository's
private Git metadata. Only `passed` or `failed` executions with confirmed cleanup
are recorded. Rejected, interrupted and unconfirmed attempts are not scheduled or
replayed. This history is separate from managed-worktree Activity and server
verification records; its worktree IDs describe temporary checkouts already removed.

The opt-in response has version 2. It preserves the existing result fields and adds
`history` with `status` (`saved`, `failed` or `not_attempted`), a stable `reason`
or `null`, and `repo_id`/`run_id` when known. A storage failure preserves the actual
execution result. It changes an otherwise successful exit to 1; existing nonzero
execution exits remain nonzero. The default invocation keeps the version 1 contract.
An incomplete first checkpoint is reported as invalid storage on later reads or
writes; it is never replaced with an empty history or used to replay an execution.

Stored output is bounded separately from the live JSON: aggregate stdout/stderr
are each limited to 16 KiB and each step's stdout/stderr to 1 KiB, cut at UTF-8
boundaries. `history_truncated` identifies reduced records. This is output retention,
not secret redaction. The directory and checkpoint use private 0700/0600 permissions
and are hidden from subsequent sandbox commands. The checkpoint is capped at 3 MiB.

`sandbox checks-history --repo PATH [--run-id UUID]` reads the records in newest
append order. Its version 1 JSON contains `status`, `reason`, `repo_id` and `records`.
As with Checks execution, missing required CLI arguments are usage errors before
this JSON contract applies.
`ok` exits 0. A requested run that is absent or no longer retained returns
`not_found`, `reason: null`, an empty list and exit 2. Unavailable or invalid storage
returns `rejected` with a fixed reason and exit 2; it is never silently treated as
an empty history. An initialized repository with no history returns an empty list.

Querying does not initialize repository identity. A repository without an existing
identity must first complete a recorded run or be initialized by Gitust. Queries
retain the same stopped-service and unresolved-execution restrictions as other
local operators. Existing offline backup/restore includes the history and repository
identity; relocated records retain their run IDs, commit/blob IDs and observations.
