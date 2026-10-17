//! Repository-policy jobs survive restart; uncertain execution is never replayed.
use crate::{
    automatic_run_model::{Document, Job, Origin, Phase, Reason},
    automatic_run_store::{Failure, Store},
    state::{check_repo_bola, map_git_err, set_worktree_status, AppState, MAX_TRACKED_WORKTREES},
};
use axum::http::StatusCode;
use gitust_core::{
    AuthenticatedPrincipal, CommitSha, EnterpriseRole, SandboxRunId, WorktreeId, WorktreeStatus,
};
use gitust_git::WorktreeServiceLock;
use parking_lot::Mutex;
#[path = "automatic_run_worker.rs"]
mod worker;

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{runtime::Handle, sync::watch, task::JoinHandle};

#[derive(Default)]
pub(crate) struct AutomaticChecks {
    inner: Mutex<Inner>,
    unavailable: AtomicBool,
    #[cfg(test)]
    pub(crate) startup_hook: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    pub(crate) pre_receive_hook: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
}
#[derive(Default)]
struct Inner {
    generation: u64,
    starting: bool,
    runner: Option<(Handle, watch::Receiver<()>)>,
    store: Option<Arc<Store>>,
    pending: HashMap<WorktreeId, (uuid::Uuid, u64)>,
    local_actors: HashMap<WorktreeId, HashMap<SandboxRunId, AuthenticatedPrincipal>>,
    tasks: Vec<JoinHandle<()>>,
}
impl AutomaticChecks {
    #[cfg(test)]
    pub(crate) fn is_running(&self, id: WorktreeId) -> bool {
        self.inner.lock().pending.contains_key(&id)
    }
    #[cfg(test)]
    pub(crate) fn live_local_actor_count(&self) -> usize {
        self.inner
            .lock()
            .local_actors
            .values()
            .map(HashMap::len)
            .sum()
    }
    #[cfg(test)]
    pub(crate) fn workers_finished(&self) -> bool {
        self.inner.lock().tasks.iter().all(JoinHandle::is_finished)
    }
    pub fn mark_unavailable(&self) {
        self.unavailable.store(true, Ordering::SeqCst);
    }
    pub fn remove_if_initialized(&self, id: WorktreeId) -> Result<(), Failure> {
        let store = self.inner.lock().store.clone();
        if let Some(store) = store {
            store.remove(id)?;
        }
        let mut inner = self.inner.lock();
        inner.local_actors.remove(&id);
        inner.pending.remove(&id);
        Ok(())
    }
    pub fn store(&self) -> Result<Arc<Store>, Failure> {
        self.inner.lock().store.clone().ok_or_else(unavailable)
    }
}
fn unavailable() -> Failure {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "automatic Checks service is unavailable".into(),
    )
}
pub(crate) struct Service {
    registry: Arc<AutomaticChecks>,
    generation: u64,
    _shutdown: watch::Sender<()>,
    _owner: WorktreeServiceLock,
}
impl Service {
    pub async fn shutdown(self) -> Result<(), std::io::Error> {
        let registry = self.registry.clone();
        drop(self);
        let tasks = std::mem::take(&mut registry.inner.lock().tasks);
        Waiting {
            registry,
            tasks: tasks.into(),
        }
        .wait()
        .await
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let mut inner = self.registry.inner.lock();
        if inner.generation == self.generation {
            inner.runner = None;
            inner.pending.clear();
            inner.local_actors.clear();
        }
    }
}
struct Waiting {
    registry: Arc<AutomaticChecks>,
    tasks: std::collections::VecDeque<JoinHandle<()>>,
}
impl Waiting {
    async fn wait(&mut self) -> Result<(), std::io::Error> {
        while let Some(task) = self.tasks.front_mut() {
            task.await.map_err(std::io::Error::other)?;
            self.tasks.pop_front();
        }
        Ok(())
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        self.registry
            .inner
            .lock()
            .tasks
            .extend(self.tasks.drain(..).filter(|task| !task.is_finished()));
    }
}
struct Starting {
    registry: Arc<AutomaticChecks>,
    generation: u64,
}
impl Drop for Starting {
    fn drop(&mut self) {
        let mut inner = self.registry.inner.lock();
        if inner.generation == self.generation {
            inner.starting = false;
        }
    }
}

pub(crate) async fn start(state: &AppState) -> Result<Service, std::io::Error> {
    let (generation, previous) = {
        let mut inner = state.automatic_checks.inner.lock();
        if inner.runner.is_some() || inner.starting {
            return Err(std::io::Error::other(
                "automatic Checks service already started",
            ));
        }
        inner.generation = inner
            .generation
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("automatic Checks generation exhausted"))?;
        inner.starting = true;
        (inner.generation, std::mem::take(&mut inner.tasks))
    };
    let starting = Starting {
        registry: state.automatic_checks.clone(),
        generation,
    };
    let mut previous = Waiting {
        registry: state.automatic_checks.clone(),
        tasks: previous.into(),
    };
    previous.wait().await?;
    let snapshot = state.clone();
    let (store, owner, starting, _recovery) = tokio::task::spawn_blocking(move || {
        let starting = starting;
        #[cfg(test)]
        if let Some((entered, release)) = snapshot.automatic_checks.startup_hook.lock().take() {
            entered
                .send(())
                .map_err(|_| std::io::Error::other("startup observer closed"))?;
            release.recv().map_err(std::io::Error::other)?;
        }
        let owner = snapshot
            .git_engine
            .lock_worktree_service()
            .map_err(|_| std::io::Error::other("repository execution recovery is required"))?;
        let recovery = snapshot.try_begin_automatic_recovery()?;
        let common = snapshot
            .git_engine
            .repository_common_directory()
            .map_err(std::io::Error::other)?;
        let store = Arc::new(
            Store::open(&common, snapshot.repo_id).map_err(|(_, e)| std::io::Error::other(e))?,
        );
        crate::automatic_run_recovery::reconcile(&snapshot, &store)
            .map_err(|(_, e)| std::io::Error::other(e))?;
        Ok::<_, std::io::Error>((store, owner, starting, recovery))
    })
    .await
    .map_err(std::io::Error::other)??;
    let documents = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || store.documents())
            .await
            .map_err(std::io::Error::other)?
            .map_err(|(_, e)| std::io::Error::other(e))?
    };
    let (shutdown, receiver) = watch::channel(());
    {
        let mut inner = state.automatic_checks.inner.lock();
        inner.store = Some(store);
        inner.runner = Some((Handle::current(), receiver));
        state
            .automatic_checks
            .unavailable
            .store(false, Ordering::SeqCst);
    }
    drop(starting);
    let service = Service {
        registry: state.automatic_checks.clone(),
        generation,
        _shutdown: shutdown,
        _owner: owner,
    };
    for doc in documents {
        if doc.queued.is_some() {
            notify(state, doc.worktree_id).map_err(|(_, e)| std::io::Error::other(e))?;
        }
    }
    Ok(service)
}

pub(crate) struct Intent {
    pub id: WorktreeId,
    pub run_id: SandboxRunId,
    pub branch: String,
    pub base: String,
    store: Arc<Store>,
    registry: Arc<AutomaticChecks>,
}
/// Runs within the mutation's blocking closure, before its first ref publication.
pub(crate) fn prepare(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: WorktreeId,
    expected: CommitSha,
    previous: Option<CommitSha>,
    origin: Origin,
) -> Result<Option<Intent>, Failure> {
    if !state.trusted_sandbox_exec || caller.role != EnterpriseRole::SiteAdmin {
        return Ok(None);
    }
    let runtime = state
        .get_worktree_runtime(id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
    if origin != Origin::LocalOperator && runtime.created_by != Some(caller.user_id) {
        return Ok(None);
    }
    let wt = state
        .worktrees
        .read()
        .iter()
        .find(|w| w.id == id)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
    let checks = state
        .git_engine
        .required_checks(&wt.base_branch)
        .map_err(map_git_err)?;
    let Some(checks) = checks.filter(|c| c.auto_run) else {
        return Ok(None);
    };
    if state.automatic_checks.unavailable.load(Ordering::SeqCst) {
        return Err(unavailable());
    }
    let cached = state.automatic_checks.inner.lock().store.clone();
    let store = match cached {
        Some(store) => store,
        None => {
            let common = state
                .git_engine
                .repository_common_directory()
                .map_err(map_git_err)?;
            let store = Arc::new(Store::open(&common, state.repo_id)?);
            let mut inner = state.automatic_checks.inner.lock();
            inner.store.get_or_insert_with(|| store.clone()).clone()
        }
    };
    let job = Job::new(caller.user_id, origin, expected, previous, checks.blob_oid);
    let run_id = job.run_id;
    store.update(id, &wt.branch, &wt.base_branch, |doc| {
        if doc.prepared.is_some() {
            return Err(unavailable());
        }
        doc.prepared = Some(job);
        prune_local(&state.automatic_checks, id, doc);
        Ok(())
    })?;
    if origin == Origin::LocalOperator {
        state
            .automatic_checks
            .inner
            .lock()
            .local_actors
            .entry(id)
            .or_default()
            .insert(run_id, caller.clone());
    }
    Ok(Some(Intent {
        id,
        run_id,
        branch: wt.branch,
        base: wt.base_branch,
        store,
        registry: state.automatic_checks.clone(),
    }))
}
/// Confirms actual ref + clean checkout. A failed index publication never becomes a queued run.
pub(crate) fn confirm(state: &AppState, intent: Intent) -> Result<bool, Failure> {
    let id = intent.id;
    let actual = state
        .git_engine
        .resolve_branch_oid(&intent.branch)
        .map_err(map_git_err)?;
    let clean = state.git_engine.clean_worktree_commit(id, &intent.branch);
    let mut blocked = false;
    let queued = intent
        .store
        .update(id, &intent.branch, &intent.base, |doc| {
            let candidate = doc.prepared.as_ref().ok_or_else(unavailable)?;
            if candidate.run_id != intent.run_id {
                return Err(unavailable());
            }
            if actual.as_deref() == Some(candidate.expected.0.as_str()) {
                if clean.as_ref().is_ok_and(|sha| sha == &candidate.expected) {
                    doc.queue_prepared();
                    prune_local(&intent.registry, id, doc);
                    return Ok(true);
                }
                let candidate = doc.prepared.take().ok_or_else(unavailable)?;
                doc.finish(candidate, Phase::Blocked);
                blocked = true;
            } else {
                let candidate = doc.prepared.take().ok_or_else(unavailable)?;
                let phase =
                    if actual.as_deref() == candidate.previous.as_ref().map(|s| s.0.as_str()) {
                        Phase::NotApplied
                    } else {
                        Phase::Superseded
                    };
                doc.finish(candidate, phase);
            }
            prune_local(&intent.registry, id, doc);
            Ok(false)
        })?;
    if blocked {
        set_worktree_status(state, id, |wt| wt.status = WorktreeStatus::Failed);
    }
    if queued {
        notify(state, id)?;
    }
    Ok(queued)
}
pub(crate) fn reject(intent: Intent) -> Result<(), Failure> {
    intent
        .store
        .update(intent.id, &intent.branch, &intent.base, |doc| {
            if doc
                .prepared
                .as_ref()
                .is_some_and(|j| j.run_id == intent.run_id)
            {
                let job = doc.prepared.take().ok_or_else(unavailable)?;
                doc.finish(job, Phase::Rejected);
            }
            prune_local(&intent.registry, intent.id, doc);
            Ok(())
        })
}
/// Compatibility for already-observed commits and focused scheduler tests.
#[cfg(test)]
pub(crate) fn after_commit(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    id: WorktreeId,
    expected: CommitSha,
    local: bool,
) -> Result<bool, Failure> {
    match prepare(
        state,
        caller,
        id,
        expected,
        None,
        if local {
            Origin::LocalOperator
        } else {
            Origin::Mcp
        },
    )? {
        Some(intent) => confirm(state, intent),
        None => Ok(false),
    }
}
fn prune_local(registry: &AutomaticChecks, id: WorktreeId, doc: &Document) {
    let mut inner = registry.inner.lock();
    if let Some(actors) = inner.local_actors.get_mut(&id) {
        actors.retain(|run, _| {
            doc.prepared
                .iter()
                .chain(doc.queued.iter())
                .chain(doc.running.iter())
                .any(|j| j.run_id == *run)
        });
        if actors.is_empty() {
            inner.local_actors.remove(&id);
        }
    }
}
fn notify(state: &AppState, id: WorktreeId) -> Result<(), Failure> {
    if state.automatic_checks.unavailable.load(Ordering::SeqCst) {
        return Err(unavailable());
    }
    let mut inner = state.automatic_checks.inner.lock();
    let Some((handle, shutdown)) = inner.runner.clone() else {
        return Err(unavailable());
    };
    if let Some((_, revision)) = inner.pending.get_mut(&id) {
        *revision = revision.checked_add(1).ok_or_else(unavailable)?;
        return Ok(());
    }
    if inner.pending.len() >= MAX_TRACKED_WORKTREES {
        return Err(unavailable());
    }
    let token = uuid::Uuid::new_v4();
    inner.pending.insert(id, (token, 0));
    inner.tasks.retain(|t| !t.is_finished());
    let snapshot = state.clone();
    let generation = inner.generation;
    inner.tasks.push(handle.spawn(async move {
        worker::run(snapshot, id, generation, token, shutdown).await;
    }));
    Ok(())
}
