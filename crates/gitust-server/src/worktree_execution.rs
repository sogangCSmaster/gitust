//! Shared manual and automatic execution; only explicit verification grants approval authority.
use crate::state::{
    check_repo_bola, find_authorized_worktree, internal_err, map_auth_err, map_git_err,
    map_sandbox_err, set_worktree_status, AppState, RunStatusGuard, MAX_SANDBOX_COMMAND_BYTES,
};
use axum::http::StatusCode;
use gitust_core::{
    AuthError, AuthenticatedPrincipal, CommitSha, EnterpriseRole, SandboxRunId, WorktreeStatus,
};
use gitust_git::GitEngineError;
use gitust_sandbox::SandboxExecutionResult;
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

type ResultValue =
    Result<(SandboxExecutionResult, WorktreeStatus, Option<CommitSha>), (StatusCode, String)>;
enum Mode {
    Ordinary,
    Verification,
    Automatic {
        expected: CommitSha,
        checks_oid: String,
        run_id: SandboxRunId,
        local: bool,
    },
}
struct Execution {
    command: String,
    timeout_secs: Option<u64>,
    mode: Mode,
}

pub(crate) async fn exec_worktree_sandbox_core(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
    command: String,
    timeout_secs: Option<u64>,
    verify: bool,
) -> ResultValue {
    execute(
        state,
        caller,
        id,
        Execution {
            command,
            timeout_secs,
            mode: if verify {
                Mode::Verification
            } else {
                Mode::Ordinary
            },
        },
        None,
    )
    .await
}

pub(crate) async fn automatic(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
    job: crate::automatic_run_model::Job,
    guard: Arc<OwnedMutexGuard<()>>,
) -> ResultValue {
    execute(
        state,
        caller,
        id,
        Execution {
            command: "true".into(),
            timeout_secs: None,
            mode: Mode::Automatic {
                expected: job.expected,
                checks_oid: job.checks_oid,
                run_id: job.run_id,
                local: job.origin == crate::automatic_run_model::Origin::LocalOperator,
            },
        },
        Some(guard),
    )
    .await
}

async fn execute(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: Uuid,
    request: Execution,
    held: Option<Arc<OwnedMutexGuard<()>>>,
) -> ResultValue {
    let Execution {
        command,
        timeout_secs,
        mode,
    } = request;
    let verify = !matches!(mode, Mode::Ordinary);
    let automatic = matches!(mode, Mode::Automatic { .. });
    let strict_automatic = matches!(&mode, Mode::Automatic { local: false, .. });
    let logical_run_id = match &mode {
        Mode::Automatic { run_id, .. } => Some(*run_id),
        _ => None,
    };
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
    if verify && wt.base_branch.is_empty() {
        return Err((StatusCode::CONFLICT,
            "worktree creation target is unknown; restore trusted metadata or recreate the worktree before verification".into()));
    }
    let runtime = state.get_worktree_runtime(wt.id).ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            "worktree has no checkout on this server".to_owned(),
        )
    })?;
    let run_guard = match held {
        Some(guard) => guard,
        None => runtime
            .exec_lock
            .clone()
            .try_lock_owned()
            .map(Arc::new)
            .map_err(|_| {
                (
                    StatusCode::CONFLICT,
                    "a sandbox run is already in progress for this worktree".to_owned(),
                )
            })?,
    };

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
    let service = tokio::task::spawn_blocking(move || engine.lock_worktree_service())
        .await
        .map_err(internal_err)?
        .map_err(map_git_err)?;
    let engine = state.git_engine.clone();
    let held_service = service.clone();
    let branch = wt.branch.clone();
    let base = wt.base_branch.clone();
    let checkout = runtime.worktree_dir.clone();
    let (checked_commit, checks) = tokio::task::spawn_blocking(move || {
        let _service = held_service;
        if verify {
            let commit = engine.clean_worktree_commit(wt.id, &branch)?;
            let checks = engine.required_checks(&base)?;
            if let Mode::Automatic {
                expected,
                checks_oid,
                ..
            } = mode
            {
                if expected != commit
                    || !checks
                        .as_ref()
                        .is_some_and(|checks| checks.auto_run && checks.blob_oid == checks_oid)
                {
                    return Err(GitEngineError::VerificationInvalid(
                        "automatic checks target or policy changed".into(),
                    ));
                }
            }
            Ok((Some(commit), checks))
        } else {
            engine
                .ensure_worktree_config_has_no_credentials(&checkout)
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
    let engine = state.git_engine.clone();
    let target = wt.clone();
    let store = state.auth_store.clone();
    let active_caller = caller.clone();
    let default_repo = state.repo_id;
    let repo_access = state.repo_access;
    let execution_guard = tokio::task::spawn_blocking(move || {
        let guard = run_guard;
        let current = if strict_automatic {
            store.authorize_automatic_checks_actor(active_caller.user_id)
        } else {
            store.verify_active_principal(&active_caller)
        }
        .map_err(map_auth_err)?;
        if current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        check_repo_bola(&store, default_repo, repo_access, &current, target.repo_id)?;
        let fence = engine.begin_execution_fence(&target).map_err(map_git_err)?;
        // A cancelled await drops the fence before releasing the worktree operation guard.
        Ok::<_, (StatusCode, String)>((fence, guard))
    })
    .await
    .map_err(internal_err)??;
    let commands: &[String] = checks
        .as_ref()
        .map_or(&[], |checks| checks.commands.as_slice());
    let outcome = match logical_run_id {
        Some(run_id) => {
            executor
                .execute_with_checks_for_run(
                    run_id,
                    wt.id,
                    &runtime.sandbox_spec,
                    &command,
                    commands,
                )
                .await
        }
        None => {
            executor
                .execute_with_checks(wt.id, &runtime.sandbox_spec, &command, commands)
                .await
        }
    };
    let checks_oid = checks.map(|checks| checks.blob_oid);
    match outcome {
        Ok(res) => {
            let (fence, run_guard) = execution_guard;
            tokio::task::spawn_blocking(move || fence.complete())
                .await
                .map_err(internal_err)?
                .map_err(map_git_err)?;
            if let Some(ref checked) = checked_commit {
                let engine = state.git_engine.clone();
                let branch = wt.branch.clone();
                let base = wt.base_branch.clone();
                let expected_policy = checks_oid.clone();
                let held_service = service.clone();
                let after = tokio::task::spawn_blocking(move || {
                    let _service = held_service;
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
            let held_service = service.clone();
            let (res, _run_guard) = tokio::task::spawn_blocking(move || {
                let _service = held_service;
                let guard = run_guard;
                let mut record = crate::run_history::RunRecord::new(
                    user_id,
                    verify && !automatic,
                    historical_commit,
                    historical_checks,
                    res.clone(),
                );
                record.automatic = automatic;
                crate::run_history::append(&common, wt.id, &branch, record)?;
                Ok::<_, (StatusCode, String)>((res, guard))
            })
            .await
            .map_err(internal_err)??;
            let store = state.auth_store.clone();
            let active_caller = caller.clone();
            let repo_id = wt.repo_id;
            let default_repo = state.repo_id;
            let repo_access = state.repo_access;
            tokio::task::spawn_blocking(move || {
                let current = if strict_automatic {
                    store.authorize_automatic_checks_actor(active_caller.user_id)
                } else {
                    store.verify_active_principal(&active_caller)
                }
                .map_err(map_auth_err)?;
                if current.role != EnterpriseRole::SiteAdmin {
                    return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
                }
                check_repo_bola(&store, default_repo, repo_access, &current, repo_id)
            })
            .await
            .map_err(internal_err)??;
            let final_status = if res.exit_code != 0 {
                WorktreeStatus::Failed
            } else if verify && !automatic {
                WorktreeStatus::Verified
            } else {
                WorktreeStatus::Pending
            };
            let result_commit = checked_commit.clone();
            set_worktree_status(state, wt.id, |w| {
                w.last_sandbox_duration_ms = res.duration_ms;
                w.status = final_status;
                w.verified_commit = if automatic { None } else { checked_commit };
                w.verified_checks_oid = if automatic { None } else { checks_oid };
            });
            status_guard.armed = false;
            Ok((res, final_status, result_commit))
        }
        Err(e) => {
            drop(execution_guard);
            set_worktree_status(state, wt.id, |w| w.status = WorktreeStatus::Failed);
            status_guard.armed = false;
            Err(map_sandbox_err(e))
        }
    }
}
