//! Step 2-4: Inbound Read-Only Mirror Sync (`POST /api/v1/sync/github`) and Blazing-Fast
//! (`< 30ms`) Repository Code, Commit, Tree & Diff Browsing HTTP Handlers.

use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use gitust_core::AuthError;
use gitust_core::{AuthenticatedPrincipal, EnterpriseRole};
use gitust_git::{
    GitEngineError, RepoBlobSnapshot, RepoCommitList, RepoDiffSnapshot, RepoTreeSnapshot,
};
use gitust_sync::SyncStatusReport;
use serde::Deserialize;

use crate::auth::{extract_principal, extract_principal_async};
use crate::state::{check_repo_bola, internal_err, map_auth_err, map_git_err, AppState};

#[derive(Debug, Default, Deserialize)]
pub struct RepoCommitsQuery {
    pub rev: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RepoTreeQuery {
    pub rev: Option<String>,
    pub path: Option<String>,
    pub recursive: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct RepoBlobQuery {
    pub rev: Option<String>,
    pub path: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct RepoDiffQuery {
    pub base: Option<String>,
    pub head: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionalSyncRequest {
    github_remote_url: Option<String>,
}

/// `GET /api/v1/repo/commits` — Lists commit history starting from `rev` (default `HEAD`).
pub(crate) async fn repo_commits_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Query(query): Query<RepoCommitsQuery>,
) -> Result<Json<RepoCommitList>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo_id = state.repo_id;
    let engine = state.git_engine.clone();
    let rev = query.rev;
    // fail-open-ok: optional page size defaults to a bounded limit; repository authorization is unchanged.
    let limit = query.limit.unwrap_or(50).clamp(1, 200);

    let list = tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, repo_id, &caller, repo_id)?;
        engine
            .list_commits(rev.as_deref(), limit)
            .map_err(map_git_err)
    })
    .await
    .map_err(internal_err)??;
    Ok(Json(list))
}

/// `GET /api/v1/repo/tree` — Inspects a directory tree (or recursive flat file tree) at `rev`.
pub(crate) async fn repo_tree_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Query(query): Query<RepoTreeQuery>,
) -> Result<Json<RepoTreeSnapshot>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo_id = state.repo_id;
    let engine = state.git_engine.clone();
    let rev = query.rev;
    let path = query.path;
    // fail-open-ok: optional query parameter default (`recursive=false` lists single directory level).
    let recursive = query.recursive.unwrap_or(false);

    let tree = tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, repo_id, &caller, repo_id)?;
        engine
            .inspect_tree(rev.as_deref(), path.as_deref(), recursive)
            .map_err(map_git_err)
    })
    .await
    .map_err(internal_err)??;
    Ok(Json(tree))
}

/// `GET /api/v1/repo/blob` — Reads a file blob at `rev:path` in-process via `gix` (1 MiB cap).
pub(crate) async fn repo_blob_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Query(query): Query<RepoBlobQuery>,
) -> Result<Json<RepoBlobSnapshot>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo_id = state.repo_id;
    let engine = state.git_engine.clone();
    let rev = query.rev;
    let path = query.path;

    let blob = tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, repo_id, &caller, repo_id)?;
        engine.read_blob(rev.as_deref(), &path).map_err(map_git_err)
    })
    .await
    .map_err(internal_err)??;
    Ok(Json(blob))
}

/// `GET /api/v1/repo/diff` — Computes unified diff lines and summary stats between `base` and `head`.
pub(crate) async fn repo_diff_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Query(query): Query<RepoDiffQuery>,
) -> Result<Json<RepoDiffSnapshot>, (StatusCode, String)> {
    let store = state.auth_store.clone();
    let repo_id = state.repo_id;
    let engine = state.git_engine.clone();
    let base = query.base;
    let head = query.head;
    let path = query.path;

    let diff = tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, repo_id, &caller, repo_id)?;
        engine
            .inspect_diff(base.as_deref(), head.as_deref(), path.as_deref())
            .map_err(map_git_err)
    })
    .await
    .map_err(internal_err)??;
    Ok(Json(diff))
}

/// `POST /api/v1/sync/github` — Triggers an inbound read-only mirror synchronization from the
/// repository's configured `github_remote_url`, keeping all `refs/heads/agent/*` branches private.
///
/// Re-authenticates after reading the optional request body (preventing slow-loris deactivation
/// bypass) and rejects any request-supplied URL that does not match the server's configured
/// `sync_config.github_remote_url` (preventing SSRF or arbitrary local filesystem probing).
pub(crate) async fn sync_github_handler(
    State(state): State<AppState>,
    req: Request,
) -> Result<Json<SyncStatusReport>, (StatusCode, String)> {
    let (parts, request_body) = req.into_parts();
    let _pre_caller = extract_principal_async(&parts.headers, &state.auth_store).await?;

    let body = axum::body::to_bytes(request_body, 4096)
        .await
        .map_err(|_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "sync request body too large".to_owned(),
            )
        })?;

    let store = state.auth_store.clone();
    let repo_id = state.repo_id;
    let headers = parts.headers;
    tokio::task::spawn_blocking(move || {
        let caller = extract_principal(&headers, &store)?;
        if !matches!(
            caller.role,
            EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
        ) {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        check_repo_bola(&store, repo_id, &caller, repo_id)
    })
    .await
    .map_err(internal_err)??;

    let configured_url = state.sync_config.github_remote_url.trim().to_owned();
    if !body.iter().all(u8::is_ascii_whitespace) {
        let parsed: OptionalSyncRequest = serde_json::from_slice(&body).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid sync JSON body: {e}"),
            )
        })?;
        if let Some(req_url) = parsed.github_remote_url.as_deref().map(str::trim) {
            if !req_url.is_empty() && req_url != configured_url {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "github_remote_url does not match the server's configured mirror URL"
                        .to_owned(),
                ));
            }
        }
    }

    let fetch_guard = state
        .sync_fetch_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another mirror synchronization fetch is currently in progress".to_owned(),
            )
        })?;

    let start = std::time::Instant::now();
    let engine_for_fetch = state.git_engine.clone();
    let url_for_fetch = configured_url.clone();
    tokio::task::spawn_blocking(move || {
        let _held_fetch = fetch_guard;
        engine_for_fetch.fetch_inbound_mirror_staging(&url_for_fetch)
    })
    .await
    .map_err(internal_err)?
    .map_err(|err| match err {
        GitEngineError::InvalidRemoteUrl(_) => (StatusCode::BAD_REQUEST, err.to_string()),
        GitEngineError::GitCommand { .. } => (StatusCode::BAD_GATEWAY, err.to_string()),
        other => map_git_err(other),
    })?;

    let repo_guard = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.repo_mutation_lock.clone().lock_owned(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::CONFLICT,
            "another repository merge is currently in progress".to_owned(),
        )
    })?;

    let engine = state.git_engine.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let _held = repo_guard;
        engine.apply_inbound_mirror_refs(&configured_url, start)
    })
    .await
    .map_err(internal_err)?
    .map_err(|err| match err {
        GitEngineError::InvalidRemoteUrl(_) => (StatusCode::BAD_REQUEST, err.to_string()),
        GitEngineError::GitCommand { .. } => (StatusCode::BAD_GATEWAY, err.to_string()),
        other => map_git_err(other),
    })?;

    Ok(Json(SyncStatusReport {
        github_remote_url: outcome.github_remote_url,
        last_synced_commit: outcome.last_synced_commit,
        sync_latency_ms: outcome.sync_latency_ms,
        private_agent_branches_filtered: outcome.private_agent_branches_filtered,
        skipped_branches: outcome.skipped_branches,
        skipped_tags: outcome.skipped_tags,
        local_only_branches: outcome.local_only_branches,
        synced_at: Utc::now(),
    }))
}
