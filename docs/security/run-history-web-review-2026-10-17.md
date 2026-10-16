# Workbench run-history review — 2026-10-17

Base: `c554795a33560f6b98e726e324d6da462f3e5825`. The completed-run storage/API step was committed and pushed before this separate web step.

## Scope

- Add an administrator-only, live worktree history dialog with the latest five completed runs, selection, refresh, per-command outcomes, captured output and execution metadata.
- Keep stored diagnostic results separate from current verification and approval authority. No replay, retry or approval controls are derived from stored commands.
- Validate incoming records and ignore stale, aborted, closed-dialog and changed-account responses. Clear displayed records before loading or showing an error; do not persist history in browser storage.
- Share presentational verification details and restore focus when controlled Dialog/Sheet surfaces close.

## Verification and independent review

- Frontend suite: 80 tests pass, including client response validation and lifecycle races, live history rendering, authority separation, error/empty states and modal focus restoration.
- Final `scripts/verify.sh` exits 0: Rust workspace tests, fmt/Clippy, frontend tests/typecheck/build, fail-open scanning, file-size limits and embedded-asset freshness gates pass.
- Production frontend build passes. Initial JavaScript is 239.57 kB / 78.09 kB gzip; the separately loaded history content is 3.90 kB / 1.57 kB gzip. These local Vite build measurements do not meet the aspirational 50 kB initial-JavaScript budget.
- Independent Gate A: CLEAR TO COMMIT for this web/shared-focus scope, with no remaining P0/P1/P2. Follow-up probes covered the original Escape failure, native Dialog/Sheet triggers, caller-controlled autofocus and nonmodal focus behavior.
- Independent Gate B: CLEAR TO COMMIT for this frontend scope, with no remaining P0/P1/P2. Safe DOM/fetch probes covered plain-text rendering, malformed and oversized data, stale/token-swap/unmount responses, role/demo guards, immediate modal keyboard blocking and focus restoration. No older backend OAuth, OS, network or sandbox attack surfaces were re-audited in this step.

## Review finding fixed

Both reviewers reproduced a P2: closing a controlled history dialog with Escape left keyboard focus on the document body. Controlled modals had no trigger reference for the underlying library to restore. A shared Dialog/Sheet adapter now captures the actual opener on opening and restores it on close when it remains connected. It preserves caller-prevented autofocus and nonmodal behavior. The history row explicitly focuses its button before opening so pointer activation also has a reliable opener in WebKit. Regression tests cover repeated opening from different buttons and initially closed surfaces.

## Live local check

The live workbench loaded a stored successful run retained across a Rust process restart. A second real command exited 7; the history showed that failure, its stdout and two skipped required Checks alongside the older success. Selecting the older success kept the current failed worktree's approval button disabled. Escape returned focus to its Run history button. Testing used only the ignored local preview repository; no upstream publication occurred.

After rebuilding and restarting the Rust server with the final frontend, all eight embedded HTML/assets matched the local production build byte for byte. Refreshing the live workbench recovered the current task as Pending while both previous run records remained available in Run history.

Desktop and 390px mobile layouts were checked for bounded dialog sizing and horizontal overflow. The dialog header reserves space for its close control. Completed responses only are available; durable start, timeout and cancellation records remain future work. Linux execution and broad deployment readiness are not established by these frontend checks.
