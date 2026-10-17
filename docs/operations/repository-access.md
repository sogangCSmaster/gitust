# Registered repository access scope

A SiteAdmin can change a configured, explicitly registered repository between
**Team only** and **Signed-in users** from its Overview. This changes the existing
`is_private` ACL metadata. Repository UUID, owning team, registration binding,
filesystem paths, Git refs, task targets and approval records remain unchanged.
The primary and additional browse-only repositories support this metadata change;
additional repositories do not gain Git transport, worktrees or execution.

- **Team only** (`is_private: true`) permits the owning team and SiteAdmins.
- **Signed-in users** (`is_private: false`) permits all active authenticated users,
  including users outside the owning team. Existing role and task-ownership
  permissions still apply. This is not a read-only sharing mode.
- Anonymous access remains disabled in both scopes. Changing scope affects later
  authorization checks; it does not erase existing clones or cancel responses
  already authorized before the change.

There is no team transfer, anonymous publication, account activation or running
inventory update in this operation. Metadata-only ACL rows, unknown or unconfigured
repositories and legacy services without a registration binding cannot be changed.

## HTTP contract

```text
PATCH /api/v1/repos/{id}/visibility
Authorization: Bearer <original login session or personal access token>
Content-Type: application/json

{"expected_is_private":true,"is_private":false}
```

The body is limited to 4 KiB and accepts exactly those two boolean fields. On
success the server returns HTTP 200 with `{"id":"<uuid>","is_private":false}`
and `Cache-Control: no-store`. The response identifies the exact repository and
confirmed value; it does not grant a new login session or change anyone's role.

The original credential, active account, current SiteAdmin role, pinned
registration binding and expected current privacy value are checked together
under the AuthStore write transaction, after request-body reading and lock waiting.
An MCP-purpose session is not accepted as a general login credential. The server
does not treat a frontend capability hint or an earlier authenticated principal
as sufficient authority for this write.

A mismatched current value returns HTTP 409, including after an earlier uncertain
write. The server never silently substitutes a new expectation. A matching no-op
still confirms persistence. Normal credential and role denials remain HTTP
401/403; unhosted targets are 404, malformed bodies are 400 and oversized bodies
are 413.

Storage and registration failures return a sanitized HTTP 503. A failure before
publication is distinguished from an **unconfirmed outcome** after publication was
attempted. In the latter case, rename may already have changed the on-disk ACL even
though directory synchronization or stamp publication failed. Do not assume that
the previous scope was restored. Read the current catalog value and explicitly
retry with that observed expectation; the UI provides refresh rather than an
automatic write retry. Existing persistence callers keep their prior error API.

The catalog optionally supplies `is_private` and `can_change_visibility`. The first
is present only for known registered metadata; the second is a current-account
display hint. Actual mutation authorization stays on the server. Older servers,
demo data and legacy repositories do not get an invented privacy value or an
unsupported change control.

## Interface and verification

The compact Overview line displays the access scope. The existing dialog and
select primitives load the form on demand. The form states the audience and role
effects before submission. It changes the displayed value only after an exact
repository/value response, binds asynchronous work to the original session and
selection, and preserves the explicit Files/Changes snapshot. Conflicts and
unconfirmed outcomes require refresh; denied authority withdraws the stale control.

Run the actual local operation and stopped-restoration scenario with a freshly
built binary:

```sh
python3 scripts/repository-visibility-rehearsal.py --binary target/debug/gitust --report /tmp/gitust-visibility.json
```

The scenario uses two temporary repositories and separate team/outsider accounts.
It exercises grant, revoke, strict conflicts, anonymous and role denial, real Git
clone/fetch authorization, unchanged task/approval metadata and relocated recovery
with the original installation unavailable. Restored passing diagnostics require
fresh verification before approval. It creates no external GitHub publication and
removes its owned services and private fixtures before writing a success report.
