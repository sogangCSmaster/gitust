//! The configured repository's display identity, without exposing its host path.
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use gitust_core::{AuthenticatedPrincipal, CommitSha, RepoId};
use serde::Serialize;

use crate::state::{check_repo_bola, internal_err, map_git_err, AppState};

#[derive(Serialize)]
struct ConnectedRepository {
    id: RepoId,
    name: String,
    branch: String,
    head_commit: CommitSha,
}

pub(crate) async fn handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let summary = tokio::task::spawn_blocking(move || {
        check_repo_bola(&state.auth_store, state.repo_id, &caller, state.repo_id)?;
        let path = state.git_engine.repo_path();
        let path = if path.file_name().is_some_and(|name| name == ".git") {
            path.parent()
        } else {
            Some(path)
        };
        // fail-open-ok: a missing display basename uses a generic label; repository authorization and HEAD validation remain mandatory.
        let name = path
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().trim_end_matches(".git").to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Repository".into());
        let head = state
            .git_engine
            .inspect_head_identity()
            .map_err(map_git_err)?;
        check_repo_bola(&state.auth_store, state.repo_id, &caller, state.repo_id)?;
        Ok::<_, (StatusCode, String)>(ConnectedRepository {
            id: state.repo_id,
            name,
            branch: head.branch_name,
            head_commit: head.head_sha,
        })
    })
    .await
    .map_err(internal_err)??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(summary)))
}
