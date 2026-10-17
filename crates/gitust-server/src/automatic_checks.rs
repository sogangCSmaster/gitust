//! Opt-in checks for server-observed commits, without granting approval authority.
use std::{collections::HashMap, sync::Arc};

use axum::http::StatusCode;
use gitust_core::{AuthenticatedPrincipal, CommitSha, EnterpriseRole, WorktreeId, WorktreeStatus};
use parking_lot::Mutex;
use tokio::{runtime::Handle, sync::watch};

use crate::state::{
    check_repo_bola, map_auth_err, map_git_err, set_worktree_status, AppState,
    MAX_TRACKED_WORKTREES,
};

type Failure = (StatusCode, String);

#[derive(Clone)]
struct Job {
    caller: AuthenticatedPrincipal,
    expected: CommitSha,
    local_operator: bool,
}

#[derive(Default)]
pub(crate) struct AutomaticChecks {
    inner: Mutex<Inner>,
}

impl AutomaticChecks {
    #[cfg(test)]
    pub(crate) fn is_running(&self, id: WorktreeId) -> bool {
        self.inner.lock().pending.contains_key(&id)
    }
}

#[derive(Default)]
struct Inner {
    generation: u64,
    runner: Option<(Handle, watch::Receiver<()>)>,
    pending: HashMap<WorktreeId, Option<Job>>,
}

pub(crate) struct Service {
    registry: Arc<AutomaticChecks>,
    generation: u64,
    _shutdown: watch::Sender<()>,
}

impl Drop for Service {
    fn drop(&mut self) {
        let mut inner = self.registry.inner.lock();
        if inner.generation == self.generation {
            inner.runner = None;
            inner.pending.clear();
        }
    }
}

pub(crate) fn start(state: &AppState) -> Result<Service, std::io::Error> {
    let (shutdown, receiver) = watch::channel(());
    let mut inner = state.automatic_checks.inner.lock();
    if inner.runner.is_some() {
        return Err(std::io::Error::other(
            "automatic Checks service already started",
        ));
    }
    inner.generation = inner
        .generation
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("automatic Checks generation exhausted"))?;
    let generation = inner.generation;
    inner.runner = Some((Handle::current(), receiver));
    Ok(Service {
        registry: state.automatic_checks.clone(),
        generation,
        _shutdown: shutdown,
    })
}

/// Called inside the accepted mutation's blocking closure, so disconnects cannot lose a trigger.
pub(crate) fn after_commit(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: WorktreeId,
    expected: CommitSha,
    local_operator: bool,
) -> Result<bool, Failure> {
    if !state.trusted_sandbox_exec || caller.role != EnterpriseRole::SiteAdmin {
        return Ok(false);
    }
    let runtime = state
        .get_worktree_runtime(id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
    if !local_operator && runtime.created_by != Some(caller.user_id) {
        return Ok(false);
    }
    let wt = state
        .worktrees
        .read()
        .iter()
        .find(|wt| wt.id == id)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
    let checks = state
        .git_engine
        .required_checks(&wt.base_branch)
        .map_err(map_git_err)?;
    if !checks.is_some_and(|checks| checks.auto_run) {
        return Ok(false);
    }
    enqueue(
        state,
        id,
        Job {
            caller: caller.clone(),
            expected,
            local_operator,
        },
    )?;
    Ok(true)
}

fn enqueue(state: &AppState, id: WorktreeId, job: Job) -> Result<(), Failure> {
    let mut inner = state.automatic_checks.inner.lock();
    let Some((handle, shutdown)) = inner.runner.clone() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "automatic Checks service is unavailable".into(),
        ));
    };
    if let Some(pending) = inner.pending.get_mut(&id) {
        *pending = Some(job);
        return Ok(());
    }
    if inner.pending.len() >= MAX_TRACKED_WORKTREES {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "automatic Checks queue is full".into(),
        ));
    }
    inner.pending.insert(id, Some(job));
    let generation = inner.generation;
    let state = state.clone();
    handle.spawn(async move { worker(state, id, generation, shutdown).await });
    Ok(())
}

fn next(state: &AppState, id: WorktreeId, generation: u64) -> Option<Job> {
    let mut inner = state.automatic_checks.inner.lock();
    if inner.generation != generation {
        return None;
    }
    let job = inner.pending.get_mut(&id)?.take();
    if job.is_none() {
        inner.pending.remove(&id);
    }
    job
}

async fn worker(
    state: AppState,
    id: WorktreeId,
    generation: u64,
    mut shutdown: watch::Receiver<()>,
) {
    while let Some(job) = next(&state, id, generation) {
        let Some(runtime) = state.get_worktree_runtime(id) else {
            continue;
        };
        let guard = tokio::select! {
            biased;
            // fail-open-ok: any watch result stops the worker before execution; closed means service shutdown.
            _ = shutdown.changed() => return,
            guard = runtime.exec_lock.clone().lock_owned() => guard,
        };
        let status = state
            .worktrees
            .read()
            .iter()
            .find(|wt| wt.id == id)
            .map(|wt| wt.status);
        // A delayed automatic run must never replace a newer manual result or terminal state.
        if status != Some(WorktreeStatus::Pending) {
            continue;
        }
        let snapshot = state.clone();
        let checked_job = job.clone();
        let allowed = tokio::task::spawn_blocking(move || {
            let caller = snapshot
                .auth_store
                .verify_active_principal(&checked_job.caller)
                .map_err(map_auth_err)?;
            if caller.role != EnterpriseRole::SiteAdmin
                || (!checked_job.local_operator && runtime.created_by != Some(caller.user_id))
            {
                return Err((
                    StatusCode::FORBIDDEN,
                    "automatic Checks authority changed".into(),
                ));
            }
            check_repo_bola(
                &snapshot.auth_store,
                snapshot.repo_id,
                &caller,
                snapshot.repo_id,
            )?;
            let wt = snapshot
                .worktrees
                .read()
                .iter()
                .find(|wt| wt.id == id)
                .cloned()
                .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
            // Coalesced updates can supersede a job while it waits for the worktree lock.
            if snapshot
                .git_engine
                .clean_worktree_commit(id, &wt.branch)
                .map_err(map_git_err)?
                != checked_job.expected
            {
                return Ok(None);
            }
            Ok(Some(caller))
        })
        .await;
        let caller = match allowed {
            Ok(Ok(Some(caller))) => caller,
            Ok(Ok(None)) => continue,
            _ => {
                set_worktree_status(&state, id, |wt| wt.status = WorktreeStatus::Failed);
                tracing::warn!(worktree_id = %id.0, "automatic Checks authority unavailable");
                continue;
            }
        };
        let result = tokio::select! {
            biased;
            // fail-open-ok: any watch result cancels execution; RunStatusGuard clears approval authority.
            _ = shutdown.changed() => return,
            result = crate::worktrees::execution::automatic(&state, &caller, id.0, job.expected, guard) => result,
        };
        if let Err((status, _)) = result {
            tracing::warn!(worktree_id = %id.0, %status, "automatic Checks did not complete");
        }
    }
}
