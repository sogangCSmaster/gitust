# Manage an existing account's group access

Use **Manage existing member** in a registered repository's access settings to
find an existing account by its exact username, then add it to or remove it from
the repository's owning group. Only an active SiteAdmin can use this operation.
The server derives the group from the configured registration; the client cannot
select a different group or supply a host path.

Membership affects every private repository owned by that group. Removing it
does not deactivate the account, revoke its credentials, change its site-wide
role or remove other groups. An active SiteAdmin still has administrative access, and a
repository open to all active signed-in accounts can remain accessible without
group membership. Secondary repositories retain their read-only capabilities.

An active account can be added. Suspended accounts can be inspected and removed
for cleanup, but cannot be granted new membership. Use the separate
[owning-group invitation](repository-group-invites.md) for a person who does not
yet have an account. This operation does not create accounts or named teams,
transfer ownership, list the whole group or edit site-wide roles.

## HTTP contract

The lookup uses one exact username rather than an account search or one request
per member. It returns only the selected account's membership in this group.

```text
GET /api/v1/repos/{id}/group-members?username=member
Authorization: Bearer <original login session or personal access token>

PATCH /api/v1/repos/{id}/group-members/{user_id}
Authorization: Bearer <original login session or personal access token>
Content-Type: application/json

{"expected_membership_version":"0","is_member":true}
```

Both successful responses contain exactly `repository_id`, `owner_team_id`,
`user_id`, `username`, `role`, `status`, `is_member` and `membership_version`.
The version is a canonical decimal string, preserving the full unsigned 64-bit
range. Clients must carry it unchanged, without converting it to a JavaScript
number. No full group list, password hash, registration nonce or credential is
returned. Every response uses `Cache-Control: no-store`.

The change body is strict and limited to 4 KiB. The original credential and user,
current active SiteAdmin and exact registered repository binding are checked
again inside the durable auth transaction. MCP-purpose tokens, legacy unbound
repositories and records outside the configured inventory cannot use this API.

## Conditional changes and recovery

A change compares the selected user's current membership version before doing
anything, including a no-op. Only an actual membership change increments it.
The version applies across that user's groups: a simultaneous change through
another repository can make a previously read version stale.

For example, adding a user changes version 0 to 1; removing that grant changes
it to 2. Replaying the original version-0 addition then returns 409
`membership_conflict`. It cannot restore a grant that an administrator has since
removed. A successful response that was lost is handled by reading current state
again, not by automatically applying the old action with a newer version.

Lookup or mutation of an unknown account returns 404 `group_member_not_found`;
this does not mean the repository disappeared. A repository outside the hosted
inventory returns 404 `repository_not_hosted`. Adding to an account already at
the 32-group limit returns 409 `membership_limit`. Legacy accounts over the limit
can still be read and have groups removed. Version exhaustion returns 409
`membership_version_exhausted` without a change. An inactive add target returns
403 `group_member_inactive`.

Storage failures before publication return 503 `membership_not_published`;
uncertain publication returns 503 `membership_unconfirmed`. Neither is success.
Refresh the selected account's membership before making another explicit
decision. The UI discards stale account, session and repository responses and
never treats optimistic display as evidence of an access grant.

## Stored-state compatibility

Unchanged legacy user records retain their flat `team_ids` representation and
start at membership version zero. After the first change, the stored user uses
a strict nested `membership` record containing its groups and positive version.
The legacy required `team_ids` field is absent from that record, causing older
writers to refuse the database rather than silently erase the version.

Use the updated binary for every server and CLI process sharing that auth store
after membership management has been used. Mixed or invalid membership records
are rejected. A compatible newer reader can still restore an older flat snapshot;
restoring a complete backup also restores its older membership state and version.
Conditional requests do not provide protection against deliberate full-backup
rollback, and a restored installation requires fresh client state.

## Verification

The existing onboarding rehearsal exercises both new-account invitations and
existing-account membership:

```sh
python3 scripts/repository-group-invite-rehearsal.py --binary target/debug/gitust --report /tmp/gitust-groups.json
```

Its disposable account initially belongs to group B. Adding group A exposes the
two A repositories while retaining B; removal through the other A repository
returns it to B alone. The scenario checks concurrent updates, stale replay,
restart and no-op versions, actual clone/fetch with an annotated tag, immediate
session/PAT access denial after removal, continued secondary write denial and
unchanged profiles, Git refs and registration anchors. Private fixtures are
removed before success is reported. Nothing is published to GitHub.
