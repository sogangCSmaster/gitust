# Read-only SSH Git hosting

Gitust can serve native SSH clone/fetch for the primary and explicitly configured
secondary repositories. Enable it with `gitust serve --ssh-bind HOST:PORT`.
SSH is otherwise disabled. HTTP remains available on its separate `--bind` address.

This first transport accepts **plain Ed25519 user keys**, the SSH username `git`,
and one `git-upload-pack` request per connection. Register the public key in account
settings or `POST /api/v1/auth/ssh-keys`. RSA/ECDSA registration remains supported by
the account API, but those keys cannot authenticate to this SSH transport yet.
Passwords, OpenSSH certificates, security-key variants, interactive shells, PTYs,
forwarding, SFTP and `git-receive-pack` are not supported. Use authenticated Smart
HTTP for pushes; SSH reads do not trigger Checks or grant approval/publication.

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

## Clone and fetch

On the client, ensure `~/.ssh` exists with mode `0700` and use a separate Ed25519 user key. If one is needed, run
`ssh-keygen -t ed25519 -f ~/.ssh/gitust_ed25519` and choose a passphrase; preserve
existing keys instead of overwriting them. Register **only** the contents of
`~/.ssh/gitust_ed25519.pub` in your Gitust account. The private file stays on the
client, and the server's `host_key` must never be used as this user key.
For a non-default key, add an entry to the client's `~/.ssh/config`:

```sshconfig
Host gitust.example
    IdentityFile ~/.ssh/gitust_ed25519
    IdentitiesOnly yes
```

Use the configured repository's canonical UUID from `GET /api/v1/repos`:

```sh
git clone ssh://git@gitust.example:2222/REPOSITORY_UUID.git project
git -C project fetch origin
```

The selected repository must be opened by the server (`--repo`, `--read-only-repo`
or `--git-write-repo`) and accessible to the current key owner. An auth database
record alone does not expose a repository. Private repositories require current
group membership or SiteAdmin access; all-user repositories still require an
active authenticated account. The original key registration, account and actual
repository binding/ACL are checked again immediately before native Git starts.
Revoking and re-registering the same public key does not renew an existing session.
Already streamed bytes cannot be recalled, and this version does not promise
mid-transfer revocation. SSH advertises the same read refs as Smart HTTP, hiding
`refs/remotes/`; it is not a new per-branch confidentiality boundary.

## Limits, recovery and evidence

Each listener admits eight connections, with a 15-second authentication deadline,
120-second total connection ceiling, 30-second inactivity timeout and one channel.
Native upload-pack has the existing 60-second transport deadline, 64 MiB input and
output caps, bounded pumps and a 64 KiB stderr cap. These are protocol/application
limits, not an overall process-memory or blocking-filesystem latency guarantee.
Native errors are redacted. EOF closes Git stdin while allowing output to finish;
disconnect requests cancellation. Ownership survives until child/pump cleanup, or
remains quarantined if cleanup cannot be confirmed. This does not add a sandbox
for arbitrary remote commands or claim automatic recovery from host process death.

The [stopped backup tool](backup-restore.md) includes this host key and its private
modes because they are inside the common Git directory. Treat the backup as secret
operator data. Restore into a fresh installation, preserve the same primary and
configured repository inventory, verify the original fingerprint and test clone/
fetch before switching clients. Outbound SSH credentials remain separate.

`scripts/ssh-read-rehearsal.py` exercises local native OpenSSH/Git clone and fetch,
exact refs/trees and annotated tags, ordinary authentication/ACL denial, unsupported
pushes, and stopped two-repository relocation with the original pinned host key.
It uses disposable local keys and repositories; it does not publish to GitHub.
