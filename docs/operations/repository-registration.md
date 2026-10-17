# Registering the configured local repository

Registration binds an existing repository's stable UUID to its auth-store ACL; it does not
configure another repository on a running service, clone
an upstream, change refs or enable anonymous access. Unregistered repositories retain the
existing trusted single-user sharing policy for authenticated users.

Stop the service and all old execution processes before registering. Use an existing active
SiteAdmin and the same auth database that `serve` will use. Supply the administrator password
through `GITUST_ADMIN_PASSWORD` without putting its value in shell history or logs.

```sh
gitust repository register --repo /path/to/repository --auth-db /path/to/auth-db.json \
  --admin-username operator --name project --owner-team-id GROUP_UUID --private true
```

`--private` is required. `false` permits all active authenticated users; it does not permit
anonymous API or Git access. Group UUIDs are opaque memberships, not named teams: this
version has no team creation or management service. Administrators can assign the same UUID
when inviting users with repeated `gitust admin invite --team-id GROUP_UUID` arguments, or
through the existing invitation API. New invitations accept up to 32 distinct nonnil UUIDs.
Existing memberships are not rewritten.

Restart the usual `gitust serve --repo ... --auth-db ...` command. No registration flag is
required. Registered repositories use authenticated HTTP `/mcp`; local stdio operator access
is deliberately unsupported for them. Existing unregistered stdio behavior remains available.
Supporting a registered stdio operator identity is a separate future task.

Registration holds exclusive repository service ownership through auth persistence, Git
receipt publication and revocation of the temporary administrator session. A live competing
service or unresolved execution fence blocks registration before auth changes. The auth map
stores one typed registration entry; Git stores `gitust-repository/registration.json` beside
the unchanged `identity.json`. Their version, UUID and nonce must match. The receipt is private
and hidden from task sandboxes. Existing metadata-only records are never automatically adopted.

The auth record is published and synchronized first. Its Git receipt is then published
without replacing another receipt and its directory is synchronized. A failure after rename
can have an uncertain disk outcome even though the command returns an error; do not assume
the old auth file survived. A DB-only partial registration blocks startup. Repeating the exact
registration reuses the stored nonce and metadata, reconfirms auth durability and completes
the receipt. A complete matching pending receipt can also be resumed. Malformed, mismatched
or unknown pending evidence requires stopped-writer inspection and trusted restoration;
ordinary retries never delete or silently replace it. Receipt-only and conflicting pairs
are refused, including after startup if the expected auth binding disappears or changes.

Back up and restore the entire supported repository and auth-store pair together. Registration
uses UUIDs rather than host paths, so complete offline relocation retains access. Restore
trusted anchors with their original private ownership and modes; deleting anchors is not a
repair procedure. Simultaneously deleting both anchors, or restoring a complete pre-registration
snapshot, is indistinguishable from the original legacy state. This protocol does not detect
trusted-operator deletion or complete rollback and does not coordinate independently active
filesystem copies or multiple nodes. Registered rows use a distinct internal wire shape so
older auth writers refuse them instead of silently dropping their binding.

The ten-task operating rehearsal first exercises legacy sharing, then explicitly registers
its stopped fixture before tasks 9 and 10. Those tasks verify registered restart, relocated
backup restoration, original-PAT MCP commits, fresh verification/approval and actual Git HTTP
clone/push/fetch with `fsck`. A separate active outsider must remain denied after each restart
and relocation. CI requires the public `registered_repository_acl_and_restore` result; this
does not replace the separate member-versus-outsider tests or prove an actual IDE-agent session.

## Browsing additional registered repositories

Keep the existing primary `--repo` and add an explicit `--read-only-repo PATH` for each
secondary repository. Register every secondary against the same auth database first. Up to
32 actual repositories, including the primary, may be configured. Only the primary retains
the existing legacy sharing option; every secondary must have a complete matching registration.

```sh
gitust serve --repo /path/to/primary --auth-db /path/to/auth-db.json \
  --read-only-repo /path/to/secondary
```

The service owns a separate Git engine, pinned ACL binding and strong service lock for each
repository. Duplicate common Git directories and UUIDs, missing or mismatched anchors, busy
services and unresolved execution fences refuse startup. A failed startup releases all newly
acquired ownership before exposing HTTP; secondary startup never initializes missing identity
or starts its worktrees, automatic jobs or synchronization. Change this inventory while stopped.

`GET /api/v1/repos` lists only configured repositories the current account can access, using
one auth snapshot for current memberships and pinned bindings. Metadata-only auth rows are
excluded. Each item contains `id`, `name`, `branch`, `head_commit`, `capability` and `is_default`.
The primary advertises `workbench`; secondary items advertise `browse`. An account may have
no visible default repository or an empty catalog. Reads use `/api/v1/repos/UUID/commits`,
`tree`, `blob` and `diff`, retaining the existing object-read response formats and limits.
Credentials and current ACLs are checked before and after Git I/O. Responses disable caching;
host paths are neither routing inputs nor response fields. An unavailable or inconsistent
configured binding fails instead of silently disappearing from the catalog.

Secondary browsing does not enable writes, managed worktrees, Actions, MCP/OAuth or Git
transport for those UUIDs. Existing `/api/v1/repo/*`, worktree and Git endpoints continue to
use the primary. Capability labels describe these supported operations; authorization always
comes from current server-side account, registration and team checks.

Back up all configured repositories with their shared auth database while every writer is
stopped. The existing backup utility snapshots one repository per invocation; it does not
orchestrate an atomic catalog-wide backup. Keep the snapshots and their common auth checkpoint
together. Restoring only one side of any registration still refuses startup. Complete matching
relocation preserves UUIDs; duplicate copied identities cannot be mounted together. This does
not add online repository creation/import, hot reload, registered stdio support or multi-node
ownership, and complete rollback of both anchors retains the documented legacy limitation.

Run `python3 scripts/multi-repository-rehearsal.py --binary target/debug/gitust --report /tmp/gitust-multi-repository.json`
with a new report path to exercise two synthetic repositories and a shared auth store. It
checks team filtering, UUID-routed reads against independent Git objects, rejected secondary
worktree creation, credential denial, restart and startup rollback, then removes its private
fixtures. The repository CI runs this rehearsal on Linux and macOS; its public evidence
excludes credentials and does not represent GitHub publication or a catalog-wide backup.

To prepare a new registered bare repository from an existing source without adopting or overwriting a destination, use [operator repository import](repository-import.md). It does not change the running catalog or enable a secondary workbench.
