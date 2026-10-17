# Git hosting for additional repositories

Configure an additional registered bare repository for ordinary Git pushes with
`--git-write-repo`. This is an explicit stopped-service setting. Existing
`--read-only-repo` entries keep their clone/fetch-only behavior.

```sh
gitust serve --repo /srv/gitust/primary \
  --auth-db /srv/gitust/private/auth-db.json \
  --read-only-repo /srv/gitust/reference.git \
  --git-write-repo /srv/gitust/project.git
```

Register each additional repository against the same auth database before starting
the service. The combined inventory is limited to 32 repositories, including the
primary. Duplicate Git common directories or repository identities are refused,
including a path supplied through both secondary flags. A writable secondary must
be bare, have a complete matching registration, and have no linked or managed
worktrees or unresolved execution fence. Invalid or busy entries fail startup
before HTTP is exposed. Change modes while stopped; arbitrary concurrent host Git
writers must be stopped or coordinated by the operator.

The repository's stable UUID selects `/git/UUID.git`. An active Maintainer or
SiteAdmin with current repository access may push ordinary branches and tags.
Developers may clone/fetch but cannot push to these additional repositories.
Anonymous clients, MCP credentials and users outside the repository's current
access scope remain denied. Agent-branch updates are rejected for every role;
one agent update in a mixed request rejects the request before Git runs. Existing
symbolic destination refs are rejected on primary and secondary receives, so an
ordinary-looking name cannot redirect a push into another protected ref.

Each writable secondary retains its own service ownership and mutation lock. The
server checks the original session or PAT and the current account, role and pinned
registration after body streaming and again before executing Git. Receive-pack
advertisements check the original credential before and after Git work. Cancellation
of an HTTP waiter does not release the target's guards while blocking Git work
continues.

Pushes use the existing bounded Smart HTTP transport, disabled hooks, sanitized
Git environment and object checks. Native Git's non-fast-forward and current-branch
deletion rules still apply. An HTTP success can contain rejected refs; an interrupted
response can follow an accepted ref update. Inspect the Git client's result and
fetch current refs before retrying. No automatic rollback or whole-request
transaction is promised.

The catalog retains `capability: "browse"` for these repositories and adds the
optional boolean `can_push`. This is the current caller's ordinary-push hint, not
authorization for a later request. Missing hints default to false. The web shows
**Git hosting** when the hint permits ordinary pushes and **Read only** otherwise;
Clone explains the caller's current scope. A primary-repository Developer can
still use the separate owned-agent-branch workflow when `can_push` is false.

Git hosting on a secondary does not start its managed worktrees, automatic Checks,
MCP execution, synchronization or approval/publication workflow. Those operations
continue to use the configured primary. UUID and registration anchors remain
stable across restart; the startup flag determines whether ordinary pushes are
enabled for that service instance.

Include every configured repository and the shared auth database in a stopped
[backup](backup-restore.md#include-configured-read-only-repositories). The backup
tool's existing `--read-only-repo` option identifies additional paths to include;
it does not record or grant a service's write mode. Record the service command
privately and explicitly select each restored path's mode when restarting.

Run the disposable three-repository scenario against a freshly built binary:

```sh
python3 scripts/secondary-git-write-rehearsal.py --binary target/debug/gitust --report /tmp/gitust-secondary-git-write.json
```

Choose an absent report path. It exercises native session/PAT clone, commit, push
and fetch; permission and agent-ref denials; restart and read-only mode fallback;
primary worktree/job preservation; raw annotated tags; and server/client `git fsck`.
It removes its owned services and private fixtures before recording success. This
scenario does not publish to GitHub or change an existing user repository.
