# Gitust

**A self-hosted Git workspace for AI coding agents.**

Gitust brings Git hosting, agent worktrees, local command execution, and human-approved publication into one Rust CLI and server with a keyboard-driven web UI.

Let agents work in separate branches on infrastructure you control, verify the exact commit you intend to merge, and publish the result to GitHub when a person approves it. Experimental `agent/*` branches stay in Gitust's local workflow.

## Why Gitust?

Running several coding agents usually means coordinating a Git server, worktree scripts, execution environments, CI, and review tools. Gitust is building a shared workflow around those pieces:

1. **Work locally.** Give each task a managed Git worktree and branch.
2. **Verify a commit.** Run commands in a local OS sandbox and bind verification to a clean commit SHA.
3. **Review and merge.** A human approves a verified change for a local fast-forward merge.
4. **Publish deliberately.** Send the approved SHA upstream separately from merging.

The goal is an open-source foundation for teams that want to operate their own agent development infrastructure while keeping their existing GitHub repositories. See the [project vision](docs/vision.md).

## Current status

Gitust is in early development. Workspace, hosting, approval, Checks and some Phase 4 foundations are implemented. Phase 3, Phase 4 and alpha acceptance remain open; passing development tests does not establish release readiness. APIs and storage formats may change.

The current milestone is a trusted single-user self-hosted alpha: operate concurrent agent worktrees, verify and approve exact commits, publish deliberately, and recover the installation. See the [alpha acceptance conditions](docs/operations/alpha-acceptance.md) for the scope and required evidence.

| Area | Available now |
|---|---|
| Workspaces | Managed `agent/*` branches, linked worktrees, persisted repository identity, ownership checks, and startup recovery |
| Execution | Local commands using macOS Seatbelt or Linux Bubblewrap (`host_os`); runner registration/heartbeat metadata only. Remote execution and `container_vm` requests are rejected |
| Verification & Workflows | Clean SHA checks, required commands from `.gitust-checks.json`, opt-in automatic Checks, and JSON workflows with DAG `needs`, matrices and encrypted secrets; local key-addressed caches and bounded text artifact previews. YAML and `uses` actions are unsupported |
| Pull Requests & Branch Protection | PR creation, head-SHA-bound reviews (`approve`, `request_changes`, `comment`), inline diff comments, branch protection rules, and lock-time CAS merge execution |
| Approval and publication | Human-approved fast-forward merge, exact-SHA push, persisted records, and explicit retry |
| Git hosting | Authenticated Smart HTTP and optional SSH (`--ssh-bind`, Ed25519 and ECDSA P-256/384/521 user keys) clone, fetch, and permission-checked pushes, plus online repository lifecycle administration |
| Mirroring & Source of Truth | Explicit inbound synchronization, per-repository `github_primary` / `gitust_primary` topology, upstream backup mirror pushes, and CAS-verified rollback preserving private `agent/*` refs |
| Code & Symbol Search | Commit-pinned trigram code and symbol search (`GET /api/v1/repos/{id}/search`) over committed `gix` blobs with `agent/*` isolation and BOLA enforcement |
| Agent integration | MCP over stdio or HTTP; explicit host-side worktree commits; OAuth with PKCE for HTTP MCP |
| Accounts & Enterprise Security | CLI bootstrap, administrator-issued invitations, roles, PATs, SSH key management, revocation, a custom HMAC-signed SSO assertion bridge, RFC 6238 TOTP 2FA, and a SHA-256 hash-chained audit log; native OIDC/LDAP integration remains planned |
| Web UI | Embedded SolidJS workbench (`< 50 kB` initial gzip JS closure) for repository browsing, search, PRs, workflows, access administration, and the agent verification/approval/publication flow |

See the [roadmap](docs/roadmap.md) for architectural details and operational validation boundaries. Existing installations must follow the [security upgrade procedure](docs/operations/security-upgrade.md), including an externally managed secret key and legacy credential rotation.

Automatic Checks require `"auto_run": true` in the merge target's committed Checks configuration, trusted single-user execution, and a SiteAdmin acting on their own managed worktree (or the explicit local stdio operator). Completed automatic results appear in run history; approval still requires manual verification. Durable queued jobs can resume after current authority and Git state are rechecked; uncertain running jobs become interrupted and are never automatically replayed. Local filesystem Git commits remain outside these triggers; the primary default branch has a separate [opt-in Git HTTP/SSH flow](docs/operations/default-branch-checks.md). See [verification behavior](docs/architecture/worktree-verification.md) and [automatic-run recovery](docs/architecture/automatic-checks.md).

SiteAdmins can browse retained automatic attempts in the repository's read-only **Actions** view and open the exact run in **Activity**. This view does not start workflows.

## Build from source

You need Rust, Git 2.34 or later, Node.js, and pnpm 10. The current development checks run with Rust 1.92.0. Repository identity storage currently supports macOS and Linux. Sandbox execution requires macOS with `sandbox-exec`, or Linux with `bwrap` and the necessary namespace support. Windows sandbox execution is not implemented.

```sh
git clone https://github.com/sogangCSmaster/gitust.git
cd gitust
pnpm install --frozen-lockfile
pnpm --filter @gitust/design-system build
cargo build --release -p gitust-cli
```

The binary is `target/release/gitust`. Build the web bundle before compiling the server: the binary embeds the stylesheet and every generated JavaScript module, including optional screens loaded on demand. No static-file directory or external CDN is needed at runtime. Gitust is written in Rust but currently invokes the Git CLI for several operations; it does not require Gitea, Forgejo, Docker, or Dagger.

## Try it locally

From the repository root, create the first administrator. The default email policy accepts `company.internal`. Use a password of at least 10 characters; the prompt keeps it out of command history.

```bash
printf 'Initial administrator password: '
read -r -s GITUST_ADMIN_PASSWORD
printf '\n'
export GITUST_ADMIN_PASSWORD
./target/release/gitust admin create \
  --username admin --email admin@company.internal \
  --auth-db "$PWD/.gitust/auth-db.json"
unset GITUST_ADMIN_PASSWORD
```

Start the server against a repository you want to manage. Replace the example paths and upstream URL:

```sh
./target/release/gitust serve \
  --repo /absolute/path/to/your/repository \
  --auth-db "$PWD/.gitust/auth-db.json" \
  --sync-github git@github.com:YOUR_ORG/YOUR_REPO.git
```

Open [localhost:3000](http://127.0.0.1:3000) and sign in. There is no public signup or web-based first-admin bootstrap. The server binds to loopback by default; TLS termination is an operator responsibility when exposing HTTP beyond the local machine.

To enable server-side command execution, add `--trusted-single-user-sandbox` on a **trusted single-user installation**. Execution also requires a SiteAdmin. The current OS sandbox is not a multi-tenant security boundary. Network access is disabled inside runs; prepare tools and dependencies before executing builds.

Add repeated `--read-only-repo /absolute/path/to/another/repository` arguments for other existing repositories registered against the same auth database. The server supports up to 32 configured repositories in total; secondary repositories expose Files, Changes and authenticated Smart HTTP clone/fetch through their UUID. To permit ordinary Git pushes on a registered bare secondary, explicitly use [`--git-write-repo`](docs/operations/secondary-git-hosting.md) instead. Worktrees, execution, MCP and synchronization remain on the primary `--repo`. See [registration and read-only repositories](docs/operations/repository-registration.md) and the [secondary Git contract](docs/operations/read-only-git.md).

Optional [SSH Git hosting](docs/operations/ssh-read-only.md) uses `--ssh-bind`, a private persisted Ed25519 host key, and registered Ed25519 or ECDSA NIST P-256/384/521 user keys. It serves UUID clone/fetch and permission-checked pushes for the same configured repositories and current access rules, and exposes `ssh_port` in `GET /api/v1/repos` so the web Clone popover can switch between HTTP and SSH URLs. SSH is disabled by default.

To prepare a new registered bare repository from an existing Git source, use the offline [`repository import`](docs/operations/repository-import.md) command. It preserves a fixed import identity through retries and never overwrites an existing destination; serving it still requires explicit configuration.

For a fresh repository without source commits, use offline [`repository create`](docs/operations/repository-create.md). It registers a new bare repository with an explicit default branch; serving it and making the first authorized push remain separate steps.

SiteAdmins can rename the display name of a configured registered repository from its overview. Its UUID, team/privacy settings, host path, Git URLs and configured Git mode stay unchanged.

Use **Invite to owning group** in a registered repository's access settings to issue a 72-hour single-use account link with its server-selected group membership. This can also grant access to other private repositories owned by that group; roles retain their site-wide meaning. See the [scope, retries and recovery contract](docs/operations/repository-group-invites.md).

For an existing account, use **Manage existing member** to find its exact username and add or remove the owning-group membership. Changes preserve other groups and site-wide roles and reject stale membership versions. See [membership changes and stored-state compatibility](docs/operations/group-membership.md).

Use the compact [branch selector](docs/operations/branch-browsing.md) in Files or Changes to read a branch at its identified commit. Other branches remain browsable when the default branch has no commit; refreshing the list preserves the selected snapshot.

SiteAdmins can change a registered bare primary or `--git-write-repo` secondary's [default branch](docs/operations/default-branch.md) from Overview. New clones use the selected branch. On the primary, omitted-base HTTP/MCP tasks also use it; existing tasks keep their original approval target.

The upstream URL configures synchronization and publication. Starting the server does not automatically mirror or publish commits. Private outbound GitHub authentication currently uses SSH keys/agent and a preconfigured `known_hosts`, rather than HTTPS credential helpers.

For frontend development, keep the Rust server running on `127.0.0.1:3000` and start Vite in another terminal:

```sh
pnpm --filter @gitust/design-system dev
```

Open [localhost:4200](http://127.0.0.1:4200). Vite forwards API, MCP, OAuth, discovery, and Git requests to the Rust server. Sign in through **Access** with the CLI-created account to use real repository data. Worktrees, repository browsing, account controls, and Actions connect to the server; Issues remains a labelled interface demo, as does Actions when signed out. The Components view is a design reference. Use a separate clone for experiments when your development checkout has uncommitted work.


## CLI and agent workflow

These examples assume `gitust` is on your `PATH`; you can also use the built binary by its full path.

```sh
# Inspect the agent-facing tools.
gitust mcp --list-tools

# Connect an MCP client using stdio for an unregistered local repository.
gitust mcp --repo /absolute/path/to/repository

# Run against a fresh worktree of the current committed revision.
gitust sandbox run "cargo test --offline" --repo /absolute/path/to/repository

# Run the current branch's committed Checks against its exact full commit ID.
gitust sandbox checks --repo /absolute/path/to/repository --expected-commit FULL_COMMIT_SHA

# Run Checks for another ordinary branch without changing the current/default branch.
gitust sandbox checks --repo /absolute/path/to/repository --ref refs/heads/topic --expected-commit TOPIC_FULL_COMMIT_SHA

# Retain a completed local result, then inspect the newest five recorded runs.
gitust sandbox checks --repo /absolute/path/to/repository --expected-commit FULL_COMMIT_SHA --record
gitust sandbox checks-history --repo /absolute/path/to/repository

# Import an upstream into a new local mirror.
gitust sync https://github.com/YOUR_ORG/YOUR_REPO.git --repo /absolute/path/to/new-mirror.git

# List managed agent worktrees.
gitust triage list --repo /absolute/path/to/repository
```

One worktree service owns a repository at a time. When `serve` is running, connect agents to its `/mcp` HTTP endpoint instead of starting another stdio service on the same repository.
Explicitly ACL-registered repositories require authenticated HTTP `/mcp`; local stdio operator access is currently unsupported for them. See [local repository registration](docs/operations/repository-registration.md).

The configured repository keeps its ID across server restarts and complete offline restoration. Keep its private Git metadata in backups; an incomplete identity record stops startup. This ID does not register a repository or bind team permissions. See the [identity and recovery contract](docs/architecture/repository-identity.md).

Sandbox commands can edit their own checkout, while shared Git administration stays read-only inside the sandbox. The host-side `gitust_worktree_commit` MCP tool commits explicitly listed files against an expected HEAD SHA. HTTP use requires trusted single-user execution, a SiteAdmin and worktree ownership; stdio runs under local operator authority. Every accepted commit attempt clears prior verification, and a successful commit requires fresh verification before human approval. See the [commit contract and recovery procedure](docs/architecture/mcp-commits.md).

The [ten-task rehearsal](docs/operations/alpha-rehearsal.md) passed with real MCP commits and relocated restoration. The [recorded hosted CI](docs/operations/evidence/alpha-ci-42bae69-2026-10-17.json) also passed on Linux/Bubblewrap and macOS/Seatbelt at revision `42bae69`. Actual intended IDE-agent use, authorized GitHub publication and a separately provisioned installation still need evidence; later changes require their own validation. See the [acceptance progress](docs/operations/alpha-progress-2026-10-17.md).

In the workbench, run verification, inspect the result, select **Approve & Merge**, then **Publish to GitHub**. Publication sends the approved SHA and its reachable history, not every local branch. A failed push preserves the local merge for explicit retry.

- [Verification and required Checks](docs/architecture/worktree-verification.md)
- [Explicit local Checks and machine-readable results](docs/operations/local-checks.md)
- [Automatic primary default-branch Checks](docs/operations/default-branch-checks.md)
- [MCP commits, supported Git normalization, and interrupted-commit recovery](docs/architecture/mcp-commits.md)
- [Approval, publication, recovery, and CLI examples](docs/architecture/outbound-sync.md)
- [Offline installation backup and relocated restoration](docs/operations/backup-restore.md)
- [Interrupted execution fences and explicit operator recovery](docs/operations/execution-recovery.md)

Local `triage approve` is an explicit operator action: it does not automatically run Checks or consume server verification evidence. Use HTTP/workbench approval when enforcing server verification policy.

For ordinary-branch diagnostics on an active trusted single-user server, a SiteAdmin can use **Actions → Local Checks → Run Checks**. It runs the primary repository's committed default branch at the displayed SHA and records the result. Execution success, saved history and merge approval are separate states. See the [local Checks contract](docs/operations/local-checks.md).

## Architecture and performance

Gitust uses Rust, Tokio, Axum, and `gix`, with a SolidJS frontend built on locally owned [solid-ui](https://www.solid-ui.com/) components, Kobalte and Tailwind CSS. Git objects, worktrees, authentication state, and approval records stay on the deployment's storage. The auth store currently uses lock-protected JSON snapshots; SQL storage is not implemented.

The [workbench interface](docs/architecture/interface-design.md) uses a compact system-font design with separate workspace and repository navigation, responsive list/detail views, and explicit verification, approval, and publication actions.

Self-hosting Gitust does not determine what an external AI model provider receives. That depends on the agent client and model configuration. Connecting a cloud-backed agent does not guarantee that all code stays local.

Fast interaction and low-overhead workspaces are design goals. Sub-millisecond CoW worktrees and fixed API latency budgets are not achieved guarantees. See the [measured worktree benchmark](docs/benchmarks/agent-worktree.md) and [architecture overview](docs/architecture/overview.md).

## Contributing

Start with a focused bug report, reproduction, or change proposal. Contributions are accepted under `AGPL-3.0-only`; contributors retain their copyright. No copyright assignment or commercial relicensing permission is implied. For code changes, read [AGENTS.md](AGENTS.md), preserve unrelated work, and include tests for behavior that could regress.

```sh
git config core.hooksPath .githooks
scripts/verify.sh
```

The script checks file sizes, fail-closed handling, Rust formatting, Clippy, Rust tests, frontend types/tests, offline backup/restore regressions (Python 3), and embedded-bundle freshness. Integration tests require local listeners and OS sandbox permissions. Benchmarks run separately. Independent review gates are documented in `AGENTS.md`.

## License and project direction

Copyright (c) 2026 Gitust contributors. Gitust is licensed under the **GNU Affero General Public License v3.0 only** (`AGPL-3.0-only`); see [LICENSE](LICENSE). Third-party dependencies retain their own licenses.

Commercial use is permitted. If you modify Gitust and let users interact with that version over a network, AGPL section 13 requires offering those users its Corresponding Source. Hosting a repository in Gitust does not by itself change that repository's license. The license text controls the full terms, including redistribution and warranty disclaimers.

This license applies to the current version; it does not revoke permissions already granted for earlier versions. Future enterprise operations are described in [Open-Core direction](docs/business/open-core.md). This is a product plan, not an implemented edition, commercial license offer, or pricing scheme.
