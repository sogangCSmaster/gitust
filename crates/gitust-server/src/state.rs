//! Shared server state, worktree runtime reservation guards, OAuth 2.0 PKCE state,
//! and common error/authorization helpers for `gitust-server`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use gitust_core::{
    AgentWorktree, AuthError, AuthenticatedPrincipal, RepoId, UserId, WorktreeId, WorktreeStatus,
};
use gitust_db::AuthStore;
use gitust_git::{GitEngine, GitEngineError};
use gitust_sandbox::{LocalSandboxExecutor, SandboxError, SandboxSpec};
use gitust_sync::BiDirectionalSyncConfig;
use parking_lot::{Mutex, RwLock};

/// Maximum number of tracked agent worktrees kept in memory.
pub const MAX_TRACKED_WORKTREES: usize = 1024;
/// Maximum number of live worktrees a single user may own.
pub const MAX_WORKTREES_PER_USER: usize = 16;
/// Maximum byte length of a worktree `prompt_summary`.
pub const MAX_PROMPT_SUMMARY_BYTES: usize = 4096;
/// Maximum byte length of a sandbox command string.
pub const MAX_SANDBOX_COMMAND_BYTES: usize = 65_536;
/// Maximum byte length of a single MCP JSON-RPC 2.0 request line/body (1 MiB).
pub const MAX_MCP_LINE_BYTES: usize = 1024 * 1024;

/// Maximum number of dynamically registered OAuth 2.0 MCP clients kept in memory.
pub const MAX_OAUTH_CLIENTS: usize = 256;
/// Maximum number of pending single-use OAuth 2.0 authorization codes kept in memory.
pub const MAX_OAUTH_CODES: usize = 512;
/// Lifetime of an OAuth 2.0 authorization code in seconds (5 minutes).
pub const OAUTH_CODE_TTL_SECS: i64 = 300;

/// Dynamically registered OAuth 2.0 public client metadata (RFC 7591) for MCP clients.
#[derive(Debug, Clone)]
pub struct RegisteredOAuthClient {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Single-use OAuth 2.0 Authorization Code bound to a user, client, redirect URI, and PKCE S256 challenge.
#[derive(Debug, Clone)]
pub struct PendingOAuthCode {
    pub code_sha256: [u8; 32],
    pub user_id: UserId,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge_s256: String,
    pub expires_at: DateTime<Utc>,
}

/// Bounded in-memory store for OAuth 2.0 Dynamic Client Registration and single-use PKCE codes.
/// Access tokens themselves are issued directly via `AuthStore::issue_oauth_session` so there is
/// zero split-brain session state and account deactivation revokes OAuth tokens immediately.
#[derive(Default)]
pub struct OAuthMcpStore {
    clients: HashMap<String, RegisteredOAuthClient>,
    client_order: VecDeque<String>,
    codes_by_hash: HashMap<[u8; 32], PendingOAuthCode>,
    code_order: VecDeque<[u8; 32]>,
}

impl OAuthMcpStore {
    pub fn insert_client(&mut self, client: RegisteredOAuthClient) {
        while self.clients.len() >= MAX_OAUTH_CLIENTS {
            if let Some(oldest_id) = self.client_order.pop_front() {
                self.clients.remove(&oldest_id);
            } else {
                break;
            }
        }
        if client.client_id != "gitust-mcp-cli" {
            self.client_order.push_back(client.client_id.clone());
        }
        self.clients.insert(client.client_id.clone(), client);
    }

    pub fn get_client(&self, client_id: &str) -> Option<&RegisteredOAuthClient> {
        self.clients.get(client_id)
    }

    pub fn insert_code(&mut self, record: PendingOAuthCode) {
        let now = Utc::now();
        self.codes_by_hash.retain(|_, c| c.expires_at > now);
        self.code_order
            .retain(|hash| self.codes_by_hash.contains_key(hash));
        while self.codes_by_hash.len() >= MAX_OAUTH_CODES {
            if let Some(oldest_hash) = self.code_order.pop_front() {
                self.codes_by_hash.remove(&oldest_hash);
            } else {
                break;
            }
        }
        let hash = record.code_sha256;
        self.code_order.push_back(hash);
        self.codes_by_hash.insert(hash, record);
    }

    /// Atomically removes and returns a single-use authorization code by its SHA-256 hash.
    pub fn take_code(&mut self, code_sha256: &[u8; 32]) -> Option<PendingOAuthCode> {
        let removed = self.codes_by_hash.remove(code_sha256)?;
        self.code_order.retain(|h| h != code_sha256);
        Some(removed)
    }
}

/// Server-only runtime state of a materialized worktree (never serialized to clients).
#[derive(Clone)]
pub struct WorktreeRuntime {
    /// Owner who spawned this worktree over HTTP (`None` for worktrees recovered on startup).
    pub created_by: Option<UserId>,
    pub worktree_dir: PathBuf,
    pub sandbox_spec: SandboxSpec,
    /// Serializes sandbox runs and removal for one worktree.
    pub exec_lock: Arc<tokio::sync::Mutex<()>>,
}

/// Tracks in-flight worktree materializations so global and per-user limits are enforced
/// atomically without holding a lock across blocking `git worktree add` checkouts.
#[derive(Default)]
pub struct SpawnQuotaTracker {
    pub(crate) in_flight_total: usize,
    pub(crate) in_flight_by_user: HashMap<UserId, usize>,
}

impl SpawnQuotaTracker {
    pub(crate) fn release(&mut self, user_id: UserId) {
        self.in_flight_total = self.in_flight_total.saturating_sub(1);
        if let Some(count) = self.in_flight_by_user.get_mut(&user_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.in_flight_by_user.remove(&user_id);
            }
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub repo_id: RepoId,
    pub auth_store: AuthStore,
    pub git_engine: GitEngine,
    pub sandbox_executor: LocalSandboxExecutor,
    pub sync_config: BiDirectionalSyncConfig,
    pub worktrees: Arc<RwLock<VecDeque<AgentWorktree>>>,
    pub worktree_runtime: Arc<RwLock<HashMap<WorktreeId, WorktreeRuntime>>>,
    /// Reserves global and per-user worktree slots before blocking checkouts.
    pub spawn_quota: Arc<Mutex<SpawnQuotaTracker>>,
    /// Serializes repository-wide ref/working-tree mutations (`approve_worktree` and local mirror ref apply).
    pub repo_mutation_lock: Arc<tokio::sync::Mutex<()>>,
    /// Serializes inbound `git fetch` staging runs so `repo_mutation_lock` is never held across network I/O.
    pub sync_fetch_lock: Arc<tokio::sync::Mutex<()>>,
    /// Bounded OAuth 2.0 Dynamic Client Registration + PKCE S256 authorization code store.
    pub oauth_mcp: Arc<Mutex<OAuthMcpStore>>,
    /// Host OS sandbox execution over HTTP is only safe for a trusted single-user deployment.
    /// Multi-tenant execution requires the microVM tier, which is not implemented yet.
    pub trusted_sandbox_exec: bool,
}

impl AppState {
    #[must_use]
    pub fn new(repo_path: PathBuf, github_mirror_url: String) -> Self {
        Self::with_auth_store(repo_path, github_mirror_url, AuthStore::new_in_memory())
    }

    #[must_use]
    pub fn with_auth_store(
        repo_path: PathBuf,
        github_mirror_url: String,
        auth_store: AuthStore,
    ) -> Self {
        Self::with_engine_and_auth_store(GitEngine::new(&repo_path), github_mirror_url, auth_store)
    }

    #[must_use]
    pub fn with_engine_and_auth_store(
        git_engine: GitEngine,
        github_mirror_url: String,
        auth_store: AuthStore,
    ) -> Self {
        let repo_id = RepoId::new();
        let mut worktrees = VecDeque::new();
        let mut worktree_runtime = HashMap::new();
        if let Ok(recovered) = git_engine.recover_agent_worktrees(repo_id) {
            for m in recovered.into_iter().take(MAX_TRACKED_WORKTREES) {
                let runtime = WorktreeRuntime {
                    created_by: None,
                    sandbox_spec: SandboxSpec::for_linked_worktree(
                        &m.worktree_dir,
                        &m.worktree_admin_dir,
                        &m.common_git_dir,
                        m.main_workdir.as_deref(),
                        &m.worktrees_base,
                        &m.sandbox_scratch_base,
                    ),
                    worktree_dir: m.worktree_dir,
                    exec_lock: Arc::new(tokio::sync::Mutex::new(())),
                };
                worktree_runtime.insert(m.worktree.id, runtime);
                worktrees.push_back(m.worktree);
            }
        }
        let mut oauth_store = OAuthMcpStore::default();
        // Pre-register a default local CLI/IDE public client so users/agents can also use
        // `client_id = "gitust-mcp-cli"` without an extra registration round-trip.
        oauth_store.insert_client(RegisteredOAuthClient {
            client_id: "gitust-mcp-cli".to_owned(),
            client_name: "Gitust Built-in MCP Client".to_owned(),
            redirect_uris: vec![
                "http://127.0.0.1/callback".to_owned(),
                "http://localhost/callback".to_owned(),
                "cursor://anysphere.cursor-mcp/oauth/callback".to_owned(),
                "claude://oauth/callback".to_owned(),
            ],
            created_at: Utc::now() - Duration::seconds(1),
        });
        Self {
            repo_id,
            auth_store,
            git_engine,
            sandbox_executor: LocalSandboxExecutor::new(16),
            sync_config: BiDirectionalSyncConfig::new(repo_id, github_mirror_url),
            worktrees: Arc::new(RwLock::new(worktrees)),
            worktree_runtime: Arc::new(RwLock::new(worktree_runtime)),
            spawn_quota: Arc::new(Mutex::new(SpawnQuotaTracker::default())),
            repo_mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            sync_fetch_lock: Arc::new(tokio::sync::Mutex::new(())),
            oauth_mcp: Arc::new(Mutex::new(oauth_store)),
            trusted_sandbox_exec: false,
        }
    }

    #[must_use]
    pub fn with_trusted_sandbox_exec(mut self, enabled: bool) -> Self {
        self.trusted_sandbox_exec = enabled;
        self
    }

    /// Atomically removes a worktree from both `worktree_runtime` and `worktrees` under the
    /// canonical `spawn_quota -> worktree_runtime -> worktrees` lock order.
    pub(crate) fn remove_worktree_entry(&self, wt_id: WorktreeId) {
        let _quota = self.spawn_quota.lock();
        self.worktree_runtime.write().remove(&wt_id);
        self.worktrees.write().retain(|w| w.id != wt_id);
    }

    /// Releases an in-flight spawn quota slot for `user_id`.
    pub(crate) fn release_in_flight_quota(&self, user_id: UserId) {
        self.spawn_quota.lock().release(user_id);
    }

    /// Returns a cloned `WorktreeRuntime` for `wt_id` if present.
    pub(crate) fn get_worktree_runtime(&self, wt_id: WorktreeId) -> Option<WorktreeRuntime> {
        self.worktree_runtime.read().get(&wt_id).cloned()
    }

    /// Verifies that `wt_id` still has an active runtime checkout and transitions its status to
    /// `WorktreeStatus::Compiling` if it has not been merged or discarded.
    pub(crate) fn try_begin_worktree_compiling(
        &self,
        wt_id: WorktreeId,
    ) -> Result<(), (StatusCode, String)> {
        if !self.worktree_runtime.read().contains_key(&wt_id) {
            return Err((
                StatusCode::NOT_FOUND,
                format!("worktree {} not found", wt_id.0),
            ));
        }
        let mut guard = self.worktrees.write();
        let Some(w) = guard.iter_mut().find(|w| w.id == wt_id) else {
            return Err((
                StatusCode::NOT_FOUND,
                format!("worktree {} not found", wt_id.0),
            ));
        };
        if matches!(w.status, WorktreeStatus::Merged | WorktreeStatus::Discarded) {
            return Err((StatusCode::CONFLICT, format!("worktree is {:?}", w.status)));
        }
        w.status = WorktreeStatus::Compiling;
        Ok(())
    }

    /// Atomically marks `wt_id` as `Merged` and records the new `merged_sha` as `base_commit`.
    pub(crate) fn complete_worktree_merge(
        &self,
        wt_id: WorktreeId,
        merged_sha: gitust_core::CommitSha,
    ) -> Result<AgentWorktree, (StatusCode, String)> {
        let mut guard = self.worktrees.write();
        if let Some(wt) = guard.iter_mut().find(|w| w.id == wt_id) {
            wt.approve_and_merge(false)
                .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
            wt.base_commit = merged_sha;
            return Ok(wt.clone());
        }
        Err((
            StatusCode::NOT_FOUND,
            format!("worktree {} not found", wt_id.0),
        ))
    }
}

pub(crate) enum SpawnLifecycle {
    InFlight,
    Materialized {
        engine: GitEngine,
        worktree_dir: PathBuf,
        branch: String,
        base_commit: gitust_core::CommitSha,
    },
    Committed,
    Failed,
    Cancelled,
}

/// RAII reservation of one global and one per-user worktree slot. Automatically releases the
/// in-flight slot and rolls back any uncommitted checkout on drop if the checkout fails or the
/// request future is cancelled.
pub(crate) struct SpawnReservation {
    pub(crate) state: AppState,
    pub(crate) user_id: UserId,
    pub(crate) lifecycle: Arc<Mutex<SpawnLifecycle>>,
}

impl SpawnReservation {
    pub(crate) fn reserve(state: &AppState, user_id: UserId) -> Result<Self, (StatusCode, String)> {
        let mut quota = state.spawn_quota.lock();
        let tracked = state.worktrees.read().len();
        if tracked.saturating_add(quota.in_flight_total) >= MAX_TRACKED_WORKTREES {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "worktree limit ({MAX_TRACKED_WORKTREES}) reached; discard worktrees first"
                ),
            ));
        }
        let owned = state
            .worktree_runtime
            .read()
            .values()
            .filter(|r| r.created_by == Some(user_id))
            .count();
        let user_in_flight = quota.in_flight_by_user.get(&user_id).copied().unwrap_or(0);
        if owned.saturating_add(user_in_flight) >= MAX_WORKTREES_PER_USER {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "per-user worktree limit ({MAX_WORKTREES_PER_USER}) reached; discard worktrees first"
                ),
            ));
        }
        quota.in_flight_total += 1;
        *quota.in_flight_by_user.entry(user_id).or_insert(0) += 1;
        Ok(Self {
            state: state.clone(),
            user_id,
            lifecycle: Arc::new(Mutex::new(SpawnLifecycle::InFlight)),
        })
    }

    pub(crate) fn commit(self, wt: AgentWorktree, runtime: WorktreeRuntime) {
        *self.lifecycle.lock() = SpawnLifecycle::Committed;
        let mut quota = self.state.spawn_quota.lock();
        self.state.worktree_runtime.write().insert(wt.id, runtime);
        self.state.worktrees.write().push_front(wt);
        quota.release(self.user_id);
    }
}

impl Drop for SpawnReservation {
    fn drop(&mut self) {
        let prev = std::mem::replace(&mut *self.lifecycle.lock(), SpawnLifecycle::Cancelled);
        match prev {
            SpawnLifecycle::Committed => {}
            SpawnLifecycle::Failed => {
                self.state.spawn_quota.lock().release(self.user_id);
            }
            SpawnLifecycle::InFlight | SpawnLifecycle::Cancelled => {
                // `spawn_blocking` is still running and will roll back the worktree + release quota.
            }
            SpawnLifecycle::Materialized {
                engine,
                worktree_dir,
                branch,
                base_commit,
            } => {
                let state = self.state.clone();
                let user_id = self.user_id;
                tokio::task::spawn_blocking(move || {
                    let _ = engine.remove_agent_worktree(&worktree_dir, &branch, &base_commit);
                    state.spawn_quota.lock().release(user_id);
                });
            }
        }
    }
}

/// Marks a run as failed if the handler future is dropped mid-run (e.g. client disconnect), so
/// the worktree never stays `Compiling` forever.
pub(crate) struct RunStatusGuard {
    pub(crate) state: AppState,
    pub(crate) id: WorktreeId,
    pub(crate) armed: bool,
}

impl Drop for RunStatusGuard {
    fn drop(&mut self) {
        if self.armed {
            set_worktree_status(&self.state, self.id, |w| w.status = WorktreeStatus::Failed);
        }
    }
}

pub(crate) fn map_auth_err(err: AuthError) -> (StatusCode, String) {
    let code = match err {
        AuthError::Unauthenticated | AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
        AuthError::AccountDeactivated
        | AuthError::AdminPrivilegeRequired
        | AuthError::ForbiddenResourceOwnership
        | AuthError::ForbiddenRepositoryAccess
        | AuthError::PublicSignupDisabled
        | AuthError::AdminAlreadyBootstrapped => StatusCode::FORBIDDEN,
        AuthError::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
        AuthError::DuplicateUser | AuthError::CannotDeactivateLastAdmin => StatusCode::CONFLICT,
        AuthError::InvalidToken
        | AuthError::TokenAlreadyUsed
        | AuthError::TokenExpired
        | AuthError::TokenRevoked
        | AuthError::TokenPurposeMismatch
        | AuthError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        AuthError::StorageError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, err.to_string())
}

pub(crate) fn map_git_err(err: GitEngineError) -> (StatusCode, String) {
    let code = match &err {
        GitEngineError::InvalidBranchName(_)
        | GitEngineError::InvalidRevision(_)
        | GitEngineError::InvalidRepoPath(_)
        | GitEngineError::InvalidRemoteUrl(_)
        | GitEngineError::BranchNotAgentScoped(_) => StatusCode::BAD_REQUEST,
        GitEngineError::BaseBranchNotFound(_)
        | GitEngineError::RevisionNotFound(_)
        | GitEngineError::PathNotFound(_) => StatusCode::NOT_FOUND,
        GitEngineError::BranchAlreadyExists(_)
        | GitEngineError::MergeConflict(_)
        | GitEngineError::WorktreeServiceBusy(_) => StatusCode::CONFLICT,
        GitEngineError::CredentialsInRepoConfig => StatusCode::PRECONDITION_FAILED,
        _ => {
            tracing::error!(error = %err, "git engine failure");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal git engine error".to_owned(),
            );
        }
    };
    (code, err.to_string())
}

pub(crate) fn map_sandbox_err(err: SandboxError) -> (StatusCode, String) {
    match &err {
        SandboxError::TimedOut { .. } => (StatusCode::REQUEST_TIMEOUT, err.to_string()),
        SandboxError::IsolationUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
        _ => {
            tracing::error!(error = %err, "sandbox failure");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal sandbox error".to_owned(),
            )
        }
    }
}

pub(crate) fn internal_err(e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(error = %e, "internal task failure");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_owned(),
    )
}

/// Enforces team repository BOLA access (fail-closed):
/// - If `repo_id` is registered in `auth_store`, enforces team membership via `authorize_repository_access`.
/// - Otherwise, only allows access if `repo_id == default_repo_id` (the primary server repository).
pub(crate) fn check_repo_bola(
    auth_store: &AuthStore,
    default_repo_id: RepoId,
    caller: &AuthenticatedPrincipal,
    repo_id: RepoId,
) -> Result<(), (StatusCode, String)> {
    auth_store
        .verify_active_principal(caller)
        .map_err(map_auth_err)?;
    if auth_store.repository_exists(repo_id) {
        auth_store
            .authorize_repository_access(caller, repo_id)
            .map_err(map_auth_err)?;
        return Ok(());
    }
    if repo_id == default_repo_id {
        return Ok(());
    }
    Err(map_auth_err(AuthError::ForbiddenRepositoryAccess))
}

/// Looks up a worktree and enforces repository access. Returns a snapshot of the public model.
pub(crate) fn find_authorized_worktree(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: uuid::Uuid,
) -> Result<AgentWorktree, (StatusCode, String)> {
    let wt = state
        .worktrees
        .read()
        .iter()
        .find(|w| w.id.0 == id)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("worktree {id} not found")))?;
    check_repo_bola(&state.auth_store, state.repo_id, caller, wt.repo_id)?;
    Ok(wt)
}

pub(crate) fn set_worktree_status(
    state: &AppState,
    id: WorktreeId,
    f: impl FnOnce(&mut AgentWorktree),
) {
    if let Some(wt) = state.worktrees.write().iter_mut().find(|w| w.id == id) {
        if !matches!(
            wt.status,
            WorktreeStatus::Merged | WorktreeStatus::Discarded
        ) {
            f(wt);
        }
    }
}
