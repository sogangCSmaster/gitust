//! Smart HTTP Git Hosting (`/git/{repo}/info/refs`, `/git/{repo}/git-upload-pack`,
//! `/git/{repo}/git-receive-pack`) with PAT/Session Basic & Bearer authentication,
//! pre/post-body Slow-Loris TOCTOU re-verification, Team Repository BOLA, and
//! role-based `refs/heads/agent/*` vs protected branch push enforcement.

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoId, WorktreeStatus};
use gitust_git::{
    inspect_receive_pack_updates, GitEngineError, SmartHttpService, MAX_SMART_HTTP_BODY_BYTES,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::git_http_auth::{
    extract_git_http_principal, extract_git_http_principal_async, git_auth_err_response,
};
use crate::state::{check_repo_bola, internal_err, map_auth_err, map_git_err, AppState};

#[derive(Debug, Deserialize)]
pub struct InfoRefsQuery {
    pub service: Option<String>,
}

async fn resolve_and_authorize_repo(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    raw_repo: &str,
) -> Result<RepoId, Response> {
    let trimmed = raw_repo.trim();
    // fail-open-ok: an optional URL suffix changes only spelling; repository lookup and BOLA checks follow.
    let clean = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let target_repo_id =
        if clean.eq_ignore_ascii_case("default") || clean.eq_ignore_ascii_case("gitust") {
            state.repo_id
        } else {
            match Uuid::parse_str(clean) {
                Ok(uuid) => RepoId(uuid),
                Err(_) => {
                    return Err((
                        StatusCode::NOT_FOUND,
                        format!("repository '{raw_repo}' not found"),
                    )
                        .into_response());
                }
            }
        };

    if target_repo_id != state.repo_id {
        return Err((
            StatusCode::NOT_FOUND,
            "repository is not served by this instance".to_owned(),
        )
            .into_response());
    }

    let store = state.auth_store.clone();
    let default_repo = state.repo_id;
    let repo_access = state.repo_access;
    let caller_owned = caller.clone();
    tokio::task::spawn_blocking(move || {
        check_repo_bola(
            &store,
            default_repo,
            repo_access,
            &caller_owned,
            target_repo_id,
        )
    })
    .await
    .map_err(|e| internal_err(e).into_response())?
    .map_err(IntoResponse::into_response)?;

    Ok(target_repo_id)
}

/// `GET /git/{repo}/info/refs?service=git-upload-pack|git-receive-pack`
pub(crate) async fn git_info_refs_handler(
    State(state): State<AppState>,
    Path(repo): Path<String>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Response {
    if query.service.as_deref().and_then(SmartHttpService::parse)
        == Some(SmartHttpService::UploadPack)
    {
        return crate::git_http_read::advertise(state, repo, headers).await;
    }
    if crate::git_http_write::secondary_id(&state, &repo).is_some()
        && query.service.as_deref().and_then(SmartHttpService::parse)
            == Some(SmartHttpService::ReceivePack)
    {
        return crate::git_http_write::advertise(state, repo, headers).await;
    }
    if query.service.as_deref().and_then(SmartHttpService::parse)
        == Some(SmartHttpService::ReceivePack)
    {
        return crate::git_http_read::advertise_service(
            state,
            repo,
            headers,
            SmartHttpService::ReceivePack,
        )
        .await;
    }
    let caller = match extract_git_http_principal_async(&headers, &state.auth_store).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(resp) = resolve_and_authorize_repo(&state, &caller, &repo).await {
        return resp;
    }

    let Some(raw_service) = query.service.as_deref() else {
        return (
            StatusCode::BAD_REQUEST,
            "Smart HTTP protocol required: specify ?service=git-upload-pack or ?service=git-receive-pack",
        )
            .into_response();
    };
    (
        StatusCode::FORBIDDEN,
        format!("unsupported git service '{raw_service}'"),
    )
        .into_response()
}

/// `POST /git/{repo}/git-upload-pack`
pub(crate) async fn git_upload_pack_handler(
    State(state): State<AppState>,
    Path(repo): Path<String>,
    req: Request,
) -> Response {
    crate::git_http_read::upload(state, repo, req).await
}

/// `POST /git/{repo}/git-receive-pack`
pub(crate) async fn git_receive_pack_handler(
    State(state): State<AppState>,
    Path(repo): Path<String>,
    req: Request,
) -> Response {
    if crate::git_http_write::secondary_id(&state, &repo).is_some() {
        return crate::git_http_write::receive(state, repo, req).await;
    }
    let (parts, request_body) = req.into_parts();
    let pre_caller = match extract_git_http_principal_async(&parts.headers, &state.auth_store).await
    {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(resp) = resolve_and_authorize_repo(&state, &pre_caller, &repo).await {
        return resp;
    }
    if state
        .read_repository(state.repo_id)
        .is_some_and(|r| r.archived)
    {
        return git_auth_err_response(
            StatusCode::FORBIDDEN,
            "repository is archived and read-only".to_owned(),
        );
    }

    // fail-open-ok: oversized receive-pack body immediately fails closed with 413 PAYLOAD_TOO_LARGE.
    let Ok(body) = axum::body::to_bytes(request_body, MAX_SMART_HTTP_BODY_BYTES).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("receive-pack body exceeds {MAX_SMART_HTTP_BODY_BYTES} bytes"),
        )
            .into_response();
    };

    // Re-authenticate after body streaming to defeat Slow-Loris mid-body account/PAT revocation TOCTOU.
    let caller = match extract_git_http_principal_async(&parts.headers, &state.auth_store).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(resp) = resolve_and_authorize_repo(&state, &caller, &repo).await {
        return resp;
    }
    if state
        .read_repository(state.repo_id)
        .is_some_and(|r| r.archived)
    {
        return git_auth_err_response(
            StatusCode::FORBIDDEN,
            "repository is archived and read-only".to_owned(),
        );
    }

    let updates = match inspect_receive_pack_updates(&body) {
        Ok(u) => u,
        Err(e) => return map_git_err(e).into_response(),
    };

    // Enforce Role-Based Branch Push Policy (Phase 3 Pillar 3):
    // - Developers & Agents may push commits ONLY to `refs/heads/agent/*` (no deletions, no `main`).
    // - Pushing directly to non-agent branches (`main`, tags) requires Maintainer or SiteAdmin.
    if caller.role == EnterpriseRole::Developer {
        for u in &updates {
            if !u.is_agent_branch || u.is_deletion {
                return map_auth_err(AuthError::AdminPrivilegeRequired).into_response();
            }
        }
    }

    let repo_guard = match state.repo_mutation_lock.clone().try_lock_owned() {
        Ok(g) => g,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                "another repository merge, push, or mirror sync is currently in progress",
            )
                .into_response();
        }
    };

    // Inspect tracked worktrees under repo_mutation_lock:
    // - Reject pushes to branches in Compiling / Merged / Discarded state.
    // - Enforce Developer ownership on tracked worktree branches.
    // - Acquire per-worktree exec_lock so sandbox execution cannot start concurrently.
    // - Reset any Verified/Failed worktree status back to Pending because new commits are being pushed.
    let mut matched_worktrees = Vec::new();
    {
        let worktrees = state.worktrees.read();
        for u in &updates {
            if let Some(short) = u.refname.strip_prefix("refs/heads/") {
                if let Some(wt) = worktrees
                    .iter()
                    .find(|w| w.repo_id == state.repo_id && w.branch == short)
                {
                    matched_worktrees.push((wt.id, wt.branch.clone(), wt.status));
                }
            }
        }
    }

    let mut held_exec_locks = Vec::with_capacity(matched_worktrees.len());
    for (wt_id, wt_branch, wt_status) in &matched_worktrees {
        if matches!(
            wt_status,
            WorktreeStatus::Compiling | WorktreeStatus::Merged | WorktreeStatus::Discarded
        ) {
            return (
                StatusCode::CONFLICT,
                format!("cannot push to agent branch '{wt_branch}' in {wt_status:?} state"),
            )
                .into_response();
        }
        let Some(rt) = state.get_worktree_runtime(*wt_id) else {
            return (
                StatusCode::CONFLICT,
                format!("worktree runtime for branch '{wt_branch}' is unavailable"),
            )
                .into_response();
        };
        if caller.role == EnterpriseRole::Developer && rt.created_by != Some(caller.user_id) {
            return map_auth_err(AuthError::ForbiddenResourceOwnership).into_response();
        }
        let guard = match rt.exec_lock.clone().try_lock_owned() {
            Ok(g) => g,
            Err(_) => {
                return (
                    StatusCode::CONFLICT,
                    format!("worktree for branch '{wt_branch}' is currently busy"),
                )
                    .into_response();
            }
        };
        if !state.worktree_runtime.read().contains_key(wt_id) {
            return (
                StatusCode::CONFLICT,
                format!("worktree runtime for branch '{wt_branch}' was removed"),
            )
                .into_response();
        }
        let Some(current_status) = state
            .worktrees
            .read()
            .iter()
            .find(|w| w.id == *wt_id)
            .map(|w| w.status)
        else {
            return (
                StatusCode::CONFLICT,
                format!("worktree for branch '{wt_branch}' was removed"),
            )
                .into_response();
        };
        if matches!(
            current_status,
            WorktreeStatus::Compiling | WorktreeStatus::Merged | WorktreeStatus::Discarded
        ) {
            return (
                StatusCode::CONFLICT,
                format!("cannot push to agent branch '{wt_branch}' in {current_status:?} state"),
            )
                .into_response();
        }
        held_exec_locks.push(guard);
    }

    let engine = state.git_engine.clone();
    let automatic_state = state.clone();
    let original_headers = parts.headers;
    let rpc_res = tokio::task::spawn_blocking(move || {
        let _held_repo = repo_guard;
        let _held_exec = held_exec_locks;
        crate::branch_protection::ensure_direct_push_allowed(
            &automatic_state.auth_store,
            automatic_state.repo_id,
            &updates,
        )?;
        let mut intents = Vec::new();
        let mut local_intent = None;
        let mut preparation_error = None;
        let result = engine.smart_http_receive_pack_with_gate(&body, || {
            let authorized = (|| {
                crate::branch_protection::ensure_direct_push_allowed(
                    &automatic_state.auth_store,
                    automatic_state.repo_id,
                    &updates,
                )?;
                // Raw refs are validated before preparation can resolve them or persist intents.
                intents = crate::git_http_automatic::prepare(&automatic_state, &caller, &matched_worktrees, &updates)?;
                match crate::local_checks_automatic::prepare(&automatic_state, &caller, &updates)
                    .map_err(|_| (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "automatic local Checks scheduling is unavailable".to_owned(),
                    ))? {
                    crate::local_checks_automatic::Eligibility::Prepared(intent) => local_intent = Some(intent),
                    crate::local_checks_automatic::Eligibility::ConsentUnproven(reason) => tracing::warn!(
                        ?reason, "automatic local Checks consent could not be established; no execution scheduled"
                    ),
                    crate::local_checks_automatic::Eligibility::Ineligible => {},
                }
                #[cfg(test)]
                if let Some((entered, release)) = automatic_state.automatic_checks.pre_receive_hook.lock().take() {
                    entered.send(()).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "push observer closed".into()))?;
                    release.recv().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "push observer closed".into()))?;
                }
                let fresh = extract_git_http_principal(&original_headers, &automatic_state.auth_store)?;
                if fresh.user_id != caller.user_id || fresh.role != caller.role {
                    return Err((StatusCode::FORBIDDEN, "Git push authority changed".into()));
                }
                check_repo_bola(&automatic_state.auth_store, automatic_state.repo_id, automatic_state.repo_access, &fresh, automatic_state.repo_id)?;
                if automatic_state
                    .read_repository(automatic_state.repo_id)
                    .is_some_and(|r| r.archived)
                {
                    return Err((
                        StatusCode::FORBIDDEN,
                        "repository is archived and read-only".into(),
                    ));
                }
                crate::branch_protection::ensure_direct_push_allowed(
                    &automatic_state.auth_store,
                    automatic_state.repo_id,
                    &updates,
                )?;
                for (wt_id, _, _) in &matched_worktrees {
                    crate::state::set_worktree_status(&automatic_state, *wt_id, |w| {
                        w.status = WorktreeStatus::Pending;
                    });
                }
                Ok(())
            })();
            authorized.map_err(|error| {
                preparation_error = Some(error);
                GitEngineError::InvalidRevision("Git push preparation or authority changed".into())
            })
        }).map_err(map_git_err);
        if let Some(original) = preparation_error {
            crate::git_http_automatic::reject_all(&automatic_state, intents);
            if let Some(intent) = local_intent {
                if crate::local_checks_automatic::reject(&automatic_state, intent).is_err() {
                    tracing::warn!("automatic local Checks preparation recovery is required");
                }
            }
            return Err(original);
        }
        // A rejected ref may accompany HTTP 200, and an RPC error may follow an accepted ref.
        // Keep the Git response authoritative; durable Prepared intents survive inspection failure.
        if let Err((status, _)) = crate::git_http_automatic::reconcile(&automatic_state, intents) {
            tracing::warn!(%status, "push completed but automatic Checks scheduling is unavailable");
        }
        if let Some(intent) = local_intent {
            if crate::local_checks_automatic::confirm(&automatic_state, intent).is_err() {
                tracing::warn!("push completed but automatic local Checks scheduling is unavailable");
            }
        }
        result
    })
    .await;

    // A failed RPC can have changed refs before losing its response. Never restore prior
    // verification from a stale snapshot; every authorized push attempt requires a new run.
    let out = match rpc_res {
        Ok(Ok(bytes)) => bytes,
        Ok(Err((status, message))) => return git_auth_err_response(status, message),
        Err(e) => return internal_err(e).into_response(),
    };

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                SmartHttpService::ReceivePack.result_content_type(),
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        out,
    )
        .into_response()
}
