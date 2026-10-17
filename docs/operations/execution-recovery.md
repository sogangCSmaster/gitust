# Interrupted execution recovery

Before a managed sandbox command can start, Gitust writes a private, durable
execution fence in the repository's common Git directory. A normal result is
published only after command cleanup is confirmed. That completed run's fence
is then removed. A nonzero command exit can still be a normally cleaned result;
it does not grant verification authority.

A crash, cancelled request, timeout, preparation failure or uncertain cleanup
can leave a fence. Gitust keeps that evidence and refuses new repository
changes. A newly started `serve`, stdio `mcp` or `sync` service also refuses
ownership while a fence remains. The backup tool rejects an unresolved fence
before creating a snapshot. This conservative behavior also applies when a
request was interrupted before a command actually started.

## Operator procedure

1. Stop the Gitust service and every sandbox command left by the previous
   service. Identify the actual installation and command tree using the host's
   tools. A PID from inside a Linux namespace is not a host PID, and an old PID
   alone is not proof of process identity. Gitust does not automatically signal
   a recorded PID or expire a fence after a timeout.
2. Inspect the managed worktree for partial edits. Preserve unfinished work.
   Confirm that no old command can continue changing it. Keep the service stopped.
3. Explicitly acknowledge that condition:

   ```sh
   gitust sandbox recover --repo /absolute/path/to/repository \
     --acknowledge-stopped-processes
   ```

   The command requires an explicit repository and acknowledgement. It must
   acquire the exclusive repository service lock, validates the complete bounded
   fence set, and refuses malformed metadata or a running service. The flag is
   the operator's confirmation; it is **not** automatic process-death detection.
4. Start the service again, inspect the recovered worktree and commit or discard
   partial edits deliberately. Run fresh verification against the exact clean
   commit before approving it. Recovery does not restore verification authority
   or publish anything upstream.

Do not manually delete markers to make a startup or backup check pass. Corrupt or
unsupported evidence requires investigation while the installation is stopped;
the recovery command does not silently erase unknown files or symlinks.

## Scope and validation

The fence prevents a replacement service from overlapping an unresolved old
execution. It does not automatically terminate a process that survives the
server's `SIGKILL`, provide an exactly-once scheduler, or make the trusted
single-user OS sandbox a multi-tenant boundary. Same-process uncertain failures
also quarantine subsequent repository mutations. Independent normally active
worktrees remain concurrent until such a failure occurs.

`scripts/execution-crash-rehearsal.py` is the permanent regression. It starts an
automatic Check in a disposable repository, kills only its own server, requires
replacement startup to fail, stops authenticated fixture process groups, runs
explicit recovery, and proves ordinary execution resumes with no restored
verification. It records whether the command survived on the observed host;
macOS and Linux behavior must be tested separately. Source/auth snapshots and
credentials are never included in its public report.

The [2026-10-17 macOS evidence](evidence/execution-crash-2026-10-17.json)
records a surviving command and overlapping replacement execution before the
fix, then rejected replacement startup and safe explicit recovery afterward.
The permanent regression failed against the old binary and passed against the
follow-up build. That build contained uncommitted changes over its recorded
base; this is not Linux runtime or final-release acceptance evidence.

When upgrading an older version that did not create fences, stop its server and
all outstanding commands first. New metadata cannot retrospectively identify an
old unrecorded execution. Restoring a snapshot must preserve unresolved fences;
relocation is not permission to clear them.
