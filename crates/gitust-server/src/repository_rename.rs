//! Display-name mutations target only configured, explicitly registered repository contexts.
use crate::{
    auth::{extract_principal, extract_principal_async},
    repository_reads::{id, response},
    state::{internal_err, map_auth_err, AppState},
};
use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::Response,
};
use gitust_core::{AuthError, EnterpriseRole, RepoId};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameRequest {
    name: String,
}
#[derive(Serialize)]
struct RenamedRepository {
    id: RepoId,
    name: String,
}
type ApiError = (StatusCode, String);

pub(crate) async fn rename(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    response(rename_value(state, raw, request).await)
}
async fn rename_value(
    state: AppState,
    raw: String,
    request: Request,
) -> Result<RenamedRepository, ApiError> {
    let repository_id = id(&raw)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 4096).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "repository rename request too large".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let current = extract_principal(&parts.headers, &state.auth_store)?;
        if current.user_id != origin.user_id || current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let request: RenameRequest = serde_json::from_slice(&body).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "invalid repository rename request".into(),
            )
        })?;
        let repository = state
            .read_repository(repository_id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
        if repository.access.registration.is_none() {
            return Err((
                StatusCode::FORBIDDEN,
                "repository registration is required for rename".into(),
            ));
        }
        let _ownership = repository.engine.lock_worktree_service().map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "repository data is unavailable".into(),
            )
        })?;
        let record = state
            .auth_store
            .rename_registered_repository(&current, repository.access, &request.name)
            .map_err(map_auth_err)?;
        Ok(RenamedRepository {
            id: record.id,
            name: record.name,
        })
    })
    .await
    .map_err(internal_err)?
}
