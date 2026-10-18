//! Repository-authorized commit-pinned code & symbol search (`GET /api/v1/repos/{id}/search`, `docs/roadmap.md` §4.5).

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use gitust_git::{GitEngineError, SearchMatchKind};
use serde::Deserialize;

use crate::{
    auth::extract_principal,
    repository_reads::{id, response},
    state::{check_repo_bola, internal_err, map_auth_err, AppState},
};

type ApiError = (StatusCode, String);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySearchQuery {
    pub q: String,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub kind: Option<SearchMatchKind>,
}

fn unavailable(_: impl std::fmt::Display) -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "repository data is unavailable".into(),
    )
}

fn forbidden() -> ApiError {
    (StatusCode::FORBIDDEN, "repository access denied".into())
}

fn map_search_err(error: GitEngineError) -> ApiError {
    match error {
        GitEngineError::InvalidRevision(_)
        | GitEngineError::InvalidRepoPath(_)
        | GitEngineError::InvalidBranchName(_)
        | GitEngineError::BranchNotAgentScoped(_) => (
            StatusCode::BAD_REQUEST,
            "invalid search query or revision".into(),
        ),
        GitEngineError::RevisionNotFound(_) | GitEngineError::PathNotFound(_) => (
            StatusCode::NOT_FOUND,
            "repository revision not found".into(),
        ),
        other => unavailable(other),
    }
}

pub async fn search_repository_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    query: Result<Query<RepositorySearchQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(q)) => q,
        Err(_) => {
            return response::<()>(Err((
                StatusCode::BAD_REQUEST,
                "invalid repository search query".into(),
            )))
        }
    };
    let repository_id = match id(&raw) {
        Ok(repo_id) => repo_id,
        Err(err) => return response::<()>(Err(err)),
    };
    // fail-open-ok: omitted search match kind defaults to All (both symbol and code matches).
    let kind = query.kind.unwrap_or_default();

    let result = tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let caller = extract_principal(&headers, &state.auth_store)?;
        let repository = state
            .read_repository(repository_id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository is not hosted".into()))?;
        let authorize = |principal: &gitust_core::AuthenticatedPrincipal| {
            let allowed = state
                .auth_store
                .authorized_repository_catalog(principal, &[repository.access])
                .map_err(map_auth_err)?;
            if allowed.len() != 1 {
                return Err(forbidden());
            }
            if repository_id == state.repo_id {
                check_repo_bola(
                    &state.auth_store,
                    state.repo_id,
                    state.repo_access,
                    principal,
                    repository_id,
                )?;
            }
            Ok(())
        };
        authorize(&caller)?;

        let _ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(unavailable)?;
        let report = repository
            .engine
            .search_repository(query.rev.as_deref(), &query.q, kind)
            .map_err(map_search_err)?;

        #[cfg(test)]
        crate::repository_reads::after_read(&state);

        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != caller.user_id {
            return Err(forbidden());
        }
        authorize(&current)?;
        Ok(report)
    })
    .await
    .map_err(internal_err)
    .and_then(|res| res);

    response(result)
}
