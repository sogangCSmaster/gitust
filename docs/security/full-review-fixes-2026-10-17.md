# Full-codebase review remediation — 2026-10-17

Base: `adb4b6e` plus the uncommitted command-specific output feature. The user requested fixes for all nine findings from the full-codebase review. No commit or push is part of this remediation request.

## Findings addressed

| Finding | Change | Regression evidence |
|---|---|---|
| P1: reflog expiry changes release worktree target to main | Immutable creation metadata in the validated Git admin directory; migrate positive legacy evidence; unknown targets cannot verify/approve; approval binds to persisted target | Four original cases failed before implementation. Reflog expiry now preserves release target and original SHA; wrong-target approval is rejected; unknown legacy target and corrupt metadata fail closed |
| P1: a failed competing sync overwrites shared upstream staging | Acquire OS service ownership before fetch and initial bare creation; keep ownership through combined sync and HTTP fetch serialization through apply | Independent child process using upstream B fails busy without changing upstream A's staged/main SHA; first-init service-lock coverage |
| P1: MCP can delete merged human approval evidence | Explicit Operator/Agent authority checked before and after the worktree lock; MCP always uses Agent authority | Before: MCP merged discard succeeded. After: MCP denied with receipt/history/checkout preserved; general HTTP operator cleanup and unmerged MCP owner cleanup still succeed |
| P2: writer can persist an auth DB its reader refuses | Check serialized snapshot size before creating/writing the temporary file; enforce 254-byte email input | Before: oversized snapshot/email accepted. After: oversized write rejected, memory rollback and previous disk snapshot/reopening preserved; UTF-8 byte boundary covered |
| P2: cross-tab logout invokes fake demo merge on a live row | Reset session identity/data/request epochs on storage changes; live mode and live IDs never invoke demo mutations | Actual Workbench DOM and delayed client-response regression cases |
| P2: Git path trimming reads another file | Preserve valid edge spaces and space-only paths in blob/diff requests | Before: `note ` returned `note`. After: independent Git oracles match all space variants; traversal rejection remains |
| P2: several files' changes appear under the first filename | Changed-file selector and immutable head/base/path-filtered diff requests | Independent file responses remain attached to the selected path; stale selections cannot overwrite it |
| P2: binary/metadata changes appear unchanged | Distinguish changed-file summaries from missing textual hunks | Binary/empty/mode/rename-style no-hunk responses show explicit changed-file information |
| P2: partial tree/diff appears complete | Persist visible truncation status in the tree/diff views | Truncated API fixtures retain a notice; later complete/error/session states clear obsolete notices |

## Additional boundaries

Creation metadata uses bounded no-follow regular-file reads, version/ID/branch/SHA validation and fsynced, create-only atomic publication. A fixed staging path bounds crash leftovers. Tests also cover a large legacy reflog, interrupted staging recovery, immutable-record preservation, unknown versions/identities, oversized records and symlinks. Detached checkouts remain recoverable but cannot satisfy attached-checkout verification.

The existing command-output implementation and its tests remain included in the working tree. This review does not add automatic Checks, timeout/cancellation history, an unknown-target repair API, Linux execution coverage, or deployment validation. Security review uses isolated local repositories and synthetic DOM/auth data; older blocked OAuth, OS-sandbox and network attack probes are not retried.

## Final verification and independent review

- `scripts/verify.sh` exits 0 with 99 frontend cases, Rust workspace tests, fmt/Clippy, typecheck, file-size limits, fail-open scanning and embedded-asset freshness checks.
- The creation-target cases failed before the fix and pass afterward. Additional metadata-boundary cases pass, including malformed identity/size/symlink checks and a large legacy reflog.
- Existing security tests preserve their original oracle: both forged checkout pointers and the second checkout's forged HEAD must be restored before successful recovery; trusted creation metadata now takes precedence over a damaged reflog, while a legacy task without that metadata remains unknown. The bare-init test explicitly unlocks its raw file before retrying, avoiding inherited descriptors from unrelated parallel Git subprocesses. Production service guards already explicitly unlock on Drop.
- Independent Gate A: CLEAR TO COMMIT, no remaining P0/P1/P2 in this scope. It checked lifecycle/target binding, lock ownership, MCP authority, snapshot rollback, session epochs and viewer states.
- Independent Gate B: CLEAR TO COMMIT, no remaining P0/P1/P2 in this scope. Eight independent Rust probes and eight frontend probes passed, including an actual competing child process, Git/fsck oracles, wrong/oversized/symlink/FIFO metadata, near-limit snapshot rollback, MCP refusal with evidence preserved and normal operator HTTP cleanup, cross-tab auth and stale responses.
- Both reviewers independently found one additional P2: a root commit's empty-tree SHA was incorrectly sent as an explicit base commit for file-specific requests. The fix preserves the immutable head and omits the canonical empty-tree base query; regression failure before the fix and passing wire/Git probes afterward confirm it.
- Local production build: initial JavaScript 244.03kB / 79.40kB gzip; the 50kB target remains unmet. The changed-file selector uses a compact list and sits beside the diff at desktop widths.

## Live local check

The rebuilt Rust preview server started successfully against the existing ignored preview repository and authentication state, including legacy worktree recovery. All eight embedded HTML/assets matched the production build byte for byte. The live browser selected a multi-file historical commit, switched between its actual source files and showed only each selected file's diff. At the default 1156px viewport, the 240px changed-file list and diff are side by side; at 390px the document width remains 390px without horizontal overflow. The temporary viewport override was reset. No upstream publication or source-repository commit/push occurred.
