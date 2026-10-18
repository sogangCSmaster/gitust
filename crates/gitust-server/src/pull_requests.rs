//! Pull-request collaboration REST handlers (`/api/v1/repos/{id}/pulls...`).

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use gitust_core::{
    AuthError, CommitSha, PullRequestMergePreview, PullRequestProtectionStatus, PullRequestRecord,
    PullRequestStatus, ReviewVerdict,
};
use gitust_db::{CreatePullRequestInput, PullRequestLookup};
use gitust_git::GitEngine;
use serde::{Deserialize, Serialize};

use crate::auth::{extract_principal, extract_principal_async};
use crate::pull_requests_merge::evaluate_protection_for_repo;
pub(crate) use crate::pull_requests_merge::merge;
use crate::repository_reads::{id as parse_repo_id, response};
use crate::state::{internal_err, map_auth_err, map_git_err, AppState};

pub(crate) type ApiError = (StatusCode, String);

#[derive(Serialize)]
struct PullRequestListResponse {
    pulls: Vec<PullRequestRecord>,
}

#[derive(Serialize)]
struct SinglePullRequestResponse {
    pull: PullRequestRecord,
}

#[derive(Serialize)]
struct PullRequestDetailResponse {
    pull: PullRequestRecord,
    merge_preview: PullRequestMergePreview,
    protection: PullRequestProtectionStatus,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePullRequestPayload {
    title: String,
    #[serde(default)]
    description: String,
    base_branch: String,
    head_branch: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    status: Option<PullRequestStatus>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddReviewPayload {
    verdict: ReviewVerdict,
    #[serde(default)]
    body: String,
    #[serde(default, alias = "expected_head_sha")]
    reviewed_head_sha: Option<CommitSha>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddCommentPayload {
    body: String,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    line_number: Option<u32>,
    #[serde(default)]
    commit_sha: Option<CommitSha>,
}

pub(crate) fn parse_lookup(raw: &str) -> Result<PullRequestLookup, ApiError> {
    PullRequestLookup::parse_str(raw).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "invalid pull request identifier".into(),
        )
    })
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    headers: HeaderMap,
) -> Response {
    response(list_inner(state, raw, headers).await)
}

async fn list_inner(
    state: AppState,
    raw: String,
    headers: HeaderMap,
) -> Result<PullRequestListResponse, ApiError> {
    let repo_id = parse_repo_id(&raw)?;
    let origin = extract_principal_async(&headers, &state.auth_store).await?;
    let repository = state
        .read_repository(repo_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let _ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(map_git_err)?;
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        let mut pulls = state
            .auth_store
            .list_pull_requests(&current, repository.access)
            .map_err(map_auth_err)?;
        let mut updates = Vec::new();
        for pr in &pulls {
            if matches!(
                pr.status,
                PullRequestStatus::Open | PullRequestStatus::Draft
            ) {
                // fail-open-ok: if a branch was deleted out-of-band, retain the last recorded SHAs in the list view.
                if let Ok((base_sha, head_sha)) = repository
                    .engine
                    .resolve_branch_pair_shas(&pr.base_branch, &pr.head_branch)
                {
                    if pr.base_sha != base_sha || pr.head_sha != head_sha {
                        updates.push((pr.id, base_sha, head_sha));
                    }
                }
            }
        }
        if !updates.is_empty() {
            pulls = state
                .auth_store
                .sync_pull_request_heads_batch(&current, repository.access, &updates)
                .map_err(map_auth_err)?;
        }
        let after = extract_principal(&headers, &state.auth_store)?;
        if after.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        Ok(PullRequestListResponse { pulls })
    })
    .await
    .map_err(internal_err)?
}

pub(crate) async fn create(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    match create_inner(state, raw, request).await {
        Ok(body) => {
            let mut resp = (StatusCode::CREATED, Json(body)).into_response();
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            resp
        }
        Err(err) => err.into_response(),
    }
}

async fn create_inner(
    state: AppState,
    raw: String,
    request: Request,
) -> Result<SinglePullRequestResponse, ApiError> {
    let repo_id = parse_repo_id(&raw)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    let body = axum::body::to_bytes(body, 256 * 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "pull request payload too large".into(),
        )
    })?;
    let repository = state.writable_repository(repo_id).ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "pull requests require a repository enabled for Git writes".into(),
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
        let current = extract_principal(&parts.headers, &state.auth_store)?;
        if current.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        let payload: CreatePullRequestPayload = serde_json::from_slice(&body).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid pull request payload: {e}"),
            )
        })?;
        let base_branch =
            GitEngine::validate_outbound_branch(&payload.base_branch).map_err(map_git_err)?;
        let head_branch =
            GitEngine::validate_branch_name(&payload.head_branch).map_err(map_git_err)?;
        if head_branch.starts_with("agent/cli/") {
            return Err((
                StatusCode::BAD_REQUEST,
                "ephemeral sandbox branches cannot be used for pull requests".into(),
            ));
        }
        let preview = repository
            .engine
            .preview_branch_merge(&base_branch, &head_branch)
            .map_err(map_git_err)?;
        let initial_status = match payload.status {
            Some(status) => status,
            None if payload.draft => PullRequestStatus::Draft,
            None => PullRequestStatus::Open,
        };
        let pull = state
            .auth_store
            .create_pull_request(
                &current,
                repository.access,
                CreatePullRequestInput {
                    repo_id,
                    title: payload.title,
                    description: payload.description,
                    base_branch,
                    head_branch,
                    base_sha: preview.base_sha,
                    head_sha: preview.head_sha,
                    status: initial_status,
                },
            )
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

pub(crate) async fn get_one(
    State(state): State<AppState>,
    Path((raw_repo, raw_pr)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    response(get_one_inner(state, raw_repo, raw_pr, headers).await)
}

async fn get_one_inner(
    state: AppState,
    raw_repo: String,
    raw_pr: String,
    headers: HeaderMap,
) -> Result<PullRequestDetailResponse, ApiError> {
    let repo_id = parse_repo_id(&raw_repo)?;
    let lookup = parse_lookup(&raw_pr)?;
    let origin = extract_principal_async(&headers, &state.auth_store).await?;
    let repository = state
        .read_repository(repo_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let _ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(map_git_err)?;
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        let mut pull = state
            .auth_store
            .get_pull_request(&current, repository.access, lookup)
            .map_err(map_auth_err)?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "pull request not found".into()))?;

        let merge_preview = if matches!(
            pull.status,
            PullRequestStatus::Open | PullRequestStatus::Draft
        ) {
            match repository
                .engine
                .preview_branch_merge(&pull.base_branch, &pull.head_branch)
            {
                Ok(preview) => {
                    if let Some(synced) = state
                        .auth_store
                        .sync_pull_request_heads(
                            &current,
                            repository.access,
                            lookup,
                            preview.base_sha.clone(),
                            preview.head_sha.clone(),
                        )
                        .map_err(map_auth_err)?
                    {
                        pull = synced;
                    }
                    preview
                }
                // fail-open-ok: if base/head branch was deleted out-of-band, report can_merge: false so the PR can still be viewed and closed.
                Err(gitust_git::GitEngineError::BaseBranchNotFound(_)) => PullRequestMergePreview {
                    can_merge: false,
                    fast_forward: false,
                    conflicting_files: Vec::new(),
                    base_sha: pull.base_sha.clone(),
                    head_sha: pull.head_sha.clone(),
                },
                Err(other) => return Err(map_git_err(other)),
            }
        } else {
            PullRequestMergePreview {
                can_merge: false,
                fast_forward: false,
                conflicting_files: Vec::new(),
                base_sha: pull.base_sha.clone(),
                head_sha: pull.head_sha.clone(),
            }
        };

        let protection = evaluate_protection_for_repo(&state, &repository, &pull)?;
        let after = extract_principal(&headers, &state.auth_store)?;
        if after.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        Ok(PullRequestDetailResponse {
            pull,
            merge_preview,
            protection,
        })
    })
    .await
    .map_err(internal_err)?
}

pub(crate) async fn add_review(
    State(state): State<AppState>,
    Path((raw_repo, raw_pr)): Path<(String, String)>,
    request: Request,
) -> Response {
    response(add_review_inner(state, raw_repo, raw_pr, request).await)
}

async fn add_review_inner(
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
            "review payload too large".into(),
        )
    })?;
    let repository = state.writable_repository(repo_id).ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "reviews require a repository enabled for Git writes".into(),
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
        let current = extract_principal(&parts.headers, &state.auth_store)?;
        if current.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        let payload: AddReviewPayload = serde_json::from_slice(&body).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid review payload: {e}"),
            )
        })?;
        let pr = state
            .auth_store
            .get_pull_request(&current, repository.access, lookup)
            .map_err(map_auth_err)?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "pull request not found".into()))?;
        if !matches!(
            pr.status,
            PullRequestStatus::Open | PullRequestStatus::Draft
        ) {
            return Err((
                StatusCode::CONFLICT,
                "cannot review a closed or merged pull request".into(),
            ));
        }
        let preview = repository
            .engine
            .preview_branch_merge(&pr.base_branch, &pr.head_branch)
            .map_err(map_git_err)?;
        if let Some(ref expected) = payload.reviewed_head_sha {
            if *expected != preview.head_sha {
                return Err((
                    StatusCode::CONFLICT,
                    "head branch changed since review was prepared; refresh before reviewing"
                        .into(),
                ));
            }
        }
        state
            .auth_store
            .sync_pull_request_heads(
                &current,
                repository.access,
                lookup,
                preview.base_sha,
                preview.head_sha.clone(),
            )
            .map_err(map_auth_err)?;
        let pull = state
            .auth_store
            .add_pull_request_review(
                &current,
                repository.access,
                lookup,
                payload.verdict,
                payload.body,
                preview.head_sha,
            )
            .map_err(map_auth_err)?;
        Ok(SinglePullRequestResponse { pull })
    })
    .await
    .map_err(internal_err)?
}

pub(crate) async fn add_comment(
    State(state): State<AppState>,
    Path((raw_repo, raw_pr)): Path<(String, String)>,
    request: Request,
) -> Response {
    response(add_comment_inner(state, raw_repo, raw_pr, request).await)
}

async fn add_comment_inner(
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
            "comment payload too large".into(),
        )
    })?;
    let repository = state.writable_repository(repo_id).ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "comments require a repository enabled for Git writes".into(),
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
        let payload: AddCommentPayload = serde_json::from_slice(&body).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid comment payload: {e}"),
            )
        })?;
        if state
            .auth_store
            .get_pull_request(&current, repository.access, lookup)
            .map_err(map_auth_err)?
            .is_none()
        {
            return Err((StatusCode::NOT_FOUND, "pull request not found".into()));
        }
        let pull = state
            .auth_store
            .add_pull_request_comment(
                &current,
                repository.access,
                lookup,
                payload.body,
                payload.file_path,
                payload.line_number,
                payload.commit_sha,
            )
            .map_err(map_auth_err)?;
        Ok(SinglePullRequestResponse { pull })
    })
    .await
    .map_err(internal_err)?
}
