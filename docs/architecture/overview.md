# Current architecture

Gitust is a Rust Cargo workspace with a SolidJS frontend embedded in the CLI/server binary. Each server currently serves one configured Git repository; the auth store also models repository and team access.

## Modules

| Module | Responsibility |
|---|---|
| `gitust-cli` | Server, MCP, sandbox, sync, triage, and administrator commands |
| `gitust-server` | Axum routes, HTTP Git/MCP, authentication boundaries, task state and coordination |
| `gitust-core` | Shared types, identifiers, roles, and worktree states |
| `gitust-git` | Git objects/refs, linked worktrees, mirrors, verification, approvals, and publication |
| `gitust-sandbox` | OS sandbox policies, execution limits, output capture, and cleanup |
| `gitust-sync` | Mirror configuration and public/private ref policy |
| `gitust-db` | Auth and permission state with lock-protected JSON persistence |
| `packages/design-system` | UI primitives, workbench patterns, live clients, and separate showcase state |

`gix` handles many read/ref operations. The Git CLI is used for checkout, transport, and other operations. There is no current SQL database, Actions engine, search service, or microVM runtime behind these module names.

## Task lifecycle

A task receives a managed `agent/*` branch and linked worktree. Ordinary execution invalidates previous verification. Explicit verification checks a clean attached commit before and after commands, including required policy from the merge target.

Approval requires current evidence and performs a fast-forward merge. Publication separately sends the approved SHA upstream. Worktree/repository locks coordinate mutations; a repository service lock prevents competing Gitust services from independently managing the same repository.

See [verification and Checks](worktree-verification.md) and [approval/publication](outbound-sync.md) for exact contracts and failure behavior.

## Persistence and recovery

- Auth state uses a configured JSON file with cross-process locking and atomic replacement.
- Git objects, refs, and linked-worktree metadata remain in Git storage.
- Approval/publication records live under the common Git directory's `gitust-approvals` directory.
- Runtime verification evidence is discarded on restart; an unmerged task needs fresh verification.

Git mutation and approval-record persistence are not one transaction. Failures between them may require operator inspection. Backups need auth state and the full repository/worktree layout; a complete restore workflow remains on the roadmap.

## Execution and trust

The executor uses macOS Seatbelt or Linux Bubblewrap, with an isolated environment, bounded output, a timeout, and network denial. Server execution is opt-in for SiteAdmins on trusted single-user deployments, not a hostile multi-tenant boundary.

MCP-purpose OAuth sessions are restricted to MCP; they cannot act as general login credentials for approval, publication, administration, or Git HTTP. Local stdio MCP and CLI commands run under the local operator's authority.

Gitust governs its local state and execution. The agent client separately controls model interactions; a remote AI provider may receive source code depending on that configuration.
