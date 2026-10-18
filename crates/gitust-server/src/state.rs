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
pub use gitust_git::MAX_PROMPT_SUMMARY_BYTES;
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
    pub authorizing_credential: [u8; 32],
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
    codes_by_hash: HashMap<[u8; 32], PendingOAuthCode>,
    code_order: VecDeque<[u8; 32]>,
}

impl OAuthMcpStore {
    pub fn insert_client(&mut self, client: RegisteredOAuthClient) -> bool {
        if self.clients.len() >= MAX_OAUTH_CLIENTS && !self.clients.contains_key(&client.client_id)
        {
            return false;
        }
        self.clients.insert(client.client_id.clone(), client);
        true
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
    /// Persisted creator; legacy worktrees with unknown ownership use `None`.
    pub created_by: Option<UserId>,
    pub worktree_dir: PathBuf,
    pub common_git_dir: PathBuf,
    pub sandbox_spec: SandboxSpec,
    /// Serializes sandbox runs, commits, approval and removal for one worktree.
    pub exec_lock: Arc<tokio::sync::Mutex<()>>,
}

pub use crate::quota::SpawnQuotaTracker;
pub(crate) use crate::quota::{RunStatusGuard, SpawnLifecycle, SpawnReservation};

#[derive(Clone)]
pub struct AppState {
    #[cfg(test)]
    pub(crate) repository_read_hook: Arc<Mutex<Option<crate::repository_reads::TestHook>>>,
    #[cfg(test)]
    pub(crate) git_write_before_lock_hook: Arc<Mutex<Option<crate::repository_reads::TestHook>>>,
    #[cfg(test)]
    pub(crate) sync_after_fetch_hook: Arc<Mutex<Option<crate::repository_reads::TestHook>>>,
    pub(crate) repository_registry: Option<Arc<crate::repository_registry::RepositoryRegistry>>,
    pub(crate) automatic_checks: Arc<crate::automatic_checks::AutomaticChecks>,
    pub repo_id: RepoId,
    pub(crate) repo_access: gitust_core::RepoAccess,
    #[cfg(test)]
    pub(crate) automatic_run_page_hook: Arc<Mutex<Option<crate::automatic_run_page::TestHook>>>,
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
    /// Serializes exact-commit publication without blocking local merges during network I/O.
    pub outbound_sync_lock: Arc<tokio::sync::Mutex<()>>,
    /// Bounded OAuth 2.0 Dynamic Client Registration + PKCE S256 authorization code store.
    pub oauth_mcp: Arc<Mutex<OAuthMcpStore>>,
    /// Host OS sandbox execution over HTTP is only safe for a trusted single-user deployment.
    /// Multi-tenant execution requires the microVM tier, which is not implemented yet.
    pub trusted_sandbox_exec: bool,
    pub(crate) local_checks_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AppState {
    pub fn new(repo_path: PathBuf, github_mirror_url: String) -> Result<Self, GitEngineError> {
        Self::with_auth_store(repo_path, github_mirror_url, AuthStore::new_in_memory())
    }

    pub fn with_auth_store(
        repo_path: PathBuf,
        github_mirror_url: String,
        auth_store: AuthStore,
    ) -> Result<Self, GitEngineError> {
        Self::with_engine_and_auth_store(GitEngine::new(&repo_path), github_mirror_url, auth_store)
    }

    pub fn with_engine_and_auth_store(
        git_engine: GitEngine,
        github_mirror_url: String,
        auth_store: AuthStore,
    ) -> Result<Self, GitEngineError> {
        let repo_id = match git_engine.load_or_create_repository_id() {
            Ok(id) => id,
            // Auth-only unit fixtures deliberately have no Git repository. Persisted identity
            // failures never receive this exception, and production always propagates OpenRepo.
            Err(GitEngineError::OpenRepo { .. }) if cfg!(test) => RepoId::new(),
            Err(error) => return Err(error),
        };
        let receipt = match git_engine.repository_registration_state(repo_id) {
            Ok(gitust_git::RegistrationReceipt::Absent) => None,
            Ok(gitust_git::RegistrationReceipt::Complete(receipt)) => Some(receipt),
            Ok(gitust_git::RegistrationReceipt::Pending(_)) => {
                return Err(GitEngineError::RepositoryIdentity(
                    "repository registration evidence is incomplete or conflicting".into(),
                ))
            }
            Err(GitEngineError::OpenRepo { .. }) if cfg!(test) => None,
            Err(error) => return Err(error),
        };
        let repo_access = auth_store
            .configured_repository_access(repo_id, receipt)
            .map_err(|_| {
                GitEngineError::RepositoryIdentity(
                    "repository registration evidence is incomplete or conflicting".into(),
                )
            })?;
        let mut worktrees = VecDeque::new();
        let mut worktree_runtime = HashMap::new();
        match git_engine.recover_agent_worktrees(repo_id) {
            Ok(recovered) => {
                if recovered.len() > MAX_TRACKED_WORKTREES {
                    return Err(GitEngineError::ApprovalState(
                        "managed worktree limit exceeded; refusing partial recovery".into(),
                    ));
                }
                for mut m in recovered {
                    if let Some(ref approval) = m.approval {
                        let destination =
                            GitEngine::validate_mirror_remote_url(&github_mirror_url)?;
                        m.worktree.github_synced = approval.published_to(&destination);
                    }
                    let runtime = WorktreeRuntime {
                        common_git_dir: m.common_git_dir.clone(),
                        created_by: crate::worktree_owners::read(&m)?,
                        sandbox_spec: sandbox_spec_for(&m, &auth_store)?,
                        worktree_dir: m.worktree_dir,
                        exec_lock: Arc::new(tokio::sync::Mutex::new(())),
                    };
                    worktree_runtime.insert(m.worktree.id, runtime);
                    worktrees.push_back(m.worktree);
                }
            }
            // Auth-only unit fixtures deliberately use no repository. Production constructors
            // propagate every recovery error before exposing HTTP/MCP or Git mutation routes.
            Err(GitEngineError::OpenRepo { .. }) if cfg!(test) => {}
            Err(err) => return Err(err),
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
        Ok(Self {
            repository_registry: None,
            #[cfg(test)]
            repository_read_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            git_write_before_lock_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            sync_after_fetch_hook: Arc::new(Mutex::new(None)),
            automatic_checks: Arc::new(crate::automatic_checks::AutomaticChecks::default()),
            repo_id,
            repo_access,
            #[cfg(test)]
            automatic_run_page_hook: Arc::new(Mutex::new(None)),
            auth_store,
            git_engine,
            sandbox_executor: LocalSandboxExecutor::new(16),
            sync_config: BiDirectionalSyncConfig::new(repo_id, github_mirror_url),
            worktrees: Arc::new(RwLock::new(worktrees)),
            worktree_runtime: Arc::new(RwLock::new(worktree_runtime)),
            spawn_quota: Arc::new(Mutex::new(SpawnQuotaTracker::default())),
            repo_mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            sync_fetch_lock: Arc::new(tokio::sync::Mutex::new(())),
            outbound_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            oauth_mcp: Arc::new(Mutex::new(oauth_store)),
            trusted_sandbox_exec: false,
            local_checks_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
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

    /// Begins an owner commit, or a local stdio operator's explicit action, under its operation guard.
    pub(crate) fn begin_worktree_commit(
        &self,
        id: WorktreeId,
        owner: UserId,
        local_operator: bool,
    ) -> Result<AgentWorktree, (StatusCode, String)> {
        let runtime = self
            .get_worktree_runtime(id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
        if !local_operator && runtime.created_by != Some(owner) {
            return Err(map_auth_err(AuthError::ForbiddenResourceOwnership));
        }
        let mut worktrees = self.worktrees.write();
        let task = worktrees
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
        if matches!(
            task.status,
            WorktreeStatus::Merged | WorktreeStatus::Discarded
        ) {
            return Err((
                StatusCode::CONFLICT,
                "terminal worktree cannot be committed".into(),
            ));
        }
        task.status = WorktreeStatus::Pending;
        task.verified_commit = None;
        task.verified_checks_oid = None;
        Ok(task.clone())
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
        w.verified_commit = None;
        w.verified_checks_oid = None;
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
        AuthError::StorageError(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    (code, err.to_string())
}

pub(crate) fn map_git_err(err: GitEngineError) -> (StatusCode, String) {
    let code = match &err {
        GitEngineError::DefaultBranchInvalid
        | GitEngineError::InvalidBranchName(_)
        | GitEngineError::InvalidRevision(_)
        | GitEngineError::InvalidRepoPath(_)
        | GitEngineError::InvalidRemoteUrl(_)
        | GitEngineError::BranchNotAgentScoped(_) => StatusCode::BAD_REQUEST,
        GitEngineError::BaseBranchNotFound(_)
        | GitEngineError::RevisionNotFound(_)
        | GitEngineError::PathNotFound(_) => StatusCode::NOT_FOUND,
        GitEngineError::DefaultBranchConflict
        | GitEngineError::DefaultBranchUnsupported
        | GitEngineError::BranchAlreadyExists(_)
        | GitEngineError::MergeConflict(_)
        | GitEngineError::VerificationInvalid(_)
        | GitEngineError::ExecutionFence(_)
        | GitEngineError::RepositoryIdentity(_)
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
/// A configured service pins legacy sharing or an exact explicit registration at startup.
/// Current binding/account/ACL checks share one auth snapshot; other metadata IDs keep team checks.
pub(crate) fn check_repo_bola(
    auth_store: &AuthStore,
    default_repo_id: RepoId,
    repo_access: gitust_core::RepoAccess,
    caller: &AuthenticatedPrincipal,
    repo_id: RepoId,
) -> Result<(), (StatusCode, String)> {
    if default_repo_id != repo_access.repo_id {
        return Err(map_auth_err(AuthError::ForbiddenRepositoryAccess));
    }
    auth_store
        .authorize_service_repository(caller, repo_access, repo_id)
        .map_err(map_auth_err)
}

/// Looks up a worktree and enforces repository access off the async worker threads.
pub(crate) async fn find_authorized_worktree(
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
    let store = state.auth_store.clone();
    let default_repo = state.repo_id;
    let repo_access = state.repo_access;
    let caller_owned = caller.clone();
    let repo_id = wt.repo_id;
    tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, default_repo, repo_access, &caller_owned, repo_id)
    })
    .await
    .map_err(internal_err)??;
    Ok(wt)
}

/// Snapshots tracked worktrees without holding `state.worktrees.read()` across `AuthStore` lock I/O,
/// and filters them by repository BOLA access on the blocking threadpool.
pub(crate) async fn filter_authorized_worktrees(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
) -> Result<Vec<AgentWorktree>, (StatusCode, String)> {
    let candidates: Vec<AgentWorktree> = state.worktrees.read().iter().cloned().collect();
    let store = state.auth_store.clone();
    let default_repo = state.repo_id;
    let repo_access = state.repo_access;
    let caller_owned = caller.clone();
    tokio::task::spawn_blocking(move || {
        // Reject a configured-ID collision even with no rows; filtering it into an empty list
        // would conceal the broken binding. Cache the already checked default once per list.
        check_repo_bola(
            &store,
            default_repo,
            repo_access,
            &caller_owned,
            default_repo,
        )?;
        let mut access_cache = HashMap::from([(default_repo, true)]);
        let mut authorized = Vec::with_capacity(candidates.len());
        for wt in candidates {
            let allowed = match access_cache.get(&wt.repo_id).copied() {
                Some(cached) => cached,
                None => {
                    let ok = match check_repo_bola(
                        &store,
                        default_repo,
                        repo_access,
                        &caller_owned,
                        wt.repo_id,
                    ) {
                        Ok(()) => true,
                        Err((StatusCode::FORBIDDEN, _)) => false,
                        Err(err) => return Err(err),
                    };
                    access_cache.insert(wt.repo_id, ok);
                    ok
                }
            };
            if allowed {
                authorized.push(wt);
            }
        }
        Ok(authorized)
    })
    .await
    .map_err(internal_err)?
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
            if wt.status != WorktreeStatus::Verified {
                wt.verified_commit = None;
                wt.verified_checks_oid = None;
            }
        }
    }
}

pub(crate) use crate::sandbox_policy::sandbox_spec_for;
