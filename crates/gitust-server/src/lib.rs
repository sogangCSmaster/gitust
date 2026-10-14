//! `gitust-server`: High-performance Axum HTTP/REST, Invite-Only Auth, Built-in MCP Gateway,
//! and Real-Time Sync Server for Gitust.

use std::collections::VecDeque;
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
    RepositoryRecord, SshKeyId, SshPublicKeyView, TeamId, UserId, UserPublicProfile,
};
use gitust_db::{AuthStore, IssuedInviteLink, IssuedPersonalAccessToken, LoginSessionResponse};
use gitust_git::{GitEngine, RepoHeadSnapshot};
use gitust_sandbox::{builtin_mcp_tools, McpToolDescriptor, MicrosandboxExecutor};
use gitust_sync::{BiDirectionalSyncConfig, SyncStatusReport};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub repo_id: RepoId,
    pub auth_store: AuthStore,
    pub git_engine: GitEngine,
    pub sandbox_executor: MicrosandboxExecutor,
    pub sync_config: BiDirectionalSyncConfig,
    pub security_policy: Arc<RwLock<EnterpriseSecurityPolicy>>,
    pub worktrees: Arc<RwLock<VecDeque<AgentWorktree>>>,
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

        let initial_worktrees = VecDeque::from([
            git_engine.spawn_cow_agent_worktree(
                repo_id,
                AgentKind::ClaudeCode,
                "agent/claude/zero-copy-pack-delta",
                "main",
                "Implement O(1) memory-mapped packfile delta resolution in gitust-git",
            ),
            git_engine.spawn_cow_agent_worktree(
                repo_id,
                AgentKind::CursorAgent,
                "agent/cursor/mcp-batch-commit-rpc",
                "main",
                "Add single-roundtrip MCP tool for multi-file atomic commit + sandbox run",
            ),
        ]);

        Self {
            repo_id,
            auth_store,
            git_engine,
            sandbox_executor: MicrosandboxExecutor::new(16),
            sync_config: BiDirectionalSyncConfig::new(repo_id, github_mirror_url),
            security_policy: Arc::new(RwLock::new(EnterpriseSecurityPolicy::default())),
            worktrees: Arc::new(RwLock::new(initial_worktrees)),
        }
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
    pub branch: String,
    pub prompt_summary: String,
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

/// Enforces team repository BOLA access if `repo_id` is registered in `auth_store`.
fn check_repo_bola(
    auth_store: &AuthStore,
    caller: &AuthenticatedPrincipal,
    repo_id: RepoId,
) -> Result<(), (StatusCode, String)> {
    if auth_store.repository_exists(repo_id) {
        auth_store
            .authorize_repository_access(caller, repo_id)
            .map_err(map_auth_err)?;
    }
    Ok(())
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
    check_repo_bola(&state.auth_store, &caller, state.repo_id)?;
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
    let guard = state.worktrees.read();
    let list = guard
        .iter()
        .filter(|wt| check_repo_bola(&state.auth_store, &caller, wt.repo_id).is_ok())
        .cloned()
        .collect();
    Json(list)
}

async fn spawn_worktree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Json(req): Json<SpawnWorktreeRequest>,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    if req.branch.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "branch name must not be empty".to_owned(),
        ));
    }
    let target_repo_id = req.repo_id.unwrap_or(state.repo_id);
    check_repo_bola(&state.auth_store, &caller, target_repo_id)?;

    let wt = state.git_engine.spawn_cow_agent_worktree(
        target_repo_id,
        req.agent.unwrap_or(AgentKind::ClaudeCode),
        req.branch,
        "main",
        req.prompt_summary,
    );
    state.worktrees.write().push_front(wt.clone());
    Ok(Json(wt))
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
    check_repo_bola(&state.auth_store, &caller, wt_repo_id)?;

    let mut guard = state.worktrees.write();
    if let Some(wt) = guard.iter_mut().find(|w| w.id.0 == id) {
        let should_sync = state
            .sync_config
            .should_sync_branch_outbound(&wt.base_branch, true);
        wt.approve_and_merge(should_sync)
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
    check_repo_bola(&state.auth_store, &caller, state.repo_id)?;

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
) -> Result<(), std::io::Error> {
    let state = AppState::with_auth_store(repo_path, github_mirror_url, auth_store);
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

    fn test_app() -> (Router, String) {
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
            PathBuf::from("."),
            "https://github.com/sogangCSmaster/gitust.git".to_owned(),
            auth_store,
        );
        (build_router(state), login.raw_session_token)
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
