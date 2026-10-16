# Completed-run history review — 2026-10-17

Base: `2979717f246bf87b9a963b5d0e1cf6c1b9ca5ab5`. The deferred-UI improvement was committed and pushed before this separate backend step.

## Scope

- Persist bounded completed HTTP/MCP execution responses and expose a SiteAdmin-only read API.
- Preserve current verification/approval authority rules. History never restores Verified after restart.
- Protect private storage, preserve the per-worktree guard during blocking persistence and clean diagnostics during discard.
- This step does not add automatic Checks, durable start/timeout/cancellation records or a web history viewer.

## Verification

- Two new integration cases failed on the original code: restart history GET returned 404; broken history storage still returned successful verification. Both pass after implementation.
- Six history tests pass: restart separation from approval authority, storage failure, newest-five retention and discard, caller restrictions/corruption, UTF-8 capture bounds and discard failure ordering.
- Final `scripts/verify.sh` exits 0, including 67 frontend tests and the Rust workspace suite, fmt/Clippy, fail-open scanner, file-size and embedded-asset freshness gates.
- Independent Gate A: CLEAR TO COMMIT; no remaining P0/P1/P2. Independently ran all six history tests and checked cancellation guard ownership, storage bounds, permissions and authority separation.
- Independent Gate B for the new storage/API surface: CLEAR TO COMMIT. Synthetic probes checked private permissions, retention, version/branch/inner and outer ID mismatches, oversized/count-limited records, leaf/directory/staging symlinks, FIFO rejection, interrupted staging recovery and cleanup. Scratch fixtures were removed.
- Gate B's stdout-dependent server tests were limited by the normal tool sandbox; their passing coverage comes from the parent's elevated full verification and Gate A's independent run. This is not a claim that all older auth, OAuth, OS, network or sandbox attack surfaces were re-audited.

## Review finding fixed

P2: removing the Git checkout before fallible history cleanup could leave inaccessible diagnostics after failure or a crash. A regression substituted a directory for the history file and failed against the original ordering. Cleanup now deletes diagnostics before Git removal under the same guard, so a history error leaves the checkout available for recovery. If later Git removal fails, diagnostics may already have been deleted; this partial progress is documented.

## Live local check

The rebuilt local Rust server completed a requested command plus two required Checks, returned the stored run and `Cache-Control: no-store`, and retained the identical run ID and checked SHA after a real process restart. The worktree recovered as Pending with no verified commit. Only the ignored local preview repository and local authentication state were used; no upstream publication occurred.

Linux execution remains untested in this session. After the above checks, the user explicitly requested a detailed commit and push for this backend step, followed by a separate web history view.
