//! Repository administration settings (`GET/PATCH /api/v1/repos/{id}/settings`)
//! and online bare repository provisioning (`POST /api/v1/admin/repos`).

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use gitust_core::{AuthError, EnterpriseRole, RepoAccess, RepoId, TeamId};
use gitust_git::GitEngine;
use serde::{Deserialize, Serialize};

use crate::auth::{extract_principal, extract_principal_async};
use crate::repository_reads::{id as parse_repo_id, response};
use crate::repository_registry::{
    persist_repo_settings, ReadRepository, RepoRuntimeSettings, MAX_REPOSITORIES,
};
use crate::state::{internal_err, map_auth_err, map_git_err, AppState};

type ApiError = (StatusCode, String);

const MAX_REPO_DESCRIPTION_BYTES: usize = 2048;

fn default_true() -> bool {
    true
}

#[derive(Serialize)]
pub(crate) struct RepositorySettingsResponse {
    pub(crate) id: RepoId,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    pub(crate) default_branch: String,
    pub(crate) is_private: bool,
    pub(crate) archived: bool,
    pub(crate) workbench_enabled: bool,
    pub(crate) can_administer: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateHostedRepositoryPayload {
    name: String,
    #[serde(default)]
    owner_team_id: Option<TeamId>,
    #[serde(default = "default_true")]
    is_private: bool,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    workbench_enabled: Option<bool>,
}

pub(crate) fn validate_description(raw: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.contains('\0') || trimmed.len() > MAX_REPO_DESCRIPTION_BYTES {
        return Err((
            StatusCode::BAD_REQUEST,
            "repository description must be <= 2048 bytes without NUL".into(),
        ));
    }
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

pub(crate) fn build_settings_response(
    state: &AppState,
    caller: &gitust_core::AuthenticatedPrincipal,
    repository: &ReadRepository,
) -> Result<RepositorySettingsResponse, ApiError> {
    let allowed = state
        .auth_store
        .authorized_repository_catalog(caller, &[repository.access])
        .map_err(map_auth_err)?;
    let Some(auth) = allowed.first() else {
        return Err((StatusCode::FORBIDDEN, "repository access denied".into()));
    };
    let (default_branch, _) = repository
        .engine
        .inspect_repository_head()
        .map_err(map_git_err)?
        .into_parts();
    let name = auth
        .metadata
        .as_ref()
        .map(|m| m.name.clone())
        // fail-open-ok: unregistered legacy primary uses a generic label without exposing host paths.
        .unwrap_or_else(|| "Repository".to_owned());
    let is_private = auth.metadata.as_ref().is_some_and(|m| m.is_private);
    let can_administer = caller.role == EnterpriseRole::SiteAdmin
        || (caller.role == EnterpriseRole::Maintainer && repository.git_write);
    Ok(RepositorySettingsResponse {
        id: repository.access.repo_id,
        name,
        description: repository.description.clone(),
        default_branch,
        is_private,
        archived: repository.archived,
        workbench_enabled: repository.is_default || repository.workbench_enabled,
        can_administer,
    })
}

pub(crate) async fn get_settings(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    headers: HeaderMap,
) -> Response {
    response(get_settings_inner(state, raw, headers).await)
}

async fn get_settings_inner(
    state: AppState,
    raw: String,
    headers: HeaderMap,
) -> Result<RepositorySettingsResponse, ApiError> {
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
        build_settings_response(&state, &current, &repository)
    })
    .await
    .map_err(internal_err)?
}

pub(crate) use crate::repository_settings_update::update_settings;

pub(crate) async fn create_hosted_repo(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    match create_hosted_repo_inner(state, request).await {
        Ok(body) => {
            let mut resp = (StatusCode::CREATED, Json(body)).into_response();
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            resp
        }
        Err(err) => response::<RepositorySettingsResponse>(Err(err)),
    }
}

async fn create_hosted_repo_inner(
    state: AppState,
    request: Request,
) -> Result<RepositorySettingsResponse, ApiError> {
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 64 * 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "create repository payload too large".into(),
        )
    })?;
    let guard = state
        .repo_mutation_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository mutation is in progress".into(),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let current = extract_principal(&parts.headers, &state.auth_store)?;
        if current.user_id != origin.user_id || current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let payload: CreateHostedRepositoryPayload =
            serde_json::from_slice(&body).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid create repository payload: {e}"),
                )
            })?;
        let clean_name =
            gitust_db::validate_repository_name(&payload.name).map_err(map_auth_err)?;
        let default_branch = match payload.default_branch {
            Some(ref b) if !b.trim().is_empty() => {
                GitEngine::validate_outbound_branch(b).map_err(map_git_err)?
            }
            _ => "main".to_owned(),
        };
        let description = validate_description(payload.description)?;
        // fail-open-ok: workbench_enabled defaults to false on newly created secondary repos unless requested by SiteAdmin.
        let workbench_enabled = payload.workbench_enabled.unwrap_or(false);
        if state.read_repositories().len() >= MAX_REPOSITORIES {
            return Err((
                StatusCode::CONFLICT,
                "maximum hosted repositories limit reached".into(),
            ));
        }

        let primary_common = state
            .git_engine
            .repository_common_directory()
            .map_err(map_git_err)?;
        let hosted_root = primary_common.join("gitust-hosted-repos");
        std::fs::create_dir_all(&hosted_root).map_err(internal_err)?;
        let candidate_dir = hosted_root.join(format!("{}.git", uuid::Uuid::new_v4()));

        let engine = GitEngine::new(&candidate_dir);
        let _service_lock = engine
            .initialize_seeded_bare_repository(&default_branch, &clean_name, &current.username)
            .map_err(map_git_err)?;
        let finalize = (|| -> Result<(RepoId, ReadRepository, RepoRuntimeSettings), ApiError> {
            let repo_id = engine.load_or_create_repository_id().map_err(map_git_err)?;
            // fail-open-ok: creates a dedicated team UUID when owner_team_id is omitted by SiteAdmin.
            let owner_team_id = payload.owner_team_id.unwrap_or_default();
            let registered = state
                .auth_store
                .register_local_repository(
                    &current,
                    repo_id,
                    &clean_name,
                    owner_team_id,
                    payload.is_private,
                    None,
                )
                .map_err(map_auth_err)?;
            engine
                .complete_repository_registration(registered.binding)
                .map_err(map_git_err)?;

            let settings = RepoRuntimeSettings {
                archived: false,
                workbench_enabled: Some(workbench_enabled),
                description: description.clone(),
            };
            persist_repo_settings(&engine, &settings).map_err(map_git_err)?;

            let access = RepoAccess {
                repo_id,
                registration: Some(registered.binding),
            };
            let read_repo = ReadRepository {
                engine: engine.clone(),
                access,
                is_default: false,
                git_write: true,
                workbench_enabled,
                archived: false,
                description,
                mutation: Arc::new(tokio::sync::Mutex::new(())),
                local_checks_lock: Arc::new(tokio::sync::Mutex::new(())),
                local_checks_automatic: Arc::new(crate::local_checks_automatic::Registry::default()),
            };
            Ok((repo_id, read_repo, settings))
        })();
        let (repo_id, read_repo, settings) = match finalize {
            Ok(v) => v,
            Err(err) => {
                // fail-open-ok: best-effort cleanup of unreferenced candidate directory on failed creation.
                let _ = std::fs::remove_dir_all(&candidate_dir);
                return Err(err);
            }
        };

        {
            let mut dyn_guard = state.dynamic_registry.write();
            dyn_guard.settings.insert(repo_id, settings);
            dyn_guard.online_entries.insert(repo_id, read_repo.clone());
            dyn_guard.ownership.insert(repo_id, _service_lock);
        }

        build_settings_response(&state, &current, &read_repo)
    })
    .await
    .map_err(internal_err)?
}
