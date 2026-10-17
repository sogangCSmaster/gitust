# Expected-value ref transactions

Agent branch creation, agent commits, bare/non-checked-out fast-forward approval,
agent branch deletion and inbound mirror batches use a small Rust adapter around
Git's [`update-ref --stdin -z` transaction protocol](https://github.com/git/git/blob/v2.34.0/Documentation/git-update-ref.txt). Git 2.34 or later is required.
The repository's previous gix-ref primitive read a ref before waiting for its lock;
a concurrent ordinary Git writer could then publish a commit that the stale
comparison overwrote. Successful `fsck` does not detect that loss of a valid ref.

The adapter queues only direct `refs/heads/*` and `refs/tags/*` updates/deletions,
with either an expected object OID or strict create-only absence. `prepare` checks
those expectations while owning the actual Git locks. Before `commit`, a bounded
raw loose-ref read rejects symbolic aliases; `--no-deref` alone could replace an
alias. Packed refs and their lock handling remain Git-owned. Annotated tags retain
their tag object OID. Ref namespaces and unsupported transaction forms are refused.

Where a caller supplies a publication gate, it runs after preparation and raw-kind
validation, immediately before commit. The original credential is therefore checked
again after ref-lock waiting. A denied gate leaves refs and reflogs unchanged. The
protocol aborts and gives Git up to two seconds to remove its own locks; forced
termination is reserved for failed or timed-out cleanup. Owned process groups and
pipe workers are collected on both successful and failed paths.

HTTP approval supplies the original session or PAT, current account/role and actual
repository access check through this gate. For the existing checked-out-main merge
path, that check runs after clean-worktree inspection immediately before native
`git merge --ff-only`; it does not run inside that command's internal ref lock.

The native process has a 30-second deadline, short bounded ref-lock waits and a
two-second abort grace. A batch admits at most 100,000 edits and 64 MiB of encoded
input. Input validation and operational failures propagate as errors. Captured
diagnostics are bounded and are not returned to clients with private repository
paths. A failed or unconfirmed commit is never retried with a fresh expected value.
This introduces a Git process on these write paths; no latency improvement is claimed.

Inbound sync still classifies ahead, diverged, dirty and checked-out branches before
constructing a pending batch. If that batch fails, the operation returns an error;
it does not retry every ref individually, bypass resource bounds, or convert I/O
and protocol failures into a skipped success. Previously completed authorized
phases, including an earlier checked-out-main merge, are not rolled back. Git's
multi-ref transaction does not provide an atomic snapshot to unrelated readers,
and this adapter does not claim crash- or power-loss durability.

Native Git owns packed-ref formatting and reflog storage. The agent-origin creation
message and zero old OID remain unchanged for legacy recovery. A mixed mirror batch
uses one diagnostic message, `gitust sync: apply inbound mirror refs`. With automatic
reflog creation disabled, native Git may still append to an existing reflog; this
is diagnostic history and does not create verification or approval authority.

Ordinary temporary-repository regressions use a separate real Git transaction to
hold a competing lock and publish while the engine operates. They cover approval,
host commits, create-only refs, deletion and mirror batch failure against independent
Git queries. Additional regressions cover packed/loose updates, raw annotated tags,
origin reflogs, gate refusal, symbolic aliases, bounded batches and subsequent writes.
An uncatchable kill can still leave a Git-owned lock: stop and identify all writers,
inspect actual refs and locks, and follow Git's recovery procedure before retrying.
Gitust does not blindly delete a lock file whose ownership cannot be confirmed.
