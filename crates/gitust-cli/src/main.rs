//! `gitust`: The All-in-One Pure Rust Git + Container-Use Worktree + Microsandbox CLI & Server.

mod sandbox_cmd;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gitust_core::{EnterpriseRole, RepoId};
use gitust_db::AuthStore;
use gitust_git::GitEngine;
use gitust_sandbox::builtin_mcp_tools;
use gitust_sync::BiDirectionalSyncConfig;

use crate::sandbox_cmd::run_cli_sandbox;

#[derive(Debug, Parser)]
#[command(
    name = "gitust",
    version,
    about = "All-in-One Pure Rust Agent-Native Git Server, Container-Use Worktree Isolation & Microsandbox CI"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum AdminCommands {
    /// Create the initial Site Administrator account (CLI-only bootstrap; web bootstrap is strictly disabled)
    Create {
        /// Initial Site Administrator username
        #[arg(long)]
        username: String,

        /// Initial Site Administrator corporate email (must match allowed domain, default @company.internal)
        #[arg(long)]
        email: String,

        /// Initial Site Administrator password (min 10 characters, hashed with Argon2id; supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        password: String,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },

    /// Issue a single-use 72-hour invite link (only SHA-256 stored in DB; share via messenger)
    Invite {
        /// SiteAdmin username or email authenticating this CLI action
        #[arg(long)]
        admin_username: String,

        /// SiteAdmin password (supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,

        /// Target invitee corporate email (must match allowed domain)
        #[arg(long)]
        email: String,

        /// Assigned role (`Developer`, `Maintainer`, or `SiteAdmin`)
        #[arg(long, default_value = "Developer")]
        role: String,

        /// Expiration in hours (1..=720, default 72)
        #[arg(long)]
        ttl_hours: Option<i64>,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },

    /// Issue a single-use password-reset link for an existing active user
    ResetPassword {
        /// SiteAdmin username or email authenticating this CLI action
        #[arg(long)]
        admin_username: String,

        /// SiteAdmin password (supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,

        /// Target user's username or email
        #[arg(long)]
        target: String,

        /// Expiration in hours (1..=72, default 72)
        #[arg(long)]
        ttl_hours: Option<i64>,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },

    /// Deactivate a user account and immediately revoke all sessions, PATs, SSH keys, and pending invites
    Deactivate {
        /// SiteAdmin username or email authenticating this CLI action
        #[arg(long)]
        admin_username: String,

        /// SiteAdmin password (supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,

        /// Target user's username or email
        #[arg(long)]
        target: String,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum SandboxSubcommands {
    /// Run a command inside an isolated CoW agent worktree and OS sandbox (`gitust sandbox run "<cmd>"`)
    Run {
        /// Command string to run inside the sandbox
        cmd: String,

        /// Repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// Keep the agent worktree and branch after the run
        #[arg(long)]
        keep: bool,

        /// Wall-clock timeout in seconds
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Site Administrator CLI management (`gitust admin create | invite | reset-password | deactivate`)
    Admin {
        #[command(subcommand)]
        action: AdminCommands,
    },

    /// Start the all-in-one Gitust server (Git + Web UI + Microsandbox + MCP + GitHub Two-Way Sync)
    Serve {
        /// Bind address for the HTTP/REST/MCP server
        #[arg(long, default_value = "127.0.0.1:3000")]
        bind: SocketAddr,

        /// Path to the local Git repository to serve
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// Upstream GitHub repository URL for real-time bi-directional mirroring
        #[arg(long, default_value = "https://github.com/sogangCSmaster/gitust.git")]
        sync_github: String,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,

        /// Allow SiteAdmins to run commands in agent worktrees over HTTP using the host OS
        /// sandbox. The host OS sandbox is not a multi-tenant boundary: only enable this on a
        /// trusted single-user deployment.
        #[arg(long)]
        trusted_single_user_sandbox: bool,

        /// Optional wall-clock ceiling in seconds for `git fetch` during mirror synchronization
        /// (defaults to 1800s for initial mirror sync, 300s for incremental sync, plus Git stall detection)
        #[arg(long)]
        sync_timeout: Option<u64>,
    },

    /// Start or inspect the built-in MCP (Model Context Protocol) stdio server for Claude Code / Cursor
    Mcp {
        /// Print registered MCP tools as JSON and exit
        #[arg(long)]
        list_tools: bool,

        /// Path to the local Git repository to serve
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// Upstream GitHub repository URL for mirror metadata
        #[arg(long, default_value = "https://github.com/sogangCSmaster/gitust.git")]
        sync_github: String,
    },

    /// Run a command in a fresh agent worktree of the current branch's last commit, inside the
    /// OS sandbox (no network, writes confined to the worktree). Supports both
    /// `gitust sandbox run "<cmd>"` and `gitust sandbox "<cmd>"`.
    #[command(args_conflicts_with_subcommands = true)]
    Sandbox {
        #[command(subcommand)]
        sub: Option<SandboxSubcommands>,

        /// Command string to run inside the sandbox (when `run` subcommand is omitted)
        cmd: Option<String>,

        /// Repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// Keep the agent worktree and branch after the run
        #[arg(long)]
        keep: bool,

        /// Wall-clock timeout in seconds
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },

    /// Synchronize an upstream GitHub (or local) repository into `--repo` as a read-only mirror
    /// while keeping all local `agent/*` branches private on-premise
    Sync {
        /// Upstream GitHub repository URL or local repository path
        #[arg(default_value = "https://github.com/sogangCSmaster/gitust.git")]
        github_url: String,

        /// Explicit target mirror repository path (required so working trees are never synced accidentally)
        #[arg(long)]
        repo: PathBuf,

        /// Only inspect local HEAD via zero-copy `gix` without fetching from the remote
        #[arg(long)]
        inspect_only: bool,

        /// Optional wall-clock ceiling in seconds for `git fetch` (defaults to 1800s for initial
        /// mirror sync and 300s for incremental sync, alongside Git stall detection)
        #[arg(long)]
        timeout: Option<u64>,
    },

    /// List agent worktrees of a repository (approval/merge arrives in Phase 3)
    Triage {
        /// Local repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

fn parse_enterprise_role(raw: &str) -> Result<EnterpriseRole> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "siteadmin" | "site-admin" | "admin" => Ok(EnterpriseRole::SiteAdmin),
        "maintainer" => Ok(EnterpriseRole::Maintainer),
        "developer" | "dev" => Ok(EnterpriseRole::Developer),
        other => {
            anyhow::bail!("unknown role '{other}' (expected Developer, Maintainer, or SiteAdmin)")
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Admin { action } => {
            tokio::task::spawn_blocking(move || handle_admin_command(action)).await??;
        }
        Commands::Serve {
            bind,
            repo,
            sync_github,
            auth_db,
            trusted_single_user_sandbox,
            sync_timeout,
        } => {
            let auth_store =
                tokio::task::spawn_blocking(move || AuthStore::open_persistent(&auth_db)).await??;
            gitust_server::serve(
                bind,
                repo,
                sync_github,
                auth_store,
                trusted_single_user_sandbox,
                sync_timeout,
            )
            .await?;
        }
        Commands::Mcp {
            list_tools,
            repo,
            sync_github,
        } => {
            if list_tools {
                let tools = builtin_mcp_tools();
                println!("{}", serde_json::to_string_pretty(&tools)?);
            } else {
                gitust_server::serve_mcp_stdio(repo, sync_github).await?;
            }
        }
        Commands::Sandbox {
            sub,
            cmd,
            repo,
            keep,
            timeout,
        } => {
            let (run_cmd, run_repo, run_keep, run_timeout) = match sub {
                Some(SandboxSubcommands::Run {
                    cmd,
                    repo,
                    keep,
                    timeout,
                }) => (cmd, repo, keep, timeout),
                None => {
                    let Some(cmd) = cmd else {
                        anyhow::bail!(
                            "missing sandbox command; usage: gitust sandbox run \"<cmd>\""
                        );
                    };
                    (cmd, repo, keep, timeout)
                }
            };
            let exit_code = run_cli_sandbox(&run_repo, &run_cmd, run_keep, run_timeout).await?;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        Commands::Sync {
            github_url,
            repo,
            inspect_only,
            timeout,
        } => {
            let sync_cfg = BiDirectionalSyncConfig::new(RepoId::new(), &github_url);
            let (outcome_opt, snapshot) = tokio::task::spawn_blocking(move || -> Result<_> {
                let mut engine = GitEngine::new(&repo);
                if let Some(secs) = timeout {
                    engine = engine.with_fetch_timeout(std::time::Duration::from_secs(secs.max(1)));
                }
                let outcome = if inspect_only {
                    None
                } else {
                    Some(engine.sync_inbound_read_only(&github_url)?)
                };
                let snap = engine.inspect_head_zero_copy()?;
                Ok((outcome, snap))
            })
            .await??;

            if let Some(outcome) = outcome_opt {
                println!(
                    "[gitust sync] Inbound read-only mirror synced in {}ms ({} branches, {} tags, {} agent/* branches kept on-prem)",
                    outcome.sync_latency_ms,
                    outcome.synced_branches_count,
                    outcome.synced_tags_count,
                    outcome.private_agent_branches_filtered
                );
                for (branch, reason) in &outcome.skipped_branches {
                    println!("   [skipped branch] {branch}: {reason}");
                }
                for (tag, reason) in &outcome.skipped_tags {
                    println!("   [skipped tag] {tag}: {reason}");
                }
                if !outcome.local_only_branches.is_empty() {
                    println!(
                        "   [local-only branches preserved] {}",
                        outcome.local_only_branches.join(", ")
                    );
                }
            }
            println!(
                "[gitust sync] gix HEAD lookup: {}µs",
                snapshot.lookup_micros
            );
            println!(
                "   Branch: {} ({}) | Root Entries: {}",
                snapshot.branch_name,
                snapshot.head_sha.0,
                snapshot.root_entries.len()
            );
            println!(
                "   Mirror: {} (agent branch prefix '{}' stays on-prem)",
                sync_cfg.github_remote_url, sync_cfg.private_agent_branch_prefix
            );
        }
        Commands::Triage { repo } => {
            let entries = tokio::task::spawn_blocking(move || -> Result<_> {
                let engine = GitEngine::new(&repo);
                Ok(engine.list_agent_worktrees()?)
            })
            .await??;
            if entries.is_empty() {
                println!("No agent worktrees.");
            }
            for e in entries {
                println!("{}  {}  {}", e.branch, e.head.0, e.worktree_dir.display());
            }
        }
    }

    Ok(())
}

fn handle_admin_command(action: AdminCommands) -> Result<()> {
    match action {
        AdminCommands::Create {
            username,
            email,
            password,
            auth_db,
        } => {
            let store = AuthStore::open_persistent(&auth_db)?;
            let admin = store.bootstrap_initial_admin_cli(&username, &email, &password)?;
            println!(
                "✓ Bootstrapped initial SiteAdmin '{}' ({}) [id: {}] in {}",
                admin.username,
                admin.email,
                admin.id.0,
                auth_db.display()
            );
        }
        AdminCommands::Invite {
            admin_username,
            admin_password,
            email,
            role,
            ttl_hours,
            auth_db,
        } => {
            let assigned_role = parse_enterprise_role(&role)?;
            let store = AuthStore::open_persistent(&auth_db)?;
            let session = store.login(&admin_username, &admin_password)?;
            let caller = store.authenticate_session(&session.raw_session_token)?;
            let issued_res =
                store.create_invite_link(&caller, &email, assigned_role, vec![], ttl_hours);
            let _ = store.revoke_session(&caller, &session.raw_session_token);
            let issued = issued_res?;
            println!(
                "✓ Issued single-use invite link for {} ({:?}, expires {}):\n  URL path: {}\n  Token:    {}",
                issued.invite.target_email,
                issued.invite.assigned_role,
                issued.invite.expires_at,
                issued.invite_url_path,
                issued.raw_token
            );
        }
        AdminCommands::ResetPassword {
            admin_username,
            admin_password,
            target,
            ttl_hours,
            auth_db,
        } => {
            let store = AuthStore::open_persistent(&auth_db)?;
            let session = store.login(&admin_username, &admin_password)?;
            let caller = store.authenticate_session(&session.raw_session_token)?;
            let target_norm = target.trim().to_ascii_lowercase();
            let users_res = store.list_users(&caller);
            let users = match users_res {
                Ok(u) => u,
                Err(e) => {
                    let _ = store.revoke_session(&caller, &session.raw_session_token);
                    return Err(e.into());
                }
            };
            let Some(found) = users.into_iter().find(|u| {
                u.username.eq_ignore_ascii_case(&target_norm)
                    || u.email.eq_ignore_ascii_case(&target_norm)
            }) else {
                let _ = store.revoke_session(&caller, &session.raw_session_token);
                anyhow::bail!("user '{target}' not found");
            };
            let issued_res = store.create_password_reset_link(&caller, found.id, ttl_hours);
            let _ = store.revoke_session(&caller, &session.raw_session_token);
            let issued = issued_res?;
            println!(
                "✓ Issued single-use password-reset link for '{}' ({}, expires {}):\n  URL path: {}\n  Token:    {}",
                found.username,
                found.email,
                issued.invite.expires_at,
                issued.invite_url_path,
                issued.raw_token
            );
        }
        AdminCommands::Deactivate {
            admin_username,
            admin_password,
            target,
            auth_db,
        } => {
            let store = AuthStore::open_persistent(&auth_db)?;
            let session = store.login(&admin_username, &admin_password)?;
            let caller = store.authenticate_session(&session.raw_session_token)?;
            let target_norm = target.trim().to_ascii_lowercase();
            let users_res = store.list_users(&caller);
            let users = match users_res {
                Ok(u) => u,
                Err(e) => {
                    let _ = store.revoke_session(&caller, &session.raw_session_token);
                    return Err(e.into());
                }
            };
            let Some(found) = users.into_iter().find(|u| {
                u.username.eq_ignore_ascii_case(&target_norm)
                    || u.email.eq_ignore_ascii_case(&target_norm)
            }) else {
                let _ = store.revoke_session(&caller, &session.raw_session_token);
                anyhow::bail!("user '{target}' not found");
            };
            let deactivated_res = store.deactivate_user(&caller, found.id);
            let _ = store.revoke_session(&caller, &session.raw_session_token);
            let deactivated = deactivated_res?;
            println!(
                "✓ Deactivated user '{}' ({}) [id: {}] and revoked all sessions, PATs, SSH keys, and pending invites",
                deactivated.username, deactivated.email, deactivated.id.0
            );
        }
    }
    Ok(())
}
