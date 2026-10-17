# Create a registered empty repository

`gitust repository create` prepares a fresh bare repository and registers it with an
existing auth database. It creates no source commit, worktree, Check or approval.
The requested default branch remains unborn until a Git client pushes a commit.

Run this as a stopped-installation operator step. The destination's parent must
already exist and the destination must be absent. The command never adopts an
existing directory or symlink. The auth database must already exist outside the
destination and its private journal. Authenticate as an active SiteAdmin; account
bootstrap remains the separate `gitust admin create` command.

Supply `GITUST_ADMIN_PASSWORD` through your execution-time secret handling rather
than a command-line password. Replace the paths, administrator name and team UUID:

```sh
gitust repository create \
  --repo /srv/gitust/repos/project.git \
  --auth-db /srv/gitust/auth/auth-db.json \
  --admin-username operator \
  --name project \
  --owner-team-id TEAM_UUID \
  --private true \
  --default-branch main
```

The explicit default branch must be a normal non-agent branch. The display name
uses the existing repository-name policy: after trimming, 1–100 ASCII letters,
digits, `-`, `_` or `.` characters, without leading/trailing dots or `..`. Invalid names fail before
login or a destination claim. The display name does not determine the host path
or Git URL. The command allocates one UUID in its
private intent and completes the matching Git/auth registration pair before
publishing the directory with a no-overwrite rename. It rechecks its original
administrator session after preparation and before registration/publication.

## Serve and make the first commit

Configure the result explicitly as the primary `--repo`, or as a registered
`--read-only-repo` for browsing. Restart the service to change its inventory; create
does not edit a running server's configuration. Primary Git transport uses the same
registered UUID before and after its first commit. Secondary repositories continue
to lack Git transport and worktree writes.

The catalog and primary summary show the actual default branch with
`head_commit: null`. An authorized Git client can clone the primary, commit locally
and push through the existing Smart HTTP endpoint. Use the Gitust username and a
personal access token when the Git client requests credentials; do not embed the
token in its remote URL. Ordinary push permissions remain role-dependent.

The primary repository's Overview offers a compact **Clone** dialog with its
credential-free UUID URL. It is absent for browse-only repositories. Copying is
optional; the read-only URL can also be selected manually.

A push to another branch does not retarget the default HEAD. That branch can be
read explicitly or used as a worktree base. Refresh the web view after the first
default-branch push to load its actual commit and files. Merely creating a repository
or accepting an ordinary push does not grant Verified or human approval authority.

## Interrupted operations and retries

Create and import share the destination claim and journal admission namespace
(`.gitust-import-<destination-digest>` beside the destination). Create intents have
their own version/operation and request digest; an import intent cannot be adopted
as a create retry or vice versa. Keep the journal and claim file; do not remove the
lock file to bypass another owner.

An identical retry before the repository changes retains the original UUID,
registration binding and sealed snapshot. The snapshot must remain bare, have the
exact unborn default HEAD and contain no refs. A changed request, replaced stage,
different HEAD, new ref or inconsistent registration is refused. After the first
push, the original create request is deliberately no longer an "ensure exists"
operation: it must not reset the now-populated repository to empty.

If preparation was interrupted, stop all previous create processes and their
children before adding `--acknowledge-stopped-create-processes` to the identical
request. This is an operator assertion; the command does not infer process death
from a PID or time limit. Torn, missing, symlinked or nonprivate journal records
require trusted inspection and are not automatically repaired.

If directory publication may have succeeded before a synchronization error, retain
the final directory and journal. Do not roll it back, delete its auth record or
generate a replacement UUID. Inspect and retry the same operation; a mismatched
state remains an error. The contract covers process interruption and conservative
retry, without claiming arbitrary power-loss durability for every Git payload.

## Operating rehearsal

Run `python3 scripts/empty-repository-rehearsal.py --binary target/debug/gitust --report /tmp/gitust-empty-repository.json`
with a fresh report path. It uses two disposable CLI-created repositories and a
private auth store, checks identical retries, restores an empty registration while
the original repository/auth file are unavailable, performs real HTTP clone,
alternate/default pushes and fetch, rejects an obsolete retry after commits exist,
restarts the service and checks Git integrity. Fixtures are removed; the report
contains only source/binary/platform/result fields. Repository CI executes the
rehearsal on Linux and macOS. No external GitHub publication is performed.
