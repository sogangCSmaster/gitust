# Import an existing repository

`gitust repository import` prepares a new **registered bare repository** from a
credential-free Git source. It does not change a running server's inventory,
create an unborn repository, expose an HTTP import endpoint, or enable a
secondary workbench. Run it as the trusted local operator, using an existing
AuthStore and an active SiteAdmin account.

The destination's parent must already exist. The destination must be absent;
even an empty directory or a symlink is not adopted. Keep the AuthStore outside
the destination and its private staging journal. Existing local source paths,
including volume names containing spaces, are passed as one canonical absolute
argument. Network URLs must not contain embedded passwords, HTTPS userinfo, query
parameters, or fragments. Do not put secrets in SOURCE or command-line flags.

```sh
# Supply the existing administrator password through GITUST_ADMIN_PASSWORD.
gitust repository import /absolute/path/to/upstream.git \
  --repo /absolute/existing/parent/imported.git \
  --auth-db /absolute/path/to/auth-db.json \
  --admin-username operator \
  --name imported \
  --owner-team-id 11111111-1111-4111-8111-111111111111 \
  --private true \
  --default-branch main
```

The explicit default branch must be an existing, non-agent branch (at most
32 KiB of UTF-8), not a guessed value or the source's symbolic HEAD. Import
reuses the inbound mirror policy: branch heads and tags are fetched, upstream
`agent/*` branches are excluded, and skipped updates are errors. It verifies the
selected commit, preserves annotated tag objects, and runs `git fsck`. It does
not copy source hooks, Gitust identities, ACL receipts, or local object
alternates through a local clone. A fresh repository UUID is fixed in the
private import intent before staging Git processes start.

A private sibling journal and a persistent destination claim serialize imports
of the same final leaf. The journal is `.gitust-import-<SHA256-of-final-leaf>`;
its `.lock` sibling is never unlinked automatically. Directories are mode 0700;
records and locks are mode 0600, bounded regular files opened without following
symlinks. The versioned intent binds the destination parent inode, leaf,
canonical source/AuthStore, requested ACL metadata and default branch. It stores
a request digest, not source URLs, passwords, session tokens, commands or PIDs.

Before the first staging `git init`, `preparation.json` binds the stage inode
and repository UUID. Once the HEAD/ref digest and filesystem checks succeed,
`prepared.json` seals that snapshot; later retries **do not fetch again**.
Registration reuses the same UUID and the existing DB/Git registration pair.
The original temporary administrator session is checked again after preparation,
before registration, and before final publication. The same service lock inode
and destination claim remain owned through publication and session revocation.
Publication uses the OS's atomic no-overwrite directory rename on the same
filesystem. It never removes an existing destination to make room.

## Failures and retries

- A failed or interrupted preparation leaves its marker. Stop **all** processes
  from that import before retrying the identical request with
  `--acknowledge-stopped-import-processes`. The flag is an operator assertion;
  Gitust does not infer process death from a PID, elapsed time or a released lock.
- After a Prepared snapshot exists, retry checks the same stage inode, UUID,
  HEAD, refs and `fsck`, then resumes the same registration pair and publication.
  Changing the source, default branch, name, team, privacy, AuthStore or target
  cannot silently start a replacement operation.
- If the DB or Git receipt was published before an error, preserve it. A retry
  recovers the matching existing nonce; it does not delete ACL rows, invent a
  new UUID, reverse a directory rename or refetch into a registered snapshot.
- A rename may have succeeded before a later directory synchronization failed
  or the process died. The identical request can confirm only the matching
  final inode, sealed snapshot and current DB/receipt pair. An unrelated final
  directory is always rejected. Later deliberate Git changes mean this is no
  longer the original sealed import; use normal repository operations instead.
- Torn, missing, unknown, symlinked or nonprivate metadata, including incomplete
  `*.pending` publication records, is **not** repaired automatically. Preserve
  the journal and original stage/final directory for trusted operator inspection;
  an identical retry or acknowledgement cannot fix arbitrary metadata damage.

Intent, preparation and registration metadata and publication directories are
synchronized. The tested crash contract concerns process interruption and
fail-closed recovery. This does **not** establish power-loss durability for all
Git objects, refs, HEAD and config: import adds no recursive Git-payload fsync
framework and has not been tested against actual power loss.

After success, add `--read-only-repo /absolute/path/to/imported.git` to a stopped
server's configuration and restart it to include the repository in the catalog.
Its team ACL applies to UUID-scoped reads. The existing primary `--repo` retains
write/MCP/Git transport responsibilities. The final repository's identity and
registration receipt plus the matching AuthStore are sufficient to serve it;
the sibling import journal is needed for retrying the original import, not for
normal startup. Back up all required repository/AuthStore pairs as described in
[repository registration](repository-registration.md). The backup utility does
not include the parent-level import journal or coordinate an unfinished import.
