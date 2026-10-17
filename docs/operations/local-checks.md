# Explicit local Repository Checks

`gitust sandbox checks` runs the current ordinary branch's committed
`.gitust-checks.json` in a temporary managed checkout. It is an explicit local
operator command for a stopped repository, including bare repositories. An active
Gitust service retains exclusive ownership; stop it before using this command.

```sh
gitust sandbox checks --repo /absolute/path/to/repository \
  --expected-commit FULL_COMMIT_SHA --timeout 300
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
replayed. The result is printed, not saved to server run history. This command
does not grant verification, approve a merge, publish refs, enable HTTP execution
for secondary repositories, or add an automatic ordinary-branch trigger.
