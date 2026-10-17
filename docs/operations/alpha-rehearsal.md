# Local alpha operational rehearsal

`scripts/alpha-rehearsal.py` performs ten real tasks against an isolated Gitust
server. It clones the committed `main` source tree of this repository, adds a
committed Checks policy in that temporary clone, and creates managed agent
worktrees, including a fresh-base retry for task 2. The current compiled Gitust
binary serves the fixture. Uncommitted
application changes can therefore affect the server binary, while the input
repository remains the committed source tree.

This is a normal local acceptance test, not a penetration test. It does not
publish to GitHub or use an existing developer server, repository, account or
credential. Its upstream is a local bare repository. The server listens on a
random `127.0.0.1` port, rather than the development ports 3000 or 4200.

## Run

Requirements are Python 3.9 or later, Git, a built Gitust binary, and the OS
sandbox backend used by Gitust: Seatbelt on macOS, or Bubblewrap on Linux.
Loopback listening and execution of that backend must be permitted by the
environment running the script. There is no fallback to unsandboxed execution.

From the repository root:

```sh
cargo build -p gitust-cli
python3 scripts/alpha-rehearsal.py \
  --binary target/debug/gitust \
  --report /tmp/gitust-alpha-mcp-commit-rehearsal.json
```

Choose a new report filename for each run. The script refuses to replace an
existing report. A successful run exits with status zero and writes a private
JSON report containing ten successful task records, the explicit MCP commit
paths and independently checked parent/commit SHAs, the binary and operator
scripts' SHA-256 hashes, platform, sandbox backend and final `git fsck` result.
It exits nonzero and does not write a successful report when any assertion fails.

Passwords and two independent PATs are generated for this run. Their raw values
stay in memory. PATs supplied to the Git HTTP client use process environment
configuration, rather than command-line arguments, URL credentials or persisted
Git configuration. Authentication data and private server logs live inside the
temporary directory. The server is stopped and the directory is removed on
normal completion or an exception. An uncatchable process kill can leave the
temporary directory behind; treat it as private authentication state.

## Ten tasks

Every task changes a real source-tree document under `docs/alpha-rehearsal/` and
creates a Git commit. These documents and commits exist only in the fixture.

| Task | Operation and required observation |
|---|---|
| 1 | Two independently initialized HTTP MCP clients create different managed worktrees, edit concurrently and commit through `gitust_worktree_commit`. Independent Git tree inspection proves each branch contains only its own new task document. Task 1 passes repository Checks, receives human approval, and is explicitly published. |
| 2 | Before task 1 publishes, a committed input fails a required Check and approval is refused. MCP edits and commits the repair, and fresh verification succeeds. The initial checkout is discarded while its committed private branch is preserved. After task 1 publishes, a separately named retry worktree starts from the new main; MCP editing, commit and verification permit approval/publication without an operator rebase. |
| 3 | A committed but unverified task cannot be approved. Verification binds the exact SHA, after which explicit approval/publication succeeds. |
| 4 | A new commit after successful verification makes the earlier approval eligibility invalid. Fresh verification is required before approval/publication. |
| 5 | Uncommitted edits prevent verification. Committing the intended content allows verification and approval/publication. |
| 6 | A command exits nonzero and approval is refused. A subsequent successful verification permits approval/publication; completed runs remain available through the history API. |
| 7 | After local approval, the local upstream is temporarily unavailable as a Git repository. Publication returns an error and the original remote SHA remains unchanged. Restoring the upstream and explicitly retrying publishes the approved SHA. |
| 8 | An unapproved task is discarded. Its checkout and active worktree entry disappear, main remains unchanged, and its committed private branch remains recoverable. |
| 9 | A verified task is backed up offline with `scripts/backup.py`. The original repository path becomes unavailable. Restore into a different path preserves authentication, ownership records, human approval/publication receipts, completed run history and main SHA. Restart recovers the unmerged task as Pending; its original MCP PAT edits and commits again at the restored location, and fresh verification is necessary for approval/publication. |
| 10 | A real Git HTTP client clones, commits, pushes into an already managed agent branch, and fetches after approval/publication. The pushed, verified, approved, published and fetched SHAs must agree. |

The requested command and both committed repository Checks must pass for each
successful verification. Local approval must not publish automatically. Each
publication is checked against the bare upstream with Git, and upstream
`refs/heads/agent/*` refs must remain absent. At the end, `git fsck --full --strict`
must pass on the server repository, upstream and HTTP client clone.

## Current boundary and remaining acceptance work

The local OS sandbox deliberately makes Git administration metadata read-only.
MCP commands edit their own checkout; the separate `gitust_worktree_commit` tool
commits only explicitly named relative paths against an expected current HEAD.
Tasks 1–9 use that tool for every task commit. The harness checks each returned
worktree identity, branch, parent, new SHA and Pending status against independent
read-only Git commands. A commit must require fresh verification. Only an
explicit repository-busy response is retried with the same expected HEAD and
paths; other commit errors fail the run.

The operator's Git writes prepare the isolated fixture before the server starts.
Task 10 intentionally uses a separate Git HTTP client clone to exercise
clone/commit/push/fetch. The operator also performs human approval/publication
API calls and the stopped backup/restore. Read-only Git inspection and final
integrity checks are independent acceptance oracles, not a substitute for MCP
mutation. No task uses an operator-side rebase or direct managed-checkout commit.

The clients are two separate protocol clients implemented with Python's HTTP
library, not actual IDE or LLM agent sessions. Passing this rehearsal proves the
tested local API/Git/OS-sandbox/restore flow. It does not prove IDE integration,
real GitHub transport/publication, deployment on another machine, Linux sandbox
behavior when run on macOS, browser usability, backup operation under concurrent
external Git writers, adversarial isolation, production readiness or performance
budget compliance. Concurrent task wall time is recorded as an observation,
without a latency guarantee. The committed upstream history of an approved SHA
is published normally; withholding private branch refs does not hide objects
reachable from that SHA.

The alpha completion decision must combine this evidence with the other
acceptance requirements. Ten local PASS records alone are not an alpha release
approval.

## Latest SHA-bound approval result, 2026-10-17

The [latest frozen-source report](evidence/alpha-rehearsal-sha-bound-macos-2026-10-17.json) passed all ten tasks,
including 15 MCP commits and explicit human approval preconditions, on the macOS
Seatbelt backend. Task 4 now refuses the previous human-observed SHA both before
and after the newer commit is successfully verified. The stale request preserves
the newer verification and main; only explicitly approving the newer observed
SHA permits its merge. The remaining publication, retry, discard, relocated
restore and real Git HTTP roundtrip assertions passed, with final fsck on server,
local bare upstream and client. The observed duration was 13.517 seconds;
this is not a performance guarantee.

Binary SHA-256: `f5899e6269836fbad5b19c3063b568c0e3a618c686ae1fb1b21e2f248b7477bc`. The report records the updated runner hashes and
its committed-input/dirty-runtime boundary. It does not establish actual IDE-agent,
GitHub, Linux, fresh-machine or final browser acceptance. Earlier reports below
remain historical evidence for their different binaries and runner contracts.

## Observed MCP commit result, 2026-10-17

The [MCP commit macOS run](evidence/alpha-rehearsal-mcp-commit-macos-2026-10-17.json)
completed all ten tasks with exit status zero on macOS 26.5.2, Apple Silicon,
using the real Seatbelt backend. The observed duration was 11.928 seconds; this
is a single local observation, not a performance guarantee. Tasks 1–9 created
15 commits through `gitust_worktree_commit`, including the two concurrent client
tasks, the failed Check repair, the new-main retry and a commit after relocated
restore using the original PAT. Task 10 used the separate real Git HTTP client.

Every MCP commit's parent, branch SHA and changed paths matched independent Git
inspection. Each commit left its server worktree Pending without a verified SHA.
All nine approved tasks were explicitly published to the local bare upstream;
the discarded task and the initial task-2 branch retained their private commits.
Server, upstream and HTTP client `git fsck --full --strict` checks passed.

The report is schema version 2, records the exact binary and runner hashes, and
sets `mcp_only_commit_demonstrated` to true for task commits 1–9. It does not prove
actual IDE-agent integration, GitHub publication or Linux behavior. The binary
was built from the dirty worktree; the fixture began at committed `main`
`adb4b6e2e09b99ff8bb3ebf268dad11199c6f718`. The earlier report below is preserved
with its different operator-commit provenance.

## Historical operator-commit result, 2026-10-17

The [original macOS run](evidence/alpha-rehearsal-macos-2026-10-17.json) completed
all ten tasks with exit status zero on macOS 26.5.2, Apple Silicon, using the real
Seatbelt backend. Both HTTP MCP clients edited their separate source checkouts
concurrently. All nine approved tasks were explicitly published to the local
bare upstream; the remaining task was discarded with its committed private
branch preserved. Server, upstream and HTTP client Git integrity checks passed.

Task 9 restored seven already approved tasks and the unmerged task's ownership
and two completed run records into a different path while the original
repository path was unavailable. Fresh login worked, the unmerged task recovered
as Pending, immediate approval was rejected, and fresh verification then allowed
approval/publication. The report records that tasks 1–9 used the operator's Git
client for commits and that actual IDE-agent/GitHub integration remains unproven.
That report is historical evidence for the earlier operator-commit harness; it
does not prove the current MCP commit path. A new report must identify the new
runner and compiled binary hashes rather than replacing the historical result.

This run followed removal of inherited `GIT_*` overrides from operator commands,
so environment variables cannot redirect fixture Git commands to a different
repository. The separate operator-script/backup regression suite also passed
11 tests. Script hashes in the report identify the exact operational runner and
backup implementation used; the runtime binary was built from the dirty local
working tree, while the fixture began at committed `main` revision
`adb4b6e2e09b99ff8bb3ebf268dad11199c6f718`.
