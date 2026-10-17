//! Agent worktree materialization, sandbox execution, fast-forward approval/merge,
//! and GitHub sync handlers.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use gitust_core::{
    AgentWorktree, AuthError, AuthenticatedPrincipal, EnterpriseRole, WorktreeStatus,
};
use gitust_git::RepoHeadSnapshot;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{
    check_repo_bola, filter_authorized_worktrees, find_authorized_worktree, internal_err,
    map_auth_err, map_git_err, AppState,
};

#[path = "worktree_spawn.rs"]
mod spawn;
pub use spawn::SpawnWorktreeRequest;
pub(crate) use spawn::{spawn_worktree_core, spawn_worktree_handler};

#[path = "worktree_approval.rs"]
mod approval;
pub(crate) use approval::approve_worktree_handler;

#[derive(Serialize, Deserialize)]
pub struct SandboxExecRequest {
    pub command: String,
    /// Explicit verification requires a clean, unchanged committed checkout.
    #[serde(default)]
    pub verify: bool,
}

pub(crate) async fn git_head_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<RepoHeadSnapshot>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo_id = state.repo_id;
    let engine = state.git_engine.clone();
    let head = tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, repo_id, &caller, repo_id)?;
        engine
            .inspect_head_zero_copy()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(head))
}

pub(crate) async fn list_worktrees_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<Json<Vec<AgentWorktree>>, (StatusCode, String)> {
    Ok(Json(filter_authorized_worktrees(&state, &caller).await?))
}

#[path = "worktree_execution.rs"]
pub(crate) mod execution;
pub(crate) use execution::exec_worktree_sandbox_core;

pub(crate) async fn exec_worktree_sandbox_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: axum::http::HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<SandboxExecRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let caller = crate::auth::extract_principal_async(&headers, &state.auth_store).await?;
    let (res, _status, checked_commit) =
        exec_worktree_sandbox_core(&state, &caller, id, req.command, None, req.verify).await?;
    let mut response = serde_json::to_value(res).map_err(internal_err)?;
    response["checked_commit"] = serde_json::json!(checked_commit);
    Ok(Json(response))
}

#[derive(Clone, Copy)]
pub(crate) enum DiscardAuthority {
    Operator,
    Agent,
}

pub(crate) async fn discard_worktree_core(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
    authority: DiscardAuthority,
) -> Result<StatusCode, (StatusCode, String)> {
    let wt = find_authorized_worktree(state, caller, id).await?;
    let runtime = state.get_worktree_runtime(wt.id);
    let is_privileged = matches!(
        caller.role,
        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
    );
    let may_remove_approval = is_privileged && matches!(authority, DiscardAuthority::Operator);
    if wt.status == WorktreeStatus::Merged && !may_remove_approval {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let is_owner = runtime
        .as_ref()
        .is_some_and(|r| r.created_by == Some(caller.user_id));
    if !is_privileged && !is_owner {
        return Err(map_auth_err(AuthError::ForbiddenResourceOwnership));
    }

    if let Some(runtime) = runtime {
        let run_guard = runtime.exec_lock.clone().try_lock_owned().map_err(|_| {
            (
                StatusCode::CONFLICT,
                "a sandbox run is in progress for this worktree".to_owned(),
            )
        })?;
        let current = find_authorized_worktree(state, caller, id).await?;
        if current.status == WorktreeStatus::Merged && !may_remove_approval {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let engine = state.git_engine.clone();
        let state_for_cleanup = state.clone();
        let wt_id = wt.id;
        let branch = wt.branch.clone();
        let base_commit = wt.base_commit.clone();
        let common = runtime.common_git_dir.clone();
        let dir = runtime.worktree_dir.clone();
        tokio::task::spawn_blocking(move || {
            let _held_guard = run_guard;
            let _service = engine.lock_worktree_service().map_err(map_git_err)?;
            // Remove diagnostics while the checkout is still recoverable if cleanup fails.
            crate::run_history::remove(&common, wt_id)?;
            engine
                .remove_agent_worktree(&dir, &branch, &base_commit)
                .map_err(map_git_err)?;
            state_for_cleanup.remove_worktree_entry(wt_id);
            Ok::<_, (StatusCode, String)>(())
        })
        .await
        .map_err(internal_err)??;
        return Ok(StatusCode::NO_CONTENT);
    }

    let engine = state.git_engine.clone();
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _service = engine.lock_worktree_service().map_err(map_git_err)?;
        state.remove_worktree_entry(wt.id);
        Ok::<_, (StatusCode, String)>(())
    })
    .await
    .map_err(internal_err)??;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn discard_worktree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    discard_worktree_core(&state, &caller, id, DiscardAuthority::Operator).await
}
