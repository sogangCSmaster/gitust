//! Slice 6 (Phase 4.1): Source-of-Truth Mode with Upstream Backup Mirror and Verified Rollback HTTP Handlers.
//!
//! - `GET /api/v1/repos/{id}/sync/topology`
//! - `PUT /api/v1/repos/{id}/sync/topology`
//! - `POST /api/v1/repos/{id}/sync/backup-push`
//! - `POST /api/v1/repos/{id}/sync/rollback`

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use gitust_core::{AuthError, EnterpriseRole, RepoId};
use gitust_git::GitEngineError;
use gitust_sync::SyncTopologyMode;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{extract_principal, extract_principal_async};
use crate::repository_reads::{id as parse_repo_id, response};
use crate::repository_registry::ReadRepository;
use crate::state::{check_repo_bola, internal_err, map_auth_err, map_git_err, AppState};

type ApiError = (StatusCode, String);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateSyncTopologyPayload {
    mode: SyncTopologyMode,
    #[serde(default)]
    backup_remote_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackSyncSnapshotPayload {
    snapshot_id: String,
}

fn map_sync_git_err(err: GitEngineError) -> ApiError {
    match err {
        GitEngineError::InvalidRemoteUrl(_) => (StatusCode::BAD_REQUEST, err.to_string()),
        GitEngineError::ApprovalState(_) => (StatusCode::CONFLICT, err.to_string()),
        GitEngineError::GitCommand { .. } => (StatusCode::BAD_GATEWAY, err.to_string()),
        other => map_git_err(other),
    }
}

fn verify_writable_repo(state: &AppState, repo_id: RepoId) -> Result<ReadRepository, ApiError> {
    let Some(repository) = state.read_repository(repo_id) else {
        return Err((StatusCode::NOT_FOUND, "repository is not hosted".into()));
    };
    if repository.archived {
        return Err((
            StatusCode::FORBIDDEN,
            "repository is archived and read-only".into(),
        ));
    }
    if !repository.is_default && !repository.git_write {
        return Err((
            StatusCode::FORBIDDEN,
            "repository is mounted read-only".into(),
        ));
    }
    Ok(repository)
}

pub(crate) async fn get_topology(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let result = async {
        let repo_id = parse_repo_id(&raw_id)?;
        let _pre = extract_principal_async(&headers, &state.auth_store).await?;
        let Some(repository) = state.read_repository(repo_id) else {
            return Err((StatusCode::NOT_FOUND, "repository is not hosted".into()));
        };
        let store = state.auth_store.clone();
        let default_url = state.sync_config.github_remote_url.trim().to_owned();
        let topology = tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&headers, &store)?;
            check_repo_bola(
                &store,
                repository.access.repo_id,
                repository.access,
                &caller,
                repo_id,
            )?;
            let loaded = repository
                .engine
                .load_sync_topology(repo_id, &default_url)
                .map_err(map_sync_git_err)?;
            let current = extract_principal(&headers, &store)?;
            if current.user_id != caller.user_id {
                return Err(map_auth_err(AuthError::Unauthenticated));
            }
            check_repo_bola(
                &store,
                repository.access.repo_id,
                repository.access,
                &current,
                repo_id,
            )?;
            Ok::<_, ApiError>(loaded)
        })
        .await
        .map_err(internal_err)??;
        Ok(topology)
    }
    .await;
    response(result)
}

pub(crate) async fn update_topology(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    req: Request,
) -> Response {
    let result = async {
        let repo_id = parse_repo_id(&raw_id)?;
        let (parts, body) = req.into_parts();
        let _pre = extract_principal_async(&parts.headers, &state.auth_store).await?;
        let bytes = axum::body::to_bytes(body, 4096).await.map_err(|_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body too large".into(),
            )
        })?;
        let payload: UpdateSyncTopologyPayload = serde_json::from_slice(&bytes).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid JSON payload: {e}"),
            )
        })?;

        let configured_url = state.sync_config.github_remote_url.trim().to_owned();
        if let Some(req_url) = payload.backup_remote_url.as_deref().map(str::trim) {
            if !req_url.is_empty() && req_url != configured_url {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "backup_remote_url does not match the server's configured mirror URL".into(),
                ));
            }
        }

        let initial_repo = verify_writable_repo(&state, repo_id)?;
        let guard = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            initial_repo.mutation.clone().lock_owned(),
        )
        .await
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository mutation is currently in progress".into(),
            )
        })?;
        let repository = verify_writable_repo(&state, repo_id)?;

        let headers = parts.headers;
        let updated = tokio::task::spawn_blocking(move || {
            let _held = guard;
            let caller = extract_principal(&headers, &state.auth_store)?;
            if caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            check_repo_bola(
                &state.auth_store,
                repository.access.repo_id,
                repository.access,
                &caller,
                repo_id,
            )?;

            let mut denied: Option<ApiError> = None;
            let res = repository.engine.configure_sync_topology_with_gate(
                repo_id,
                payload.mode,
                &configured_url,
                || {
                    let recheck = verify_writable_repo(&state, repo_id).and_then(|current_repo| {
                        let current = extract_principal(&headers, &state.auth_store)?;
                        if current.user_id != caller.user_id
                            || current.role != EnterpriseRole::SiteAdmin
                        {
                            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
                        }
                        check_repo_bola(
                            &state.auth_store,
                            current_repo.access.repo_id,
                            current_repo.access,
                            &current,
                            repo_id,
                        )
                    });
                    if let Err(err) = recheck {
                        denied = Some(err);
                        return Err(GitEngineError::VerificationInvalid(
                            "sync topology authority changed".into(),
                        ));
                    }
                    Ok(())
                },
            );
            if let Some(err) = denied {
                return Err(err);
            }
            res.map_err(map_sync_git_err)
        })
        .await
        .map_err(internal_err)??;
        Ok(updated)
    }
    .await;
    response(result)
}

pub(crate) async fn backup_push(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    req: Request,
) -> Response {
    let result = async {
        let repo_id = parse_repo_id(&raw_id)?;
        let (parts, body) = req.into_parts();
        let _pre = extract_principal_async(&parts.headers, &state.auth_store).await?;
        let _bytes = axum::body::to_bytes(body, 4096).await.map_err(|_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body too large".into(),
            )
        })?;

        let outbound_guard = state
            .outbound_sync_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| {
                (
                    StatusCode::CONFLICT,
                    "another outbound mirror push is currently in progress".into(),
                )
            })?;

        let initial_repo = verify_writable_repo(&state, repo_id)?;
        let prep_guard = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            initial_repo.mutation.clone().lock_owned(),
        )
        .await
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository mutation is currently in progress".into(),
            )
        })?;

        let configured_url = state.sync_config.github_remote_url.trim().to_owned();
        let headers = parts.headers;
        let state_for_prep = state.clone();
        let headers_for_prep = headers.clone();
        let url_for_prep = configured_url.clone();

        // Phase A: capture snapshot and seed staging repository under mutation + service lock, then drop both locks.
        let (prepared, caller, engine_for_push) = tokio::task::spawn_blocking(move || {
            let _held = prep_guard;
            let repository = verify_writable_repo(&state_for_prep, repo_id)?;
            let caller = extract_principal(&headers_for_prep, &state_for_prep.auth_store)?;
            if !matches!(
                caller.role,
                EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
            ) {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            check_repo_bola(
                &state_for_prep.auth_store,
                repository.access.repo_id,
                repository.access,
                &caller,
                repo_id,
            )?;
            let prepared = repository
                .engine
                .prepare_backup_mirror_push(repo_id, &url_for_prep)
                .map_err(map_sync_git_err)?;
            Ok::<_, ApiError>((prepared, caller, repository.engine))
        })
        .await
        .map_err(internal_err)??;

        // Phase B: run git push --prune and git ls-remote under outbound_sync_lock ONLY (no mutation or service lock held).
        let (push_res, prepared, outbound_guard) = tokio::task::spawn_blocking(move || {
            let res = engine_for_push.execute_backup_mirror_push(&prepared);
            (res, prepared, outbound_guard)
        })
        .await
        .map_err(internal_err)?;
        push_res.map_err(map_sync_git_err)?;

        // Phase C: re-acquire mutation + service lock, re-verify caller and repository state via gate, and persist report.
        let fin_repo = verify_writable_repo(&state, repo_id)?;
        let fin_guard = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fin_repo.mutation.clone().lock_owned(),
        )
        .await
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository mutation is currently in progress".into(),
            )
        })?;

        let report = tokio::task::spawn_blocking(move || {
            let _held = fin_guard;
            let _held_outbound = outbound_guard;
            let repository = verify_writable_repo(&state, repo_id)?;
            let mut denied: Option<ApiError> = None;
            let res = repository.engine.finalize_backup_mirror_push_with_gate(
                prepared,
                &configured_url,
                || {
                    let recheck = verify_writable_repo(&state, repo_id).and_then(|current_repo| {
                        let current = extract_principal(&headers, &state.auth_store)?;
                        if current.user_id != caller.user_id
                            || !matches!(
                                current.role,
                                EnterpriseRole::SiteAdmin | EnterpriseRole::Maintainer
                            )
                        {
                            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
                        }
                        check_repo_bola(
                            &state.auth_store,
                            current_repo.access.repo_id,
                            current_repo.access,
                            &current,
                            repo_id,
                        )
                    });
                    if let Err(err) = recheck {
                        denied = Some(err);
                        return Err(GitEngineError::VerificationInvalid(
                            "backup mirror push authority changed".into(),
                        ));
                    }
                    Ok(())
                },
            );
            if let Some(err) = denied {
                return Err(err);
            }
            res.map_err(map_sync_git_err)
        })
        .await
        .map_err(internal_err)??;
        Ok(report)
    }
    .await;
    response(result)
}

pub(crate) async fn rollback(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    req: Request,
) -> Response {
    let result = async {
        let repo_id = parse_repo_id(&raw_id)?;
        let (parts, body) = req.into_parts();
        let _pre = extract_principal_async(&parts.headers, &state.auth_store).await?;
        let bytes = axum::body::to_bytes(body, 4096).await.map_err(|_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body too large".into(),
            )
        })?;
        let payload: RollbackSyncSnapshotPayload = serde_json::from_slice(&bytes).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid JSON payload: {e}"),
            )
        })?;
        let snapshot_id = Uuid::parse_str(payload.snapshot_id.trim())
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid snapshot_id UUID".into()))?;

        let configured_url = state.sync_config.github_remote_url.trim().to_owned();
        let headers = parts.headers;

        // Phase A: ensure all snapshot objects exist locally (fetching under sync_fetch_lock if needed, without holding mutation lock).
        let fetch_guard = state
            .sync_fetch_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| {
                (
                    StatusCode::CONFLICT,
                    "another mirror synchronization fetch is currently in progress".into(),
                )
            })?;
        let state_for_fetch = state.clone();
        let headers_for_fetch = headers.clone();
        let url_for_fetch = configured_url.clone();
        let caller = tokio::task::spawn_blocking(move || {
            let _held_fetch = fetch_guard;
            let repository = verify_writable_repo(&state_for_fetch, repo_id)?;
            let caller = extract_principal(&headers_for_fetch, &state_for_fetch.auth_store)?;
            if caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            check_repo_bola(
                &state_for_fetch.auth_store,
                repository.access.repo_id,
                repository.access,
                &caller,
                repo_id,
            )?;
            repository
                .engine
                .ensure_rollback_snapshot_objects(repo_id, &url_for_fetch, snapshot_id)
                .map_err(map_sync_git_err)?;
            Ok::<_, ApiError>(caller)
        })
        .await
        .map_err(internal_err)??;

        // Phase B: acquire mutation + service lock, re-verify authority & repository state via gate, and atomically apply rollback.
        let initial_repo = verify_writable_repo(&state, repo_id)?;
        let guard = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            initial_repo.mutation.clone().lock_owned(),
        )
        .await
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository mutation is currently in progress".into(),
            )
        })?;

        let report = tokio::task::spawn_blocking(move || {
            let _held = guard;
            let repository = verify_writable_repo(&state, repo_id)?;
            let mut denied: Option<ApiError> = None;
            let res = repository.engine.rollback_sync_snapshot_with_gate(
                repo_id,
                &configured_url,
                snapshot_id,
                || {
                    let recheck = verify_writable_repo(&state, repo_id).and_then(|current_repo| {
                        let current = extract_principal(&headers, &state.auth_store)?;
                        if current.user_id != caller.user_id
                            || current.role != EnterpriseRole::SiteAdmin
                        {
                            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
                        }
                        check_repo_bola(
                            &state.auth_store,
                            current_repo.access.repo_id,
                            current_repo.access,
                            &current,
                            repo_id,
                        )
                    });
                    if let Err(err) = recheck {
                        denied = Some(err);
                        return Err(GitEngineError::VerificationInvalid(
                            "rollback authority changed".into(),
                        ));
                    }
                    Ok(())
                },
            );
            if let Some(err) = denied {
                return Err(err);
            }
            res.map_err(map_sync_git_err)
        })
        .await
        .map_err(internal_err)??;
        Ok(report)
    }
    .await;
    response(result)
}
