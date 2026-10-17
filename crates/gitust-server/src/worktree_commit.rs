//! Explicit host-side commits; the execution sandbox keeps shared Git metadata read-only.
use axum::http::{HeaderMap, StatusCode};
use gitust_core::{
    AuthError, AuthenticatedPrincipal, CommitSha, EnterpriseRole, WorktreeId, WorktreeStatus,
};
use gitust_git::GitEngineError;
use serde::Deserialize;
use uuid::Uuid;

use crate::state::{
    check_repo_bola, internal_err, map_auth_err, map_git_err, set_worktree_status, AppState,
};

#[path = "worktree_commit_policy.rs"]
mod policy;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommitRequest {
    worktree_id: Uuid,
    expected_head: String,
    paths: Vec<String>,
    message: String,
}

type HttpError = (StatusCode, String);

#[derive(Clone)]
pub(super) enum CommitAuthority {
    Principal,
    LocalOperator,
    Credential(HeaderMap),
}

fn active_caller(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    authority: &CommitAuthority,
) -> Result<AuthenticatedPrincipal, HttpError> {
    let current = match authority {
        CommitAuthority::Credential(headers) => {
            crate::auth::extract_mcp_principal(headers, &state.auth_store)?
        }
        CommitAuthority::Principal | CommitAuthority::LocalOperator => state
            .auth_store
            .verify_active_principal(caller)
            .map_err(map_auth_err)?,
    };
    if current.user_id != caller.user_id || current.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    Ok(current)
}

pub(super) async fn commit_worktree(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    request: CommitRequest,
    authority: CommitAuthority,
) -> Result<serde_json::Value, HttpError> {
    if !state.trusted_sandbox_exec || caller.role != EnterpriseRole::SiteAdmin {
        return Err((
            StatusCode::FORBIDDEN,
            "MCP commits require a SiteAdmin and trusted single-user execution".into(),
        ));
    }
    if request.message.trim().is_empty()
        || request.message.len() > 8192
        || request.message.contains('\0')
        || request.paths.is_empty()
        || request.paths.len() > 128
        || request
            .paths
            .iter()
            .any(|path| path.is_empty() || path.len() > 4096)
        || request.paths.iter().map(String::len).sum::<usize>() > 65_536
        || request.expected_head.len() != 40
        || !request
            .expected_head
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid commit message, paths, or expected_head".into(),
        ));
    }
    let state = state.clone();
    let caller = caller.clone();
    // The blocking job owns all guards and the state transition, including after a disconnect.
    tokio::task::spawn_blocking(move || {
        let caller = active_caller(&state, &caller, &authority)?;
        let id = WorktreeId(request.worktree_id);
        let runtime = state
            .get_worktree_runtime(id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
        let _worktree_guard = runtime
            .exec_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| (StatusCode::CONFLICT, "worktree is busy".into()))?;
        let wt = state.begin_worktree_commit(
            id,
            caller.user_id,
            matches!(authority, CommitAuthority::LocalOperator),
        )?;
        let result = (|| {
            check_repo_bola(&state.auth_store, state.repo_id, &caller, wt.repo_id)?;
            policy::check(&runtime, &request.paths)?;
            let prepared = state
                .git_engine
                .prepare_worktree_commit(
                    wt.id,
                    &wt.branch,
                    &CommitSha(request.expected_head.to_ascii_lowercase()),
                    &request.paths,
                    &request.message,
                    &format!("{}@agents.gitust.local", caller.user_id.0),
                )
                .map_err(map_git_err)?;
            let _repository_guard = state
                .repo_mutation_lock
                .clone()
                .try_lock_owned()
                .map_err(|_| (StatusCode::CONFLICT, "repository is busy".into()))?;
            let next = prepared.result();
            let intent = crate::automatic_checks::prepare(
                &state, &caller, wt.id, next.commit.clone(), Some(next.parent.clone()),
                if matches!(authority, CommitAuthority::LocalOperator) { crate::automatic_run_model::Origin::LocalOperator }
                else { crate::automatic_run_model::Origin::Mcp },
            )?;
            let mut denied = None;
            let committed = state
                .git_engine
                .finish_worktree_commit_with_gate(prepared, || {
                    let authority = active_caller(&state, &caller, &authority)
                        .and_then(|fresh| {
                            check_repo_bola(&state.auth_store, state.repo_id, &fresh, wt.repo_id)
                        })
                        .and_then(|_| policy::check(&runtime, &request.paths));
                    if let Err(error) = authority {
                        denied = Some(error);
                        return Err(GitEngineError::VerificationInvalid(
                            "commit authority changed".into(),
                        ));
                    }
                    Ok(())
                });
            if let Some(error) = denied {
                if let Some(intent) = intent {
                    if let Err((status, _)) = crate::automatic_checks::reject(intent) {
                        state.automatic_checks.mark_unavailable();
                        tracing::warn!(%status, "commit rejected; automatic intent recovery is required");
                    }
                }
                return Err(error);
            }
            match committed {
                Ok(committed) => Ok((committed, intent)),
                Err(error) => {
                    if let Some(intent) = intent {
                        if let Err((status, _)) = crate::automatic_checks::confirm(&state, intent) {
                            state.automatic_checks.mark_unavailable();
                            tracing::warn!(%status, "Git commit failed; automatic intent recovery is required");
                        }
                    }
                    Err(map_git_err(error))
                }
            }
        })();
        set_worktree_status(&state, wt.id, |task| {
            task.status = if result.is_ok() {
                WorktreeStatus::Pending
            } else {
                WorktreeStatus::Failed
            };
        });
        let (committed, intent) = result?;
        // A trigger failure cannot undo the already accepted Git commit. Return its identity
        // and an explicit scheduling outcome so callers do not retry the mutation blindly.
        let automatic_checks = match intent {
            None => "disabled",
            Some(intent) => match crate::automatic_checks::confirm(&state, intent) {
                Ok(true) => "queued",
                Ok(false) => "unavailable",
                Err((status, _)) => {
                    set_worktree_status(&state, wt.id, |task| task.status = WorktreeStatus::Failed);
                    tracing::warn!(worktree_id = %wt.id.0, %status, "commit accepted but automatic Checks could not be queued");
                    "unavailable"
                }
            }
        };
        Ok(serde_json::json!({
            "worktree_id": committed.worktree_id.0,
            "branch": committed.branch,
            "commit": committed.commit.0,
            "parent": committed.parent.0,
            "status": if automatic_checks == "unavailable" { "failed" } else { "pending" },
            "automatic_checks": automatic_checks,
            "verification_required": true,
        }))
    })
    .await
    .map_err(internal_err)?
}
