# Development roadmap

This document separates planned capabilities from implemented behavior. It is a sequence of work, not a release commitment. A UI prototype or passing test suite alone does not establish deployment readiness.

## Phase 1 — Interface foundations

Implemented: SolidJS components, CSS tokens, SVG icons, keyboard navigation, and workbench views. Repository browsing and the agent workflow have live server connections. Other views, including broader Actions and enterprise administration concepts, still include demonstrations.

## Phase 2 — Agent workspaces and inbound mirrors

Implemented foundations:

- Rust CLI/server and `gix`-based object reads.
- Managed agent branches and linked worktrees with ownership, operation coordination, and recovery.
- Local OS sandbox execution through CLI and MCP.
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
- Per-command verification outcomes, measured durations, and skipped checks in HTTP/MCP and the workbench.
- Bounded durable history of completed HTTP/MCP runs, with a SiteAdmin-only read API and no restoration of approval authority.
- A live workbench history viewer for the latest five completed runs, including per-command outcomes, captured output and execution metadata, separate from current verification authority.

Remaining work:

- Automatic Checks on branch updates; durable start/timeout/cancellation records.
- Full pull-request collaboration.
- SSH Git transport; existing SSH key registration is not an SSH Git server.
- Broader administration UI and repository lifecycle management.
- Documented backups and tested restoration of auth state, Git data, worktrees, and approval records.
- Deployment validation, platform coverage, and remaining independent security review.

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
