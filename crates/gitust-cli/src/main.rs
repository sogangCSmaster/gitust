//! `gitust`: The All-in-One Pure Rust Git + Container-Use Worktree + Microsandbox CLI & Server.

mod admin_cmd;
mod sandbox_cmd;
#[cfg(test)]
mod tests_cli_review;
mod triage_cmd;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gitust_core::RepoId;
use gitust_db::AuthStore;
use gitust_git::GitEngine;
use gitust_sandbox::builtin_mcp_tools;
use gitust_sync::BiDirectionalSyncConfig;

use crate::admin_cmd::{handle_admin_command, AdminCommands};
mod repository_cmd;
#[cfg(unix)]
mod repository_create_cmd;
#[cfg(unix)]
mod repository_import_cmd;
#[cfg(unix)]
mod repository_import_storage;
#[cfg(unix)]
mod repository_provision;
#[cfg(all(test, unix))]
mod tests_repository_create;
#[cfg(all(test, unix))]
mod tests_repository_import;
use crate::sandbox_cmd::{recover_cli_sandbox, run_cli_sandbox};
use crate::triage_cmd::{
    run_triage_approve, run_triage_discard, run_triage_list, run_triage_publish, TriageSubcommands,
};

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
enum SandboxSubcommands {
    /// Run a command inside an isolated agent worktree and OS sandbox (`gitust sandbox run "<cmd>"`)
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
    /// Clear interrupted execution fences only after the operator stops all prior commands
    Recover {
        /// Explicit repository path; the server and all old sandbox processes must be stopped
        #[arg(long)]
        repo: PathBuf,

        /// Confirm that every command from the previous service has stopped
        #[arg(long, required = true)]
        acknowledge_stopped_processes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Explicitly register the configured local Git repository with its authorization store
    Repository {
        #[command(subcommand)]
        action: repository_cmd::RepositoryCommands,
    },
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

        /// Existing ACL-registered repository available for browsing only (repeatable; total limit 32)
        #[arg(long = "read-only-repo")]
        read_only_repos: Vec<PathBuf>,

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

    /// Local stdio MCP for unregistered repositories; registered repositories use authenticated HTTP /mcp
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

    /// List, approve (fast-forward merge), or discard isolated `agent/*` branches (`gitust triage [list | approve | discard]`)
    #[command(args_conflicts_with_subcommands = true)]
    Triage {
        #[command(subcommand)]
        sub: Option<TriageSubcommands>,

        /// Local repository path (when `list` subcommand is omitted)
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // fail-open-ok: invalid logging filter only changes log verbosity; no authorization or execution policy is relaxed.
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Repository { action } => {
            tokio::task::spawn_blocking(move || repository_cmd::handle(action)).await??;
        }
        Commands::Admin { action } => {
            tokio::task::spawn_blocking(move || handle_admin_command(action)).await??;
        }
        Commands::Serve {
            bind,
            repo,
            read_only_repos,
            sync_github,
            auth_db,
            trusted_single_user_sandbox,
            sync_timeout,
        } => {
            let auth_store =
                tokio::task::spawn_blocking(move || AuthStore::open_persistent(&auth_db)).await??;
            gitust_server::serve_with_read_only_repositories(
                bind,
                repo,
                sync_github,
                auth_store,
                trusted_single_user_sandbox,
                sync_timeout,
                read_only_repos,
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
                Some(SandboxSubcommands::Recover {
                    repo,
                    acknowledge_stopped_processes,
                }) => {
                    recover_cli_sandbox(&repo, acknowledge_stopped_processes).await?;
                    return Ok(());
                }
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
        Commands::Triage { sub, repo } => {
            tokio::task::spawn_blocking(move || -> Result<()> {
                match sub {
                    None => run_triage_list(&repo),
                    Some(TriageSubcommands::List { repo }) => run_triage_list(&repo),
                    Some(TriageSubcommands::Approve {
                        branch,
                        base,
                        repo,
                        push_to,
                    }) => run_triage_approve(&repo, &branch, &base, push_to.as_deref()),
                    Some(TriageSubcommands::Discard { branch, repo }) => {
                        run_triage_discard(&repo, &branch)
                    }
                    Some(TriageSubcommands::Publish {
                        approved_commit,
                        base,
                        to,
                        repo,
                    }) => run_triage_publish(&repo, &base, &approved_commit, &to),
                }
            })
            .await??;
        }
    }

    Ok(())
}
