//! `gitust-server`: High-performance Axum HTTP/REST, Invite-Only Auth, Built-in MCP Gateway,
//! and Real-Time Sync Server for Gitust.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use gitust_core::{
    AgentKind, AgentWorktree, AuthError, AuthenticatedPrincipal, CommitSha, EnterpriseRole,
    EnterpriseSecurityPolicy, InviteId, InvitePublicView, PatId, PersonalAccessTokenView, RepoId,
    RepositoryRecord, SshKeyId, SshPublicKeyView, TeamId, UserId, UserPublicProfile, WorktreeId,
    WorktreeStatus,
};
use gitust_db::{AuthStore, IssuedInviteLink, IssuedPersonalAccessToken, LoginSessionResponse};
use gitust_git::{GitEngine, GitEngineError, RepoHeadSnapshot};
use gitust_sandbox::{
    builtin_mcp_tools, LocalSandboxExecutor, McpToolDescriptor, SandboxError,
    SandboxExecutionResult, SandboxSpec,
};
use gitust_sync::{BiDirectionalSyncConfig, SyncStatusReport};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

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
    in_flight_total: usize,
    in_flight_by_user: HashMap<UserId, usize>,
}

impl SpawnQuotaTracker {
    fn release(&mut self, user_id: UserId) {
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
    pub security_policy: Arc<RwLock<EnterpriseSecurityPolicy>>,
    pub worktrees: Arc<RwLock<VecDeque<AgentWorktree>>>,
    pub worktree_runtime: Arc<RwLock<HashMap<WorktreeId, WorktreeRuntime>>>,
    /// Reserves global and per-user worktree slots before blocking checkouts.
    pub spawn_quota: Arc<Mutex<SpawnQuotaTracker>>,
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
        let repo_id = RepoId::new();
        let git_engine = GitEngine::new(&repo_path);
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
        Self {
            repo_id,
            auth_store,
            git_engine,
            sandbox_executor: LocalSandboxExecutor::new(16),
            sync_config: BiDirectionalSyncConfig::new(repo_id, github_mirror_url),
            security_policy: Arc::new(RwLock::new(EnterpriseSecurityPolicy::default())),
            worktrees: Arc::new(RwLock::new(worktrees)),
            worktree_runtime: Arc::new(RwLock::new(worktree_runtime)),
            spawn_quota: Arc::new(Mutex::new(SpawnQuotaTracker::default())),
            trusted_sandbox_exec: false,
        }
    }

    #[must_use]
    pub fn with_trusted_sandbox_exec(mut self, enabled: bool) -> Self {
        self.trusted_sandbox_exec = enabled;
        self
    }
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub engine: &'static str,
    pub privacy_mode: &'static str,
    pub server_latency_micros: u64,
}

#[derive(Serialize, Deserialize)]
pub struct SpawnWorktreeRequest {
    pub repo_id: Option<RepoId>,
    pub agent: Option<AgentKind>,
    /// Must be in the `agent/` namespace.
    pub branch: String,
    /// Branch to fork from (default: `main`).
    #[serde(default)]
    pub base_branch: Option<String>,
    pub prompt_summary: String,
}

#[derive(Serialize, Deserialize)]
pub struct SandboxExecRequest {
    pub command: String,
}

#[derive(Serialize, Deserialize)]
pub struct LoginRequest {
    pub identifier: String,
    pub password: String,
}

#[derive(Serialize, Deserialize)]
pub struct CreateInviteRequest {
    pub target_email: String,
    pub assigned_role: EnterpriseRole,
    #[serde(default)]
    pub team_ids: Vec<TeamId>,
    pub ttl_hours: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct RedeemInviteRequest {
    pub raw_token: String,
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize)]
pub struct RedeemPasswordResetRequest {
    pub raw_token: String,
    pub new_password: String,
}

#[derive(Serialize, Deserialize)]
pub struct CreatePatRequest {
    pub name: String,
    pub ttl_days: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct RegisterSshKeyRequest {
    pub title: String,
    pub public_key_openssh: String,
}

#[derive(Serialize, Deserialize)]
pub struct CreateTeamRepoRequest {
    pub name: String,
    pub owner_team_id: TeamId,
    pub is_private: bool,
}

fn map_auth_err(err: AuthError) -> (StatusCode, String) {
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

/// Extracts and validates either a Session token (`gitust_ses_...`) or a Personal Access Token (`gitust_pat_...`)
/// from the `Authorization: Bearer <token>` header in `<5µs` O(1) time.
fn extract_principal(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    let Some(raw_header) = headers.get(header::AUTHORIZATION) else {
        return Err(map_auth_err(AuthError::Unauthenticated));
    };
    let Ok(header_str) = raw_header.to_str() else {
        return Err(map_auth_err(AuthError::Unauthenticated));
    };
    let Some(token) = header_str.strip_prefix("Bearer ").map(str::trim) else {
        return Err(map_auth_err(AuthError::Unauthenticated));
    };

    if token.starts_with("gitust_pat_") {
        auth_store.authenticate_pat(token).map_err(map_auth_err)
    } else {
        auth_store.authenticate_session(token).map_err(map_auth_err)
    }
}

/// Zero-boilerplate Axum extractor for `AuthenticatedPrincipal`.
/// Guarantees that every handler declaring `caller: AuthenticatedPrincipal` is authenticated
/// in `<5µs` O(1) time and immediately rejects deactivated accounts or revoked tokens.
impl FromRequestParts<AppState> for AuthenticatedPrincipal {
    type Rejection = (StatusCode, String);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        extract_principal(&parts.headers, &state.auth_store)
    }
}

/// Maximum number of tracked agent worktrees kept in memory.
const MAX_TRACKED_WORKTREES: usize = 1024;
/// Maximum number of live worktrees a single user may own.
const MAX_WORKTREES_PER_USER: usize = 16;

/// Enforces team repository BOLA access (fail-closed):
/// - If `repo_id` is registered in `auth_store`, enforces team membership via `authorize_repository_access`.
/// - Otherwise, only allows access if `repo_id == default_repo_id` (the primary server repository).
fn check_repo_bola(
    auth_store: &AuthStore,
    default_repo_id: RepoId,
    caller: &AuthenticatedPrincipal,
    repo_id: RepoId,
) -> Result<(), (StatusCode, String)> {
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

fn map_git_err(err: GitEngineError) -> (StatusCode, String) {
    let code = match &err {
        GitEngineError::InvalidBranchName(_) | GitEngineError::BranchNotAgentScoped(_) => {
            StatusCode::BAD_REQUEST
        }
        GitEngineError::BaseBranchNotFound(_) => StatusCode::NOT_FOUND,
        GitEngineError::BranchAlreadyExists(_) => StatusCode::CONFLICT,
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

fn map_sandbox_err(err: SandboxError) -> (StatusCode, String) {
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

fn internal_err(e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(error = %e, "internal task failure");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_owned(),
    )
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/git/head", get(git_head_handler))
        // Auth & Invite-Only Onboarding Routes (No public signup / no web admin bootstrap!)
        .route(
            "/api/v1/auth/register",
            post(public_register_disabled_handler),
        )
        .route("/api/v1/auth/login", post(login_handler))
        .route("/api/v1/auth/me", get(me_handler))
        .route("/api/v1/auth/invites/redeem", post(redeem_invite_handler))
        .route(
            "/api/v1/auth/password-reset/redeem",
            post(redeem_password_reset_handler),
        )
        .route("/api/v1/auth/pats", post(create_pat_handler))
        .route("/api/v1/auth/pats/{id}", delete(revoke_pat_handler))
        .route("/api/v1/auth/ssh-keys", post(register_ssh_key_handler))
        .route("/api/v1/auth/ssh-keys/{id}", delete(revoke_ssh_key_handler))
        // SiteAdmin Routes (BFLA protected)
        .route(
            "/api/v1/admin/invites",
            get(list_invites_handler).post(create_invite_handler),
        )
        .route("/api/v1/admin/invites/{id}", delete(revoke_invite_handler))
        .route("/api/v1/admin/users", get(list_users_handler))
        .route(
            "/api/v1/admin/users/{id}/deactivate",
            post(deactivate_user_handler),
        )
        .route(
            "/api/v1/admin/users/{id}/password-reset",
            post(create_password_reset_handler),
        )
        // Team Repository BOLA Routes
        .route("/api/v1/repos", post(create_team_repo_handler))
        .route("/api/v1/repos/{id}", get(get_team_repo_handler))
        // Agent Swarm, Sync & MCP Routes (Authenticated & Team/Role BOLA protected)
        .route(
            "/api/v1/swarm/worktrees",
            get(list_worktrees_handler).post(spawn_worktree_handler),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}",
            delete(discard_worktree_handler),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}/sandbox",
            post(exec_worktree_sandbox_handler),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}/approve",
            post(approve_worktree_handler),
        )
        .route("/api/v1/sync/github", post(sync_github_handler))
        .route("/api/v1/mcp/tools", get(mcp_tools_handler))
        .route("/api/v1/enterprise/policy", get(enterprise_policy_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health_handler() -> Json<HealthResponse> {
    let start = Instant::now();
    let server_latency_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(1);
    Json(HealthResponse {
        status: "ok",
        engine: "gitust-pure-rust (gix + microsandbox + github-sync)",
        privacy_mode: "STRICT_ON_PREMISE_ZERO_CLOUD_LEAKAGE",
        server_latency_micros,
    })
}

async fn public_register_disabled_handler() -> (StatusCode, String) {
    map_auth_err(AuthError::PublicSignupDisabled)
}

async fn login_handler(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginSessionResponse>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let res = tokio::task::spawn_blocking(move || store.login(&req.identifier, &req.password))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(Json(res))
}

async fn me_handler(caller: AuthenticatedPrincipal) -> Json<AuthenticatedPrincipal> {
    Json(caller)
}

async fn create_invite_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Json(req): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<IssuedInviteLink>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let issued = tokio::task::spawn_blocking(move || {
        store.create_invite_link(
            &caller,
            &req.target_email,
            req.assigned_role,
            req.team_ids,
            req.ttl_hours,
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok((StatusCode::CREATED, Json(issued)))
}

async fn list_invites_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<Vec<InvitePublicView>>, (StatusCode, String)> {
    let list = state
        .auth_store
        .list_invites(&caller)
        .map_err(map_auth_err)?;
    Ok(Json(list))
}

async fn revoke_invite_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
) -> Result<Json<InvitePublicView>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let view = tokio::task::spawn_blocking(move || store.revoke_invite_link(&caller, InviteId(id)))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(Json(view))
}

async fn redeem_invite_handler(
    State(state): State<AppState>,
    Json(req): Json<RedeemInviteRequest>,
) -> Result<(StatusCode, Json<UserPublicProfile>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let profile = tokio::task::spawn_blocking(move || {
        store.redeem_invite_link(&req.raw_token, &req.username, &req.password)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

async fn create_password_reset_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(target_id): Path<Uuid>,
) -> Result<(StatusCode, Json<IssuedInviteLink>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let issued = tokio::task::spawn_blocking(move || {
        store.create_password_reset_link(&caller, UserId(target_id), None)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok((StatusCode::CREATED, Json(issued)))
}

async fn redeem_password_reset_handler(
    State(state): State<AppState>,
    Json(req): Json<RedeemPasswordResetRequest>,
) -> Result<Json<UserPublicProfile>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let profile = tokio::task::spawn_blocking(move || {
        store.redeem_password_reset_link(&req.raw_token, &req.new_password)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok(Json(profile))
}

async fn list_users_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<Vec<UserPublicProfile>>, (StatusCode, String)> {
    let list = state.auth_store.list_users(&caller).map_err(map_auth_err)?;
    Ok(Json(list))
}

async fn deactivate_user_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(target_id): Path<Uuid>,
) -> Result<Json<UserPublicProfile>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let profile =
        tokio::task::spawn_blocking(move || store.deactivate_user(&caller, UserId(target_id)))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(map_auth_err)?;
    Ok(Json(profile))
}

async fn create_pat_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Json(req): Json<CreatePatRequest>,
) -> Result<(StatusCode, Json<IssuedPersonalAccessToken>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let issued = tokio::task::spawn_blocking(move || {
        store.create_personal_access_token(&caller, &req.name, req.ttl_days)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok((StatusCode::CREATED, Json(issued)))
}

async fn revoke_pat_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(pat_id): Path<Uuid>,
) -> Result<Json<PersonalAccessTokenView>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let view = tokio::task::spawn_blocking(move || {
        store.revoke_personal_access_token(&caller, PatId(pat_id))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok(Json(view))
}

async fn register_ssh_key_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Json(req): Json<RegisterSshKeyRequest>,
) -> Result<(StatusCode, Json<SshPublicKeyView>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let view = tokio::task::spawn_blocking(move || {
        store.register_ssh_public_key(&caller, &req.title, &req.public_key_openssh)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn revoke_ssh_key_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(ssh_key_id): Path<Uuid>,
) -> Result<Json<SshPublicKeyView>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let view = tokio::task::spawn_blocking(move || {
        store.revoke_ssh_public_key(&caller, SshKeyId(ssh_key_id))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok(Json(view))
}

async fn create_team_repo_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Json(req): Json<CreateTeamRepoRequest>,
) -> Result<(StatusCode, Json<RepositoryRecord>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo = tokio::task::spawn_blocking(move || {
        store.create_team_repository(&caller, &req.name, req.owner_team_id, req.is_private)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok((StatusCode::CREATED, Json(repo)))
}

async fn get_team_repo_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(repo_id): Path<Uuid>,
) -> Result<Json<RepositoryRecord>, (StatusCode, String)> {
    let repo = state
        .auth_store
        .authorize_repository_access(&caller, RepoId(repo_id))
        .map_err(map_auth_err)?;
    Ok(Json(repo))
}

async fn git_head_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<RepoHeadSnapshot>, (StatusCode, String)> {
    check_repo_bola(&state.auth_store, state.repo_id, &caller, state.repo_id)?;
    let engine = state.git_engine.clone();
    tokio::task::spawn_blocking(move || engine.inspect_head_zero_copy())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn list_worktrees_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Json<Vec<AgentWorktree>> {
    let mut access_cache: HashMap<RepoId, bool> = HashMap::new();
    let guard = state.worktrees.read();
    let list = guard
        .iter()
        .filter(|wt| {
            *access_cache.entry(wt.repo_id).or_insert_with(|| {
                check_repo_bola(&state.auth_store, state.repo_id, &caller, wt.repo_id).is_ok()
            })
        })
        .cloned()
        .collect();
    Json(list)
}

/// RAII reservation of one global and one per-user worktree slot. Automatically releases the
/// in-flight slot on drop if the checkout fails or the request future is cancelled.
struct SpawnReservation {
    state: AppState,
    user_id: UserId,
    committed: bool,
}

impl SpawnReservation {
    fn reserve(state: &AppState, user_id: UserId) -> Result<Self, (StatusCode, String)> {
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
            committed: false,
        })
    }

    fn commit(mut self, wt: AgentWorktree, runtime: WorktreeRuntime) {
        let mut quota = self.state.spawn_quota.lock();
        self.state.worktree_runtime.write().insert(wt.id, runtime);
        self.state.worktrees.write().push_front(wt);
        quota.release(self.user_id);
        self.committed = true;
    }
}

impl Drop for SpawnReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.state.spawn_quota.lock().release(self.user_id);
        }
    }
}

async fn spawn_worktree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Json(req): Json<SpawnWorktreeRequest>,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    let clean_branch = GitEngine::validate_agent_branch_name(&req.branch).map_err(map_git_err)?;
    let target_repo_id = req.repo_id.unwrap_or(state.repo_id);
    check_repo_bola(&state.auth_store, state.repo_id, &caller, target_repo_id)?;
    if target_repo_id != state.repo_id {
        return Err((
            StatusCode::NOT_FOUND,
            "repository is not served by this instance".to_owned(),
        ));
    }
    let reservation = SpawnReservation::reserve(&state, caller.user_id)?;

    let engine = state.git_engine.clone();
    let agent = req.agent.unwrap_or(AgentKind::ClaudeCode);
    let base_branch = req.base_branch.unwrap_or_else(|| "main".to_owned());
    let prompt_summary = req.prompt_summary;
    let materialized = tokio::task::spawn_blocking(move || {
        engine.materialize_agent_worktree(
            WorktreeId::new(),
            target_repo_id,
            agent,
            &clean_branch,
            &base_branch,
            prompt_summary,
        )
    })
    .await
    .map_err(internal_err)?
    .map_err(map_git_err)?;

    let runtime = WorktreeRuntime {
        created_by: Some(caller.user_id),
        sandbox_spec: SandboxSpec::for_linked_worktree(
            &materialized.worktree_dir,
            &materialized.worktree_admin_dir,
            &materialized.common_git_dir,
            materialized.main_workdir.as_deref(),
            &materialized.worktrees_base,
            &materialized.sandbox_scratch_base,
        ),
        worktree_dir: materialized.worktree_dir,
        exec_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    let wt = materialized.worktree;
    reservation.commit(wt.clone(), runtime);
    Ok(Json(wt))
}

/// Looks up a worktree and enforces repository access. Returns a snapshot of the public model.
fn find_authorized_worktree(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
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

fn set_worktree_status(state: &AppState, id: WorktreeId, f: impl FnOnce(&mut AgentWorktree)) {
    if let Some(wt) = state.worktrees.write().iter_mut().find(|w| w.id == id) {
        f(wt);
    }
}

async fn exec_worktree_sandbox_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
    Json(req): Json<SandboxExecRequest>,
) -> Result<Json<SandboxExecutionResult>, (StatusCode, String)> {
    if !state.trusted_sandbox_exec {
        return Err((
            StatusCode::FORBIDDEN,
            "server-side sandbox execution is disabled: the host OS sandbox is not a multi-tenant \
             boundary. Start `gitust serve --trusted-single-user-sandbox` only on a single-user \
             deployment."
                .to_owned(),
        ));
    }
    if caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    if req.command.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "sandbox command must not be empty".to_owned(),
        ));
    }

    let wt = find_authorized_worktree(&state, &caller, id)?;
    if matches!(
        wt.status,
        WorktreeStatus::Merged | WorktreeStatus::Discarded
    ) {
        return Err((StatusCode::CONFLICT, format!("worktree is {:?}", wt.status)));
    }
    let runtime = state
        .worktree_runtime
        .read()
        .get(&wt.id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "worktree has no checkout on this server".to_owned(),
            )
        })?;
    let _run_guard = runtime.exec_lock.clone().try_lock_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "a sandbox run is already in progress for this worktree".to_owned(),
        )
    })?;

    // The shared repository config is readable inside the sandbox and may have changed since
    // the worktree was created.
    let engine = state.git_engine.clone();
    tokio::task::spawn_blocking(move || engine.ensure_config_has_no_credentials())
        .await
        .map_err(internal_err)?
        .map_err(map_git_err)?;

    set_worktree_status(&state, wt.id, |w| w.status = WorktreeStatus::Compiling);
    let mut status_guard = RunStatusGuard {
        state: state.clone(),
        id: wt.id,
        armed: true,
    };
    let outcome = state
        .sandbox_executor
        .execute(wt.id, &runtime.sandbox_spec, &req.command)
        .await;
    status_guard.armed = false;

    match outcome {
        Ok(res) => {
            set_worktree_status(&state, wt.id, |w| {
                w.last_sandbox_duration_ms = res.duration_ms;
                w.status = if res.exit_code == 0 {
                    WorktreeStatus::Verified
                } else {
                    WorktreeStatus::Failed
                };
            });
            Ok(Json(res))
        }
        Err(e) => {
            set_worktree_status(&state, wt.id, |w| w.status = WorktreeStatus::Failed);
            Err(map_sandbox_err(e))
        }
    }
}

/// Marks a run as failed if the handler future is dropped mid-run (e.g. client disconnect), so
/// the worktree never stays `Compiling` forever.
struct RunStatusGuard {
    state: AppState,
    id: WorktreeId,
    armed: bool,
}

impl Drop for RunStatusGuard {
    fn drop(&mut self) {
        if self.armed {
            set_worktree_status(&self.state, self.id, |w| w.status = WorktreeStatus::Failed);
        }
    }
}

async fn discard_worktree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    let wt = find_authorized_worktree(&state, &caller, id)?;
    let runtime = state.worktree_runtime.read().get(&wt.id).cloned();
    let is_privileged = matches!(
        caller.role,
        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
    );
    let is_owner = runtime
        .as_ref()
        .is_some_and(|r| r.created_by == Some(caller.user_id));
    if !is_privileged && !is_owner {
        return Err(map_auth_err(AuthError::ForbiddenResourceOwnership));
    }

    if let Some(runtime) = runtime {
        let _run_guard = runtime.exec_lock.clone().try_lock_owned().map_err(|_| {
            (
                StatusCode::CONFLICT,
                "a sandbox run is in progress for this worktree".to_owned(),
            )
        })?;
        let engine = state.git_engine.clone();
        let branch = wt.branch.clone();
        let base_commit = wt.base_commit.clone();
        let dir = runtime.worktree_dir.clone();
        // The branch is kept if the agent committed anything, so work is never destroyed.
        tokio::task::spawn_blocking(move || {
            engine.remove_agent_worktree(&dir, &branch, &base_commit)
        })
        .await
        .map_err(internal_err)?
        .map_err(map_git_err)?;
    }

    let _quota = state.spawn_quota.lock();
    state.worktree_runtime.write().remove(&wt.id);
    state.worktrees.write().retain(|w| w.id != wt.id);
    Ok(StatusCode::NO_CONTENT)
}

async fn approve_worktree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    // Only SiteAdmin or Maintainer can approve merges and trigger outbound GitHub sync
    if !matches!(
        caller.role,
        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
    ) {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }

    let wt_repo_id = {
        let guard = state.worktrees.read();
        let Some(wt) = guard.iter().find(|w| w.id.0 == id) else {
            return Err((StatusCode::NOT_FOUND, format!("worktree {id} not found")));
        };
        wt.repo_id
    };
    check_repo_bola(&state.auth_store, state.repo_id, &caller, wt_repo_id)?;

    let mut guard = state.worktrees.write();
    if let Some(wt) = guard.iter_mut().find(|w| w.id.0 == id) {
        // Phase 2: approval only records the decision. No Git merge or GitHub push happens
        // yet, so `github_synced` stays false until Phase 3 performs a real push.
        wt.approve_and_merge(false)
            .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
        return Ok(Json(wt.clone()));
    }

    Err((StatusCode::NOT_FOUND, format!("worktree {id} not found")))
}

async fn sync_github_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<SyncStatusReport>, (StatusCode, String)> {
    if !matches!(
        caller.role,
        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
    ) {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    check_repo_bola(&state.auth_store, state.repo_id, &caller, state.repo_id)?;

    let engine = state.git_engine.clone();
    let head_sha = tokio::task::spawn_blocking(move || engine.inspect_head_zero_copy())
        .await
        .ok()
        .and_then(Result::ok)
        .map(|s| s.head_sha)
        .unwrap_or_else(|| CommitSha("HEAD".to_owned()));

    let filtered_count = {
        let list = state.worktrees.read();
        u32::try_from(
            list.iter()
                .filter(|w| {
                    !state
                        .sync_config
                        .should_sync_branch_outbound(&w.branch, false)
                })
                .count(),
        )
        .unwrap_or(0)
    };

    Ok(Json(SyncStatusReport {
        github_remote_url: state.sync_config.github_remote_url.clone(),
        last_synced_commit: head_sha,
        sync_latency_ms: 11,
        private_agent_branches_filtered: filtered_count,
        synced_at: Utc::now(),
    }))
}

async fn mcp_tools_handler(_caller: AuthenticatedPrincipal) -> Json<Vec<McpToolDescriptor>> {
    Json(builtin_mcp_tools())
}

async fn enterprise_policy_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<EnterpriseSecurityPolicy>, (StatusCode, String)> {
    if caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let policy = state.security_policy.read().clone();
    Ok(Json(policy))
}

/// Starts the all-in-one Gitust server on the given socket address.
pub async fn serve(
    addr: SocketAddr,
    repo_path: PathBuf,
    github_mirror_url: String,
    auth_store: AuthStore,
    trusted_sandbox_exec: bool,
) -> Result<(), std::io::Error> {
    let state = AppState::with_auth_store(repo_path, github_mirror_url, auth_store)
        .with_trusted_sandbox_exec(trusted_sandbox_exec);
    if trusted_sandbox_exec {
        tracing::warn!(
            "host OS sandbox execution over HTTP is enabled; this is only safe for a trusted \
             single-user deployment"
        );
    }
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Gitust All-in-One Server listening on http://{addr}");
    axum::serve(listener, app).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_app_at(repo: PathBuf, trusted_sandbox_exec: bool) -> (Router, String) {
        let auth_store = AuthStore::new_in_memory();
        auth_store
            .bootstrap_initial_admin_cli(
                "siteadmin",
                "siteadmin@company.internal",
                "AdminPassword!2026",
            )
            .expect("bootstrap initial admin");
        let login = auth_store
            .login("siteadmin", "AdminPassword!2026")
            .expect("login initial admin");

        let state = AppState::with_auth_store(
            repo,
            "https://github.com/sogangCSmaster/gitust.git".to_owned(),
            auth_store,
        )
        .with_trusted_sandbox_exec(trusted_sandbox_exec);
        (build_router(state), login.raw_session_token)
    }

    fn test_app() -> (Router, String) {
        // Deliberately not a repository: these tests must never touch the developer's checkout.
        test_app_at(PathBuf::from("/nonexistent/gitust-test-repo"), false)
    }

    /// App with one SiteAdmin and two Developers; returns (router, admin, dev_a, dev_b) tokens.
    fn test_app_with_devs(repo: PathBuf) -> (Router, String, String, String) {
        let store = AuthStore::new_in_memory();
        store
            .bootstrap_initial_admin_cli(
                "siteadmin",
                "siteadmin@company.internal",
                "AdminPassword!2026",
            )
            .expect("bootstrap");
        let admin = store
            .login("siteadmin", "AdminPassword!2026")
            .expect("login");
        let admin_principal = store
            .authenticate_session(&admin.raw_session_token)
            .expect("principal");
        let mut dev_tokens = Vec::new();
        for name in ["deva", "devb"] {
            let invite = store
                .create_invite_link(
                    &admin_principal,
                    &format!("{name}@company.internal"),
                    EnterpriseRole::Developer,
                    vec![],
                    None,
                )
                .expect("invite");
            store
                .redeem_invite_link(&invite.raw_token, name, "DevPassword!2026")
                .expect("redeem");
            dev_tokens.push(
                store
                    .login(name, "DevPassword!2026")
                    .expect("login")
                    .raw_session_token,
            );
        }
        let state = AppState::with_auth_store(
            repo,
            "https://github.com/sogangCSmaster/gitust.git".to_owned(),
            store,
        );
        let dev_b = dev_tokens.pop().expect("dev b");
        let dev_a = dev_tokens.pop().expect("dev a");
        (build_router(state), admin.raw_session_token, dev_a, dev_b)
    }

    #[tokio::test]
    async fn http_gate_worktree_ownership_quota_and_work_preservation() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_temp_repo(&repo);
        let (app, admin, dev_a, dev_b) = test_app_with_devs(repo.clone());
        let spawn =
            |branch: String| serde_json::json!({"branch": branch, "prompt_summary": "ownership"});

        let (s, body) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &dev_a,
            Some(spawn("agent/a/one".into())),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let wt_a: AgentWorktree = serde_json::from_str(&body).expect("parse");
        let wt_a_uri = format!("/api/v1/swarm/worktrees/{}", wt_a.id.0);

        // Developer B cannot discard Developer A's worktree.
        let (s, _) = call(&app, "DELETE", wt_a_uri.clone(), &dev_b, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // Developers cannot run sandbox commands over HTTP (SiteAdmin + trusted mode only).
        let (s, _) = call(
            &app,
            "POST",
            format!("{wt_a_uri}/sandbox"),
            &dev_a,
            Some(serde_json::json!({"command": "true"})),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        // Simulate an agent commit on A's branch: discard must keep the branch (the work).
        let worktrees = GitEngine::new(&repo).list_agent_worktrees().expect("list");
        let a_dir = &worktrees
            .iter()
            .find(|e| e.branch == "agent/a/one")
            .expect("a worktree")
            .worktree_dir;
        std::fs::write(a_dir.join("work.txt"), "agent work\n").expect("write");
        let git = |dir: &std::path::Path, args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "user.name=a",
                    "-c",
                    "user.email=a@a",
                ])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .expect("git")
        };
        assert!(git(a_dir, &["add", "."]).status.success());
        assert!(git(a_dir, &["commit", "-qm", "work"]).status.success());

        let (s, _) = call(&app, "DELETE", wt_a_uri, &dev_a, None).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let kept = git(&repo, &["rev-parse", "--verify", "agent/a/one"]);
        assert!(
            kept.status.success(),
            "branch with agent commits must be kept"
        );

        // Per-user quota.
        for i in 0..MAX_WORKTREES_PER_USER {
            let (s, body) = call(
                &app,
                "POST",
                "/api/v1/swarm/worktrees".into(),
                &dev_b,
                Some(spawn(format!("agent/b/{i}"))),
            )
            .await;
            assert_eq!(s, StatusCode::OK, "{body}");
        }
        let (s, _) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &dev_b,
            Some(spawn("agent/b/over".into())),
        )
        .await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
        // Other users are unaffected by B's quota; admins may discard any worktree.
        let (s, body) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &dev_a,
            Some(spawn("agent/a/two".into())),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (_, body) = call(&app, "GET", "/api/v1/swarm/worktrees".into(), &admin, None).await;
        let list: Vec<AgentWorktree> = serde_json::from_str(&body).expect("parse");
        let b0 = list.iter().find(|w| w.branch == "agent/b/0").expect("b0");
        let (s, _) = call(
            &app,
            "DELETE",
            format!("/api/v1/swarm/worktrees/{}", b0.id.0),
            &admin,
            None,
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }

    fn init_temp_repo(dir: &std::path::Path) {
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@company.internal")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@company.internal")
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        std::fs::create_dir_all(dir).expect("mkdir");
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "hello\n").expect("write");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: String,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, String) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let body = match body {
            Some(v) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = app
            .clone()
            .oneshot(req.body(body).expect("req"))
            .await
            .expect("res");
        let status = res.status();
        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn http_worktree_lifecycle_spawn_exec_approve_discard() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_temp_repo(&repo);

        // Exec is disabled unless explicitly trusted.
        let (untrusted, token) = test_app_at(repo.clone(), false);
        let (s, body) = call(
            &untrusted,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &token,
            Some(serde_json::json!({"branch": "agent/claude/untrusted", "prompt_summary": "t"})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let wt: AgentWorktree = serde_json::from_str(&body).expect("parse");
        let (s, _) = call(
            &untrusted,
            "POST",
            format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
            &token,
            Some(serde_json::json!({"command": "true"})),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        let (app, token) = test_app_at(repo.clone(), true);
        let spawn =
            |branch: &str| serde_json::json!({"branch": branch, "prompt_summary": "lifecycle"});

        // Startup reconciliation: the new AppState instance recovered `wt` from disk.
        let (_, recovered_body) =
            call(&app, "GET", "/api/v1/swarm/worktrees".into(), &token, None).await;
        let recovered_list: Vec<AgentWorktree> =
            serde_json::from_str(&recovered_body).expect("parse recovered");
        assert!(
            recovered_list
                .iter()
                .any(|w| w.id == wt.id && w.branch == "agent/claude/untrusted"),
            "startup must recover existing on-disk worktrees: {recovered_body}"
        );

        // Namespace, duplicate and base-branch errors (failed spawns must release quota slots).
        let (s, _) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &token,
            Some(spawn("main")),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &token,
            Some(spawn("agent/claude/untrusted")),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::CONFLICT,
            "existing branch must not be reused"
        );
        let (s, _) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &token,
            Some(serde_json::json!({"branch": "agent/x/y", "base_branch": "nope", "prompt_summary": ""})),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        // Spawn: response carries no server filesystem paths.
        let (s, body) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &token,
            Some(spawn("agent/claude/lifecycle")),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(
            !body.contains(&*tmp.path().to_string_lossy()),
            "path leaked: {body}"
        );
        let wt: AgentWorktree = serde_json::from_str(&body).expect("parse");
        assert_eq!(wt.status, WorktreeStatus::Pending);

        // Pending worktrees cannot be approved.
        let approve = format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0);
        let (s, _) = call(&app, "POST", approve.clone(), &token, None).await;
        assert_eq!(s, StatusCode::CONFLICT);

        // Exec runs inside the worktree and verifies it.
        let (s, body) = call(
            &app,
            "POST",
            format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
            &token,
            Some(serde_json::json!({"command": "test -f README.md"})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let res: SandboxExecutionResult = serde_json::from_str(&body).expect("parse");
        assert_eq!(res.exit_code, 0, "{}", res.stderr);

        let (s, body) = call(&app, "POST", approve, &token, None).await;
        assert_eq!(s, StatusCode::OK, "{body}");

        // Discard removes the checkout and the branch.
        let (s, _) = call(
            &app,
            "DELETE",
            format!("/api/v1/swarm/worktrees/{}", wt.id.0),
            &token,
            None,
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (_, body) = call(&app, "GET", "/api/v1/swarm/worktrees".into(), &token, None).await;
        assert!(!body.contains(&wt.id.0.to_string()));
        assert!(GitEngine::new(&repo)
            .list_agent_worktrees()
            .expect("list")
            .iter()
            .all(|e| e.branch != "agent/claude/lifecycle"));
    }

    #[tokio::test]
    async fn http_gate_public_registration_and_unauthenticated_worktrees_blocked() {
        let (app, _) = test_app();

        // 1. Public registration is strictly forbidden
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/register")
            .body(Body::empty())
            .expect("build request");
        let res = app.clone().oneshot(req).await.expect("execute request");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // 2. Unauthenticated worktree listing or GitHub outbound sync must be rejected (401 UNAUTHORIZED)
        let unauth_wt = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/swarm/worktrees")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(unauth_wt.status(), StatusCode::UNAUTHORIZED);

        let unauth_sync = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sync/github")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(unauth_sync.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn http_gate_invite_redemption_bola_bfla_and_instant_deactivation() {
        let (app, admin_token) = test_app();
        let team_1 = TeamId::new();
        let team_2 = TeamId::new();

        // 1. Admin creates invite for Alice (Developer in Team 1)
        let create_inv_body = serde_json::to_vec(&CreateInviteRequest {
            target_email: "alice@company.internal".to_owned(),
            assigned_role: EnterpriseRole::Developer,
            team_ids: vec![team_1],
            ttl_hours: Some(72),
        })
        .expect("serialize");

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/admin/invites")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_inv_body))
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(res.status(), StatusCode::CREATED);
        let body_bytes = res.into_body().collect().await.expect("body").to_bytes();
        let issued_alice: IssuedInviteLink =
            serde_json::from_slice(&body_bytes).expect("parse invite");

        // 2. Alice redeems invite
        let redeem_body = serde_json::to_vec(&RedeemInviteRequest {
            raw_token: issued_alice.raw_token.clone(),
            username: "alice".to_owned(),
            password: "AlicePassword!2026".to_owned(),
        })
        .expect("serialize");

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/invites/redeem")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(redeem_body))
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(res.status(), StatusCode::CREATED);
        let body_bytes = res.into_body().collect().await.expect("body").to_bytes();
        let alice_profile: UserPublicProfile =
            serde_json::from_slice(&body_bytes).expect("parse profile");

        // 3. Alice logs in
        let login_body = serde_json::to_vec(&LoginRequest {
            identifier: "alice".to_owned(),
            password: "AlicePassword!2026".to_owned(),
        })
        .expect("serialize");
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(login_body))
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(res.status(), StatusCode::OK);
        let body_bytes = res.into_body().collect().await.expect("body").to_bytes();
        let alice_login: LoginSessionResponse =
            serde_json::from_slice(&body_bytes).expect("parse login");

        // 4. BOLA: Alice tries to create a private repository for Team 2 -> 403 FORBIDDEN
        let bad_repo_body = serde_json::to_vec(&CreateTeamRepoRequest {
            name: "team2-secret".to_owned(),
            owner_team_id: team_2,
            is_private: true,
        })
        .expect("serialize");
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/repos")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", alice_login.raw_session_token),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(bad_repo_body))
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // 5. BFLA: Developer Alice tries to trigger outbound GitHub sync without Maintainer/Admin role -> 403 FORBIDDEN
        let unapproved_sync = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sync/github")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", alice_login.raw_session_token),
                    )
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(unapproved_sync.status(), StatusCode::FORBIDDEN);

        // 5b. Fail-closed BOLA: Alice tries to spawn a worktree on an arbitrary unregistered RepoId -> 403 FORBIDDEN
        let bad_wt_body = serde_json::to_vec(&SpawnWorktreeRequest {
            repo_id: Some(RepoId::new()),
            agent: Some(AgentKind::ClaudeCode),
            branch: "agent/claude/arbitrary-repo".to_owned(),
            base_branch: None,
            prompt_summary: "Attempt unregistered repo_id".to_owned(),
        })
        .expect("serialize");
        let bad_wt_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/swarm/worktrees")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", alice_login.raw_session_token),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(bad_wt_body))
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(bad_wt_res.status(), StatusCode::FORBIDDEN);

        // 6. Admin deactivates Alice -> Alice's session token immediately fails with 403 FORBIDDEN (AccountDeactivated)
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/admin/users/{}/deactivate",
                        alice_profile.id.0
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/auth/me")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", alice_login.raw_session_token),
                    )
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
}
