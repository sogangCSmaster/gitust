//! Human approval is bound to the exact commit observed by the requesting client.
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::Json;
use gitust_core::{AgentWorktree, AuthError, CommitSha, EnterpriseRole, WorktreeStatus};
use gitust_git::GitEngineError;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{extract_principal, extract_principal_async};
use crate::state::{
    check_repo_bola, find_authorized_worktree, internal_err, map_auth_err, map_git_err,
    set_worktree_status, AppState,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveRequest {
    expected_commit: String,
}

pub(crate) async fn approve_worktree_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    request: Request,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    let (parts, body) = request.into_parts();
    let caller = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if !matches!(
        caller.role,
        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
    ) {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "approval request too large".into(),
        )
    })?;
    let caller = extract_principal_async(&parts.headers, &state.auth_store).await?;
    let request: ApproveRequest = serde_json::from_slice(&body).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "approval requires an explicit expected_commit".into(),
        )
    })?;
    if request.expected_commit.len() != 40
        || !request
            .expected_commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "expected_commit must be a full commit SHA".into(),
        ));
    }
    let expected_commit = CommitSha(request.expected_commit.to_ascii_lowercase());
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

    if wt.verified_commit.as_ref() != Some(&expected_commit) {
        return Err((
            StatusCode::CONFLICT,
            "verified commit differs from the reviewed commit; refresh and review before approval"
                .into(),
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
    let verified_commit = expected_commit;
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
        let authorize = || {
            let current = extract_principal(&parts.headers, &state_for_merge.auth_store)?;
            if current.user_id != caller.user_id
                || !matches!(
                    current.role,
                    EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
                )
            {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            check_repo_bola(
                &state_for_merge.auth_store,
                state_for_merge.repo_id,
                state_for_merge.repo_access,
                &current,
                wt.repo_id,
            )
        };
        authorize()?;
        let mut denied = None;
        let approved = engine.approve_verified_worktree_with_gate(
            wt_id,
            &branch,
            &base_branch,
            &verified_commit,
            || {
                if let Err(error) = authorize() {
                    denied = Some(error);
                    return Err(GitEngineError::VerificationInvalid(
                        "approval authority changed".into(),
                    ));
                }
                Ok(())
            },
        );
        if let Some(error) = denied {
            return Err(error);
        }
        let merged_sha = match approved {
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
