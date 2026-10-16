use crate::state::{
    check_repo_bola, internal_err, map_git_err, AppState, SpawnLifecycle, SpawnReservation,
    WorktreeRuntime, MAX_PROMPT_SUMMARY_BYTES,
};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use gitust_core::AuthenticatedPrincipal;
use gitust_core::{AgentKind, AgentWorktree, RepoId, WorktreeId};
use gitust_git::GitEngine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SpawnWorktreeRequest {
    pub repo_id: Option<RepoId>,
    pub agent: Option<AgentKind>,
    /// Must be in the `agent/` namespace.
    pub branch: String,
    /// Branch to fork from (default: `main`).
    #[serde(default)]
    pub base_branch: Option<String>,
    pub prompt_summary: String,
}

pub(crate) async fn spawn_worktree_core(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    branch: &str,
    base_branch: String,
    agent: AgentKind,
    prompt_summary: String,
    target_repo_id: RepoId,
) -> Result<(AgentWorktree, PathBuf), (StatusCode, String)> {
    if prompt_summary.len() > MAX_PROMPT_SUMMARY_BYTES {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("prompt_summary exceeds {MAX_PROMPT_SUMMARY_BYTES} bytes"),
        ));
    }
    let clean_branch = GitEngine::validate_agent_branch_name(branch).map_err(map_git_err)?;
    let clean_base = GitEngine::validate_outbound_branch(&base_branch).map_err(map_git_err)?;
    let store = state.auth_store.clone();
    let default_repo = state.repo_id;
    let caller_owned = caller.clone();
    tokio::task::spawn_blocking(move || {
        check_repo_bola(&store, default_repo, &caller_owned, target_repo_id)
    })
    .await
    .map_err(internal_err)??;
    if target_repo_id != state.repo_id {
        return Err((
            StatusCode::NOT_FOUND,
            "repository is not served by this instance".to_owned(),
        ));
    }
    let repo_guard = state
        .repo_mutation_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                "another repository mutation is in progress".into(),
            )
        })?;
    let reservation = SpawnReservation::reserve(state, caller.user_id)?;
    let lifecycle = Arc::clone(&reservation.lifecycle);
    let state_for_spawn = state.clone();
    let user_id = caller.user_id;

    let engine = state.git_engine.clone();
    let (materialized, _repo_guard) = tokio::task::spawn_blocking(move || {
        let res = engine.materialize_agent_worktree(
            WorktreeId::new(),
            target_repo_id,
            agent,
            &clean_branch,
            &clean_base,
            prompt_summary,
        );
        let res = res.and_then(|m| {
            if let Err(err) = crate::worktree_owners::persist(&m, user_id) {
                engine.remove_agent_worktree(
                    &m.worktree_dir,
                    &m.worktree.branch,
                    &m.worktree.base_commit,
                )?;
                return Err(err);
            }
            Ok(m)
        });
        let mut life = lifecycle.lock();
        match (&*life, &res) {
            (SpawnLifecycle::Cancelled, Ok(m)) => {
                drop(life);
                // fail-open-ok: best-effort rollback of materialized worktree when request future was cancelled mid-checkout.
                let _ = engine.remove_agent_worktree(
                    &m.worktree_dir,
                    &m.worktree.branch,
                    &m.worktree.base_commit,
                );
                state_for_spawn.release_in_flight_quota(user_id);
            }
            (SpawnLifecycle::Cancelled, Err(_)) => {
                drop(life);
                state_for_spawn.release_in_flight_quota(user_id);
            }
            (_, Ok(m)) => {
                *life = SpawnLifecycle::Materialized {
                    engine: engine.clone(),
                    worktree_dir: m.worktree_dir.clone(),
                    branch: m.worktree.branch.clone(),
                    base_commit: m.worktree.base_commit.clone(),
                };
            }
            (_, Err(_)) => {
                *life = SpawnLifecycle::Failed;
            }
        }
        (res, repo_guard)
    })
    .await
    .map_err(internal_err)?;
    let materialized = materialized.map_err(map_git_err)?;

    let sandbox_spec =
        crate::state::sandbox_spec_for(&materialized, &state.auth_store).map_err(map_git_err)?;
    let worktree_dir = materialized.worktree_dir;
    let runtime = WorktreeRuntime {
        created_by: Some(caller.user_id),
        sandbox_spec,
        worktree_dir: worktree_dir.clone(),
        exec_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    let wt = materialized.worktree;
    reservation.commit(wt.clone(), runtime);
    Ok((wt, worktree_dir))
}

pub(crate) async fn spawn_worktree_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: axum::http::HeaderMap,
    Json(req): Json<SpawnWorktreeRequest>,
) -> Result<Json<AgentWorktree>, (StatusCode, String)> {
    let caller = crate::auth::extract_principal_async(&headers, &state.auth_store).await?;
    // fail-open-ok: default repository still passes check_repo_bola before any checkout.
    let target_repo_id = req.repo_id.unwrap_or(state.repo_id);
    // fail-open-ok: agent kind is a display classification, not a privilege.
    let agent = req.agent.unwrap_or(AgentKind::ClaudeCode);
    // fail-open-ok: the default base must still pass outbound-branch validation and exist in Git.
    let base_branch = req.base_branch.unwrap_or_else(|| "main".to_owned());
    let (wt, _worktree_dir) = spawn_worktree_core(
        &state,
        &caller,
        &req.branch,
        base_branch,
        agent,
        req.prompt_summary,
        target_repo_id,
    )
    .await?;
    Ok(Json(wt))
}
