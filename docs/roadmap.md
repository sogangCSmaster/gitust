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
- Opt-in SSH clone/fetch (`git-upload-pack`) and permission-checked pushes (`git-receive-pack`) for configured repositories using Ed25519 and ECDSA NIST P-256/384/521 user keys, a persisted private Ed25519 host identity, multi-stage key-binding/account/ACL/RBAC checks, automatic Checks integration on the primary repository, and web Clone popover SSH address discovery via `ssh_port`. RSA SSH transport authentication remains separate work. See the [SSH hosting contract](operations/ssh-read-only.md).
- Local triage commands and the live web agent workflow.
- Clean-commit verification, fast-forward approval, and separate exact-SHA publication.
- Durable approval/publication records and explicit remote-failure retry.
- Required Checks from committed merge-target configuration during explicit verification.
- Explicit local CLI Checks for the current or explicitly selected ordinary branch at an expected full commit ID, with committed configuration, real per-step results and temporary-checkout cleanup. Selecting a branch does not change HEAD, grant approval or add ordinary-branch automatic triggers. See the [local execution contract](operations/local-checks.md).
- Opt-in local Checks history retains five completed results under the persisted repository identity, with bounded output and CLI list/run-ID queries. It survives offline relocation and remains separate from managed-worktree verification and server Activity.
- SiteAdmin read-only inspection of primary and writable secondary repository local Checks history in Actions, using a bounded UUID-scoped API and a separate compact Local Checks view. Reading stored diagnostics does not enable HTTP execution or restore approval authority.
- Explicit SiteAdmin execution of committed ordinary-branch (`refs/heads/*`) Local Checks from Actions on the active primary or explicitly writable secondary repositories (`--git-write-repo`), when trusted single-user execution is enabled. Expected ref/SHA, original-credential checks and confirmed cleanup bind the result; completed runs share the bounded local history. Execution and saving failures remain distinct, with no automatic retry or approval authority.
- Opt-in automatic Local Checks after accepted SiteAdmin Git HTTP or SSH updates to ordinary branches (`refs/heads/*`) on the primary or explicitly writable secondary repositories (`--git-write-repo`), with prior committed consent on the pushed ref, bounded queue and conservative restart recovery. Diagnostics share the existing Local Checks history without granting approval. See the [default-branch contract](operations/default-branch-checks.md).
- Opt-in automatic Checks after accepted managed-worktree MCP commits and Git HTTP/SSH pushes, with bounded latest-update coalescing and diagnostic history. Automatic success does not grant approval; explicit manual verification remains required.
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
- A bounded operator-configured repository catalog with UUID-routed commit/tree/blob/diff reads and team filtering. Additional repositories retain independent Git service ownership; explicitly enabled writable secondaries can host managed worktrees and ordinary-branch Local Checks. Metadata-only records do not create a Git service.
- Authenticated UUID Smart HTTP clone/fetch for configured additional repositories, preserving current credential/ACL checks before and after Git work and service ownership through cancellation. Entries configured with `--read-only-repo` keep receive-pack disabled for every role. The compact Clone dialog uses explicit current-access hints. See the [transport contract](operations/read-only-git.md).
- Explicit `--git-write-repo` hosting for registered bare secondary repositories without worktrees. Current Maintainers and SiteAdmins with access may push ordinary branches and tags; agent refs remain denied. The compact catalog distinguishes Git hosting from a full workbench. See the [configuration and authority contract](operations/secondary-git-hosting.md).
- Offline import of an existing Git source into a fresh registered bare repository, with an explicit default branch, private fixed-identity journal, original administrator-session rechecks and no-overwrite publication. Sealed retries preserve the same snapshot and registration pair. Serving an offline import still requires explicit configuration; the separate online creation API can register newly created hosted repositories without restarting. See the [import contract](operations/repository-import.md).
- SiteAdmin display-name editing for configured registered repositories, with a compact Overview dialog and durable metadata-only changes. UUID, registration binding, team, privacy and Git paths remain unchanged. Metadata-only records and legacy unregistered services cannot be renamed through this API.
- SiteAdmin access-scope management for configured registered repositories, with current-value comparison and original-credential checks inside the durable auth transaction. Team-only and all-active-signed-in-user scopes retain existing role permissions and keep anonymous access disabled. The compact Overview form confirms the server result; uncertain writes require refresh. See the [scope and recovery contract](operations/repository-access.md).
- SiteAdmin invitations to a configured repository's owning group, with a compact form inside access settings, a server-derived grant and durable per-key duplicate prevention. Original credentials are rechecked after auth write-lock waits for group invitations, the existing HTTP invitation family, credential mutations, account deactivation and repository metadata writes. Replays do not recover the one-use link or remint consumed/revoked invitations. Roles remain site-wide, and group membership may open other private repositories. See the [invitation contract](operations/repository-group-invites.md).
- Exact-account owning-group membership management from access settings, with durable version comparisons that reject stale grants after removal. Other groups, site-wide roles and account status remain unchanged; inactive accounts can be removed but cannot be added. See the [membership and stored-state compatibility contract](operations/group-membership.md).
- Catalog and web support for a valid default branch without a commit, using an explicit nullable HEAD. Primary Smart HTTP accepts an authorized first ordinary push; other branches and existing tasks remain distinct from the missing default commit. The compact Clone dialog provides credential-free UUID URLs for hosted primary and read-only secondary repositories.
- Offline creation of a fresh registered bare repository with an explicit unborn default branch, fixed identity, shared create/import destination admission, sealed-empty retries and no-overwrite publication. Serving it still requires explicit configuration. See the [creation and recovery contract](operations/repository-create.md).
- Bounded UUID branch pages and a compact Files/Changes/Actions selector for primary and hosted secondary repositories, including alternate branches before a default commit exists. Reads use identified commit SHAs; list refresh preserves an explicit snapshot. This adds browsing without branch mutation. See the [branch browsing contract](operations/branch-browsing.md).
- SiteAdmin default-branch management for configured registered bare primaries and explicitly writable secondaries, with a compact Overview dialog and symbolic HEAD comparison under its actual lock. Existing worktree targets remain pinned; new Git clones and primary omitted-base HTTP/MCP tasks use the current committed default. See the [default-branch contract](operations/default-branch.md).
- A stopped-installation backup/relocated-restore utility, regression tests and recorded Linux/macOS rehearsals at `42bae69`, including MCP commits after relocation. Explicit multi-repository snapshots now preserve a shared auth checkpoint and every included repository, with v1 single-repository compatibility and local restored-only ACL/approval checks. See the [backup contract](operations/backup-restore.md); final-candidate acceptance remains open.
- Initial JavaScript static-import closure remains below the `< 50 kB` (`< 50,000` bytes) gzip target (`49,656` gzip9 bytes / `49,684` gzip6 bytes in the 2026-10-19 candidate), measured with `measure-bundle.mjs --enforce`. See [web bundle measurements](benchmarks/web-bundle.md).
- Pull-request collaboration and branch protection on configured repositories: PR creation, head-SHA-bound reviews (`approve`, `request_changes`, `comment`), general and inline file/line comments, merge preview with conflict detection, branch protection rules (required approvals, passing Local Checks, direct-push restrictions), and lock-time CAS merge execution integrated into the lazy-loaded Changes view.
- Runtime repository administration, lifecycle management and online inventory registration (`POST /api/v1/admin/repos`, `GET/PATCH /api/v1/repos/{id}/settings` with `name`, `default_branch`, `is_private`, `archived`, `description`, and `workbench_enabled`), enforced across Smart HTTP, SSH, PR mutations, Local Checks, and worktree creation.

Remaining operational & validation work:

- RSA user-key authentication on the SSH transport.
- Final-candidate restoration evidence for auth state, Git data, worktrees and approval records on the intended platforms.
- External deployment validation, platform coverage, and final-candidate acceptance checks.

The MCP commit implementation passed full checks, independent architecture review and ten-task rehearsals. Hosted Linux/Bubblewrap and macOS/Seatbelt checks, relocation and crash recovery passed at `42bae69`. The historical independent HTTP review has partial executed evidence but no final verdict because further checks were interrupted by tool policy. Actual intended IDE-agent sessions, authorized GitHub publication, a separately provisioned installation and validation of later changes remain open. Track those boundaries in the [alpha progress report](operations/alpha-progress-2026-10-17.md); all nine acceptance conditions remain in force.

## Phase 4 — Source of truth and enterprise operations

In progress. The following foundations exist; they do not complete Phase 4:

- **Source-of-truth topology (`4.1`)**: per-repository topology (`github_primary` or `gitust_primary`) via `GET/PUT /api/v1/repos/{id}/sync/topology`, non-agent ordinary-branch/tag backup mirror pushes (`POST /api/v1/repos/{id}/sync/backup-push`) with durable `BackupPushReceipt` records, and CAS-verified rollback (`POST /api/v1/repos/{id}/sync/rollback`) that restores ordinary refs from the recorded snapshot without touching `refs/heads/agent/*`.
- **JSON workflow subset (`4.2`)**: committed `.gitust-workflows.json` and `.github/workflows/*.json` definitions with DAG `needs`, bounded matrix expansion and local command execution. YAML workflows and `uses` actions are unsupported. Repository secrets use AES-256-GCM with an external key; see the [security upgrade procedure](operations/security-upgrade.md). Caches are local and addressed by workflow keys; SHA-256-derived filenames do not make them content-addressed or multi-node storage. Artifacts are bounded text previews, not general binary artifact upload/download.
- **Execution and runner metadata (`4.3`)**: local `host_os` execution uses Seatbelt or Bubblewrap. SiteAdmins can register runner metadata, labels and heartbeats, but remote runner execution is not implemented and requests for remote runners are rejected. `container_vm` is unsupported and rejected. Container/microVM isolation, remote dispatch and shared multi-node caches remain planned.
- **Account security foundations (`4.4`)**: encrypted provider configuration and a custom HMAC-SHA256 signed assertion bridge (`POST /api/v1/auth/sso/login`), RFC 6238 TOTP enrollment/verification with replay protection, and a SHA-256 hash-chained audit log. Provider labels `oidc`/`ldap` do not implement OIDC discovery/token verification or LDAP connection/bind. Native identity-provider integration and external integration validation remain open. Encryption and legacy account migration requirements are in the [security upgrade procedure](operations/security-upgrade.md).
- **Indexed repository-authorized code & symbol search (`4.5`)**: commit-pinned trigram and symbol indexing (`GET /api/v1/repos/{id}/search?q=<query>&rev=<rev>&kind=all|code|symbol`) reading committed blobs from the `gix` object database (`<common_git_dir>/gitust-search`, hidden from sandboxes), rejecting `refs/heads/agent/*` revisions and agent-only commits, and enforcing BOLA and post-read credential re-verification.


Phase 4 remains open for the missing integrations above, operating evidence and final-candidate validation. Phase 3 and the alpha checkpoint also retain their remaining acceptance requirements.

## Validation

Follow [AGENTS.md sections 7.4 and 7.5](../AGENTS.md). Publish measurements with their environment and keep incomplete review/platform coverage visible. The [worktree benchmark](benchmarks/agent-worktree.md) records the gap between current checkout performance and the CoW target.
