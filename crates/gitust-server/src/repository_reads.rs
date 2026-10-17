//! UUID-selected object reads. Paths are repository-relative inputs, never host routing authority.
use crate::{
    auth::extract_principal,
    repo::{RepoBlobQuery, RepoCommitsQuery, RepoDiffQuery, RepoTreeQuery},
    state::{internal_err, map_auth_err, AppState},
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use gitust_core::{RepoId, RepositoryRecord};
use gitust_git::{GitEngine, GitEngineError};
use serde::{Deserialize, Serialize};

type ApiError = (StatusCode, String);
#[cfg(test)]
pub(crate) type TestHook = std::sync::Arc<dyn Fn() + Send + Sync>;
#[cfg(test)]
pub(crate) fn after_read(state: &AppState) {
    let hook = state.repository_read_hook.lock().take();
    if let Some(hook) = hook {
        hook();
    }
}

// Backend errors can include host paths; retain failure while exposing only a fixed diagnostic.
fn unavailable(_: impl std::fmt::Display) -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "repository data is unavailable".into(),
    )
}
fn read_error(error: GitEngineError) -> ApiError {
    match error {
        GitEngineError::InvalidRevision(_) | GitEngineError::InvalidRepoPath(_) => (
            StatusCode::BAD_REQUEST,
            "invalid repository revision or path".into(),
        ),
        GitEngineError::RevisionNotFound(_) | GitEngineError::PathNotFound(_) => (
            StatusCode::NOT_FOUND,
            "repository revision or path not found".into(),
        ),
        other => unavailable(other),
    }
}
fn forbidden() -> ApiError {
    (StatusCode::FORBIDDEN, "repository access denied".into())
}
pub(crate) fn response<T: Serialize>(value: Result<T, ApiError>) -> Response {
    let value = value.map_err(|(status, message)| {
        if status.is_server_error() {
            (status, "repository data is unavailable".into())
        } else {
            (status, message)
        }
    });
    let mut response = value.map(Json).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
pub(crate) fn id(raw: &str) -> Result<RepoId, ApiError> {
    let invalid = || (StatusCode::BAD_REQUEST, "invalid repository ID".into());
    let id = uuid::Uuid::parse_str(raw).map_err(|_| invalid())?;
    if id.is_nil() {
        return Err(invalid());
    }
    Ok(RepoId(id))
}

async fn read<T: Send + 'static>(
    state: AppState,
    headers: HeaderMap,
    raw: String,
    operation: impl FnOnce(&GitEngine) -> Result<T, GitEngineError> + Send + 'static,
) -> Result<T, ApiError> {
    let repository_id = id(&raw)?;
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let caller = extract_principal(&headers, &state.auth_store)?;
        let repository = state
            .read_repository(repository_id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
        let authorize = |caller: &gitust_core::AuthenticatedPrincipal| {
            let allowed = state
                .auth_store
                .authorized_repository_catalog(caller, &[repository.access])
                .map_err(map_auth_err)?;
            if allowed.len() != 1 {
                return Err(forbidden());
            }
            Ok(())
        };
        authorize(&caller)?;
        let _ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(unavailable)?;
        let value = operation(&repository.engine).map_err(read_error)?;
        #[cfg(test)]
        after_read(&state);
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != caller.user_id {
            return Err(forbidden());
        }
        authorize(&current)?;
        Ok(value)
    })
    .await
    .map_err(internal_err)?
}

#[derive(Serialize)]
struct Catalog {
    repositories: Vec<CatalogEntry>,
}
#[derive(Serialize)]
struct CatalogEntry {
    id: RepoId,
    name: String,
    branch: String,
    head_commit: Option<gitust_core::CommitSha>,
    capability: &'static str,
    is_default: bool,
    can_rename: bool,
}
fn display_name(engine: &GitEngine, metadata: Option<&RepositoryRecord>) -> String {
    if let Some(metadata) = metadata {
        return metadata.name.clone();
    }
    let path = engine.repo_path();
    let path = if path.file_name().is_some_and(|leaf| leaf == ".git") {
        path.parent()
    } else {
        Some(path)
    };
    // fail-open-ok: the generic label contains no host path and never changes authorization.
    path.and_then(|path| path.file_name())
        .map(|leaf| leaf.to_string_lossy().trim_end_matches(".git").to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Repository".into())
}
pub(crate) async fn catalog(State(state): State<AppState>, headers: HeaderMap) -> Response {
    response(catalog_value(state, headers).await)
}
async fn catalog_value(state: AppState, headers: HeaderMap) -> Result<Catalog, ApiError> {
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let caller = extract_principal(&headers, &state.auth_store)?;
        let contexts = state.read_repositories();
        let expected: Vec<_> = contexts.iter().map(|row| row.access).collect();
        let allowed = state
            .auth_store
            .authorized_repository_catalog(&caller, &expected)
            .map_err(map_auth_err)?;
        let mut repositories = Vec::with_capacity(allowed.len());
        for context in contexts {
            let Some(authorization) = allowed.iter().find(|row| row.id == context.access.repo_id)
            else {
                continue;
            };
            let _ownership = context
                .engine
                .lock_worktree_service()
                .map_err(unavailable)?;
            let (branch_name, head_commit) = context
                .engine
                .inspect_repository_head()
                .map_err(read_error)?
                .into_parts();
            let name = display_name(&context.engine, authorization.metadata.as_ref());
            if branch_name.len() > 32 * 1024 || name.len() > 1024 {
                return Err(unavailable("repository metadata exceeds its limit"));
            }
            repositories.push(CatalogEntry {
                id: context.access.repo_id,
                name,
                branch: branch_name,
                head_commit,
                capability: if context.is_default {
                    "workbench"
                } else {
                    "browse"
                },
                is_default: context.is_default,
                can_rename: authorization.can_rename,
            });
        }
        #[cfg(test)]
        after_read(&state);
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != caller.user_id {
            return Err(forbidden());
        }
        let after = state
            .auth_store
            .authorized_repository_catalog(&current, &expected)
            .map_err(map_auth_err)?;
        if after.iter().any(|row| {
            row.metadata
                .as_ref()
                .is_some_and(|metadata| metadata.name.len() > 1024)
        }) {
            return Err(unavailable("repository metadata exceeds its limit"));
        }
        // Names are presentation metadata; every other field and capability must still match.
        let same_access = allowed.len() == after.len()
            && allowed.iter().zip(&after).all(|(before, after)| {
                let mut before = before.clone();
                if let (Some(before), Some(after)) = (&mut before.metadata, &after.metadata) {
                    before.name.clone_from(&after.name);
                }
                before == *after
            });
        if !same_access {
            return Err(forbidden());
        }
        for entry in &mut repositories {
            let current = after
                .iter()
                .find(|row| row.id == entry.id)
                .ok_or_else(forbidden)?;
            if let Some(metadata) = &current.metadata {
                entry.name.clone_from(&metadata.name);
            }
            entry.can_rename = current.can_rename;
        }
        Ok(Catalog { repositories })
    })
    .await
    .map_err(internal_err)?
}

pub(crate) async fn commits(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(query): Query<RepoCommitsQuery>,
) -> Response {
    // fail-open-ok: omitted limit is bounded; authorization still runs before Git reads.
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    response(
        read(state, headers, raw, move |engine| {
            engine.list_commits(query.rev.as_deref(), limit)
        })
        .await,
    )
}
pub(crate) async fn tree(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(query): Query<RepoTreeQuery>,
) -> Response {
    // fail-open-ok: omitted recursive selects the existing nonrecursive read behavior.
    let recursive = query.recursive.unwrap_or(false);
    response(
        read(state, headers, raw, move |engine| {
            engine.inspect_tree(query.rev.as_deref(), query.path.as_deref(), recursive)
        })
        .await,
    )
}
pub(crate) async fn blob(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(query): Query<RepoBlobQuery>,
) -> Response {
    response(
        read(state, headers, raw, move |engine| {
            engine.read_blob(query.rev.as_deref(), &query.path)
        })
        .await,
    )
}
pub(crate) async fn diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(query): Query<RepoDiffQuery>,
) -> Response {
    response(
        read(state, headers, raw, move |engine| {
            engine.inspect_diff(
                query.base.as_deref(),
                query.head.as_deref(),
                query.path.as_deref(),
            )
        })
        .await,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepoBranchesQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}
pub(crate) async fn branches(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    query: Result<Query<RepoBranchesQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => {
            return response::<()>(Err((
                StatusCode::BAD_REQUEST,
                "invalid branch page query".into(),
            )))
        }
    };
    // fail-open-ok: omitted limit selects the bounded default; explicit invalid values fail closed in the core.
    let limit = query.limit.unwrap_or(50);
    response(
        read(state, headers, raw, move |engine| {
            engine.list_local_branches(limit, query.cursor.as_deref())
        })
        .await,
    )
}
