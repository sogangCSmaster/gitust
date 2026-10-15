//! `gitust`: The All-in-One Pure Rust Git + Container-Use Worktree + Microsandbox CLI & Server.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gitust_core::{AgentKind, RepoId, WorktreeId};
use gitust_db::AuthStore;
use gitust_git::GitEngine;
use gitust_sandbox::{builtin_mcp_tools, MicrosandboxExecutor, SandboxSpec};
use gitust_sync::BiDirectionalSyncConfig;

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
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Site Administrator CLI management (`gitust admin create`)
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
    },

    /// Start or inspect the built-in MCP (Model Context Protocol) server for Claude Code / Cursor
    Mcp {
        /// Print registered MCP tools as JSON
        #[arg(long)]
        list_tools: bool,
    },

    /// Run a command in a fresh agent worktree of the current branch's last commit, inside the
    /// OS sandbox (no network, writes confined to the worktree). Uncommitted changes are not
    /// included. The worktree and its branch are removed afterwards unless --keep is given.
    Sandbox {
        /// Command string to run inside the sandbox
        cmd: String,

        /// Repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// Keep the agent worktree and branch after the run
        #[arg(long)]
        keep: bool,

        /// Wall-clock timeout in seconds (each run starts from a fresh checkout and a cold
        /// `target/`, so builds can take a while)
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },

    /// Inspect HEAD and root tree via zero-copy `gitoxide` (`gix`) engine and verify GitHub Two-Way Sync
    Sync {
        /// Upstream GitHub repository URL
        #[arg(default_value = "https://github.com/sogangCSmaster/gitust.git")]
        github_url: String,

        /// Local repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },

    /// List agent worktrees of a repository (approval/merge arrives in Phase 3)
    Triage {
        /// Local repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Admin { action } => match action {
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
        },
        Commands::Serve {
            bind,
            repo,
            sync_github,
            auth_db,
            trusted_single_user_sandbox,
        } => {
            let auth_store = AuthStore::open_persistent(&auth_db)?;
            gitust_server::serve(
                bind,
                repo,
                sync_github,
                auth_store,
                trusted_single_user_sandbox,
            )
            .await?;
        }
        Commands::Mcp { list_tools } => {
            let tools = builtin_mcp_tools();
            if list_tools {
                println!("{}", serde_json::to_string_pretty(&tools)?);
            } else {
                println!(
                    "Gitust MCP Server ready ({} tools registered for Claude Code / Cursor):",
                    tools.len()
                );
                for t in tools {
                    println!("  - {}: {}", t.name, t.description);
                }
            }
        }
        Commands::Sandbox {
            cmd,
            repo,
            keep,
            timeout,
        } => {
            let exit_code = run_cli_sandbox(&repo, &cmd, keep, timeout).await?;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        Commands::Sync { github_url, repo } => {
            let engine = GitEngine::new(&repo);
            let snapshot = engine.inspect_head_zero_copy()?;
            let sync_cfg = BiDirectionalSyncConfig::new(RepoId::new(), &github_url);

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
            let engine = GitEngine::new(&repo);
            let entries = engine.list_agent_worktrees()?;
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

/// Creates an ephemeral agent worktree from the current branch, runs `cmd` in the sandbox, and
/// removes the worktree (and its branch) afterwards unless `keep` is set.
async fn run_cli_sandbox(
    repo: &std::path::Path,
    cmd: &str,
    keep: bool,
    timeout_secs: u64,
) -> Result<i32> {
    let engine = GitEngine::new(repo);
    let head = engine.inspect_head_zero_copy()?;
    if head.branch_name == "detached" {
        anyhow::bail!("HEAD is detached; check out a branch first");
    }

    let worktree_id = WorktreeId::new();
    let id_str = worktree_id.0.to_string();
    let branch = format!("agent/cli/sandbox-{}", &id_str[..8]);
    let m = engine.materialize_agent_worktree(
        worktree_id,
        RepoId::new(),
        AgentKind::InternalBot,
        &branch,
        &head.branch_name,
        cmd,
    )?;
    let spec = SandboxSpec::for_linked_worktree(
        &m.worktree_dir,
        &m.worktree_admin_dir,
        &m.common_git_dir,
        m.main_workdir.as_deref(),
        &m.worktrees_base,
        &m.sandbox_scratch_base,
    );

    let executor = MicrosandboxExecutor::new(1).with_timeout_secs(timeout_secs);
    // On Ctrl-C the execution future is dropped (killing the sandboxed process group) and the
    // worktree is still cleaned up below.
    let outcome = tokio::select! {
        res = executor.execute(m.worktree.id, &spec, cmd) => Some(res),
        _ = tokio::signal::ctrl_c() => None,
    };

    if keep {
        eprintln!(
            "[gitust sandbox] kept worktree {} on branch {}",
            m.worktree_dir.display(),
            m.worktree.branch
        );
    } else if let Err(e) =
        engine.remove_agent_worktree(&m.worktree_dir, &m.worktree.branch, &m.worktree.base_commit)
    {
        eprintln!("[gitust sandbox] warning: cleanup failed: {e}");
    }

    let Some(outcome) = outcome else {
        eprintln!("[gitust sandbox] interrupted");
        return Ok(130);
    };
    let res = outcome?;
    if !res.stdout.is_empty() {
        print!("{}", res.stdout);
    }
    if !res.stderr.is_empty() {
        eprint!("{}", res.stderr);
    }
    eprintln!(
        "[gitust sandbox] backend={:?} worktree_setup={}µs run={}ms exit={}",
        res.backend, m.worktree.materialize_micros, res.duration_ms, res.exit_code
    );
    Ok(res.exit_code)
}
