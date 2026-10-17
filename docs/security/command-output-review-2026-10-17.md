# Command-specific output review — 2026-10-17

Base: `adb4b6e`. The workbench history viewer was committed and pushed before this follow-up.

## Scope and behavior

- Capture each executed command's stdout/stderr and capture-truncation flag alongside its host-observed exit status and duration. Skipped commands have no output. Existing aggregate output and cumulative per-stream capture limits remain unchanged.
- Show command output in a collapsed solid-ui disclosure in both the current result and stored history. Render text without HTML interpretation or command replay.
- Preserve old HTTP/MCP results and version 1 history without the optional output field.
- Store at most 1KiB per command stream at a UTF-8 boundary. Additional history clipping sets `history_truncated` without changing the actual execution's capture flags. The existing five-record, nine-step and 3MiB file limits remain.

## Verification

- Before implementation, the sandbox regression failed because command-specific output was missing; all three new frontend tests failed because parsing and disclosures were absent.
- Final `scripts/verify.sh` exits 0. This includes 83 frontend behavioral tests, Rust workspace tests, fmt/Clippy, typecheck, file-size/fail-open checks and embedded frontend freshness.
- Tests cover correct output attribution, nonzero exit/skipped steps, shared capture caps, per-command UTF-8 decoding, persisted output after restart, old history compatibility and worst-case JSON escaping. Frontend cases cover optional-field compatibility, malformed/oversized/skipped output rejection and plain-text disclosure rendering.
- Production build: initial JavaScript 240.69kB / 78.39kB gzip. This is a local Vite build measurement, not a claim that the 50kB budget has been met.
- Independent Gate A: CLEAR TO COMMIT for this scope, no remaining P0/P1/P2. Five independent synthetic probes and 83 frontend tests passed. Its separate all-NUL JSON oracle measured 2,773,505 bytes for five maximal nine-command records, below 3MiB. It also checked UTF-8 boundaries, parser compatibility, command-disclosure independence and authority separation.
- Independent Gate B: CLEAR TO COMMIT for this scope, no remaining P0/P1/P2. Four independent temporary DOM/JSON probes passed, covering both parsers, malformed/skipped output, byte caps, legacy compatibility, independent disclosures and plain-text rendering. The reviewer separately ran the Rust history bounds/legacy test. This was not a new child-process pipe attack or a broad backend penetration test.

## Live local check

The local preview repository ran `printf step-output-proof; printf step-stderr-proof >&2` plus its two required Checks. The current verification dialog exposed separate stdout/stderr under the requested command and empty streams for the two successful Checks. After an actual Rust-server restart, the task recovered as Pending with approval disabled while Run history retained the same completed run and its per-command output. Older records without command output remained available. The expanded history dialog also fit a 390px viewport with no horizontal document overflow; the temporary viewport override was reset. No upstream publication occurred.

## Limits

The run still has one cumulative capture budget, so a later command can have empty or clipped output after an earlier command exhausts it. Individual command streams are decoded separately; partial UTF-8 bytes split across different commands can display replacement characters while the aggregate stream decodes the combined bytes. Stored command output is intentionally shorter than the current execution response. Timeout/cancellation/start records and automatic Checks remain separate roadmap work.

Execution policy, authentication, approval, subprocess lifecycle and network-isolation rules are unchanged. Existing broad OAuth, OS sandbox and network exploit surfaces were not re-audited in this follow-up. Linux execution remains untested in this session.
