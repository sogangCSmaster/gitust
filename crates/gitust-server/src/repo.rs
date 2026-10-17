//! Step 2-4: Inbound Read-Only Mirror Sync (`POST /api/v1/sync/github`) and Blazing-Fast
//! (`< 30ms`) Repository Code, Commit, Tree & Diff Browsing HTTP Handlers.

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use chrono::Utc;
use gitust_core::AuthError;
use gitust_core::EnterpriseRole;
use gitust_git::GitEngineError;
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

/// Legacy primary route; all authorization and bounded reads share the UUID implementation.
pub(crate) async fn repo_commits_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RepoCommitsQuery>,
) -> Response {
    let primary = state.repo_id.0.to_string();
    crate::repository_reads::commits(State(state), headers, Path(primary), Query(query)).await
}

/// Legacy primary route; all authorization and bounded reads share the UUID implementation.
pub(crate) async fn repo_tree_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RepoTreeQuery>,
) -> Response {
    let primary = state.repo_id.0.to_string();
    crate::repository_reads::tree(State(state), headers, Path(primary), Query(query)).await
}

/// Legacy primary route; all authorization and bounded reads share the UUID implementation.
pub(crate) async fn repo_blob_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RepoBlobQuery>,
) -> Response {
    let primary = state.repo_id.0.to_string();
    crate::repository_reads::blob(State(state), headers, Path(primary), Query(query)).await
}

/// Legacy primary route; all authorization and bounded reads share the UUID implementation.
pub(crate) async fn repo_diff_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RepoDiffQuery>,
) -> Response {
    let primary = state.repo_id.0.to_string();
    crate::repository_reads::diff(State(state), headers, Path(primary), Query(query)).await
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
    let repo_access = state.repo_access;
    let headers = parts.headers;
    let initial_headers = headers.clone();
    let caller = tokio::task::spawn_blocking(move || {
        let caller = extract_principal(&initial_headers, &store)?;
        if !matches!(
            caller.role,
            EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
        ) {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        check_repo_bola(&store, repo_id, repo_access, &caller, repo_id)?;
        Ok::<_, (StatusCode, String)>(caller)
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
    let (fetch_result, fetch_guard) = tokio::task::spawn_blocking(move || {
        let result = engine_for_fetch.fetch_inbound_mirror_staging(&url_for_fetch);
        (result, fetch_guard)
    })
    .await
    .map_err(internal_err)?;
    fetch_result.map_err(|err| match err {
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
    let store = state.auth_store.clone();
    #[cfg(test)]
    let hook = state.sync_after_fetch_hook.lock().take();
    let outcome = tokio::task::spawn_blocking(move || {
        let _held = repo_guard;
        let _held_fetch = fetch_guard;
        #[cfg(test)]
        if let Some(hook) = hook {
            hook();
        }
        let mut denied = None;
        let outcome = engine.apply_inbound_mirror_refs_with_gate(&configured_url, start, || {
            let authority = extract_principal(&headers, &store).and_then(|current| {
                if current.user_id != caller.user_id
                    || !matches!(
                        current.role,
                        EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
                    )
                {
                    return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
                }
                check_repo_bola(&store, repo_id, repo_access, &current, repo_id)
            });
            if let Err(error) = authority {
                denied = Some(error);
                return Err(GitEngineError::VerificationInvalid(
                    "mirror synchronization authority changed".into(),
                ));
            }
            Ok(())
        });
        if let Some(error) = denied {
            return Err(error);
        }
        outcome.map_err(|err| match err {
            GitEngineError::InvalidRemoteUrl(_) => (StatusCode::BAD_REQUEST, err.to_string()),
            GitEngineError::GitCommand { .. } => (StatusCode::BAD_GATEWAY, err.to_string()),
            other => map_git_err(other),
        })
    })
    .await
    .map_err(internal_err)??;

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
