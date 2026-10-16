# Project vision

Gitust aims to make self-hosted agent development a coherent workflow: Git hosting, independent task workspaces, local execution, verification, human review, and deliberate publication.

## The problem

An agent can edit code quickly, but teams still need to know which task owns a workspace, what ran against which commit, who approved the result, and what may be published upstream. Those answers often span a Git server, scripts, execution tools, CI, and review interfaces.

Gitust brings these responsibilities together. It takes inspiration from self-hosted Git platforms, worktree-oriented agent tools, and sandbox runtimes while implementing its own Rust services.

## Four design goals

1. **A Rust implementation with a small operational footprint.** One `gitust` binary provides CLI commands, HTTP serving, MCP integration, and the embedded UI. Existing Rust libraries provide Git object access and HTTP foundations. The runtime currently also uses the Git CLI and an OS sandbox.
2. **Infrastructure you control.** Agents use local managed branches and worktrees. Stronger isolation and scheduling across local machines are future work. Capacity remains bounded by hardware and configured limits. The current executor is for trusted single-user use.
3. **Keep GitHub in the workflow.** Import upstream changes, keep intermediate work local, and publish a human-approved commit to the selected upstream branch. Synchronization is currently explicit; continuous event-driven mirroring and broader pull-request integration are planned.
4. **Fast, inspectable interaction.** Support keyboard-driven review and immediate feedback without mistaking a requested action for a completed Git operation. Publish measurements rather than presenting performance goals as results.

## Privacy and publication boundaries

An on-premise Gitust server does not make an external AI provider local. Organizations must separately choose agent/model configuration and policies for outbound data.

Publishing an approved commit also publishes the history and objects reachable from it. Keeping private branch names local does not remove their commits if those commits are ancestors of an approved change.

An optional remote compute service would require an explicit data-transfer and execution-trust design. It must not become a requirement for the self-hosted core or be described as zero data transfer without evidence.

## Adoption path

Start as a workspace and verification companion with GitHub as source of truth. Expand to internal hosting and approved publication. Consider Gitust as source of truth for selected repositories only after operational validation, including backup and recovery.

See the [roadmap](roadmap.md), [current architecture](architecture/overview.md), and [Open-Core direction](business/open-core.md). Agent implementation rules belong in [AGENTS.md](../AGENTS.md).
