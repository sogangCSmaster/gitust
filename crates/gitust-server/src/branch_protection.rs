//! Branch protection rule REST handlers (`/api/v1/repos/{id}/branch-protection`)
//! and direct-push protection gate for Smart HTTP and SSH `git-receive-pack`.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use gitust_core::{AuthError, BranchProtectionRule, EnterpriseRole, RepoId};
use gitust_db::AuthStore;
use gitust_git::ReceivePackRefUpdate;
use serde::{Deserialize, Serialize};

use crate::auth::{extract_principal, extract_principal_async};
use crate::repository_reads::{id as parse_repo_id, response};
use crate::state::{internal_err, map_auth_err, AppState};

type ApiError = (StatusCode, String);

#[derive(Serialize)]
struct BranchProtectionResponse {
    rules: Vec<BranchProtectionRule>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum UpdateBranchProtectionPayload {
    Rules { rules: Vec<BranchProtectionRule> },
    SingleWrapped { rule: BranchProtectionRule },
    Single(BranchProtectionRule),
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
) -> Result<BranchProtectionResponse, ApiError> {
    let repo_id = parse_repo_id(&raw)?;
    let origin = extract_principal_async(&headers, &state.auth_store).await?;
    let repository = state
        .read_repository(repo_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::Unauthenticated));
        }
        let rules = state
            .auth_store
            .list_branch_protection_rules(&current, repository.access)
            .map_err(map_auth_err)?;
        Ok(BranchProtectionResponse { rules })
    })
    .await
    .map_err(internal_err)?
}

pub(crate) async fn update(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    response(update_inner(state, raw, request).await)
}

async fn update_inner(
    state: AppState,
    raw: String,
    request: Request,
) -> Result<BranchProtectionResponse, ApiError> {
    let repo_id = parse_repo_id(&raw)?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if origin.role == EnterpriseRole::Developer {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let body = axum::body::to_bytes(body, 256 * 1024).await.map_err(|_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "branch protection request too large".into(),
        )
    })?;
    let repository = state.writable_repository(repo_id).ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "branch protection requires a repository enabled for Git writes".into(),
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
        if current.user_id != origin.user_id || current.role == EnterpriseRole::Developer {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let payload: UpdateBranchProtectionPayload =
            serde_json::from_slice(&body).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid branch protection payload: {e}"),
                )
            })?;
        let rules = match payload {
            UpdateBranchProtectionPayload::Rules { rules } => state
                .auth_store
                .set_branch_protection_rules(&current, repository.access, rules)
                .map_err(map_auth_err)?,
            UpdateBranchProtectionPayload::SingleWrapped { rule }
            | UpdateBranchProtectionPayload::Single(rule) => state
                .auth_store
                .upsert_branch_protection_rule(&current, repository.access, rule)
                .map_err(map_auth_err)?,
        };
        Ok(BranchProtectionResponse { rules })
    })
    .await
    .map_err(internal_err)?
}

/// Rejects direct pushes to any ordinary branch protected by `require_pull_request`
/// or `restrict_direct_pushes`.
pub(crate) fn ensure_direct_push_allowed(
    auth_store: &AuthStore,
    repo_id: RepoId,
    updates: &[ReceivePackRefUpdate],
) -> Result<(), ApiError> {
    for update in updates {
        if update.is_agent_branch {
            continue;
        }
        if let Some(branch) = update.refname.strip_prefix("refs/heads/") {
            let rule = auth_store
                .effective_branch_protection(repo_id, branch)
                .map_err(map_auth_err)?;
            if let Some(rule) = rule {
                if rule.require_pull_request || rule.restrict_direct_pushes {
                    return Err((
                        StatusCode::FORBIDDEN,
                        format!(
                            "direct push to protected branch '{branch}' is forbidden; merge via pull request"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}
