//! Invite-only authentication, session/PAT extraction, SiteAdmin user/invite management,
//! and team repository BOLA handlers.

use std::net::SocketAddr;

use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use gitust_core::{
    AuthError, AuthenticatedPrincipal, EnterpriseRole, EnterpriseSecurityPolicy, InviteId,
    InvitePublicView, PatId, PersonalAccessTokenView, RepoId, RepositoryRecord, SshKeyId,
    SshPublicKeyView, TeamId, UserId, UserPublicProfile,
};
use gitust_db::{AuthStore, IssuedInviteLink, IssuedPersonalAccessToken, LoginSessionResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{map_auth_err, AppState};

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

/// Extracts and validates either a Session token (`gitust_ses_...`) or a Personal Access Token (`gitust_pat_...`)
/// from the `Authorization: Bearer <token>` header in O(1) time.
pub(crate) fn extract_principal(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    extract_principal_for(headers, auth_store, false)
}

fn extract_principal_for(
    headers: &HeaderMap,
    auth_store: &AuthStore,
    allow_mcp: bool,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    let Some(raw_header) = headers.get(header::AUTHORIZATION) else {
        return Err(map_auth_err(AuthError::Unauthenticated));
    };
    // fail-open-ok: non-ASCII Authorization header immediately fails closed with `AuthError::Unauthenticated`.
    let Ok(header_str) = raw_header.to_str() else {
        return Err(map_auth_err(AuthError::Unauthenticated));
    };
    let Some(token) = header_str.strip_prefix("Bearer ").map(str::trim) else {
        return Err(map_auth_err(AuthError::Unauthenticated));
    };

    if token.starts_with("gitust_pat_") {
        auth_store.authenticate_pat(token).map_err(map_auth_err)
    } else if allow_mcp {
        auth_store
            .authenticate_mcp_session(token)
            .map_err(map_auth_err)
    } else {
        auth_store.authenticate_session(token).map_err(map_auth_err)
    }
}

/// Async wrapper around `extract_principal` that runs `AuthStore` lock/reload checks on the
/// blocking pool so a held cross-process `.lock` cannot stall Tokio async worker threads.
pub(crate) async fn extract_principal_async(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    extract_principal_async_for(headers, auth_store, false).await
}

pub(crate) async fn extract_mcp_principal_async(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    extract_principal_async_for(headers, auth_store, true).await
}

async fn extract_principal_async_for(
    headers: &HeaderMap,
    auth_store: &AuthStore,
    allow_mcp: bool,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    let Some(raw_header) = headers.get(header::AUTHORIZATION).cloned() else {
        return Err(map_auth_err(AuthError::Unauthenticated));
    };
    let store = auth_store.clone();
    tokio::task::spawn_blocking(move || {
        let mut h = HeaderMap::with_capacity(1);
        h.insert(header::AUTHORIZATION, raw_header);
        extract_principal_for(&h, &store, allow_mcp)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

/// Zero-boilerplate Axum extractor for `AuthenticatedPrincipal`.
/// Guarantees that every handler declaring `caller: AuthenticatedPrincipal` is authenticated
/// in O(1) time and immediately rejects deactivated accounts or revoked tokens.
impl FromRequestParts<AppState> for AuthenticatedPrincipal {
    type Rejection = (StatusCode, String);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        extract_principal_async(&parts.headers, &state.auth_store).await
    }
}

pub(crate) async fn public_register_disabled_handler() -> (StatusCode, String) {
    map_auth_err(AuthError::PublicSignupDisabled)
}

pub(crate) async fn login_handler(
    State(state): State<AppState>,
    extensions: axum::http::Extensions,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginSessionResponse>, (StatusCode, String)> {
    let client_scope = extensions
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());
    let store = state.auth_store.clone();
    let res = tokio::task::spawn_blocking(move || {
        store.login_with_client_scope(&req.identifier, &req.password, client_scope.as_deref())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok(Json(res))
}

pub(crate) async fn me_handler(caller: AuthenticatedPrincipal) -> Json<AuthenticatedPrincipal> {
    Json(caller)
}

pub(crate) async fn logout_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let raw_token = headers
        .get(header::AUTHORIZATION)
        // fail-open-ok: invalid header immediately fails closed via `.ok_or_else(|| map_auth_err(AuthError::Unauthenticated))?`.
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(str::trim))
        .ok_or_else(|| map_auth_err(AuthError::Unauthenticated))?
        .to_owned();
    if raw_token.starts_with("gitust_pat_") {
        return Err(map_auth_err(AuthError::InvalidInput(
            "logout requires a session bearer token (use DELETE /api/v1/auth/pats/{id} to revoke a PAT)".to_owned(),
        )));
    }
    let store = state.auth_store.clone();
    tokio::task::spawn_blocking(move || store.revoke_session(&caller, &raw_token))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_invite_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: HeaderMap,
    Json(req): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<IssuedInviteLink>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let issued = tokio::task::spawn_blocking(move || {
        let caller = extract_principal(&headers, &store)?;
        store
            .create_invite_link(
                &caller,
                &req.target_email,
                req.assigned_role,
                req.team_ids,
                req.ttl_hours,
            )
            .map_err(map_auth_err)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok((StatusCode::CREATED, Json(issued)))
}

pub(crate) async fn list_invites_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<Vec<InvitePublicView>>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let list = tokio::task::spawn_blocking(move || store.list_invites(&caller))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(Json(list))
}

pub(crate) async fn revoke_invite_handler(
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

pub(crate) async fn redeem_invite_handler(
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

pub(crate) async fn create_password_reset_handler(
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

pub(crate) async fn redeem_password_reset_handler(
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

pub(crate) async fn list_users_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<Vec<UserPublicProfile>>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let list = tokio::task::spawn_blocking(move || store.list_users(&caller))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(Json(list))
}

pub(crate) async fn deactivate_user_handler(
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

pub(crate) async fn list_pats_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<Vec<PersonalAccessTokenView>>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let list = tokio::task::spawn_blocking(move || store.list_personal_access_tokens(&caller))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(Json(list))
}

pub(crate) async fn create_pat_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: HeaderMap,
    Json(req): Json<CreatePatRequest>,
) -> Result<(StatusCode, Json<IssuedPersonalAccessToken>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let issued = tokio::task::spawn_blocking(move || {
        let caller = extract_principal(&headers, &store)?;
        store
            .create_personal_access_token(&caller, &req.name, req.ttl_days)
            .map_err(map_auth_err)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok((StatusCode::CREATED, Json(issued)))
}

pub(crate) async fn revoke_pat_handler(
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

pub(crate) async fn list_ssh_keys_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<Vec<SshPublicKeyView>>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let list = tokio::task::spawn_blocking(move || store.list_ssh_public_keys(&caller))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(Json(list))
}

pub(crate) async fn register_ssh_key_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: HeaderMap,
    Json(req): Json<RegisterSshKeyRequest>,
) -> Result<(StatusCode, Json<SshPublicKeyView>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let view = tokio::task::spawn_blocking(move || {
        let caller = extract_principal(&headers, &store)?;
        store
            .register_ssh_public_key(&caller, &req.title, &req.public_key_openssh)
            .map_err(map_auth_err)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok((StatusCode::CREATED, Json(view)))
}

pub(crate) async fn revoke_ssh_key_handler(
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

pub(crate) async fn create_team_repo_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: HeaderMap,
    Json(req): Json<CreateTeamRepoRequest>,
) -> Result<(StatusCode, Json<RepositoryRecord>), (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo = tokio::task::spawn_blocking(move || {
        let caller = extract_principal(&headers, &store)?;
        store
            .create_team_repository(&caller, &req.name, req.owner_team_id, req.is_private)
            .map_err(map_auth_err)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok((StatusCode::CREATED, Json(repo)))
}

pub(crate) async fn get_team_repo_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(repo_id): Path<Uuid>,
) -> Result<Json<RepositoryRecord>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo = tokio::task::spawn_blocking(move || {
        store.authorize_repository_access(&caller, RepoId(repo_id))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(map_auth_err)?;
    Ok(Json(repo))
}

pub(crate) async fn enterprise_policy_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<EnterpriseSecurityPolicy>, (StatusCode, String)> {
    if caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let store = state.auth_store.clone();
    let policy = tokio::task::spawn_blocking(move || store.security_policy())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(map_auth_err)?;
    Ok(Json(policy))
}
