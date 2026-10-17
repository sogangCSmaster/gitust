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

The UUID is an identifier, not authorization. The auth store's repository records do not
yet bind a Git path to an ACL. A matching configured UUID in that metadata is refused at
startup and on later access, even for a SiteAdmin; it is never adopted by matching a name,
path or upstream URL. The existing access policy for the unregistered configured
repository remains in effect. Short-lived CLI operations that use temporary task IDs are
outside this configured-server identity contract.
