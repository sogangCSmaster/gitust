//! Read-only primary repository diagnostics recorded by the explicit local CLI operator.
use crate::{
    auth::extract_principal,
    local_checks_history::{LocalChecksHistoryStore, LocalChecksRecord, MAX_BYTES},
    repository_reads::{id, response},
    state::{check_repo_bola, internal_err, map_auth_err, AppState},
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use gitust_core::{AuthError, EnterpriseRole, RepoId};
use serde::Serialize;

type ApiError = (StatusCode, String);
#[derive(Serialize)]
struct Page {
    version: u32,
    repo_id: RepoId,
    records: Vec<LocalChecksRecord>,
}
fn unavailable(_: impl std::fmt::Display) -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "repository data is unavailable".into(),
    )
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    headers: HeaderMap,
) -> Response {
    match value(state, raw, headers).await {
        Ok(payload) => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            payload,
        )
            .into_response(),
        Err(error) => response::<()>(Err(error)),
    }
}
async fn value(state: AppState, raw: String, headers: HeaderMap) -> Result<Vec<u8>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let origin = extract_principal(&headers, &state.auth_store)?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let repo_id = id(&raw)?;
        let Some(repo) = state.writable_repository(repo_id) else {
            return Err((
                StatusCode::NOT_FOUND,
                "repository is not hosted for local Checks history".into(),
            ));
        };
        let expected_id = if repo.is_default {
            state.repo_id
        } else {
            repo_id
        };
        check_repo_bola(
            &state.auth_store,
            expected_id,
            repo.access,
            &origin,
            repo_id,
        )?;
        let _ownership = repo.engine.lock_worktree_service().map_err(unavailable)?;
        let confirm_identity = || {
            let actual = repo
                .engine
                .load_existing_repository_id()
                .map_err(unavailable)?;
            if actual != repo_id {
                return Err(unavailable("repository identity mismatch"));
            }
            Ok(())
        };
        confirm_identity()?;
        let common = repo
            .engine
            .repository_common_directory()
            .map_err(unavailable)?;
        let records = LocalChecksHistoryStore::open(&common, repo_id)
            .and_then(|store| store.list())
            .map_err(unavailable)?;
        // Match the storage envelope and enforce its byte ceiling before returning any output.
        let payload = serde_json::to_vec(&Page {
            version: 1,
            repo_id,
            records,
        })
        .map_err(unavailable)?;
        if payload.len() as u64 > MAX_BYTES {
            return Err(unavailable("local Checks response exceeds limit"));
        }
        confirm_identity()?;
        // Recheck the original credential after filesystem reads and JSON serialization.
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != origin.user_id || current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        check_repo_bola(
            &state.auth_store,
            expected_id,
            repo.access,
            &current,
            repo_id,
        )?;
        Ok(payload)
    })
    .await
    .map_err(internal_err)?
}
