//! Default-branch management never expands the configured primary's write scope.
use crate::{
    auth::{extract_principal, extract_principal_async},
    repository_reads::{id, response},
    state::{internal_err, map_auth_err, map_git_err, AppState},
};
use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::Response,
};
use gitust_core::{AuthError, CommitSha, EnterpriseRole, RepoId};
use gitust_git::GitEngineError;
use serde::{Deserialize, Serialize};
type ApiError = (StatusCode, String);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeRequest {
    target_ref: String,
    expected_current_ref: String,
    expected_target_commit: CommitSha,
}
#[derive(Serialize)]
struct Changed {
    id: RepoId,
    branch: String,
    head_commit: CommitSha,
}
pub(crate) async fn change(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    response(value(state, raw, request).await)
}
async fn value(state: AppState, raw: String, request: Request) -> Result<Changed, ApiError> {
    let repo_id = id(&raw)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 256 * 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "default branch request too large".into(),
        )
    })?;
    let guard = state
        .repo_mutation_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository mutation is in progress".into(),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _inventory = state.repository_registry.clone();
        let authorize = || {
            let current = extract_principal(&parts.headers, &state.auth_store)?;
            if current.user_id != origin.user_id || current.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let rows = state
                .auth_store
                .authorized_repository_catalog(&current, &[state.repo_access])
                .map_err(map_auth_err)?;
            if rows.len() != 1 || !rows[0].can_rename {
                return Err(map_auth_err(AuthError::ForbiddenRepositoryAccess));
            }
            Ok(())
        };
        authorize()?;
        let repository = state
            .read_repository(repo_id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
        if !repository.is_default || repository.access.registration.is_none() {
            return Err((
                StatusCode::FORBIDDEN,
                "default branch changes require the registered primary repository".into(),
            ));
        }
        let request: ChangeRequest = serde_json::from_slice(&body).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "invalid default branch request".into(),
            )
        })?;
        let _ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(map_git_err)?;
        let mut denied = None;
        let changed = repository.engine.change_default_branch_with_gate(
            &request.target_ref,
            &request.expected_current_ref,
            &request.expected_target_commit,
            || {
                #[cfg(test)]
                crate::repository_reads::after_read(&state);
                if let Err(error) = authorize() {
                    denied = Some(error);
                    return Err(GitEngineError::DefaultBranchUnsupported);
                }
                Ok(())
            },
        );
        if let Some(error) = denied {
            return Err(error);
        }
        let changed = changed.map_err(map_git_err)?;
        Ok(Changed {
            id: repo_id,
            branch: changed.branch,
            head_commit: changed.head_commit,
        })
    })
    .await
    .map_err(internal_err)?
}
