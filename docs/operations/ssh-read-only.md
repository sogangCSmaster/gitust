# SSH Git hosting

Gitust can serve native SSH clone/fetch (`git-upload-pack`) and permission-checked pushes
(`git-receive-pack`) for the primary and explicitly configured secondary repositories.
Enable it with `gitust serve --ssh-bind HOST:PORT`. SSH is otherwise disabled. HTTP remains
available on its separate `--bind` address, and `GET /api/v1/repos` exposes the active
non-zero `ssh_port` so the web Clone popover can offer an `ssh://git@<host>:<ssh_port>/<repo_id>.git`
address alongside Smart HTTP.

The SSH transport accepts **Ed25519 (`ssh-ed25519`) and ECDSA NIST P-256/384/521
(`ecdsa-sha2-nistp256`, `ecdsa-sha2-nistp384`, `ecdsa-sha2-nistp521`) user keys**, the SSH
username `git`, and one `git-upload-pack` or `git-receive-pack` request per connection.
Register the public key in account settings or `POST /api/v1/auth/ssh-keys`. RSA key
registration remains supported by the account API, but RSA keys are not accepted by this
SSH transport yet. Passwords, OpenSSH certificates, security-key variants, interactive
shells, PTYs, forwarding and SFTP are not supported.

## Configure the server identity

Stop Gitust and other writers. Create a dedicated host key in the **primary**
repository's common Git directory. Keep it distinct from a user's authentication
key and any outbound GitHub key. For an existing installation, preserve the current
host key instead of generating another one.

```sh
common=$(git -C /srv/gitust/project rev-parse --path-format=absolute --git-common-dir)
mkdir -m 700 "$common/gitust-ssh"
ssh-keygen -q -t ed25519 -N '' -C '' -f "$common/gitust-ssh/host_key"
chmod 600 "$common/gitust-ssh/host_key"
ssh-keygen -l -E sha256 -f "$common/gitust-ssh/host_key.pub"

gitust serve --repo /srv/gitust/project \
  --auth-db /srv/gitust/private/auth-db.json \
  --bind 127.0.0.1:3000 --ssh-bind 127.0.0.1:2222
```

The directory must be owned by the server account with mode `0700`; the regular,
single-link private key must have mode `0600`. The loader accepts a bounded,
unencrypted OpenSSH Ed25519 key from this fixed location. It does not create,
replace or search for another key. An unavailable or invalid key prevents enabled
SSH startup. The complete `gitust-ssh` directory is hidden by the shared sandbox
policy, including local CLI execution without an auth database.

Distribute the host fingerprint through a trusted channel and verify it before
accepting the host in `known_hosts`. Do not disable host verification. The example
binds to loopback; external bind addresses, firewall access and the advertised host
name are explicit operator configuration.

## Clone, fetch and push

On the client, ensure `~/.ssh` exists with mode `0700` and use a separate Ed25519 or ECDSA
user key. If one is needed, run `ssh-keygen -t ed25519 -f ~/.ssh/gitust_ed25519` (or
`ssh-keygen -t ecdsa -b 256 -f ~/.ssh/gitust_ecdsa`) and choose a passphrase; preserve
existing keys instead of overwriting them. Register **only** the contents of the `.pub`
file in your Gitust account. The private file stays on the client, and the server's
`host_key` must never be used as this user key.
For a non-default key, add an entry to the client's `~/.ssh/config`:

```sshconfig
Host gitust.example
    IdentityFile ~/.ssh/gitust_ed25519
    IdentitiesOnly yes
```

Use the configured repository's canonical UUID from `GET /api/v1/repos` or the web
Clone popover:

```sh
git clone ssh://git@gitust.example:2222/REPOSITORY_UUID.git project
git -C project fetch origin
git -C project push origin HEAD:refs/heads/agent/my-task
```

The selected repository must be opened by the server (`--repo`, `--read-only-repo`
or `--git-write-repo`) and accessible to the current key owner. An auth database
record alone does not expose a repository. Private repositories require current
group membership or SiteAdmin access; all-user repositories still require an
active authenticated account.

For `git-receive-pack` pushes:

-Repositories configured with `--read-only-repo` reject `git-receive-pack` for every role.
-Repositories configured with `--git-write-repo` require a bare secondary repository without
 linked/managed worktrees, and allow only current Maintainers and SiteAdmins with repository
 access to push ordinary branches (`refs/heads/*` outside `refs/heads/agent/*`) and tags
 (`refs/tags/*`).
-On the primary `--repo`, Developers may push only non-deleting updates to `refs/heads/agent/*`
 (and must own any tracked managed worktree for that branch), whereas Maintainers and SiteAdmins
 may also push ordinary branches and tags. Pushes to tracked branches in `Compiling`, `Merged`,
 or `Discarded` state are rejected, and accepted SiteAdmin pushes participate in the same
 opt-in managed-worktree and primary default-branch automatic Checks preparation/reconciliation
 as Smart HTTP.

The original key registration (`key_id`), account, role and repository binding/ACL are
re-verified after waiting for the repository mutation lock, before spawning `git receive-pack`,
and after parsing the client's ref-update pkt-lines (before any command or packfile bytes reach
`git receive-pack` stdin). Revoking and re-registering the same public key does not renew an
existing session. SSH advertises the same refs as Smart HTTP, hiding `refs/remotes/`.

## Limits, recovery and evidence

Each listener admits eight connections, with a 15-second authentication deadline,
120-second total connection ceiling, 30-second inactivity timeout and one channel.
Native `upload-pack` and `receive-pack` share the 60-second transport deadline, 64 MiB
input and output caps, 256 KiB receive-pack command header cap, bounded pumps and a
64 KiB stderr cap. These are protocol/application limits, not an overall process-memory
or blocking-filesystem latency guarantee. Native errors are redacted. EOF closes Git
stdin while allowing output to finish; disconnect requests cancellation. Ownership
survives until child/pump cleanup and post-receive automatic Checks reconciliation/rejection,
or remains quarantined if cleanup cannot be confirmed.

The [stopped backup tool](backup-restore.md) includes this host key and its private
modes because they are inside the common Git directory. Treat the backup as secret
operator data. Restore into a fresh installation, preserve the same primary and
configured repository inventory, verify the original fingerprint and test clone/
fetch before switching clients. Outbound SSH credentials remain separate.

`scripts/ssh-read-rehearsal.py` exercises local native OpenSSH/Git clone, fetch and
authenticated push with Ed25519 and ECDSA P-256 keys, catalog `ssh_port` discovery,
exact refs/trees and annotated tags, ordinary authentication/ACL/RBAC denial, and
stopped two-repository relocation with the original pinned host key. It uses disposable
local keys and repositories; it does not publish to GitHub.
