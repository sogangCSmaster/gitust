//! Agent worktree materialization, sandbox execution, fast-forward approval/merge,
//! and GitHub sync handlers.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use gitust_core::{
    AgentWorktree, AuthError, AuthenticatedPrincipal, EnterpriseRole, WorktreeStatus,
};
use gitust_git::{GitEngineError, RepoHeadSnapshot};
use gitust_sandbox::SandboxExecutionResult;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{
    check_repo_bola, filter_authorized_worktrees, find_authorized_worktree, internal_err,
    map_auth_err, map_git_err, map_sandbox_err, set_worktree_status, AppState, RunStatusGuard,
    MAX_SANDBOX_COMMAND_BYTES,
};

#[path = "worktree_spawn.rs"]
mod spawn;
pub use spawn::SpawnWorktreeRequest;
pub(crate) use spawn::{spawn_worktree_core, spawn_worktree_handler};

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

pub(crate) async fn exec_worktree_sandbox_core(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
    command: String,
    timeout_secs: Option<u64>,
    verify: bool,
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
    let run_guard = runtime.exec_lock.clone().try_lock_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "a sandbox run is already in progress for this worktree".to_owned(),
        )
    })?;

    if state.get_worktree_runtime(wt.id).is_none() {
        return Err((StatusCode::NOT_FOUND, format!("worktree {id} not found")));
    }

    state.try_begin_worktree_compiling(wt.id)?;

    let mut status_guard = RunStatusGuard {
        state: state.clone(),
        id: wt.id,
        armed: true,
    };
    let engine = state.git_engine.clone();
    let branch = wt.branch.clone();
    let base = wt.base_branch.clone();
    let (checked_commit, checks) = tokio::task::spawn_blocking(move || {
        if verify {
            let commit = engine.clean_worktree_commit(wt.id, &branch)?;
            Ok((Some(commit), engine.required_checks(&base)?))
        } else {
            engine
                .ensure_config_has_no_credentials()
                .map(|_| (None, None))
        }
    })
    .await
    .map_err(internal_err)?
    .map_err(map_git_err)?;
    let executor = match timeout_secs {
        Some(secs) => state.sandbox_executor.clone().with_timeout_secs(secs),
        None => state.sandbox_executor.clone(),
    };
    let outcome = executor
        .execute_with_checks(
            wt.id,
            &runtime.sandbox_spec,
            &command,
            checks
                .as_ref()
                .map_or(&[], |checks| checks.commands.as_slice()),
        )
        .await;
    let checks_oid = checks.map(|checks| checks.blob_oid);
    match outcome {
        Ok(res) => {
            if let Some(ref checked) = checked_commit {
                let engine = state.git_engine.clone();
                let branch = wt.branch.clone();
                let base = wt.base_branch.clone();
                let expected_policy = checks_oid.clone();
                let after = tokio::task::spawn_blocking(move || {
                    if engine.required_checks(&base)?.map(|c| c.blob_oid) != expected_policy {
                        return Err(GitEngineError::VerificationInvalid(
                            "required Checks changed during verification; run verification again"
                                .into(),
                        ));
                    }
                    engine.clean_worktree_commit(wt.id, &branch)
                })
                .await
                .map_err(internal_err)?
                .map_err(map_git_err)?;
                if &after != checked {
                    return Err((
                        StatusCode::CONFLICT,
                        "checkout changed during verification; run verification again".into(),
                    ));
                }
            }
            let common = runtime.common_git_dir.clone();
            let branch = wt.branch.clone();
            let user_id = caller.user_id;
            let historical_commit = checked_commit.clone();
            let historical_checks = checks_oid.clone();
            // Keep removal excluded even if this request is dropped while persistence continues.
            let (res, _run_guard) = tokio::task::spawn_blocking(move || {
                let guard = run_guard;
                let record = crate::run_history::RunRecord::new(
                    user_id,
                    verify,
                    historical_commit,
                    historical_checks,
                    res.clone(),
                );
                crate::run_history::append(&common, wt.id, &branch, record)?;
                Ok::<_, (StatusCode, String)>((res, guard))
            })
            .await
            .map_err(internal_err)??;
            let store = state.auth_store.clone();
            let active_caller = caller.clone();
            let repo_id = wt.repo_id;
            let default_repo = state.repo_id;
            tokio::task::spawn_blocking(move || {
                check_repo_bola(&store, default_repo, &active_caller, repo_id)
            })
            .await
            .map_err(internal_err)??;
            let final_status = if res.exit_code != 0 {
                WorktreeStatus::Failed
            } else if verify {
                WorktreeStatus::Verified
            } else {
                WorktreeStatus::Pending
            };
            set_worktree_status(state, wt.id, |w| {
                w.last_sandbox_duration_ms = res.duration_ms;
                w.status = final_status;
                w.verified_commit = checked_commit;
                w.verified_checks_oid = checks_oid;
            });
            status_guard.armed = false;
            Ok((res, final_status))
        }
        Err(e) => {
            set_worktree_status(state, wt.id, |w| w.status = WorktreeStatus::Failed);
            status_guard.armed = false;
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
    let (res, _status) =
        exec_worktree_sandbox_core(&state, &caller, id, req.command, None, req.verify).await?;
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
    if wt.status == WorktreeStatus::Merged && !is_privileged {
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
        if current.status == WorktreeStatus::Merged && !is_privileged {
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
    if wt.status != WorktreeStatus::Verified || wt.verified_commit.is_none() {
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
    let verified_checks_oid = wt.verified_checks_oid.clone();
    let verified_commit = wt.verified_commit.ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            "verification commit is missing; run verification again".into(),
        )
    })?;
    let merged_wt = tokio::task::spawn_blocking(move || {
        let _held_wt = run_guard;
        let _held_repo = repo_guard;
        let policy_matches = engine
            .required_checks(&base_branch)
            .map(|checks| checks.map(|c| c.blob_oid) == verified_checks_oid);
        if !matches!(policy_matches, Ok(true)) {
            set_worktree_status(&state_for_merge, wt_id, |w| {
                w.status = WorktreeStatus::Pending
            });
            policy_matches.map_err(map_git_err)?;
            return Err((
                StatusCode::CONFLICT,
                "required Checks changed since verification; run verification again".into(),
            ));
        }
        let merged_sha = match engine.approve_verified_worktree(
            wt_id,
            &branch,
            &base_branch,
            &verified_commit,
        ) {
            Ok(sha) => sha,
            Err(err) => {
                if matches!(err, GitEngineError::VerificationInvalid(_)) {
                    set_worktree_status(&state_for_merge, wt_id, |w| {
                        w.status = WorktreeStatus::Pending
                    });
                }
                return Err(map_git_err(err));
            }
        };
        state_for_merge.complete_worktree_merge(wt_id, merged_sha)
    })
    .await
    .map_err(internal_err)??;

    Ok(Json(merged_wt))
}
