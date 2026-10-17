//! Authenticated hosted reads and primary advertisements, with original credential rechecks.
use crate::{
    git_http_auth::{extract_git_http_principal, git_auth_err_response},
    repository_registry::{ReadRepository, RepositoryRegistry},
    state::{map_auth_err, map_git_err, AppState},
};
use axum::{
    body,
    extract::Request,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use gitust_core::{AuthenticatedPrincipal, RepoId, UserId};
use gitust_git::{
    GitEngine, GitEngineError, SmartHttpService, WorktreeServiceLock, MAX_SMART_HTTP_BODY_BYTES,
};
use std::sync::Arc;

type ApiError = (StatusCode, String);
struct ReadTarget {
    repository: ReadRepository,
    original_user: UserId,
    _inventory: Option<Arc<RepositoryRegistry>>,
    _ownership: WorktreeServiceLock,
}
fn unavailable(_: impl std::fmt::Display) -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "repository data is unavailable".into(),
    )
}
fn forbidden() -> ApiError {
    (StatusCode::FORBIDDEN, "repository access denied".into())
}
fn target_id(state: &AppState, raw: &str) -> Result<RepoId, ApiError> {
    let trimmed = raw.trim();
    // fail-open-ok: suffix removal only selects a spelling; configured inventory and ACL are checked below.
    let clean = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    if clean.eq_ignore_ascii_case("default") || clean.eq_ignore_ascii_case("gitust") {
        return Ok(state.repo_id);
    }
    uuid::Uuid::parse_str(clean)
        .map(RepoId)
        .map_err(|_| (StatusCode::NOT_FOUND, "repository is not hosted".into()))
}
fn authorize(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    target: &ReadRepository,
) -> Result<(), ApiError> {
    let rows = state
        .auth_store
        .authorized_repository_catalog(caller, &[target.access])
        .map_err(map_auth_err)?;
    if rows.len() != 1 {
        return Err(forbidden());
    }
    Ok(())
}
async fn prepare(state: AppState, raw: String, headers: HeaderMap) -> Result<ReadTarget, ApiError> {
    tokio::task::spawn_blocking(move || {
        let caller = extract_git_http_principal(&headers, &state.auth_store)?;
        let id = target_id(&state, &raw)?;
        let repository = state
            .read_repository(id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
        authorize(&state, &caller, &repository)?;
        let ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(unavailable)?;
        Ok(ReadTarget {
            repository,
            original_user: caller.user_id,
            _inventory: state.repository_registry.clone(),
            _ownership: ownership,
        })
    })
    .await
    .map_err(unavailable)?
}
async fn execute(
    state: AppState,
    headers: HeaderMap,
    target: ReadTarget,
    operation: impl FnOnce(&GitEngine) -> Result<Vec<u8>, GitEngineError> + Send + 'static,
) -> Result<Vec<u8>, ApiError> {
    tokio::task::spawn_blocking(move || {
        // Move the complete holder into the blocking task, including fields not read by Git.
        let target = target;
        let current = || {
            let caller = extract_git_http_principal(&headers, &state.auth_store)?;
            if caller.user_id != target.original_user {
                return Err(forbidden());
            }
            authorize(&state, &caller, &target.repository)
        };
        // The original credential and current binding/ACL are checked after body/ownership waits.
        current()?;
        let value = operation(&target.repository.engine);
        #[cfg(test)]
        crate::repository_reads::after_read(&state);
        current()?;
        value.map_err(map_git_err)
    })
    .await
    .map_err(unavailable)?
}
fn response(
    value: Result<Vec<u8>, ApiError>,
    service: SmartHttpService,
    advertisement: bool,
) -> Response {
    let mut response = match value {
        Ok(bytes) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                if advertisement {
                    service.advertisement_content_type()
                } else {
                    service.result_content_type()
                },
            )],
            bytes,
        )
            .into_response(),
        Err((status, message)) => {
            let message = if status.is_server_error() {
                "repository data is unavailable".into()
            } else {
                message
            };
            git_auth_err_response(status, message)
        }
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
pub(crate) async fn advertise(state: AppState, raw: String, headers: HeaderMap) -> Response {
    advertise_service(state, raw, headers, SmartHttpService::UploadPack).await
}
pub(crate) async fn advertise_service(
    state: AppState,
    raw: String,
    headers: HeaderMap,
    service: SmartHttpService,
) -> Response {
    let value = match prepare(state.clone(), raw, headers.clone()).await {
        Ok(target) => {
            execute(state, headers, target, move |engine| {
                engine.smart_http_advertise_refs(service)
            })
            .await
        }
        Err(error) => Err(error),
    };
    response(value, service, true)
}
pub(crate) async fn upload(state: AppState, raw: String, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let target = match prepare(state.clone(), raw, parts.headers.clone()).await {
        Ok(target) => target,
        Err(error) => return response(Err(error), SmartHttpService::UploadPack, false),
    };
    let body = match body::to_bytes(body, MAX_SMART_HTTP_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return response(
                Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "upload-pack body exceeds its limit".into(),
                )),
                SmartHttpService::UploadPack,
                false,
            )
        }
    };
    response(
        execute(state, parts.headers, target, move |engine| {
            engine.smart_http_upload_pack(&body)
        })
        .await,
        SmartHttpService::UploadPack,
        false,
    )
}
