# Configured repository identity

Each server still serves one configured Git repository. Its public UUID is stored in
`<common-git-dir>/gitust-repository/identity.json`; linked worktrees use that same common
directory. The repository summary, recovered tasks and synchronization configuration
share this ID across process restarts. No host path is added to the summary API.

An entirely absent `gitust-repository` directory is treated as a legacy first start.
Initialization holds repository service ownership and serializes cloned engine callers.
It creates a private directory, synchronizes it, then publishes a versioned record through
an exclusive staging file and an atomic rename that cannot replace an existing identity.
Directory and file synchronization failures stop initialization.

The directory must have mode `0700`; the regular file must have mode `0600`, the current
effective owner and exactly one hard link. Reads are bounded to 4 KiB and reject symlinks,
FIFOs, malformed or unknown metadata, a nil UUID and unsupported versions. An existing
directory with a missing or invalid record is an error, including after an interrupted
first initialization. Startup never repairs it by inventing another UUID. Current atomic
publication supports macOS and Linux; unsupported platforms fail explicitly.

## Recovery and scope

Stop all writers before restoring trusted metadata with its original permissions and
ownership. Preserve the entire common Git directory through the supported
[offline backup and restore procedure](../operations/backup-restore.md). The identity
directory is hidden from task execution and does not contain credentials.

A complete backup, filesystem copy or relocation intentionally retains the UUID. A normal
`git clone` does not transfer this private metadata and receives a new ID when first
managed. Independently active filesystem copies are not coordinated: this is not a global
repository registry or a multi-node identity protocol. Complete deletion of the identity
directory is indistinguishable from a legacy repository and causes new initialization;
do not use directory deletion as a repair procedure. Old transient server IDs have no
compatibility aliases.

The UUID is an identifier, not authorization. Metadata-only auth records never bind a Git
repository to an ACL: matching UUIDs still collide. Explicit offline registration pairs a
private Git receipt with a typed auth record using the same registration nonce. Only an
exact pair enables registered access; neither anchor and no matching auth metadata retain
legacy sharing. Partial, unsafe or conflicting evidence fails startup. The server pins that
mode and rechecks the current binding and account/team ACL together on each resource access.
No name, host path or upstream URL establishes ownership. See the
[registration procedure and recovery limits](../operations/repository-registration.md).
Short-lived CLI operations that use temporary task IDs remain outside this configured-server
identity contract.
