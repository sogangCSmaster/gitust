//! Agent worktree materialization, sandbox execution, fast-forward approval/merge,
//! and GitHub sync handlers.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use gitust_core::{
    AgentKind, AgentWorktree, AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoId,
    WorktreeId, WorktreeStatus,
};
use gitust_git::{GitEngine, RepoHeadSnapshot};
use gitust_sandbox::{SandboxExecutionResult, SandboxSpec};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{
    check_repo_bola, filter_authorized_worktrees, find_authorized_worktree, internal_err,
    map_auth_err, map_git_err, map_sandbox_err, set_worktree_status, AppState, RunStatusGuard,
    SpawnLifecycle, SpawnReservation, WorktreeRuntime, MAX_PROMPT_SUMMARY_BYTES,
    MAX_SANDBOX_COMMAND_BYTES,
};

#[derive(Serialize, Deserialize)]
pub struct SpawnWorktreeRequest {
    pub repo_id: Option<RepoId>,
    pub agent: Option<AgentKind>,
    /// Must be in the `agent/` namespace.
    pub branch: String,
    /// Branch to fork from (default: `main`).
    #[serde(default)]
    pub base_branch: Option<String>,
    pub prompt_summary: String,
}

#[derive(Serialize, Deserialize)]
pub struct SandboxExecRequest {
    pub command: String,
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

pub(crate) async fn spawn_worktree_core(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    branch: &str,
    base_branch: String,
    agent: AgentKind,
    prompt_summary: String,
    target_repo_id: RepoId,
) -> Result<(AgentWorktree, PathBuf), (StatusCode, String)> {
    if prompt_summary.len() > MAX_PROMPT_SUMMARY_BYTES {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("prompt_summary exceeds {MAX_PROMPT_SUMMARY_BYTES} bytes"),
        ));
    }
    let clean_branch = GitEngine::validate_agent_branch_name(branch).map_err(map_git_err)?;
    let clean_base = GitEngine::validate_branch_name(&base_branch).map_err(map_git_err)?;
    let store = state.auth_store.clone();
    let default_repo = state.repo_id;
    let caller_owned = caller.clone();
    tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, default_repo, &caller_owned, target_repo_id)
    })
    .await
    .map_err(internal_err)??;
    if target_repo_id != state.repo_id {
        return Err((
            StatusCode::NOT_FOUND,
            "repository is not served by this instance".to_owned(),
        ));
    }
    let reservation = SpawnReservation::reserve(state, caller.user_id)?;
    let lifecycle = Arc::clone(&reservation.lifecycle);
    let state_for_spawn = state.clone();
    let user_id = caller.user_id;

    let engine = state.git_engine.clone();
    let materialized = tokio::task::spawn_blocking(move || {
        let res = engine.materialize_agent_worktree(
            WorktreeId::new(),
            target_repo_id,
            agent,
            &clean_branch,
            &clean_base,
            prompt_summary,
        );
        let mut life = lifecycle.lock();
        match (&*life, &res) {
            (SpawnLifecycle::Cancelled, Ok(m)) => {
                drop(life);
                let _ = engine.remove_agent_worktree(
                    &m.worktree_dir,
                    &m.worktree.branch,
                    &m.worktree.base_commit,
                );
                state_for_spawn.release_in_flight_quota(user_id);
            }
            (SpawnLifecycle::Cancelled, Err(_)) => {
                drop(life);
                state_for_spawn.release_in_flight_quota(user_id);
            }
            (_, Ok(m)) => {
                *life = SpawnLifecycle::Materialized {
                    engine: engine.clone(),
                    worktree_dir: m.worktree_dir.clone(),
                    branch: m.worktree.branch.clone(),
                    base_commit: m.worktree.base_commit.clone(),
                };
            }
            (_, Err(_)) => {
                *life = SpawnLifecycle::Failed;
            }
        }
        res
    })
    .await
    .map_err(internal_err)?
    .map_err(map_git_err)?;

    let worktree_dir = materialized.worktree_dir;
    let runtime = WorktreeRuntime {
        created_by: Some(caller.user_id),
        sandbox_spec: SandboxSpec::for_linked_worktree(
            &worktree_dir,
            &materialized.worktree_admin_dir,
            &materialized.common_git_dir,
            materialized.main_workdir.as_deref(),
            &materialized.worktrees_base,
            &materialized.sandbox_scratch_base,
        ),
        worktree_dir: worktree_dir.clone(),
        exec_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    let wt = materialized.worktree;
    reservation.commit(wt.clone(), runtime);
    Ok((wt, worktree_dir))
}

pub(crate) async fn spawn_worktree_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: axum::http::HeaderMap,
    Json(req): Json<SpawnWorktreeRequest>,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    let caller = crate::auth::extract_principal_async(&headers, &state.auth_store).await?;
    let target_repo_id = req.repo_id.unwrap_or(state.repo_id);
    let agent = req.agent.unwrap_or(AgentKind::ClaudeCode);
    let base_branch = req.base_branch.unwrap_or_else(|| "main".to_owned());
    let (wt, _worktree_dir) = spawn_worktree_core(
        &state,
        &caller,
        &req.branch,
        base_branch,
        agent,
        req.prompt_summary,
        target_repo_id,
    )
    .await?;
    Ok(Json(wt))
}

pub(crate) async fn exec_worktree_sandbox_core(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
    command: String,
    timeout_secs: Option<u64>,
) -> Result<(SandboxExecutionResult, WorktreeStatus), (StatusCode, String)> {
    if !state.trusted_sandbox_exec {
        return Err((
            StatusCode::FORBIDDEN,
            "server-side sandbox execution is disabled: the host OS sandbox is not a multi-tenant \
             boundary. Start `gitust serve --trusted-single-user-sandbox` only on a single-user \
             deployment."
                .to_owned(),
        ));
    }
    if caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    if command.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "sandbox command must not be empty".to_owned(),
        ));
    }
    if command.len() > MAX_SANDBOX_COMMAND_BYTES {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("sandbox command exceeds {MAX_SANDBOX_COMMAND_BYTES} bytes"),
        ));
    }
    if let Some(t) = timeout_secs {
        if !(1..=600).contains(&t) {
            return Err((
                StatusCode::BAD_REQUEST,
                "timeout_secs must be between 1 and 600 seconds".to_owned(),
            ));
        }
    }

    let wt = find_authorized_worktree(state, caller, id).await?;
    if matches!(
        wt.status,
        WorktreeStatus::Merged | WorktreeStatus::Discarded
    ) {
        return Err((StatusCode::CONFLICT, format!("worktree is {:?}", wt.status)));
    }
    let runtime = state.get_worktree_runtime(wt.id).ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            "worktree has no checkout on this server".to_owned(),
        )
    })?;
    let _run_guard = runtime.exec_lock.clone().try_lock_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "a sandbox run is already in progress for this worktree".to_owned(),
        )
    })?;

    if state.get_worktree_runtime(wt.id).is_none() {
        return Err((StatusCode::NOT_FOUND, format!("worktree {id} not found")));
    }

    let engine = state.git_engine.clone();
    tokio::task::spawn_blocking(move || engine.ensure_config_has_no_credentials())
        .await
        .map_err(internal_err)?
        .map_err(map_git_err)?;

    state.try_begin_worktree_compiling(wt.id)?;

    let mut status_guard = RunStatusGuard {
        state: state.clone(),
        id: wt.id,
        armed: true,
    };
    let executor = match timeout_secs {
        Some(secs) => state.sandbox_executor.clone().with_timeout_secs(secs),
        None => state.sandbox_executor.clone(),
    };
    let outcome = executor
        .execute(wt.id, &runtime.sandbox_spec, &command)
        .await;
    status_guard.armed = false;

    match outcome {
        Ok(res) => {
            let final_status = if res.exit_code == 0 {
                WorktreeStatus::Verified
            } else {
                WorktreeStatus::Failed
            };
            set_worktree_status(state, wt.id, |w| {
                w.last_sandbox_duration_ms = res.duration_ms;
                w.status = final_status;
            });
            Ok((res, final_status))
        }
        Err(e) => {
            set_worktree_status(state, wt.id, |w| w.status = WorktreeStatus::Failed);
            Err(map_sandbox_err(e))
        }
    }
}

pub(crate) async fn exec_worktree_sandbox_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: axum::http::HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<SandboxExecRequest>,
) -> Result<Json<SandboxExecutionResult>, (StatusCode, String)> {
    let caller = crate::auth::extract_principal_async(&headers, &state.auth_store).await?;
    let (res, _status) = exec_worktree_sandbox_core(&state, &caller, id, req.command, None).await?;
    Ok(Json(res))
}

pub(crate) async fn discard_worktree_core(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
) -> Result<StatusCode, (StatusCode, String)> {
    let wt = find_authorized_worktree(state, caller, id).await?;
    let runtime = state.get_worktree_runtime(wt.id);
    let is_privileged = matches!(
        caller.role,
        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
    );
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
        let engine = state.git_engine.clone();
        let state_for_cleanup = state.clone();
        let wt_id = wt.id;
        let branch = wt.branch.clone();
        let base_commit = wt.base_commit.clone();
        let dir = runtime.worktree_dir.clone();
        tokio::task::spawn_blocking(move || {
            let _held_guard = run_guard;
            let res = engine.remove_agent_worktree(&dir, &branch, &base_commit);
            if res.is_ok() {
                state_for_cleanup.remove_worktree_entry(wt_id);
            }
            res
        })
        .await
        .map_err(internal_err)?
        .map_err(map_git_err)?;
        return Ok(StatusCode::NO_CONTENT);
    }

    state.remove_worktree_entry(wt.id);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn discard_worktree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    discard_worktree_core(&state, &caller, id).await
}

pub(crate) async fn approve_worktree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    if !matches!(
        caller.role,
        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
    ) {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }

    let wt_initial = find_authorized_worktree(&state, &caller, id).await?;
    let runtime = state.get_worktree_runtime(wt_initial.id).ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            "worktree has no checkout on this server".to_owned(),
        )
    })?;
    let run_guard = runtime.exec_lock.try_lock_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "a sandbox run is in progress for this worktree".to_owned(),
        )
    })?;

    if state.get_worktree_runtime(wt_initial.id).is_none() {
        return Err((StatusCode::NOT_FOUND, format!("worktree {id} not found")));
    }
    let wt = find_authorized_worktree(&state, &caller, id).await?;
    if wt.status != WorktreeStatus::Verified {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot approve worktree in {:?} state; sandbox verification required first",
                wt.status
            ),
        ));
    }

    let repo_guard = state
        .repo_mutation_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository merge or mirror sync is currently in progress".to_owned(),
            )
        })?;

    let engine = state.git_engine.clone();
    let state_for_merge = state.clone();
    let wt_id = wt.id;
    let branch = wt.branch.clone();
    let base_branch = wt.base_branch.clone();
    let merged_wt = tokio::task::spawn_blocking(move || {
        let _held_wt = run_guard;
        let _held_repo = repo_guard;
        let merged_sha = engine
            .merge_agent_branch(&branch, &base_branch)
            .map_err(map_git_err)?;
        state_for_merge.complete_worktree_merge(wt_id, merged_sha)
    })
    .await
    .map_err(internal_err)??;

    Ok(Json(merged_wt))
}
