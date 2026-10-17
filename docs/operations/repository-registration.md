# Registering the configured local repository

Each service still serves one configured Git repository. Registration binds that existing
repository's stable UUID to its auth-store ACL; it does not add another repository, clone
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
