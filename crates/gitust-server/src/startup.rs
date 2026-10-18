//! Server startup entrypoints and transport listener orchestration.

use std::net::SocketAddr;
use std::path::PathBuf;

use gitust_db::AuthStore;

use crate::{
    automatic_checks, build_router, local_checks_automatic, repository_registry, ssh_git,
    transports, AppState,
};

/// Starts the all-in-one Gitust server on the given socket address.
pub async fn serve(
    addr: SocketAddr,
    repo_path: PathBuf,
    github_mirror_url: String,
    auth_store: AuthStore,
    trusted_sandbox_exec: bool,
    sync_timeout_secs: Option<u64>,
) -> Result<(), std::io::Error> {
    serve_with_read_only_repositories(
        addr,
        repo_path,
        github_mirror_url,
        auth_store,
        trusted_sandbox_exec,
        sync_timeout_secs,
        Vec::new(),
    )
    .await
}

/// Adds explicitly registered read-only secondary repositories without widening primary tools.
pub async fn serve_with_read_only_repositories(
    addr: SocketAddr,
    repo_path: PathBuf,
    github_mirror_url: String,
    auth_store: AuthStore,
    trusted_sandbox_exec: bool,
    sync_timeout_secs: Option<u64>,
    read_only_repos: Vec<PathBuf>,
) -> Result<(), std::io::Error> {
    serve_with_repository_modes(
        addr,
        repo_path,
        github_mirror_url,
        auth_store,
        trusted_sandbox_exec,
        sync_timeout_secs,
        RepositoryPaths {
            read_only: read_only_repos,
            git_write: Vec::new(),
        },
    )
    .await
}

/// Explicit stopped configuration; a Git-only secondary never gains a workbench runtime by default.
#[derive(Default)]
pub struct RepositoryPaths {
    pub read_only: Vec<PathBuf>,
    pub git_write: Vec<PathBuf>,
}

pub async fn serve_with_repository_modes(
    addr: SocketAddr,
    repo_path: PathBuf,
    github_mirror_url: String,
    auth_store: AuthStore,
    trusted_sandbox_exec: bool,
    sync_timeout_secs: Option<u64>,
    repositories: RepositoryPaths,
) -> Result<(), std::io::Error> {
    serve_with_transports(
        addr,
        repo_path,
        github_mirror_url,
        auth_store,
        trusted_sandbox_exec,
        sync_timeout_secs,
        TransportOptions {
            repositories,
            ssh_bind: None,
        },
    )
    .await
}

/// Optional network services; SSH is disabled unless the operator supplies a bind address.
#[derive(Default)]
pub struct TransportOptions {
    pub repositories: RepositoryPaths,
    pub ssh_bind: Option<SocketAddr>,
}

pub async fn serve_with_transports(
    addr: SocketAddr,
    repo_path: PathBuf,
    github_mirror_url: String,
    auth_store: AuthStore,
    trusted_sandbox_exec: bool,
    sync_timeout_secs: Option<u64>,
    options: TransportOptions,
) -> Result<(), std::io::Error> {
    let TransportOptions {
        repositories,
        ssh_bind,
    } = options;
    let (mut state, _service_lock) = tokio::task::spawn_blocking(
        move || -> Result<(AppState, gitust_git::WorktreeServiceLock), std::io::Error> {
            let mut engine = gitust_git::GitEngine::new(&repo_path);
            if let Some(secs) = sync_timeout_secs {
                engine = engine.with_fetch_timeout(std::time::Duration::from_secs(secs.max(1)));
            }
            let lock = engine
                .lock_worktree_service()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::WouldBlock, e))?;
            let mut state =
                AppState::with_engine_and_auth_store(engine, github_mirror_url, auth_store)
                    .map_err(std::io::Error::other)?
                    .with_trusted_sandbox_exec(trusted_sandbox_exec);
            let registry = if repositories.git_write.is_empty() {
                repository_registry::RepositoryRegistry::load(&state, repositories.read_only)
            } else {
                repository_registry::RepositoryRegistry::load_with_git_write(
                    &state,
                    repositories.read_only,
                    repositories.git_write,
                )
            }
            .map_err(std::io::Error::other)?;
            state.repository_registry = Some(registry);
            Ok((state, lock))
        },
    )
    .await
    .map_err(std::io::Error::other)??;
    if trusted_sandbox_exec {
        tracing::warn!(
            "host OS sandbox execution over HTTP is enabled; this is only safe for a trusted \
             single-user deployment"
        );
    }
    let ssh = match ssh_bind {
        Some(bind) => Some(ssh_git::Listener::bind(bind, state.clone()).await?),
        None => None,
    };
    if let Some(ref ssh_listener) = ssh {
        state.ssh_port = Some(ssh_listener.local_addr()?.port());
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let automatic_checks = automatic_checks::start(&state).await?;
    let local_checks_automatic = local_checks_automatic::start(&state).await?;
    let app = build_router(state);
    tracing::info!("Gitust All-in-One Server listening on http://{addr}");
    if let Some(bind) = ssh_bind {
        tracing::info!("Gitust SSH listening on {bind}");
    }
    let result = transports::serve(listener, app, ssh).await;
    let local_result = local_checks_automatic.shutdown().await;
    automatic_checks.shutdown().await?;
    local_result?;
    result
}
