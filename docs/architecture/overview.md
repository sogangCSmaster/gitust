# Current architecture

Gitust is a Rust Cargo workspace with a SolidJS frontend embedded in the CLI/server binary. Each server has one primary repository and a bounded catalog of registered repositories, with read-only, Git-write and explicitly enabled secondary workbench capabilities. The auth store binds configured repository IDs to current team access.

## Modules

| Module | Responsibility |
|---|---|
| `gitust-cli` | Server, MCP, sandbox, sync, triage, and administrator commands |
| `gitust-server` | Axum routes, HTTP Git/MCP, optional SSH Git transport, authentication boundaries, task state and coordination |
| `gitust-core` | Shared types, identifiers, roles, and worktree states |
| `gitust-git` | Git objects/refs, linked worktrees, mirrors, verification, approvals, and publication |
| `gitust-sandbox` | OS sandbox policies, execution limits, output capture, and cleanup |
| `gitust-sync` | Mirror configuration and public/private ref policy |
| `gitust-db` | Auth/permission JSON snapshots and SQLite WAL-backed high-volume records |
| `packages/design-system` | UI primitives, workbench patterns, live clients, and separate showcase state |

`gix` handles many read/ref operations. The Git CLI is used for checkout, transport, and other operations. High-volume records use SQLite WAL storage; local JSON workflows and commit-pinned code/symbol indexing are implemented. There is no microVM runtime or remote runner execution engine.

Optional [SSH hosting](../operations/ssh-read-only.md) uses the in-process Rust
`russh` server and the same repository inventory/ACL, then streams fixed native
upload-pack/receive-pack through an owned supervisor. It does not start an external SSH daemon
or accept arbitrary commands. Its persisted host identity is hidden from task sandboxes.

Expected-value changes to hosted object refs use [prepared native Git transactions](ref-transactions.md), so comparisons occur under Git's actual ref locks. Object reads remain in-process.

## Task lifecycle

A task receives a managed `agent/*` branch and linked worktree. Ordinary execution invalidates previous verification. Agents can commit explicitly named files through the host-side `gitust_worktree_commit` MCP tool without gaining sandbox write access to shared Git metadata. Starting an authorized commit attempt clears prior verification; success leaves the task Pending and requires fresh verification. Explicit verification checks a clean attached commit before and after commands, including required policy from the merge target.

Approval requires current evidence and performs a fast-forward merge. Publication separately sends the approved SHA upstream. Worktree/repository locks coordinate mutations; a repository service lock prevents competing Gitust services from independently managing the same repository.

Inbound mirroring acquires service ownership before repository initialization or shared staging fetches. HTTP synchronization keeps its fetch guard until apply finishes while taking the repository mutation lock only for local application. A failed competing sync must not replace the first operation's upstream refs.

Offline [repository import](../operations/repository-import.md) prepares a fresh registered bare repository under a private destination claim and journal. A sealed snapshot is not fetched again on retry. Registration and final directory publication are separate operations; uncertain results preserve the same intent and anchors for inspection or an identical retry. Import does not update a running server's inventory or establish Git-payload power-loss durability.

See [MCP commits and recovery](mcp-commits.md), [verification and Checks](worktree-verification.md) and [approval/publication](outbound-sync.md) for exact contracts and failure behavior.

## Persistence and recovery

- Auth state uses a configured JSON file with cross-process locking, atomic replacement and file/directory synchronization. Both reading and writing enforce a 64MiB snapshot ceiling; pre-publication failures preserve the previous file and failed writes roll back memory. An error after rename can leave a newer disk snapshot, so callers must treat that outcome as uncertain. Email input is limited to 254 UTF-8 bytes.
- High-volume PR/audit records and workflow/Checks history use SQLite WAL-backed stores. Backups must preserve the supported JSON/SQLite checkpoint together; copying only the auth JSON file is insufficient.
- Persistent SSO/TOTP/workflow secrets require an externally managed `GITUST_SECRET_KEY`; see the [security upgrade procedure](../operations/security-upgrade.md). The key is not stored in the auth DB or installation backup.
- Git objects, refs, and linked-worktree metadata remain in Git storage.
- The configured repository ID is persisted in private `gitust-repository/identity.json` under the common Git directory. Restart and complete offline relocation retain it; incomplete records and unbound auth-metadata collisions fail closed. See [repository identity and recovery](repository-identity.md).
- Each managed worktree has a bounded, immutable `gitust-origin.json` in its Git administration directory. It binds the worktree ID/branch to the creation target and SHA independently of reflog retention. Valid legacy creation entries are migrated; missing evidence leaves the target unknown, and verification/approval refuse it. Corrupt new metadata fails recovery rather than falling back to `main`. Restore trusted metadata or discard and recreate an unknown-target task; retargeting is not implemented.
- Approval/publication records live under the common Git directory's `gitust-approvals` directory.
- Runtime verification evidence is discarded on restart; an unmerged task needs fresh verification.

Repository settings validate inputs before writing, but auth metadata, Git HEAD and runtime settings are separate durability boundaries. A later write failure reports possible partial application and requires refresh before retrying.

Git mutation and approval-record persistence are not one transaction. Failures between them may require operator inspection. Host-side commits also publish the branch before installing its prepared index; an index-installation failure reports the created SHA and requires index recovery without restoring verification authority. An uncatchable kill can leave a stale worktree `index.lock` that needs the stopped-writer procedure in the commit documentation.

The [offline backup tool and runbook](../operations/backup-restore.md) preserve auth state and the full supported repository/worktree layout. Hosted Linux and macOS ten-task rehearsals include relocated restoration and the MCP commit path at `42bae69`; final-candidate acceptance and later-change validation remain tracked in the [alpha progress report](../operations/alpha-progress-2026-10-17.md).

## Execution and trust

The executor uses macOS Seatbelt or Linux Bubblewrap, with an isolated environment, bounded output, a timeout, and network denial. Server execution is opt-in for SiteAdmins on trusted single-user deployments, not a hostile multi-tenant boundary. `container_vm` and remote runner requests are rejected; runner registration and heartbeats do not dispatch commands to another machine.

Linux seals hidden directory masks read-only after installing nested worktree/admin/scratch grants. Missing hidden names under a read-only source use filtered private parent views, preserving symlinks inside the namespace. Allowed subdirectories retain live read-only binds; new names at a filtered parent become visible on the next preparation. A missing deny inside a writable source is refused. Source inspection and mounting still require a trusted host writer.

MCP-purpose OAuth sessions are restricted to MCP; they cannot act as general login credentials for approval, publication, administration, or Git HTTP. Local stdio MCP and CLI commands run under the local operator's authority.

HTTP MCP commits require trusted execution, a live SiteAdmin credential and the persisted worktree owner. The original credential and repository authority are checked again immediately before the branch update. Local stdio commits use operator authority, so a recovered task can be continued without transferring its stored owner. Both transports reject terminal/approved tasks, stale HEADs, staged conflicts and unsupported commit inputs. Builtin Git content normalization is supported through bounded private configuration/attribute snapshots; executable filters, hooks and signing programs do not run.

MCP worktree discard never removes a merged task or its human approval evidence, even for a SiteAdmin. Merged-task cleanup requires an authorized Maintainer/SiteAdmin through the general HTTP operation or an explicit local CLI command. Unmerged owner cleanup remains available through MCP.

## Repository browsing

The operator-configured catalog is bounded to 32 repositories. UUID-routed commit, tree, blob, diff and Smart HTTP clone/fetch select the actual opened repository and check its current binding, team access and original credential again after Git work. Additional repositories retain independent service ownership. Explicitly writable secondaries support Git push; workbench-enabled secondaries also support managed tasks and execution. Metadata-only records do not create a Git service. See [registration and read-only repositories](../operations/repository-registration.md).

Git file paths retain valid leading/trailing spaces. The workbench shows a changed-file selector and loads each diff with immutable head/base SHAs and a literal path. Binary and metadata-only changes are distinguished from an unchanged commit. Partial diff/tree results retain a visible truncation notice. Cross-tab session changes clear live identity/data/request epochs; live worktrees can never fall back to simulated demo mutations.

Gitust governs its local state and execution. The agent client separately controls model interactions; a remote AI provider may receive source code depending on that configuration.
