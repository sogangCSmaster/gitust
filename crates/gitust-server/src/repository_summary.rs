//! The configured repository's display identity, without exposing its host path.
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use gitust_core::{AuthError, CommitSha, RepoId};
use serde::Serialize;

use crate::{
    auth::extract_principal,
    repository_reads::response,
    state::{check_repo_bola, internal_err, map_auth_err, map_git_err, AppState},
};

#[derive(Serialize)]
struct ConnectedRepository {
    id: RepoId,
    name: String,
    branch: String,
    head_commit: CommitSha,
}

pub(crate) async fn handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let task = tokio::task::spawn_blocking(move || {
        let caller = extract_principal(&headers, &state.auth_store)?;
        check_repo_bola(
            &state.auth_store,
            state.repo_id,
            state.repo_access,
            &caller,
            state.repo_id,
        )?;
        let path = state.git_engine.repo_path();
        let path = if path.file_name().is_some_and(|name| name == ".git") {
            path.parent()
        } else {
            Some(path)
        };
        // fail-open-ok: a missing display basename uses a generic label; repository authorization and HEAD validation remain mandatory.
        let mut name = path
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().trim_end_matches(".git").to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Repository".into());
        let head = state
            .git_engine
            .inspect_head_identity()
            .map_err(map_git_err)?;
        #[cfg(test)]
        crate::repository_reads::after_read(&state);
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != caller.user_id {
            return Err(map_auth_err(AuthError::ForbiddenRepositoryAccess));
        }
        let current = state
            .auth_store
            .authorized_repository_catalog(&current, &[state.repo_access])
            .map_err(map_auth_err)?;
        let current = current
            .into_iter()
            .next()
            .ok_or_else(|| map_auth_err(AuthError::ForbiddenRepositoryAccess))?;
        if let Some(metadata) = current.metadata {
            name = metadata.name;
        }
        Ok::<_, (StatusCode, String)>(ConnectedRepository {
            id: state.repo_id,
            name,
            branch: head.branch_name,
            head_commit: head.head_sha,
        })
    })
    .await;
    response(match task {
        Ok(value) => value,
        Err(error) => Err(internal_err(error)),
    })
}
