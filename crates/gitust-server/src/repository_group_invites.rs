//! Issues one-use account links for the actual hosted repository's owning group.
use crate::{
    auth::{bearer_token, extract_principal_async},
    repository_reads::id,
    state::{map_auth_err, AppState},
};
use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use gitust_core::{AuthError, EnterpriseRole, InviteId};
use gitust_db::{
    RepositoryGroupInviteError, RepositoryGroupInviteOutcome, RepositoryGroupInviteRequest,
};
use serde::Deserialize;
type ApiError = (StatusCode, String);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    invite_id: InviteId,
    target_email: String,
    assigned_role: EnterpriseRole,
}
pub(crate) async fn create(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    let mut response = match value(state, raw, request).await {
        Ok((status, value)) => (status, Json(value)).into_response(),
        Err(error) => error.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
fn auth_error(error: AuthError) -> ApiError {
    match error {
        AuthError::StorageError(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "repository data is unavailable".into(),
        ),
        other => map_auth_err(other),
    }
}
async fn value(
    state: AppState,
    raw: String,
    request: Request,
) -> Result<(StatusCode, serde_json::Value), ApiError> {
    let repository_id = id(&raw)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store)
        .await
        .map_err(|(status, message)| {
            if status.is_server_error() {
                (status, "repository data is unavailable".into())
            } else {
                (status, message)
            }
        })?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 4096).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "group invitation request too large".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let request: CreateRequest = serde_json::from_slice(&body).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "invalid group invitation request".into(),
            )
        })?;
        let repository = state
            .read_repository(repository_id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
        if repository.access.registration.is_none() {
            return Err((
                StatusCode::FORBIDDEN,
                "repository registration is required for group invitations".into(),
            ));
        }
        let _ownership = repository.engine.lock_worktree_service().map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "repository data is unavailable".into(),
            )
        })?;
        #[cfg(test)]
        crate::repository_reads::after_read(&state);
        let invite_id = request.invite_id;
        let outcome = state.auth_store.create_registered_repository_group_invite(
            bearer_token(&parts.headers)?,
            origin.user_id,
            repository.access,
            RepositoryGroupInviteRequest {
                invite_id,
                target_email: request.target_email,
                assigned_role: request.assigned_role,
            },
        );
        match outcome {
            Ok(result) => match result.outcome {
                RepositoryGroupInviteOutcome::Created(issued) => Ok((
                    StatusCode::CREATED,
                    serde_json::json!({
                        "repository_id":result.repository_id,"owner_team_id":result.owner_team_id,
                        "invite":issued.invite,"invite_url_path":issued.invite_url_path
                    }),
                )),
                RepositoryGroupInviteOutcome::AlreadyIssued(invite) => Ok((
                    StatusCode::CONFLICT,
                    serde_json::json!({
                        "code":"invite_already_issued","repository_id":result.repository_id,
                        "owner_team_id":result.owner_team_id,"invite":invite
                    }),
                )),
            },
            Err(RepositoryGroupInviteError::Auth(error)) => Err(auth_error(error)),
            Err(RepositoryGroupInviteError::Conflict) => Ok((
                StatusCode::CONFLICT,
                serde_json::json!({"code":"invite_request_conflict"}),
            )),
            Err(RepositoryGroupInviteError::NotPublished(_)) => Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"code":"invite_not_published","invite_id":invite_id}),
            )),
            Err(RepositoryGroupInviteError::Unconfirmed(_)) => Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"code":"invite_unconfirmed","invite_id":invite_id}),
            )),
        }
    })
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "repository data is unavailable".into(),
        )
    })?
}
