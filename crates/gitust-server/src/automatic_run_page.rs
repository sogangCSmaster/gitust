//! Bounded current-worktree diagnostics. Pages are a live traversal, not global run chronology.
use crate::{
    auth::extract_principal,
    automatic_run_model::Job,
    automatic_run_store::{DiagnosticReadError, Failure},
    state::{check_repo_bola, internal_err, map_auth_err, AppState, MAX_TRACKED_WORKTREES},
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use gitust_core::{
    AgentWorktree, AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoId, WorktreeId,
};
use serde::{Deserialize, Serialize};

const MAX_WORKTREES: usize = 20;
const MAX_WORKTREE_METADATA_BYTES: usize = 32 * 1024;
const CURSOR_BYTES: usize = 76;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}
fn bad_query() -> Failure {
    (
        StatusCode::BAD_REQUEST,
        "invalid automatic-run page query".into(),
    )
}
fn changed_targets() -> Failure {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "automatic-run targets are unavailable".into(),
    )
}
impl PageQuery {
    fn parse(self, repo: RepoId) -> Result<(usize, Option<WorktreeId>), Failure> {
        // fail-open-ok: the absent page size has a fixed small default, with an explicit cap below.
        let limit = self.limit.unwrap_or(10);
        if !(1..=MAX_WORKTREES).contains(&limit) {
            return Err(bad_query());
        }
        let Some(cursor) = self.cursor else {
            return Ok((limit, None));
        };
        if cursor.len() != CURSOR_BYTES {
            return Err(bad_query());
        }
        let parts: Vec<_> = cursor.split('.').collect();
        if parts.len() != 3 || parts[0] != "v1" {
            return Err(bad_query());
        }
        let repo_id = uuid::Uuid::parse_str(parts[1]).map_err(|_| bad_query())?;
        let after = uuid::Uuid::parse_str(parts[2]).map_err(|_| bad_query())?;
        if repo_id.is_nil()
            || after.is_nil()
            || repo_id != repo.0
            || repo_id.to_string() != parts[1]
            || after.to_string() != parts[2]
        {
            return Err(bad_query());
        }
        Ok((limit, Some(WorktreeId(after))))
    }
}
#[derive(Serialize)]
struct WorktreeRuns {
    worktree_id: WorktreeId,
    branch: String,
    base_branch: String,
    runs: Vec<Job>,
}
#[derive(Serialize)]
struct Page {
    repo_id: RepoId,
    order: &'static str,
    scanned_worktrees: usize,
    worktrees: Vec<WorktreeRuns>,
    next_cursor: Option<String>,
}
impl AppState {
    /// Only small ID/reference inventory is sorted under the state lock; at most one page is cloned.
    fn automatic_run_page_targets(
        &self,
        after: Option<WorktreeId>,
        limit: usize,
    ) -> Result<(Vec<AgentWorktree>, bool), Failure> {
        let all = self.worktrees.read();
        if all.len() > MAX_TRACKED_WORKTREES {
            return Err(changed_targets());
        }
        let mut selected: Vec<_> = all
            .iter()
            .filter(|wt| wt.repo_id == self.repo_id)
            .filter(|wt| after.is_none_or(|after| wt.id.0 > after.0))
            .collect();
        selected.sort_unstable_by_key(|wt| wt.id.0);
        if selected.iter().any(|wt| wt.id.0.is_nil())
            || selected.windows(2).any(|rows| rows[0].id == rows[1].id)
        {
            return Err(changed_targets());
        }
        if selected.iter().take(limit).any(|wt| {
            wt.branch.len() > MAX_WORKTREE_METADATA_BYTES
                || wt.base_branch.len() > MAX_WORKTREE_METADATA_BYTES - wt.branch.len()
        }) {
            return Err(changed_targets());
        }
        let more = selected.len() > limit;
        Ok((selected.into_iter().take(limit).cloned().collect(), more))
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestStage {
    Selected,
    Read,
}
#[cfg(test)]
pub(crate) struct TestHook {
    pub stage: TestStage,
    pub entered: tokio::sync::oneshot::Sender<()>,
    pub release: std::sync::mpsc::Receiver<()>,
}
#[cfg(test)]
fn test_pause(state: &AppState, stage: TestStage) -> Result<(), Failure> {
    let hook = {
        let mut hook = state.automatic_run_page_hook.lock();
        if hook.as_ref().is_some_and(|hook| hook.stage == stage) {
            hook.take()
        } else {
            None
        }
    };
    if let Some(hook) = hook {
        hook.entered.send(()).map_err(|_| changed_targets())?;
        hook.release.recv().map_err(internal_err)?;
    }
    Ok(())
}

pub(crate) async fn list(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<impl IntoResponse, Failure> {
    if caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let (limit, after) = query.parse(state.repo_id)?;
    let store = state.automatic_checks.store()?;
    let page = tokio::task::spawn_blocking(move || {
        let (targets, more) = state.automatic_run_page_targets(after, limit)?;
        let scanned_worktrees = targets.len();
        let next_cursor = if more {
            let last = targets.last().ok_or_else(changed_targets)?;
            Some(format!("v1.{}.{}", state.repo_id.0, last.id.0))
        } else {
            None
        };
        #[cfg(test)]
        test_pause(&state, TestStage::Selected)?;
        let mut worktrees = Vec::with_capacity(targets.len());
        for wt in targets {
            let document = match store.diagnostic_document(wt.id, &wt.branch, &wt.base_branch) {
                Ok(document) => document,
                // fail-open-ok: Retired is positive fsynced discard evidence; other read errors fail.
                Err(DiagnosticReadError::Retired) => continue,
                Err(DiagnosticReadError::Storage(error)) => return Err(error),
            };
            let mut runs = match document {
                Some(doc) => doc.records(),
                None => Vec::new(),
            };
            runs.sort_unstable_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| b.run_id.0.cmp(&a.run_id.0))
            });
            worktrees.push(WorktreeRuns {
                worktree_id: wt.id,
                branch: wt.branch,
                base_branch: wt.base_branch,
                runs,
            });
        }
        #[cfg(test)]
        test_pause(&state, TestStage::Read)?;
        // Recheck the original session/PAT, not merely its still-active owning account.
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != caller.user_id || current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        check_repo_bola(&state.auth_store, state.repo_id, &current, state.repo_id)?;
        Ok(Page {
            repo_id: state.repo_id,
            order: "worktree_id_asc",
            scanned_worktrees,
            worktrees,
            next_cursor,
        })
    })
    .await
    .map_err(internal_err)??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(page)))
}
