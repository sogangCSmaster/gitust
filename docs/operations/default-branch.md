# Default branch management

A SiteAdmin can change the default branch from the branch line in a configured,
registered **bare primary or `--git-write-repo` secondary** repository's Overview. Select an existing committed
branch, then confirm the change. The small dialog uses the current branch and
selected commit as preconditions. Additional read-only repositories, normal working
repositories, legacy unregistered services and non-SiteAdmins have no management
capability. The server enforces these rules independently of the UI hint.
Maintainers' ordinary push permission does not include default-branch management.

The operation changes only symbolic `HEAD`. It does not create, rename, delete or
move a branch/tag, or rewrite existing worktree origins, Checks, history or approval
targets. A current unborn default may be replaced with an existing committed branch.
Agent branches, detached HEAD, indirect targets and branch names incompatible with
the existing worktree-base policy are refused.

New Git clones use the changed default. On the primary, HTTP/MCP worktree creation that omits its
base resolves the current committed symbolic default under the repository mutation
guard. Explicit bases remain explicit, and existing worktrees retain their original
merge target. There is no implicit `main` fallback for detached or unborn defaults.
The local CLI's explicit default arguments retain their existing semantics.

## API and comparison boundary

`PATCH /api/v1/repos/{id}/default-branch` accepts only:

```json
{
  "target_ref": "refs/heads/develop",
  "expected_current_ref": "refs/heads/main",
  "expected_target_commit": "FULL_SELECTED_COMMIT_SHA"
}
```

The response contains `id`, the short `branch` and `head_commit`, with no-store
headers. Request bodies are bounded to 256 KiB. Full head-ref names retain the
existing 32 KiB branch-name limit; the larger body permits escaped JSON names.
Invalid input is rejected, stale current-ref or target-commit preconditions conflict,
and unsupported repository layouts are refused.

The server resolves the requested UUID and retains that repository's mutation and
service ownership guards. Writable secondaries must still satisfy their bare,
worktree-free hosting contract. It rechecks
the original session/PAT, active SiteAdmin role and current registered repository
access after body reading, validation and HEAD-lock waiting. The current symbolic
target is read and compared **after acquiring `HEAD.lock`**, without following a
filesystem symlink or accepting an oversized, nonregular or multiply linked file.
The selected direct target is checked again immediately before publication. Its
OID comparison is serialized with Gitust writes; it is not an atomic target-tip
guarantee against an independent external Git writer. Coordinate such writers.

Only the new `HEAD` file is atomically renamed, with file and directory
synchronization. Failure after rename has an uncertain outcome: refresh the catalog
and inspect actual Git HEAD before retrying. A failure is not represented as a
successful change or silently undone. This operation does not establish a broader
Git payload power-loss guarantee.

The UI validates response identity and selected SHA, keeps explicit Files/Changes
snapshots pinned, and scopes late responses to their initiating session, repository
and dialog epoch. A conflicting request retains its inputs for review; refresh
current metadata before making another decision. A capability flag is a display
hint, never authorization.

## Rehearsal

```sh
python3 scripts/default-branch-rehearsal.py \
  --binary target/debug/gitust --report /tmp/gitust-default-branch.json
```

The operator creates three disposable registered bare repositories and uses actual
HTTP MCP, the OS sandbox and a separate Git Smart HTTP client. It checks denied and
stale requests, all three repositories' Git-state isolation, original worktree approval
to `main`, a new omitted-base checkout at `develop`, restart, HEAD advertisement,
fresh clone contents, fetch and strict `fsck`. The writable secondary changes to
its independent `release` branch while primary tasks and raw annotated tags remain
unchanged; ordinary push permissions alone cannot make that change. Services and private fixtures are
removed before its private success report is created. It never publishes to GitHub.
The required repository CI step runs this rehearsal on Linux and macOS; a green
historical run does not establish coverage of a later candidate.
