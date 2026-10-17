//! Explicit publication of a human-approved worktree's immutable merge commit.

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::Json;
use gitust_core::{AgentWorktree, AuthError, EnterpriseRole, WorktreeId, WorktreeStatus};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{extract_principal, extract_principal_async};
use crate::state::{check_repo_bola, internal_err, map_auth_err, map_git_err, AppState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishRequest {}

pub(crate) async fn publish_worktree_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    request: Request,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    let (parts, body) = request.into_parts();
    extract_principal_async(&parts.headers, &state.auth_store).await?;
    let body = axum::body::to_bytes(body, 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "publish request too large".into(),
        )
    })?;
    if !body.iter().all(u8::is_ascii_whitespace) {
        serde_json::from_slice::<PublishRequest>(&body).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid publish request: {e}"),
            )
        })?;
    }
    let published = tokio::task::spawn_blocking(move || {
        // Authenticate after body streaming and inside the blocking job, immediately before
        // authorization. The client cannot supply a different SHA, branch, or destination.
        let caller = extract_principal(&parts.headers, &state.auth_store)?;
        if !matches!(
            caller.role,
            EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
        ) {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        check_repo_bola(
            &state.auth_store,
            state.repo_id,
            state.repo_access,
            &caller,
            state.repo_id,
        )?;
        let _publish_guard = state
            .outbound_sync_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| {
                (
                    StatusCode::CONFLICT,
                    "another publication is in progress".into(),
                )
            })?;
        let runtime = state
            .get_worktree_runtime(WorktreeId(id))
            .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".to_owned()))?;
        let _worktree_guard = runtime
            .exec_lock
            .try_lock_owned()
            .map_err(|_| (StatusCode::CONFLICT, "worktree is busy".into()))?;
        let wt = state
            .worktrees
            .read()
            .iter()
            .find(|w| w.id.0 == id)
            .cloned()
            .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".to_owned()))?;
        check_repo_bola(
            &state.auth_store,
            state.repo_id,
            state.repo_access,
            &caller,
            wt.repo_id,
        )?;
        if wt.status != WorktreeStatus::Merged
            || !state
                .sync_config
                .should_sync_branch_outbound(&wt.base_branch, true)
        {
            return Err((
                StatusCode::CONFLICT,
                "only human-approved merged worktrees can be published".into(),
            ));
        }
        let approval = state
            .git_engine
            .load_worktree_approval(wt.id)
            .map_err(map_git_err)?
            .ok_or_else(|| {
                (
                    StatusCode::CONFLICT,
                    "worktree has no durable approval".to_owned(),
                )
            })?;
        if approval.approved_commit != wt.base_commit
            || approval.base_branch != wt.base_branch
            || approval.branch != wt.branch
        {
            return Err((
                StatusCode::CONFLICT,
                "persisted approval differs from runtime state".into(),
            ));
        }
        let receipt = state
            .git_engine
            .publish_managed_worktree(wt.id, &state.sync_config.github_remote_url)
            .map_err(|err| match err {
                gitust_git::GitEngineError::GitCommand { .. } => {
                    (StatusCode::BAD_GATEWAY, err.to_string())
                }
                other => map_git_err(other),
            })?;
        // Complete state while retaining both guards, even if the HTTP request is cancelled.
        let mut worktrees = state.worktrees.write();
        let entry = worktrees
            .iter_mut()
            .find(|w| w.id == wt.id)
            .ok_or_else(|| {
                (
                    StatusCode::CONFLICT,
                    "worktree removed during publication".to_owned(),
                )
            })?;
        if receipt.approved_commit != entry.base_commit || receipt.base_branch != entry.base_branch
        {
            return Err((
                StatusCode::CONFLICT,
                "persisted approval differs from runtime state".into(),
            ));
        }
        entry.github_synced = true;
        Ok(Json(entry.clone()))
    })
    .await
    .map_err(internal_err)??;
    Ok(published)
}
