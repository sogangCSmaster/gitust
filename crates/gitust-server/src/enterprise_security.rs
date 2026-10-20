//! HTTP handlers for Slice 9 (Phase 4.4):
//! - RFC 6238 TOTP Two-Factor Authentication (`/api/v1/auth/totp/*`)
//! - Enterprise OIDC/LDAP SSO Provider Management & Invite-Bound Login (`/api/v1/admin/sso/providers`, `/api/v1/auth/sso/login`)
//! - Tamper-Evident SHA-256 Hash-Chained Audit Trail (`/api/v1/admin/audit-log`)

use std::net::SocketAddr;

use axum::{
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use gitust_core::{
    AuthError, EnterpriseRole, SsoProviderRecord, TotpEnrollmentChallenge, TotpStatus,
};
use gitust_db::{LoginSessionResponse, RegisterSsoProviderInput, SsoAssertionLoginInput};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{bearer_token, extract_principal, extract_principal_async};
use crate::repository_reads::response;
use crate::state::{internal_err, map_auth_err, AppState};
use crate::workflows::{bad_req, ApiError};

const MAX_SECURITY_BODY_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotpCodeInput {
    pub code: String,
}

pub(crate) async fn totp_status_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || -> Result<TotpStatus, ApiError> {
        let caller = extract_principal(&headers, &state.auth_store)?;
        state.auth_store.totp_status(&caller).map_err(map_auth_err)
    })
    .await
    .map_err(internal_err);
    response(res.and_then(|r| r))
}

pub(crate) async fn totp_enroll_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let res = async move {
        let origin = extract_principal_async(&headers, &state.auth_store).await?;
        tokio::task::spawn_blocking(move || -> Result<TotpEnrollmentChallenge, ApiError> {
            let caller = extract_principal(&headers, &state.auth_store)?;
            if caller.user_id != origin.user_id {
                return Err(map_auth_err(AuthError::Unauthenticated));
            }
            let token = bearer_token(&headers)?;
            state
                .auth_store
                .enroll_totp_with_credential(token, origin.user_id)
                .map_err(map_auth_err)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

pub(crate) async fn totp_verify_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        let bytes = axum::body::to_bytes(body, MAX_SECURITY_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: TotpCodeInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        tokio::task::spawn_blocking(move || -> Result<TotpStatus, ApiError> {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id {
                return Err(map_auth_err(AuthError::Unauthenticated));
            }
            let token = bearer_token(&parts.headers)?;
            state
                .auth_store
                .verify_totp_with_credential(token, origin.user_id, &input.code)
                .map_err(map_auth_err)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

pub(crate) async fn totp_disable_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        let bytes = axum::body::to_bytes(body, MAX_SECURITY_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: TotpCodeInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        tokio::task::spawn_blocking(move || -> Result<TotpStatus, ApiError> {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id {
                return Err(map_auth_err(AuthError::Unauthenticated));
            }
            let token = bearer_token(&parts.headers)?;
            state
                .auth_store
                .disable_totp_with_credential(token, origin.user_id, &input.code)
                .map_err(map_auth_err)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

pub(crate) async fn list_sso_providers_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let caller = extract_principal(&headers, &state.auth_store)?;
        if caller.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let providers = state
            .auth_store
            .list_sso_providers_for_admin(&caller)
            .map_err(map_auth_err)?;
        Ok(json!({ "providers": providers }))
    })
    .await
    .map_err(internal_err);
    response(res.and_then(|r| r))
}

pub(crate) async fn register_sso_provider_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let res: Result<(StatusCode, Json<SsoProviderRecord>), ApiError> = async move {
        let (parts, body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let bytes = axum::body::to_bytes(body, MAX_SECURITY_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: RegisterSsoProviderInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let token = bearer_token(&parts.headers)?;
            let record = state
                .auth_store
                .register_sso_provider_with_credential(token, origin.user_id, input)
                .map_err(map_auth_err)?;
            Ok((StatusCode::CREATED, Json(record)))
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    match res {
        Ok(pair) => pair.into_response(),
        Err(err) => response::<()>(Err(err)),
    }
}

pub(crate) async fn delete_sso_provider_handler(
    State(state): State<AppState>,
    AxumPath(raw_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let res: Result<StatusCode, ApiError> = async move {
        let origin = extract_principal_async(&headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let provider_id =
            Uuid::parse_str(raw_id.trim()).map_err(|_| bad_req("invalid SSO provider UUID"))?;
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let token = bearer_token(&headers)?;
            state
                .auth_store
                .delete_sso_provider_with_credential(token, origin.user_id, provider_id)
                .map_err(map_auth_err)?;
            Ok(StatusCode::NO_CONTENT)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    match res {
        Ok(code) => code.into_response(),
        Err(err) => response::<()>(Err(err)),
    }
}

pub(crate) async fn sso_login_handler(State(state): State<AppState>, request: Request) -> Response {
    let res: Result<LoginSessionResponse, ApiError> = async move {
        let (parts, body) = request.into_parts();
        let client_scope = parts
            .extensions
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip().to_string());
        let bytes = axum::body::to_bytes(body, MAX_SECURITY_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: SsoAssertionLoginInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        tokio::task::spawn_blocking(move || {
            state
                .auth_store
                .login_with_sso_assertion(input, client_scope.as_deref())
                .map_err(map_auth_err)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotateMasterSecretsInput {
    pub new_secret_key_hex: String,
    #[serde(default)]
    pub revoke_existing_credentials: bool,
}

pub(crate) async fn rotate_secrets_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let bytes = axum::body::to_bytes(body, MAX_SECURITY_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: RotateMasterSecretsInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        let raw_hex = zeroize::Zeroizing::new(input.new_secret_key_hex.trim().to_owned());
        if raw_hex.len() != 64 {
            return Err(bad_req("new_secret_key_hex must be 64 hex characters"));
        }
        let mut new_key = [0u8; 32];
        hex::decode_to_slice(raw_hex.as_bytes(), &mut new_key)
            .map_err(|_| bad_req("new_secret_key_hex must be valid hex"))?;
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let token = bearer_token(&parts.headers)?;
            let rotated = state
                .auth_store
                .rotate_master_secret_key_with_credential(
                    token,
                    origin.user_id,
                    new_key,
                    input.revoke_existing_credentials,
                )
                .map_err(map_auth_err)?;
            Ok(json!({
                "status": "rotated",
                "rotated_secrets_count": rotated,
                "revoked_existing_credentials": input.revoke_existing_credentials
            }))
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

pub(crate) async fn audit_log_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let caller = extract_principal(&headers, &state.auth_store)?;
        if caller.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let (entries, verification) = state
            .auth_store
            .audit_log_for_admin(&caller)
            .map_err(map_auth_err)?;
        Ok(json!({
            "entries": entries,
            "verification": verification
        }))
    })
    .await
    .map_err(internal_err);
    response(res.and_then(|r| r))
}
