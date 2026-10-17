# Automatic managed-worktree Checks — 2026-10-17

## Scope

This change adds explicit `auto_run: true` opt-in to committed merge-target Checks policy. Accepted managed-worktree MCP commits and actual Git HTTP ref updates schedule bounded automatic execution in trusted single-user mode. Active SiteAdmin and worktree ownership checks remain mandatory; local stdio retains its explicit operator authority. Automatic completion records the checked SHA and policy blob but never grants approval authority. Approval still requires manual verification.

The shared execution path was extracted from `worktrees.rs` into `worktree_execution.rs`. It retains operation/service guards and rechecks active SiteAdmin authority before publishing a result. The web history parser and viewer distinguish automatic results from ordinary commands and explicit verification.

## Independent Gate A

Verdict: **CLEAR TO COMMIT**. The final independent review found no unresolved P0/P1/P2 in the reviewed source. No Gate B was run, following the user's instruction to stop automatic Gate B reviews.

One verified P2 was fixed: a rejected receive-pack request with stale old SHA and already-current new SHA could produce HTTP 200 while leaving the ref unchanged. Testing only the final ref would incorrectly trigger another run. The handler now compares both pre-request and post-request refs under the repository operation guard. The real wire regression produced two records instead of one before the fix (exit 101) and passes after the fix.

The reviewer independently ran seven server automatic-run tests and thirteen history parser/view tests. Reviewed file identities are recorded in `automatic-checks-gate-a-2026-10-17.json`; generated bundle consistency is covered by the full verification below.

## Executed verification

- `scripts/verify.sh`: exit 0, including formatting, Clippy, fail-open scanner/self-test, all 189 Rust/TS/TSX files below 600 lines, Python backup tests, frontend tests and embedded-bundle freshness.
- Python: 12 passed. Frontend: 138 passed. Rust: CLI 6, core 2, database 28, Git 86 with 2 benchmark tests ignored, sandbox 20, server 77 and sync 1 passed. The Git crate's nested focused subprocess tests also passed.
- Policy opt-in and automatic MCP triggering tests were written first and failed before their implementation.
- Automatic success/failure/disabled policy, current SHA and policy provenance, approval rejection until manual verification, latest pending update coalescing, preservation of manual verification, inactive account/ownership/trusted-mode restrictions, scheduling unavailability, queued shutdown and active-command shutdown were checked.
- Active-command shutdown checked Failed state, cleared approval authority, worktree lock release, process-group termination and absence of the command's final marker.
- Real Git client clone, commit, push, fetch and client/server/worktree `git fsck --strict` passed. Up-to-date pushes and rejected stale wire updates did not add runs.
- A new local preview task exercised actual HTTP MCP edit, commit and automatic Checks using a separate opt-in target branch. The stored SHA matched the accepted commit, the worktree remained Pending, and approval returned 409. The browser displayed Automatic checks, command results and the manual-verification requirement. This was performed by the main agent, separately from the independent review.

## Remaining work

Scheduling is in memory. Restart replay, durable start/timeout/cancellation records and a live Actions workflow screen remain open. Direct filesystem Git commits, non-managed branches and mirror updates do not trigger this implementation. Existing bounded history stores completed command results only; preparation, timeout, cancellation or final checkout-validation failures do not create completed records. Automatic records may prevent older binaries that reject unknown history fields from reading that history.

The executed platform was local macOS. Linux runtime, final release acceptance and deployment are not established by these results. The production entry remains above the 50 kB gzip target (80.79 kB in this build).
