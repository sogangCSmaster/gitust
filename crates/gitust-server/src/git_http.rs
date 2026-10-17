//! Smart HTTP Git Hosting (`/git/{repo}/info/refs`, `/git/{repo}/git-upload-pack`,
//! `/git/{repo}/git-receive-pack`) with PAT/Session Basic & Bearer authentication,
//! pre/post-body Slow-Loris TOCTOU re-verification, Team Repository BOLA, and
//! role-based `refs/heads/agent/*` vs protected branch push enforcement.

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoId, WorktreeStatus};
use gitust_db::AuthStore;
use gitust_git::{inspect_receive_pack_updates, SmartHttpService, MAX_SMART_HTTP_BODY_BYTES};
use serde::Deserialize;
use uuid::Uuid;

use crate::state::{check_repo_bola, internal_err, map_auth_err, map_git_err, AppState};

#[derive(Debug, Deserialize)]
pub struct InfoRefsQuery {
    pub service: Option<String>,
}

fn decode_standard_base64(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity((trimmed.len() / 4) * 3);
    let bytes = trimmed.as_bytes();
    for chunk in bytes.chunks_exact(4) {
        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        for (i, &b) in chunk.iter().enumerate() {
            vals[i] = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if i >= 2 => {
                    pad += 1;
                    0
                }
                _ => return None,
            };
        }
        if pad == 1 && chunk[3] != b'=' {
            return None;
        }
        out.push((vals[0] << 2) | (vals[1] >> 4));
        if pad < 2 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if pad == 0 {
            out.push((vals[2] << 6) | vals[3]);
        }
    }
    Some(out)
}

fn extract_git_credentials(
    headers: &HeaderMap,
) -> Result<(Option<String>, String), (StatusCode, String)> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Missing Authorization header (Basic or Bearer PAT required)".to_owned(),
            )
        })?
        .to_str()
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Authorization header is not valid ASCII".to_owned(),
            )
        })?
        .trim();

    if let Some(bearer) = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
    {
        let tok = bearer.trim();
        if tok.is_empty() {
            return Err((StatusCode::UNAUTHORIZED, "Empty Bearer token".to_owned()));
        }
        return Ok((None, tok.to_owned()));
    }

    if let Some(b64) = raw
        .strip_prefix("Basic ")
        .or_else(|| raw.strip_prefix("basic "))
    {
        let decoded = decode_standard_base64(b64).ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Malformed Basic authorization base64".to_owned(),
            )
        })?;
        let cred_str = String::from_utf8(decoded).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Basic authorization is not valid UTF-8".to_owned(),
            )
        })?;
        let (user_part, pass_part) = match cred_str.split_once(':') {
            Some((u, p)) => (u.trim(), p.trim()),
            None => ("", cred_str.trim()),
        };
        let token = if !pass_part.is_empty() {
            pass_part
        } else if !user_part.is_empty() {
            user_part
        } else {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Empty Basic authorization credentials".to_owned(),
            ));
        };
        let explicit_user = if !pass_part.is_empty()
            && !user_part.is_empty()
            && !matches!(
                user_part.to_ascii_lowercase().as_str(),
                "x-access-token" | "git" | "token" | "oauth2" | "pat"
            ) {
            Some(user_part.to_owned())
        } else {
            None
        };
        return Ok((explicit_user, token.to_owned()));
    }

    Err((
        StatusCode::UNAUTHORIZED,
        "Unsupported Authorization scheme (expected Basic or Bearer)".to_owned(),
    ))
}

pub(crate) fn extract_git_http_principal(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    let (explicit_user, token) = extract_git_credentials(headers)?;
    let principal = if token.starts_with("gitust_pat_") {
        auth_store.authenticate_pat(&token)
    } else {
        auth_store.authenticate_session(&token)
    }
    .map_err(map_auth_err)?;
    if explicit_user
        .as_ref()
        .is_some_and(|expected| !principal.username.eq_ignore_ascii_case(expected))
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Basic username does not match authenticated token owner".into(),
        ));
    }
    Ok(principal)
}
pub(crate) async fn extract_git_http_principal_async(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, Response> {
    let headers = headers.clone();
    let store = auth_store.clone();
    tokio::task::spawn_blocking(move || extract_git_http_principal(&headers, &store))
        .await
        .map_err(|e| internal_err(e).into_response())?
        .map_err(|(status, message)| git_auth_err_response(status, message))
}

fn git_auth_err_response(status: StatusCode, msg: String) -> Response {
    if status == StatusCode::UNAUTHORIZED {
        (
            status,
            [(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"Gitust On-Premise Git\"",
            )],
            msg,
        )
            .into_response()
    } else {
        (status, msg).into_response()
    }
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

    let store = state.auth_store.clone();
    let default_repo = state.repo_id;
    let caller_owned = caller.clone();
    tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, default_repo, &caller_owned, target_repo_id)
    })
    .await
    .map_err(|e| internal_err(e).into_response())?
    .map_err(IntoResponse::into_response)?;

    if target_repo_id != state.repo_id {
        return Err((
            StatusCode::NOT_FOUND,
            "repository is not served by this instance".to_owned(),
        )
            .into_response());
    }

    Ok(target_repo_id)
}

/// `GET /git/{repo}/info/refs?service=git-upload-pack|git-receive-pack`
pub(crate) async fn git_info_refs_handler(
    State(state): State<AppState>,
    Path(repo): Path<String>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Response {
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
    let Some(service) = SmartHttpService::parse(raw_service) else {
        return (
            StatusCode::FORBIDDEN,
            format!("unsupported git service '{raw_service}'"),
        )
            .into_response();
    };

    let engine = state.git_engine.clone();
    let payload = match tokio::task::spawn_blocking(move || {
        engine.smart_http_advertise_refs(service)
    })
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => return map_git_err(e).into_response(),
        Err(e) => return internal_err(e).into_response(),
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, service.advertisement_content_type()),
            (
                header::CACHE_CONTROL,
                "no-cache, max-age=0, must-revalidate",
            ),
        ],
        payload,
    )
        .into_response()
}

/// `POST /git/{repo}/git-upload-pack`
pub(crate) async fn git_upload_pack_handler(
    State(state): State<AppState>,
    Path(repo): Path<String>,
    req: Request,
) -> Response {
    let (parts, request_body) = req.into_parts();
    let pre_caller = match extract_git_http_principal_async(&parts.headers, &state.auth_store).await
    {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(resp) = resolve_and_authorize_repo(&state, &pre_caller, &repo).await {
        return resp;
    }

    // fail-open-ok: oversized upload-pack body immediately fails closed with 413 PAYLOAD_TOO_LARGE.
    let Ok(body) = axum::body::to_bytes(request_body, MAX_SMART_HTTP_BODY_BYTES).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("upload-pack body exceeds {MAX_SMART_HTTP_BODY_BYTES} bytes"),
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

    let engine = state.git_engine.clone();
    let out = match tokio::task::spawn_blocking(move || engine.smart_http_upload_pack(&body)).await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => return map_git_err(e).into_response(),
        Err(e) => return internal_err(e).into_response(),
    };

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                SmartHttpService::UploadPack.result_content_type(),
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        out,
    )
        .into_response()
}

/// `POST /git/{repo}/git-receive-pack`
pub(crate) async fn git_receive_pack_handler(
    State(state): State<AppState>,
    Path(repo): Path<String>,
    req: Request,
) -> Response {
    let (parts, request_body) = req.into_parts();
    let pre_caller = match extract_git_http_principal_async(&parts.headers, &state.auth_store).await
    {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(resp) = resolve_and_authorize_repo(&state, &pre_caller, &repo).await {
        return resp;
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
                if let Some(wt) = worktrees.iter().find(|w| w.branch == short) {
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
        let runtime = state.get_worktree_runtime(*wt_id);
        if caller.role == EnterpriseRole::Developer
            && runtime
                .as_ref()
                .is_none_or(|r| r.created_by != Some(caller.user_id))
        {
            return map_auth_err(AuthError::ForbiddenResourceOwnership).into_response();
        }
        if let Some(rt) = runtime {
            match rt.exec_lock.clone().try_lock_owned() {
                Ok(g) => held_exec_locks.push(g),
                Err(_) => {
                    return (
                        StatusCode::CONFLICT,
                        format!("worktree for branch '{wt_branch}' is currently busy"),
                    )
                        .into_response();
                }
            }
        }
    }

    for (wt_id, _, _) in &matched_worktrees {
        crate::state::set_worktree_status(&state, *wt_id, |w| {
            w.status = WorktreeStatus::Pending;
        });
    }

    let engine = state.git_engine.clone();
    let automatic_state = state.clone();
    let original_headers = parts.headers;
    let rpc_res = tokio::task::spawn_blocking(move || {
        let _held_repo = repo_guard;
        let _held_exec = held_exec_locks;
        let intents = crate::git_http_automatic::prepare(&automatic_state, &caller, &matched_worktrees, &updates)?;
        #[cfg(test)]
        if let Some((entered, release)) = automatic_state.automatic_checks.pre_receive_hook.lock().take() {
            entered.send(()).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "push observer closed".into()))?;
            release.recv().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "push observer closed".into()))?;
        }
        let authorized = (|| {
            let fresh = extract_git_http_principal(&original_headers, &automatic_state.auth_store)?;
            if fresh.user_id != caller.user_id || fresh.role != caller.role {
                return Err((StatusCode::FORBIDDEN, "Git push authority changed".into()));
            }
            check_repo_bola(&automatic_state.auth_store, automatic_state.repo_id, &fresh, automatic_state.repo_id)
        })();
        if let Err(original) = authorized {
            crate::git_http_automatic::reject_all(&automatic_state, intents);
            return Err(original);
        }
        let result = engine.smart_http_receive_pack(&body).map_err(map_git_err);
        // A rejected ref may accompany HTTP 200, and an RPC error may follow an accepted ref.
        // Keep the Git response authoritative; durable Prepared intents survive inspection failure.
        if let Err((status, _)) = crate::git_http_automatic::reconcile(&automatic_state, intents) {
            tracing::warn!(%status, "push completed but automatic Checks scheduling is unavailable");
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
