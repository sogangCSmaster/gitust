# Development roadmap

This document separates planned capabilities from implemented behavior. It is a sequence of work, not a release commitment. A UI prototype or passing test suite alone does not establish deployment readiness.

The immediate milestone is the [trusted single-user self-hosted alpha](operations/alpha-acceptance.md). Its acceptance conditions bound the current work; the full Phase 3 and Phase 4 lists are broader future scope.

## Phase 1 — Interface foundations

Implemented: SolidJS components, CSS tokens, SVG icons, keyboard navigation, and workbench views. Repository browsing and the agent workflow have live server connections. Other views, including broader Actions and enterprise administration concepts, still include demonstrations.

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
- Per-command verification outcomes, measured durations, skipped checks and expandable command-specific stdout/stderr in HTTP/MCP and the workbench; stored output is bounded separately for history.
- Bounded durable history of completed HTTP/MCP runs, with a SiteAdmin-only read API and no restoration of approval authority.
- A live workbench history viewer for the latest five completed runs, including per-command outcomes, captured output and execution metadata, separate from current verification authority.
- A live worktree-to-diff review entry that pins head/base commits across file switches, refuses unknown targets and keeps tree/blob snapshots consistent. The follow-up passed local checks and Gate A; its Gate B exception was approved before automatic Gate B reviews were stopped by the user.
- Bounded task display labels in creation metadata, preserved on restart and relocation for new tasks. Legacy records retain a generic recovered label; labels do not restore verification authority.
- A stopped-installation backup/relocated-restore utility, regression tests and recorded local macOS rehearsals, including MCP commits after relocation. Linux and release acceptance remain open.

Remaining work:

- Automatic Checks on branch updates; durable start/timeout/cancellation records.
- Reduce the initial JavaScript bundle toward the 50 kB gzip target; the current production entry is 80.73 kB gzip.
- Full pull-request collaboration.
- SSH Git transport; existing SSH key registration is not an SSH Git server.
- Broader administration UI and repository lifecycle management.
- Final-candidate restoration evidence for auth state, Git data, worktrees and approval records on the intended platforms.
- Deployment validation, platform coverage, and final-candidate checks and Gate A review.

The MCP commit implementation passed current full checks, an independent architecture review and the local ten-task rehearsal after index-aware normalization fixes. The independent HTTP review has partial executed evidence but no final verdict because further checks were interrupted by tool policy. Actual intended IDE-agent sessions, authorized GitHub publication, Linux execution and a separately provisioned installation remain open. Track those boundaries in the [alpha progress report](operations/alpha-progress-2026-10-17.md); all nine acceptance conditions remain in force.

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
