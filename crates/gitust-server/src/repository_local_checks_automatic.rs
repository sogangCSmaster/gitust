//! Current administrators can inspect disabled scheduling without granting execution.
use crate::{
    auth::extract_principal,
    local_checks_automatic::identity,
    local_checks_automatic_model::{View, MAX_BYTES},
    repository_local_checks_result::Failure,
    state::{check_repo_bola, AppState},
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use gitust_core::EnterpriseRole;
use serde::Serialize;
fn unavailable() -> Failure {
    Failure::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "local_checks_automatic_unavailable",
    )
}
fn auth_error((status, _): (StatusCode, String)) -> Failure {
    if status.is_server_error() {
        unavailable()
    } else if status == StatusCode::UNAUTHORIZED {
        Failure::new(status, "local_checks_automatic_unauthorized")
    } else {
        Failure::new(StatusCode::FORBIDDEN, "local_checks_automatic_forbidden")
    }
}
#[derive(Serialize)]
struct Page {
    version: u32,
    repo_id: gitust_core::RepoId,
    runs: Vec<View>,
}
pub(crate) async fn list(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    headers: HeaderMap,
) -> Response {
    let value = tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let origin = extract_principal(&headers, &state.auth_store).map_err(auth_error)?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(Failure::new(
                StatusCode::FORBIDDEN,
                "local_checks_automatic_forbidden",
            ));
        }
        let id = crate::repository_reads::id(&raw).map_err(|_| {
            Failure::new(
                StatusCode::BAD_REQUEST,
                "invalid_local_checks_automatic_request",
            )
        })?;
        if id != state.repo_id {
            return Err(Failure::new(
                StatusCode::NOT_FOUND,
                "local_checks_automatic_not_hosted",
            ));
        }
        check_repo_bola(
            &state.auth_store,
            state.repo_id,
            state.repo_access,
            &origin,
            id,
        )
        .map_err(auth_error)?;
        let _owner = state
            .git_engine
            .lock_worktree_service()
            .map_err(|_| unavailable())?;
        identity(&state).map_err(|_| unavailable())?;
        if state
            .local_checks_automatic
            .unavailable
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(unavailable());
        }
        let doc = state
            .local_checks_automatic
            .store(&state)
            .and_then(|store| store.read())
            .map_err(|_| unavailable())?;
        let payload = serde_json::to_vec(&Page {
            version: 1,
            repo_id: id,
            runs: doc.public(),
        })
        .map_err(|_| unavailable())?;
        if payload.len() as u64 > MAX_BYTES {
            return Err(unavailable());
        }
        identity(&state).map_err(|_| unavailable())?;
        let current = extract_principal(&headers, &state.auth_store).map_err(auth_error)?;
        if current.user_id != origin.user_id || current.role != EnterpriseRole::SiteAdmin {
            return Err(Failure::new(
                StatusCode::FORBIDDEN,
                "local_checks_automatic_forbidden",
            ));
        }
        check_repo_bola(
            &state.auth_store,
            state.repo_id,
            state.repo_access,
            &current,
            id,
        )
        .map_err(auth_error)?;
        Ok(payload)
    })
    .await;
    match value {
        Ok(Ok(payload)) => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            payload,
        )
            .into_response(),
        Ok(Err(error)) => error.response(),
        Err(_) => unavailable().response(),
    }
}
