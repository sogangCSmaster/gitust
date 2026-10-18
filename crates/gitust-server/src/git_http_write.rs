//! Explicitly configured ordinary Git hosting; secondary workbench state is never consulted.
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
use gitust_core::{EnterpriseRole, RepoId, UserId};
use gitust_git::{
    inspect_receive_pack_updates, GitEngineError, SmartHttpService, WorktreeServiceLock,
    MAX_SMART_HTTP_BODY_BYTES, SMART_HTTP_TIMEOUT,
};
use std::sync::Arc;
type Failure = (StatusCode, String);
struct Target {
    repository: ReadRepository,
    user: UserId,
    role: EnterpriseRole,
    _inventory: Option<Arc<RepositoryRegistry>>,
    _ownership: WorktreeServiceLock,
}
fn missing() -> Failure {
    (
        StatusCode::NOT_FOUND,
        "repository is not enabled for ordinary Git pushes".into(),
    )
}
fn unavailable(_: impl std::fmt::Display) -> Failure {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "Git repository is unavailable".into(),
    )
}
pub(crate) fn secondary_id(state: &AppState, raw: &str) -> Option<RepoId> {
    let raw = raw.trim();
    // fail-open-ok: spelling only; prepare resolves the actual configured service and current ACL.
    let raw = raw.strip_suffix(".git").unwrap_or(raw);
    match uuid::Uuid::parse_str(raw) {
        Ok(id) if RepoId(id) != state.repo_id => Some(RepoId(id)),
        _ => None,
    }
}
fn current(
    state: &AppState,
    headers: &HeaderMap,
    target: &Target,
) -> Result<gitust_core::AuthenticatedPrincipal, Failure> {
    let caller = extract_git_http_principal(headers, &state.auth_store)?;
    if caller.user_id != target.user || caller.role != target.role {
        return Err((StatusCode::FORBIDDEN, "Git push authority changed".into()));
    }
    let authorized = state
        .auth_store
        .authorized_repository_catalog(&caller, &[target.repository.access])
        .map_err(map_auth_err)?;
    if !authorized.iter().any(|row| row.can_push_ordinary) {
        return Err((
            StatusCode::FORBIDDEN,
            "ordinary Git push requires current repository access and Maintainer or SiteAdmin role"
                .into(),
        ));
    }
    Ok(caller)
}
async fn prepare(state: AppState, raw: String, headers: HeaderMap) -> Result<Target, Failure> {
    tokio::task::spawn_blocking(move || {
        let caller = extract_git_http_principal(&headers, &state.auth_store)?;
        let id = secondary_id(&state, &raw).ok_or_else(missing)?;
        let repository = state
            .read_repository(id)
            .filter(|r| r.git_write && !r.is_default)
            .ok_or_else(missing)?;
        let ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(unavailable)?;
        let target = Target {
            repository,
            user: caller.user_id,
            role: caller.role,
            _inventory: state.repository_registry.clone(),
            _ownership: ownership,
        };
        current(&state, &headers, &target)?;
        Ok(target)
    })
    .await
    .map_err(unavailable)?
}
enum Operation {
    Advertisement,
    Receive(axum::body::Bytes),
}
async fn execute(
    state: AppState,
    headers: HeaderMap,
    target: Target,
    operation: Operation,
) -> Result<Vec<u8>, Failure> {
    let advertisement = matches!(&operation, Operation::Advertisement);
    if !advertisement {
        #[cfg(test)]
        if let Some(hook) = state.git_write_before_lock_hook.lock().clone() {
            hook();
        }
    }
    let guard = tokio::time::timeout(
        SMART_HTTP_TIMEOUT,
        target.repository.mutation.clone().lock_owned(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::CONFLICT,
            "another Git push is in progress".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let target = target;
        let _guard = guard;
        current(&state, &headers, &target)?;
        let updates = if let Operation::Receive(body) = &operation {
            let updates = inspect_receive_pack_updates(body).map_err(map_git_err)?;
            if updates.iter().any(|update| update.is_agent_branch) {
                return Err((
                    StatusCode::FORBIDDEN,
                    "secondary Git hosting does not accept managed agent refs".into(),
                ));
            }
            updates
        } else {
            Vec::new()
        };
        target
            .repository
            .engine
            .validate_ordinary_git_service()
            .map_err(unavailable)?;
        if !advertisement {
            #[cfg(test)]
            crate::repository_reads::after_read(&state);
        }
        // Validation and the async mutation-lock wait cannot authorize a revoked credential.
        current(&state, &headers, &target)?;
        let result = match operation {
            Operation::Advertisement => target
                .repository
                .engine
                .smart_http_advertise_refs(SmartHttpService::ReceivePack),
            Operation::Receive(body) => {
                let mut local_intent = None;
                let mut authority_error = None;
                let result =
                    target
                        .repository
                        .engine
                        .smart_http_receive_pack_with_gate(&body, || {
                            let step = (|| -> Result<(), Failure> {
                                let caller = current(&state, &headers, &target)?;
                                match crate::local_checks_automatic::prepare_for_repo(
                                    &state,
                                    &target.repository,
                                    &caller,
                                    &updates,
                                )
                                .map_err(|_| unavailable("local Checks unavailable"))?
                                {
                                    crate::local_checks_automatic::Eligibility::Prepared(
                                        intent,
                                    ) => {
                                        local_intent = Some(intent);
                                    }
                                    crate::local_checks_automatic::Eligibility::ConsentUnproven(
                                        reason,
                                    ) => {
                                        tracing::warn!(
                                            ?reason,
                                            "automatic local Checks consent could not be established; no execution scheduled"
                                        );
                                    }
                                    crate::local_checks_automatic::Eligibility::Ineligible => {}
                                }
                                current(&state, &headers, &target).map(|_| ())
                            })();
                            step.map_err(|error| {
                                authority_error = Some(error);
                                GitEngineError::InvalidRevision("Git push authority changed".into())
                            })
                        });
                if let Some(error) = authority_error {
                    if let Some(intent) = local_intent {
                        if crate::local_checks_automatic::reject_for_repo(
                            &target.repository,
                            intent,
                        )
                        .is_err()
                        {
                            tracing::warn!(
                                "automatic local Checks preparation recovery is required"
                            );
                        }
                    }
                    return Err(error);
                }
                if let Some(intent) = local_intent {
                    if crate::local_checks_automatic::confirm_for_repo(&target.repository, intent)
                        .is_err()
                    {
                        tracing::warn!(
                            "push completed but automatic local Checks scheduling is unavailable"
                        );
                    }
                }
                result
            }
        };
        if advertisement {
            #[cfg(test)]
            crate::repository_reads::after_read(&state);
            current(&state, &headers, &target)?;
        }
        // A failed/uncertain receive RPC may have applied refs; its Git result stays authoritative.
        result.map_err(map_git_err)
    })
    .await
    .map_err(unavailable)?
}
fn response(value: Result<Vec<u8>, Failure>, advertisement: bool) -> Response {
    let mut result = match value {
        Ok(bytes) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                if advertisement {
                    SmartHttpService::ReceivePack.advertisement_content_type()
                } else {
                    SmartHttpService::ReceivePack.result_content_type()
                },
            )],
            bytes,
        )
            .into_response(),
        Err((status, message)) => git_auth_err_response(
            status,
            if status.is_server_error() {
                "Git repository is unavailable".into()
            } else {
                message
            },
        ),
    };
    result.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    result
}
pub(crate) async fn advertise(state: AppState, raw: String, headers: HeaderMap) -> Response {
    let value = match prepare(state.clone(), raw, headers.clone()).await {
        Ok(target) => execute(state, headers, target, Operation::Advertisement).await,
        Err(error) => Err(error),
    };
    response(value, true)
}
pub(crate) async fn receive(state: AppState, raw: String, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let value = async {
        let target = prepare(state.clone(), raw, parts.headers.clone()).await?;
        let body = body::to_bytes(body, MAX_SMART_HTTP_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "receive-pack body exceeds its limit".into(),
                )
            })?;
        execute(state, parts.headers, target, Operation::Receive(body)).await
    }
    .await;
    response(value, false)
}
