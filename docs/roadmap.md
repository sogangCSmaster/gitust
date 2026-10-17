# Development roadmap

This document separates planned capabilities from implemented behavior. It is a sequence of work, not a release commitment. A UI prototype or passing test suite alone does not establish deployment readiness.

The overall execution scope now covers all four phases, following the user's instruction on 2026-10-17. The immediate release checkpoint remains the [trusted single-user self-hosted alpha](operations/alpha-acceptance.md); its acceptance conditions must be verified before that checkpoint is declared complete. Later capabilities below remain planned until implemented and verified. See the [execution plan](operations/roadmap-execution.md) for the sequence and evidence boundaries.

## Phase 1 — Interface foundations

Implemented: SolidJS components, CSS tokens, SVG icons, keyboard navigation, and workbench views. Repository browsing and the agent workflow have live server connections. Other views, including broader Actions and enterprise administration concepts, still include demonstrations.

The adopted [workbench interface](architecture/interface-design.md) separates workspace and repository navigation, uses responsive list/detail layouts, and shares compact light/dark theme primitives. The repository list shows accessible configured Git repositories. The primary repository supports the workbench; explicitly registered additional repositories support file, commit and diff browsing plus authenticated clone/fetch.

## Phase 2 — Agent workspaces and inbound mirrors

Implemented foundations:

- Rust CLI/server and `gix`-based object reads.
- Managed agent branches and linked worktrees with ownership, operation coordination, and recovery.
- Local OS sandbox execution through CLI and MCP.
- Host-side MCP commits of explicit managed-worktree paths against an expected HEAD, retaining read-only Git metadata inside execution sandboxes. See the [commit contract and recovery limits](architecture/mcp-commits.md).
- Explicit inbound synchronization and tree/commit/diff browsing.
- Administrator-issued invitations, CLI-only first-admin creation, persistent credentials, and revocation.

CoW and microVM ambitions remain future work. Current worktrees use Git checkout, and execution uses Seatbelt or Bubblewrap.

## Phase 3 — Internal Git and approved publication

In progress. Implemented portions:

- Smart HTTP hosting with access checks and managed-agent-branch push restrictions.
- Local triage commands and the live web agent workflow.
- Clean-commit verification, fast-forward approval, and separate exact-SHA publication.
- Durable approval/publication records and explicit remote-failure retry.
- Required Checks from committed merge-target configuration during explicit verification.
- Explicit local CLI Checks for the current ordinary branch at an expected full commit ID, with committed configuration, real per-step results and temporary-checkout cleanup. This does not grant approval or add ordinary-branch automatic triggers. See the [local execution contract](operations/local-checks.md).
- Opt-in local Checks history retains five completed results under the persisted repository identity, with bounded output and CLI list/run-ID queries. It survives offline relocation and remains separate from managed-worktree verification and server Activity.
- Opt-in automatic Checks after accepted managed-worktree MCP commits and Git HTTP pushes, with bounded latest-update coalescing and diagnostic history. Automatic success does not grant approval; explicit manual verification remains required.
- Per-command verification outcomes, measured durations, skipped checks and expandable command-specific stdout/stderr in HTTP/MCP and the workbench; stored output is bounded separately for history.
- Durable managed-worktree automatic-run intents, latest-update coalescing, lifecycle records and conservative restart recovery. Queued work is reauthorized; uncertain running work is interrupted without replay. See the [execution contract](architecture/automatic-checks.md).
- Bounded durable history of completed HTTP/MCP runs, with a SiteAdmin-only read API and no restoration of approval authority.
- Durable execution fences reject replacement service ownership and further mutations after an unconfirmed run. Explicit operator recovery is required; this prevents overlap but does not implement automatic orphan termination.
- A live workbench Activity viewer combining the latest five completed results with bounded automatic-run lifecycle records. It includes per-command output when retained, makes queued/interrupted attempts explicit, and keeps diagnostics separate from current verification authority.
- A SiteAdmin-only read-only Actions view and bounded repository API for current tracked worktrees. Pages follow worktree ID order and link to the exact retained run in Activity; they do not provide a global newest-run feed or workflow execution controls.
- A live worktree-to-diff review entry that pins head/base commits across file switches, refuses unknown targets and keeps tree/blob snapshots consistent. The follow-up passed local checks and Gate A; its Gate B exception was approved before automatic Gate B reviews were stopped by the user.
- Bounded task display labels in creation metadata, preserved on restart and relocation for new tasks. Legacy records retain a generic recovered label; labels do not restore verification authority.
- Stable configured repository IDs across restart and complete offline relocation, with bounded private metadata and collision rejection. Identity alone does not bind access permissions or implement repository routing.
- Explicit offline ACL registration, with matching private Git/auth-store anchors, current team access checks and conservative partial-registration recovery. Registered repositories currently use authenticated HTTP MCP rather than local stdio.
- A bounded operator-configured repository catalog with UUID-routed commit/tree/blob/diff reads and team filtering. Additional repositories retain independent Git service ownership; worktrees, Checks, MCP and synchronization still target the primary repository. Metadata-only records do not create a Git service.
- Authenticated UUID Smart HTTP clone/fetch for configured additional repositories, preserving current credential/ACL checks before and after Git work and service ownership through cancellation. Entries configured with `--read-only-repo` keep receive-pack disabled for every role. The compact Clone dialog uses explicit current-access hints. See the [transport contract](operations/read-only-git.md).
- Explicit `--git-write-repo` hosting for registered bare secondary repositories without worktrees. Current Maintainers and SiteAdmins with access may push ordinary branches and tags; agent refs remain denied. The compact catalog distinguishes Git hosting from a full workbench. See the [configuration and authority contract](operations/secondary-git-hosting.md).
- Offline import of an existing Git source into a fresh registered bare repository, with an explicit default branch, private fixed-identity journal, original administrator-session rechecks and no-overwrite publication. Sealed retries preserve the same snapshot and registration pair. Serving the result still requires explicit configuration; inventory changes on a running server are not implemented. See the [import contract](operations/repository-import.md).
- SiteAdmin display-name editing for configured registered repositories, with a compact Overview dialog and durable metadata-only changes. UUID, registration binding, team, privacy and Git paths remain unchanged. Metadata-only records and legacy unregistered services cannot be renamed through this API.
- SiteAdmin access-scope management for configured registered repositories, with current-value comparison and original-credential checks inside the durable auth transaction. Team-only and all-active-signed-in-user scopes retain existing role permissions and keep anonymous access disabled. The compact Overview form confirms the server result; uncertain writes require refresh. See the [scope and recovery contract](operations/repository-access.md).
- SiteAdmin invitations to a configured repository's owning group, with a compact form inside access settings, a server-derived grant and durable per-key duplicate prevention. Original credentials are rechecked after auth write-lock waits for group invitations, the existing HTTP invitation family, credential mutations, account deactivation and repository metadata writes. Replays do not recover the one-use link or remint consumed/revoked invitations. Roles remain site-wide, and group membership may open other private repositories. See the [invitation contract](operations/repository-group-invites.md).
- Exact-account owning-group membership management from access settings, with durable version comparisons that reject stale grants after removal. Other groups, site-wide roles and account status remain unchanged; inactive accounts can be removed but cannot be added. See the [membership and stored-state compatibility contract](operations/group-membership.md).
- Catalog and web support for a valid default branch without a commit, using an explicit nullable HEAD. Primary Smart HTTP accepts an authorized first ordinary push; other branches and existing tasks remain distinct from the missing default commit. The compact Clone dialog provides credential-free UUID URLs for hosted primary and read-only secondary repositories.
- Offline creation of a fresh registered bare repository with an explicit unborn default branch, fixed identity, shared create/import destination admission, sealed-empty retries and no-overwrite publication. Serving it still requires explicit configuration. See the [creation and recovery contract](operations/repository-create.md).
- Bounded UUID branch pages and a compact Files/Changes selector for primary and read-only repositories, including alternate branches before a default commit exists. Reads use identified commit SHAs; list refresh preserves an explicit snapshot. This adds browsing without branch mutation. See the [branch browsing contract](operations/branch-browsing.md).
- SiteAdmin default-branch management for configured registered bare primaries and explicitly writable secondaries, with a compact Overview dialog and symbolic HEAD comparison under its actual lock. Existing worktree targets remain pinned; new Git clones and primary omitted-base HTTP/MCP tasks use the current committed default. See the [default-branch contract](operations/default-branch.md).
- A stopped-installation backup/relocated-restore utility, regression tests and recorded Linux/macOS rehearsals at `42bae69`, including MCP commits after relocation. Explicit multi-repository snapshots now preserve a shared auth checkpoint and every included repository, with v1 single-repository compatibility and local restored-only ACL/approval checks. See the [backup contract](operations/backup-restore.md); final-candidate acceptance remains open.

Remaining work:

- Non-managed branch triggers and workflow execution beyond managed-worktree automatic Checks. GitHub Actions-compatible workflows remain Phase 4 work.
- Reduce the initial JavaScript bundle toward the 50 kB gzip target; the current initial static-import closure is approximately 70 kB gzip after the first import-boundary reduction. The [measurement](benchmarks/web-bundle.md) includes every static dependency and the first Overview's cumulative cost; the target remains unmet.
- Full pull-request collaboration.
- SSH Git transport; existing SSH key registration is not an SSH Git server.
- Repository administration UI, lifecycle management and online inventory changes; additional repositories still need isolated worktree and execution routing before gaining full workbench capabilities.
- Final-candidate restoration evidence for auth state, Git data, worktrees and approval records on the intended platforms.
- Deployment validation, platform coverage, and final-candidate checks and Gate A review.

The MCP commit implementation passed full checks, independent architecture review and ten-task rehearsals. Hosted Linux/Bubblewrap and macOS/Seatbelt checks, relocation and crash recovery passed at `42bae69`. The historical independent HTTP review has partial executed evidence but no final verdict because further checks were interrupted by tool policy. Actual intended IDE-agent sessions, authorized GitHub publication, a separately provisioned installation and validation of later changes remain open. Track those boundaries in the [alpha progress report](operations/alpha-progress-2026-10-17.md); all nine acceptance conditions remain in force.

GitHub remains the initial source of truth. Local merge and upstream publication are separate operations. Publication failure must not erase approval or silently select a different commit.

## Phase 4 — Source of truth and enterprise operations

Planned after the relevant reliability and recovery gates:

- Move selected repositories to Gitust as source of truth, retaining an upstream backup mirror.
- GitHub Actions-compatible workflows: dependencies, matrices, secrets, artifacts, and caches.
- Stronger execution isolation, multi-node scheduling, and shared build caches.
- SSO/OIDC/LDAP, two-factor authentication, and a broader audit trail.
- Indexed code and symbol search.

These capabilities are not implied by demo screens or domain models. Introduce SQL storage, search services, and an Actions engine only when needed by an agreed implementation step.

## Validation

Follow [AGENTS.md sections 7.4 and 7.5](../AGENTS.md). Publish measurements with their environment and keep incomplete review/platform coverage visible. The [worktree benchmark](benchmarks/agent-worktree.md) records the gap between current checkout performance and the CoW target.
