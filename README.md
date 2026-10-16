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

Gitust is in early development. Workspace and mirror foundations are implemented; internal Git hosting, approval, and Checks are being developed as Phase 3. APIs and storage formats may change.

| Area | Available now |
|---|---|
| Workspaces | Managed `agent/*` branches, linked worktrees, ownership checks, and startup recovery |
| Execution | Local commands using macOS Seatbelt or Linux Bubblewrap |
| Verification | Clean SHA checks, required commands from the merge target's committed `.gitust-checks.json`, and per-command results |
| Approval and publication | Human-approved fast-forward merge, exact-SHA push, persisted records, and explicit retry |
| Git hosting | Smart HTTP clone/fetch and restricted pushes to managed agent branches |
| Mirroring | Explicit inbound synchronization that preserves local work and private agent refs |
| Agent integration | MCP over stdio or HTTP; OAuth with PKCE for HTTP MCP |
| Accounts | CLI bootstrap, administrator-issued invitations, roles, PATs, SSH key management, and revocation |
| Web UI | Embedded SolidJS workbench for browsing and the agent verification/approval/publication flow |

SSH Git serving, automatic Checks on branch updates, full pull-request collaboration, GitHub Actions compatibility, microVM execution, SSO, and multi-node scheduling are **planned**. Some additional workbench views are UI demonstrations. The [roadmap](docs/roadmap.md) separates implementation from plans.

## Build from source

You need Rust, Git, Node.js, and pnpm 10. The current development checks run with Rust 1.92.0. Sandbox execution requires macOS with `sandbox-exec`, or Linux with `bwrap` and the necessary namespace support. Windows sandbox execution is not implemented.

```sh
git clone https://github.com/sogangCSmaster/gitust.git
cd gitust
pnpm install --frozen-lockfile
pnpm --filter @gitust/design-system build
cargo build --release -p gitust-cli
```

The binary is `target/release/gitust`. Build the web bundle before compiling the server: the binary embeds it. Gitust is written in Rust but currently invokes the Git CLI for several operations; it does not require Gitea, Forgejo, Docker, or Dagger.

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

The upstream URL configures synchronization and publication. Starting the server does not automatically mirror or publish commits. Private outbound GitHub authentication currently uses SSH keys/agent and a preconfigured `known_hosts`, rather than HTTPS credential helpers.

For frontend development, keep the Rust server running on `127.0.0.1:3000` and start Vite in another terminal:

```sh
pnpm --filter @gitust/design-system dev
```

Open [localhost:4200](http://127.0.0.1:4200). Vite forwards API, MCP, OAuth, discovery, and Git requests to the Rust server. Sign in through **Access** with the CLI-created account to use real repository data. Worktrees, repository browsing, and account controls connect to the server; Issues and Actions remain labelled interface demos. The Components view is a design reference. Use a separate clone for experiments when your development checkout has uncommitted work.


## CLI and agent workflow

These examples assume `gitust` is on your `PATH`; you can also use the built binary by its full path.

```sh
# Inspect the agent-facing tools.
gitust mcp --list-tools

# Connect an MCP client using stdio.
gitust mcp --repo /absolute/path/to/repository

# Run against a fresh worktree of the current committed revision.
gitust sandbox run "cargo test --offline" --repo /absolute/path/to/repository

# Import an upstream into a new local mirror.
gitust sync https://github.com/YOUR_ORG/YOUR_REPO.git --repo /absolute/path/to/new-mirror.git

# List managed agent worktrees.
gitust triage list --repo /absolute/path/to/repository
```

One worktree service owns a repository at a time. When `serve` is running, connect agents to its `/mcp` HTTP endpoint instead of starting another stdio service on the same repository.

In the workbench, run verification, inspect the result, select **Approve & Merge**, then **Publish to GitHub**. Publication sends the approved SHA and its reachable history, not every local branch. A failed push preserves the local merge for explicit retry.

- [Verification and required Checks](docs/architecture/worktree-verification.md)
- [Approval, publication, recovery, and CLI examples](docs/architecture/outbound-sync.md)

Local `triage approve` is an explicit operator action: it does not automatically run Checks or consume server verification evidence. Use HTTP/workbench approval when enforcing server verification policy.

## Architecture and performance

Gitust uses Rust, Tokio, Axum, and `gix`, with a SolidJS frontend built on locally owned [solid-ui](https://www.solid-ui.com/) components, Kobalte and Tailwind CSS. Git objects, worktrees, authentication state, and approval records stay on the deployment's storage. The auth store currently uses lock-protected JSON snapshots; SQL storage is not implemented.

Self-hosting Gitust does not determine what an external AI model provider receives. That depends on the agent client and model configuration. Connecting a cloud-backed agent does not guarantee that all code stays local.

Fast interaction and low-overhead workspaces are design goals. Sub-millisecond CoW worktrees and fixed API latency budgets are not achieved guarantees. See the [measured worktree benchmark](docs/benchmarks/agent-worktree.md) and [architecture overview](docs/architecture/overview.md).

## Contributing

Start with a focused bug report, reproduction, or change proposal. Contributions are accepted under `AGPL-3.0-only`; contributors retain their copyright. No copyright assignment or commercial relicensing permission is implied. For code changes, read [AGENTS.md](AGENTS.md), preserve unrelated work, and include tests for behavior that could regress.

```sh
git config core.hooksPath .githooks
scripts/verify.sh
```

The script checks file sizes, fail-closed handling, Rust formatting, Clippy, Rust tests, frontend types/tests, and embedded-bundle freshness. Integration tests require local listeners and OS sandbox permissions. Benchmarks run separately. Independent review gates are documented in `AGENTS.md`.

## License and project direction

Copyright (c) 2026 Gitust contributors. Gitust is licensed under the **GNU Affero General Public License v3.0 only** (`AGPL-3.0-only`); see [LICENSE](LICENSE). Third-party dependencies retain their own licenses.

Commercial use is permitted. If you modify Gitust and let users interact with that version over a network, AGPL section 13 requires offering those users its Corresponding Source. Hosting a repository in Gitust does not by itself change that repository's license. The license text controls the full terms, including redistribution and warranty disclaimers.

This license applies to the current version; it does not revoke permissions already granted for earlier versions. Future enterprise operations are described in [Open-Core direction](docs/business/open-core.md). This is a product plan, not an implemented edition, commercial license offer, or pricing scheme.
