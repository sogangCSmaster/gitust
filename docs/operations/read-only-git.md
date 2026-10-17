# Read-only Git for additional repositories

An explicitly configured `--read-only-repo` supports authenticated Smart HTTP
clone and fetch at `/git/UUID.git`, alongside its Files and Changes views. The
UUID selects the actual opened Git engine and pinned registration. Auth-store
metadata alone does not host a repository. Inventory changes still require a
stopped service and a valid registration against the same auth database.

```sh
git clone https://gitust.example/git/REPOSITORY-UUID.git
```

Use your account's personal access token when Git prompts for a password. The
URL and copied command contain no credentials. HTTPS is required when sending
credentials across an untrusted network. The existing primary aliases `default`
and `gitust` continue to select only the primary repository.

## Authority and limits

Only `git-upload-pack` advertisement and RPC are enabled for additional
repositories. `git-receive-pack` advertisement, RPC and native push return 404
even for a SiteAdmin. This does not add secondary worktrees, execution, Checks,
MCP, approval, synchronization or an SSH Git server. Ordinary primary push keeps
its existing permissions and managed-agent-branch restrictions.

Login sessions and PATs use the existing Git HTTP authentication policy, including
Basic PAT authentication. MCP-purpose sessions are rejected. Anonymous, revoked,
expired or inactive credentials do not grant read access. Team-only repositories
require the owning team or a SiteAdmin; the signed-in-user scope admits active
authenticated users under the existing role rules.

The original credential, original user and current registration/ACL are checked
before Git work, after request-body/ownership waiting and after Git returns.
Responses disable caching. Revocation affects later authorization checks; it
cannot retract existing clones or bytes already authorized for delivery. The
selected service and inventory owners remain alive through blocking Git work,
including after the outer HTTP future is cancelled.

Existing Smart HTTP subprocess time/output bounds and the 64 MiB request-body
limit remain in force. Unsupported services, unknown UUIDs and configured-binding
failures remain errors; failures do not fall back to the primary repository.

## Interface and verification

The catalog's optional `can_clone` boolean is a display hint, not authorization.
The compact Clone dialog appears for a secondary only when that hint is true and
states that push is disabled. An explicit false also disables the primary
control. Older servers without the hint retain the existing primary-only
fallback. Session and repository selection changes invalidate the open dialog
and late clipboard results.

Run against a freshly built binary with a new report path:

```sh
python3 scripts/read-only-git-rehearsal.py --binary target/debug/gitust --report /tmp/gitust-read-only-git.json
```

The local scenario uses distinct primary/secondary repositories, session and
Basic PAT clones, raw annotated-tag identities, an offline secondary update and
incremental fetch after restart. It checks primary refs, team/shared ACLs,
revoked/deactivated credentials, denied secondary pushes, stable registration and
strict server/client `git fsck`. Owned services and private fixtures are removed
before success is recorded. CI runs the same scenario on Linux and macOS. It
does not publish to GitHub or change a user repository.
