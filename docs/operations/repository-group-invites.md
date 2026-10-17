# Inviting users to a repository's owning group

An active SiteAdmin can open **Invite to owning group** from a registered
repository's access settings. The server derives the owning group from the actual
configured registration. A new account receives that group membership when it
redeems the single-use link, which expires after 72 hours.

This grants membership in an opaque group UUID, not access to one repository
alone. Other private repositories owned by the same group can also become
accessible. Existing site-wide role rules still apply: a SiteAdmin role has
administrator authority across the installation. Developer is the form's default.
The operation does not edit existing members, create a named team, change the
repository's owner or enable secondary push, worktrees, execution or MCP.

The global Access invitation form retains its account-only behavior. CLI/API
invitations can still specify their existing group lists and custom TTLs. The
invitation list exposes actual group metadata; a missing field from an older
server is unknown scope, not evidence of an empty grant.

## HTTP contract

```text
POST /api/v1/repos/{id}/group-invites
Authorization: Bearer <original login session or personal access token>
Content-Type: application/json

{"invite_id":"<new nonnil UUID>","target_email":"member@company.internal","assigned_role":"Developer"}
```

The strict body is limited to 4 KiB. It does not accept a client-selected group,
registration nonce, TTL or filesystem path. Only configured registered primary
or secondary repositories are eligible; metadata-only records and legacy services
without a pinned registration do not qualify. Responses use `Cache-Control:
no-store`, including failures.

The original credential, original user, current active SiteAdmin and exact pinned
registration are checked inside the AuthStore write transaction after the OS
lock wait and disk reload. MCP-purpose credentials cannot use this route.
The original-credential check also protects the existing HTTP invitation-family
create, password-reset and invite-revocation mutations across that wait. The
same transaction-bound credential check covers HTTP PAT creation/revocation,
SSH-key registration/revocation, administrator account deactivation, metadata-only
repository creation and registered repository renaming. Current ownership, role,
expiry, token-purpose and last-administrator checks remain in force. Trusted CLI
principal entry points and their existing TTL/reset semantics remain available.

A first confirmed durable creation returns 201 with `repository_id`,
`owner_team_id`, public `invite` metadata and `invite_url_path`. The one-use link
appears once. The API does not return a redundant raw-token field. Only its hash
is stored; no token is derived from the request UUID. Public metadata includes
`team_ids` and optional `repository_id`, never a registration nonce or token hash.

## Retry and uncertain outcomes

Generate one request UUID for each explicit issuance intent. Retrying that intent
must use the same UUID, email, role and repository. The server compares the exact
registration origin, original creator, normalized email, role, purpose and sole
owning group. A key used for different semantics returns 409
`invite_request_conflict` without exposing the unrelated invitation.

A confirmed same-key replay returns 409 `invite_already_issued` with public
metadata and no link. It does not mint another credential, reset the 72-hour
deadline or change consumption/revocation state. This remains true after restart
and after the invitation is used, revoked or expired. The original link cannot
be reconstructed from its hash.

Persistence failure before publication returns 503 `invite_not_published`;
an outcome that cannot be confirmed after publication was attempted returns 503
`invite_unconfirmed`. Neither returns a link. Do not assume the record is absent
after an uncertain write. A same-key replay reconfirms durability before returning
metadata. Failed storage does not become successful replay.

The form retains the current intent for an explicit retry after a transient
failure or closing/reopening in the same session and repository selection. It
does not automatically issue a fresh key. Session/selection changes clear the
displayed link and stale results; no link is stored in browser storage. If the
first successful response was lost, inspect the invitation in Access and revoke
it there if appropriate. **New invitation** explicitly starts another intent;
it does not automatically revoke an earlier potentially delivered link.

The guarantee is per stored request key. Restoring a complete checkpoint from
before issuance cannot preserve knowledge of that later key. Older writers may
discard optional origin metadata; a subsequent group-route replay then conflicts
rather than reminting a credential. Keep installation versions and backups
consistent when using this feature.

## Local verification

With a freshly built binary and a new report path:

```sh
python3 scripts/repository-group-invite-rehearsal.py --binary target/debug/gitust --report /tmp/gitust-group-invite.json
```

The scenario configures three real repositories: a primary and secondary sharing
group A, and another secondary owned by B. It checks concurrent same-key issuance,
hash-only storage, the original expiry, replay/conflicts and restart. A redeemed
Developer sees exactly the A pair, while an account-only Developer sees neither
private group. Native Git verifies secondary clone/fetch and annotated tags;
secondary push/worktree creation, outside-group reads, revoked administrator
credentials and inactive-member fetch remain denied. Consumed/revoked replays
preserve the existing invitation. Git refs/registration anchors and strict
server/client `fsck` are checked independently. Private fixtures and owned
services are removed before success is recorded. It publishes nothing to GitHub.
