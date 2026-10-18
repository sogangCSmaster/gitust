//! Pull-request protection evaluation and gated merge execution (`POST /api/v1/repos/{id}/pulls/{pr_id}/merge`).

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use gitust_core::{
    AuthError, BranchProtectionRule, CommitSha, EnterpriseRole, PullRequestMergePreview,
    PullRequestProtectionStatus, PullRequestRecord, PullRequestStatus,
};
use gitust_git::GitEngineError;
use serde::{Deserialize, Serialize};

use crate::auth::{extract_principal, extract_principal_async};
use crate::local_checks_history::{LocalChecksHistoryStore, LocalChecksStatus};
use crate::pull_requests::{parse_lookup, ApiError};
use crate::repository_reads::{id as parse_repo_id, response};
use crate::repository_registry::ReadRepository;
use crate::state::{internal_err, map_auth_err, map_git_err, AppState};

#[derive(Serialize)]
struct MergePullRequestResponse {
    pull: PullRequestRecord,
    merged_sha: CommitSha,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MergePullRequestPayload {
    #[serde(default)]
    expected_base_sha: Option<CommitSha>,
    #[serde(default)]
    expected_head_sha: Option<CommitSha>,
    #[serde(default)]
    commit_message: Option<String>,
}

pub(crate) fn evaluate_protection_for_repo(
    state: &AppState,
    repository: &ReadRepository,
    pr: &PullRequestRecord,
) -> Result<PullRequestProtectionStatus, ApiError> {
    let policy = match repository.engine.required_checks_at_commit(&pr.head_sha) {
        Ok(policy) => policy,
        Err(GitEngineError::VerificationInvalid(_)) => {
            let rule: Option<BranchProtectionRule> = state
                .auth_store
                .effective_branch_protection(repository.access.repo_id, &pr.base_branch)
                .map_err(map_auth_err)?;
            let mut status = state
                .auth_store
                .evaluate_pull_request_protection(repository.access.repo_id, pr, true, false)
                .map_err(map_auth_err)?;
            status.protected = rule.is_some();
            return Ok(status);
        }
        Err(other) => return Err(map_git_err(other)),
    };

    let has_checks_policy = policy.is_some();
    let checks_passed = if let Some(ref policy) = policy {
        let common = repository
            .engine
            .repository_common_directory()
            .map_err(map_git_err)?;
        let store =
            LocalChecksHistoryStore::open(&common, repository.access.repo_id).map_err(|_| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "local Checks history storage is unavailable".into(),
                )
            })?;
        let records = store.list().map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "local Checks history storage is unavailable".into(),
            )
        })?;
        let full_head_ref = format!("refs/heads/{}", pr.head_branch);
        records.iter().any(|r| {
            r.source_ref == full_head_ref
                && r.checked_commit == pr.head_sha
                && r.checks_blob_oid == policy.blob_oid
                && r.status == LocalChecksStatus::Passed
                && r.result.exit_code == 0
        })
    } else {
        false
    };

    state
        .auth_store
        .evaluate_pull_request_protection(
            repository.access.repo_id,
            pr,
            has_checks_policy,
            checks_passed,
        )
        .map_err(map_auth_err)
}

pub(crate) async fn merge(
    State(state): State<AppState>,
    Path((raw_repo, raw_pr)): Path<(String, String)>,
    request: Request,
) -> Response {
    response(merge_inner(state, raw_repo, raw_pr, request).await)
}

async fn merge_inner(
    state: AppState,
    raw_repo: String,
    raw_pr: String,
    request: Request,
) -> Result<MergePullRequestResponse, ApiError> {
    let repo_id = parse_repo_id(&raw_repo)?;
    let lookup = parse_lookup(&raw_pr)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if origin.role == EnterpriseRole::Developer {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 256 * 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "merge request payload too large".into(),
        )
    })?;
    let repository = state.writable_repository(repo_id).ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "merging requires a repository enabled for Git writes".into(),
        )
    })?;
    let guard = repository.mutation.clone().try_lock_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "another repository mutation is in progress".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _inventory = state.repository_registry.clone();
        let _ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(map_git_err)?;
        if !repository.is_default {
            repository
                .engine
                .validate_git_write_service(repository.workbench_enabled)
                .map_err(map_git_err)?;
        }
        let payload: MergePullRequestPayload = if body.is_empty() {
            MergePullRequestPayload::default()
        } else {
            serde_json::from_slice(&body)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid merge payload: {e}")))?
        };
        if let Some(ref msg) = payload.commit_message {
            if msg.contains('\0') || msg.len() > 4096 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "commit_message must be <= 4096 bytes without NUL".into(),
                ));
            }
        }
        let verify_caller_and_pr = || -> Result<
            (
                gitust_core::AuthenticatedPrincipal,
                PullRequestRecord,
                PullRequestMergePreview,
            ),
            ApiError,
        > {
            let current = extract_principal(&parts.headers, &state.auth_store)?;
            if current.user_id != origin.user_id || current.role == EnterpriseRole::Developer {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let pr = state
                .auth_store
                .get_pull_request(&current, repository.access, lookup)
                .map_err(map_auth_err)?
                .ok_or_else(|| (StatusCode::NOT_FOUND, "pull request not found".into()))?;
            if pr.status != PullRequestStatus::Open {
                return Err((
                    StatusCode::CONFLICT,
                    "only open pull requests can be merged".into(),
                ));
            }
            let preview = repository
                .engine
                .preview_branch_merge(&pr.base_branch, &pr.head_branch)
                .map_err(map_git_err)?;
            if let Some(ref exp_base) = payload.expected_base_sha {
                if *exp_base != preview.base_sha {
                    return Err((
                        StatusCode::CONFLICT,
                        "base branch changed; refresh before merging".into(),
                    ));
                }
            }
            if let Some(ref exp_head) = payload.expected_head_sha {
                if *exp_head != preview.head_sha {
                    return Err((
                        StatusCode::CONFLICT,
                        "head branch changed; refresh before merging".into(),
                    ));
                }
            }
            if !preview.can_merge {
                return Err((
                    StatusCode::CONFLICT,
                    "pull request has merge conflicts or no commits ahead of base".into(),
                ));
            }
            let pr = state
                .auth_store
                .sync_pull_request_heads(
                    &current,
                    repository.access,
                    lookup,
                    preview.base_sha.clone(),
                    preview.head_sha.clone(),
                )
                .map_err(map_auth_err)?
                .ok_or_else(|| (StatusCode::NOT_FOUND, "pull request not found".into()))?;
            let protection = evaluate_protection_for_repo(&state, &repository, &pr)?;
            if protection.changes_requested {
                return Err((
                    StatusCode::CONFLICT,
                    "pull request merge blocked: changes were requested by a reviewer".into(),
                ));
            }
            if protection.approved_count < protection.required_approvals {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "pull request merge blocked: requires {} approval(s) on current head commit, found {}",
                        protection.required_approvals, protection.approved_count
                    ),
                ));
            }
            if protection.require_passing_checks && !protection.checks_passed {
                return Err((
                    StatusCode::CONFLICT,
                    "pull request merge blocked: required checks have not passed for current head commit"
                        .into(),
                ));
            }
            Ok((current, pr, preview))
        };

        let (caller, pr, preview) = verify_caller_and_pr()?;
        // fail-open-ok: uses a deterministic default merge commit message when none is supplied.
        let commit_msg = payload.commit_message.clone().unwrap_or_else(|| {
            format!(
                "Merge pull request #{} from {}: {}",
                pr.number, pr.head_branch, pr.title
            )
        });
        let author_email = format!("{}@gitust.local", caller.username);
        let mut gate_error: Option<ApiError> = None;
        let merged_sha = repository.engine.merge_branches_with_gate(
            &pr.base_branch,
            &pr.head_branch,
            &preview.base_sha,
            &preview.head_sha,
            &commit_msg,
            &caller.username,
            &author_email,
            || {
                if let Err(err) = verify_caller_and_pr() {
                    gate_error = Some(err);
                    return Err(GitEngineError::DefaultBranchConflict);
                }
                Ok(())
            },
        );
        if let Some(err) = gate_error {
            return Err(err);
        }
        let merged_sha = merged_sha.map_err(map_git_err)?;
        let updated_pr = state
            .auth_store
            .record_pull_request_merged(
                &caller,
                repository.access,
                lookup,
                preview.base_sha,
                preview.head_sha,
                merged_sha.clone(),
            )
            .map_err(map_auth_err)?;
        Ok(MergePullRequestResponse {
            pull: updated_pr,
            merged_sha,
        })
    })
    .await
    .map_err(internal_err)?
}

#[derive(Serialize)]
struct SinglePullRequestResponse {
    pull: PullRequestRecord,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateStatusPayload {
    status: PullRequestStatus,
}

pub(crate) async fn update_status(
    State(state): State<AppState>,
    Path((raw_repo, raw_pr)): Path<(String, String)>,
    request: Request,
) -> Response {
    response(update_status_inner(state, raw_repo, raw_pr, request).await)
}

async fn update_status_inner(
    state: AppState,
    raw_repo: String,
    raw_pr: String,
    request: Request,
) -> Result<SinglePullRequestResponse, ApiError> {
    let repo_id = parse_repo_id(&raw_repo)?;
    let lookup = parse_lookup(&raw_pr)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    let body = axum::body::to_bytes(body, 256 * 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "status payload too large".into(),
        )
    })?;
    let repository = state.writable_repository(repo_id).ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "status updates require a repository enabled for Git writes".into(),
        )
    })?;
    let guard = repository.mutation.clone().try_lock_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "another repository mutation is in progress".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _inventory = state.repository_registry.clone();
        let current = extract_principal(&parts.headers, &state.auth_store)?;
        if current.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        let payload: UpdateStatusPayload = serde_json::from_slice(&body).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid status payload: {e}"),
            )
        })?;
        let existing = state
            .auth_store
            .get_pull_request(&current, repository.access, lookup)
            .map_err(map_auth_err)?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "pull request not found".into()))?;
        if existing.status == PullRequestStatus::Merged {
            return Err((
                StatusCode::CONFLICT,
                "merged pull request status cannot be changed".into(),
            ));
        }
        let pull = state
            .auth_store
            .update_pull_request_status(&current, repository.access, lookup, payload.status)
            .map_err(|err| match err {
                AuthError::InvalidInput(msg) if msg.contains("already exists") => {
                    (StatusCode::CONFLICT, msg)
                }
                other => map_auth_err(other),
            })?;
        Ok(SinglePullRequestResponse { pull })
    })
    .await
    .map_err(internal_err)?
}
