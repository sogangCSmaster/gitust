//! Validated repository settings updates across auth metadata, Git HEAD and runtime settings.
use crate::auth::{extract_principal, extract_principal_async};
use crate::repository_admin::{
    build_settings_response, validate_description, RepositorySettingsResponse,
};
use crate::repository_reads::{id as parse_repo_id, response};
use crate::repository_registry::{load_persisted_repo_settings, persist_repo_settings_with_gate};
use crate::state::{internal_err, map_auth_err, map_git_err, AppState};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use gitust_core::{AuthError, EnterpriseRole};
use gitust_git::GitEngine;
use serde::Deserialize;
type ApiError = (StatusCode, String);

fn deserialize_optional_nullable_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRepositorySettingsPayload {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    description: Option<Option<String>>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    is_private: Option<bool>,
    #[serde(default)]
    archived: Option<bool>,
    #[serde(default)]
    workbench_enabled: Option<bool>,
}

fn map_visibility_err(error: gitust_db::RepositoryVisibilityError) -> ApiError {
    match error {
        gitust_db::RepositoryVisibilityError::Auth(err) => map_auth_err(err),
        gitust_db::RepositoryVisibilityError::Conflict => (
            StatusCode::CONFLICT,
            "repository visibility changed; refresh before retrying".into(),
        ),
        gitust_db::RepositoryVisibilityError::NotPublished(_)
        | gitust_db::RepositoryVisibilityError::Unconfirmed(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "repository visibility update could not be confirmed".into(),
        ),
    }
}

pub(crate) async fn update_settings(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    response(update_settings_inner(state, raw, request).await)
}

async fn update_settings_inner(
    state: AppState,
    raw: String,
    request: Request,
) -> Result<RepositorySettingsResponse, ApiError> {
    let repo_id = parse_repo_id(&raw)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if origin.role == EnterpriseRole::Developer {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 64 * 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "repository settings payload too large".into(),
        )
    })?;
    let repository = state
        .read_repository(repo_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
    if !repository.git_write && !repository.is_default {
        return Err((
            StatusCode::FORBIDDEN,
            "read-only secondary repository settings cannot be mutated".into(),
        ));
    }
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
        if current.user_id != origin.user_id || current.role == EnterpriseRole::Developer {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let payload: UpdateRepositorySettingsPayload =
            serde_json::from_slice(&body).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid repository settings payload: {e}"),
                )
            })?;
        let requires_site_admin = payload.name.is_some()
            || payload.is_private.is_some()
            || payload.workbench_enabled.is_some()
            || payload.default_branch.is_some();
        if requires_site_admin && current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }

        let before = build_settings_response(&state, &current, &repository)?;
        // fail-open-ok: preserves current archived setting when omitted from PATCH payload.
        let effective_archived = payload.archived.unwrap_or(before.archived);
        if effective_archived && (payload.name.is_some() || payload.default_branch.is_some()) {
            return Err((
                StatusCode::FORBIDDEN,
                "repository is archived and read-only".into(),
            ));
        }

        let clean_name = payload.name.as_deref()
            .map(gitust_db::validate_repository_name)
            .transpose()
            .map_err(map_auth_err)?;
        let clean_branch = payload.default_branch.as_deref()
            .map(GitEngine::validate_outbound_branch)
            .transpose()
            .map_err(map_git_err)?;
        let branch_update = match clean_branch {
            Some(branch) if branch != before.default_branch => {
                let (_, target_sha) = repository.engine
                    .resolve_branch_pair_shas(&before.default_branch, &branch).map_err(map_git_err)?;
                Some((branch, target_sha))
            }
            _ => None,
        };
        let cached_settings = state
            .dynamic_registry
            .read()
            .settings
            .get(&repo_id)
            .cloned();
        let mut settings = match cached_settings {
            Some(s) => s,
            None => load_persisted_repo_settings(&repository.engine).map_err(map_git_err)?,
        };
        if let Some(archived) = payload.archived {
            settings.archived = archived;
        }
        if let Some(wb) = payload.workbench_enabled {
            settings.workbench_enabled = Some(repository.is_default || wb);
        }
        if let Some(raw_desc) = payload.description {
            settings.description = validate_description(raw_desc)?;
        }
        // These stores have separate durability boundaries. Once a write is attempted,
        // failures must tell the caller to refresh rather than imply an unchanged PATCH.
        let mut may_have_changed = false;
        let apply = (|| {
            if let Some(clean_name) = clean_name {
                if clean_name != before.name {
                    may_have_changed = true;
                    #[cfg(test)]
                    crate::repository_reads::after_read(&state);
                    state.auth_store.rename_registered_repository_with_credential(
                        crate::auth::bearer_token(&parts.headers)?, origin.user_id,
                        repository.access, &clean_name,
                    ).map_err(map_auth_err)?;
                }
            }

            if let Some(next_private) = payload.is_private {
                if next_private != before.is_private {
                    may_have_changed = true;
                    state
                        .auth_store
                        .change_registered_repository_visibility(
                            crate::auth::bearer_token(&parts.headers)?,
                            current.user_id,
                            repository.access,
                            before.is_private,
                            next_private,
                        )
                        .map_err(map_visibility_err)?;
                }
            }

            if let Some((clean_branch, target_sha)) = branch_update {
                let target_ref = format!("refs/heads/{clean_branch}");
                let current_ref = format!("refs/heads/{}", before.default_branch);
                let mut denied = None;
                may_have_changed = true;
                let changed = repository.engine.change_default_branch_with_gate(
                    &target_ref,
                    &current_ref,
                    &target_sha,
                    || {
                        let check = (|| {
                            let fresh = extract_principal(&parts.headers, &state.auth_store)?;
                            if fresh.user_id != origin.user_id
                                || fresh.role != EnterpriseRole::SiteAdmin
                            {
                                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
                            }
                            let rows = state
                                .auth_store
                                .authorized_repository_catalog(&fresh, &[repository.access])
                                .map_err(map_auth_err)?;
                            if rows.len() != 1 || !rows[0].can_rename {
                                return Err(map_auth_err(AuthError::ForbiddenRepositoryAccess));
                            }
                            Ok(())
                        })();
                        if let Err(err) = check {
                            denied = Some(err);
                            return Err(gitust_git::GitEngineError::DefaultBranchUnsupported);
                        }
                        Ok(())
                    },
                );
                if let Some(err) = denied {
                    return Err(err);
                }
                changed.map_err(map_git_err)?;
            }

            may_have_changed = true;
            let mut denied = None;
            let published = persist_repo_settings_with_gate(&repository.engine, &settings, || {
                #[cfg(test)]
                crate::repository_reads::after_read(&state);
                let authorize = (|| {
                    let fresh = extract_principal(&parts.headers, &state.auth_store)?;
                    if fresh.user_id != origin.user_id || fresh.role == EnterpriseRole::Developer
                        || (requires_site_admin && fresh.role != EnterpriseRole::SiteAdmin) {
                        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
                    }
                    let allowed = state.auth_store
                        .authorized_repository_catalog(&fresh, &[repository.access]).map_err(map_auth_err)?;
                    if allowed.len() != 1 {
                        return Err(map_auth_err(AuthError::ForbiddenRepositoryAccess));
                    }
                    Ok(())
                })();
                if let Err(error) = authorize {
                    denied = Some(error);
                    return Err(gitust_git::GitEngineError::RepositoryIdentity(
                        "repository settings authority changed".into()));
                }
                Ok(())
            });
            if let Err(failure) = &published {
                if failure.published {
                    state.dynamic_registry.write().settings.insert(repo_id, settings.clone());
                }
            }
            if let Some(error) = denied { return Err(error); }
            published.map_err(|failure| map_git_err(failure.error))?;
            state
                .dynamic_registry
                .write()
                .settings
                .insert(repo_id, settings);

            let refreshed = state
                .read_repository(repo_id)
                .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
            build_settings_response(&state, &current, &refreshed)
        })();
        apply.map_err(|(status, detail)| {
            if may_have_changed {
                (status, format!(
                    "repository settings may be partially applied; refresh before retrying: {detail}"
                ))
            } else {
                (status, detail)
            }
        })
    })
    .await
    .map_err(internal_err)?
}
