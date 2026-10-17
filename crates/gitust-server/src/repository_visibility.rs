//! Changes only the current ACL visibility of an explicitly hosted registered repository.
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
use gitust_core::{AuthError, EnterpriseRole, RepoId};
use gitust_db::RepositoryVisibilityError;
use serde::{Deserialize, Serialize};
type ApiError = (StatusCode, String);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeRequest {
    expected_is_private: bool,
    is_private: bool,
}
#[derive(Serialize)]
struct Changed {
    id: RepoId,
    is_private: bool,
}

pub(crate) async fn change(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    let mut response = match value(state, raw, request).await {
        Ok(value) => Json(value).into_response(),
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
fn change_error(error: RepositoryVisibilityError) -> ApiError {
    match error {
        RepositoryVisibilityError::Auth(error) => auth_error(error),
        RepositoryVisibilityError::Conflict => (StatusCode::CONFLICT, "repository visibility changed; refresh before retrying".into()),
        RepositoryVisibilityError::NotPublished(_) => (StatusCode::SERVICE_UNAVAILABLE, "repository visibility update was not published".into()),
        RepositoryVisibilityError::Unconfirmed(_) => (StatusCode::SERVICE_UNAVAILABLE, "repository visibility update could not be confirmed; re-read current visibility before retrying".into()),
    }
}
async fn value(state: AppState, raw: String, request: Request) -> Result<Changed, ApiError> {
    let repo_id = id(&raw)?;
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
            "repository visibility request too large".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let request: ChangeRequest = serde_json::from_slice(&body).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "invalid repository visibility request".into(),
            )
        })?;
        let repository = state
            .read_repository(repo_id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
        if repository.access.registration.is_none() {
            return Err((
                StatusCode::FORBIDDEN,
                "repository registration is required for visibility changes".into(),
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
        let row = state
            .auth_store
            .change_registered_repository_visibility(
                bearer_token(&parts.headers)?,
                origin.user_id,
                repository.access,
                request.expected_is_private,
                request.is_private,
            )
            .map_err(change_error)?;
        Ok(Changed {
            id: row.id,
            is_private: row.is_private,
        })
    })
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "repository data is unavailable".into(),
        )
    })?
}
