//! One primary ordinary-branch queue; original Git credentials never enter durable jobs.
use super::{
    local_checks_automatic_model::{History, Job, Phase, Reason},
    local_checks_automatic_store::{Error, Store},
    state::AppState,
};
use gitust_core::{AuthenticatedPrincipal, CommitSha, EnterpriseRole, SandboxRunId};
use gitust_git::{ReceivePackRefUpdate, WorktreeServiceLock};
use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::{
    sync::{watch, Notify},
    task::JoinHandle,
};

#[derive(Default)]
pub(crate) struct Registry {
    store: Mutex<Option<Arc<Store>>>,
    pub unavailable: AtomicBool,
    started: AtomicBool,
    wake: Notify,
}
impl Registry {
    pub fn mark_unavailable(&self) {
        self.unavailable.store(true, Ordering::SeqCst);
    }
    pub fn store(&self, state: &AppState) -> Result<Arc<Store>, Error> {
        identity(state)?;
        let mut slot = self.store.lock();
        if let Some(store) = slot.as_ref() {
            return Ok(store.clone());
        }
        let common = state
            .git_engine
            .repository_common_directory()
            .map_err(|_| Error)?;
        let store = Arc::new(Store::open(&common, state.repo_id)?);
        *slot = Some(store.clone());
        Ok(store)
    }
    pub fn notify(&self) {
        self.wake.notify_one();
    }
}
pub(crate) fn identity(state: &AppState) -> Result<(), Error> {
    if state
        .git_engine
        .load_existing_repository_id()
        .map_err(|_| Error)?
        != state.repo_id
    {
        return Err(Error);
    }
    Ok(())
}
pub(crate) struct Intent {
    pub run_id: SandboxRunId,
    store: Arc<Store>,
}
#[derive(Debug)]
pub(crate) enum ConsentFailure {
    PolicyInvalid,
    PolicyUnavailable,
}
pub(crate) enum Eligibility {
    Ineligible,
    ConsentUnproven(ConsentFailure),
    Prepared(Intent),
}
/// Called by the existing validated receive gate while its mutation/service owner is held.
pub(crate) fn prepare(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    updates: &[ReceivePackRefUpdate],
) -> Result<Eligibility, Error> {
    if !state.trusted_sandbox_exec || caller.role != EnterpriseRole::SiteAdmin {
        return Ok(Eligibility::Ineligible);
    }
    let base = match state.git_engine.committed_default_branch() {
        Ok(base) => base,
        Err(_) => {
            return Ok(Eligibility::ConsentUnproven(
                ConsentFailure::PolicyUnavailable,
            ))
        }
    };
    let source = format!("refs/heads/{base}");
    let Some(update) = updates
        .iter()
        .find(|u| u.refname == source && !u.is_deletion && u.old_oid != u.new_oid)
    else {
        return Ok(Eligibility::Ineligible);
    };
    let actual = match state.git_engine.resolve_checks_ref(&source) {
        Ok(sha) => sha,
        Err(_) => {
            return Ok(Eligibility::ConsentUnproven(
                ConsentFailure::PolicyUnavailable,
            ))
        }
    };
    if actual.0 != update.old_oid {
        return Ok(Eligibility::Ineligible);
    }
    let policy = match state.git_engine.required_checks_at_commit(&actual) {
        Ok(Some(policy)) if policy.auto_run => policy,
        Ok(_) => return Ok(Eligibility::Ineligible),
        Err(gitust_git::GitEngineError::VerificationInvalid(_)) => {
            return Ok(Eligibility::ConsentUnproven(ConsentFailure::PolicyInvalid))
        }
        Err(_) => {
            return Ok(Eligibility::ConsentUnproven(
                ConsentFailure::PolicyUnavailable,
            ))
        }
    };
    let persist = || {
        if state
            .local_checks_automatic
            .unavailable
            .load(Ordering::SeqCst)
            || !state.local_checks_automatic.started.load(Ordering::SeqCst)
        {
            return Err(Error);
        }
        let store = state.local_checks_automatic.store(state)?;
        let job = Job::new(
            caller.user_id,
            source,
            actual,
            CommitSha(update.new_oid.clone()),
            policy.blob_oid,
        );
        let run_id = job.run_id;
        store.update(|doc| {
            if doc.prepared.is_some() {
                return Err(Error);
            }
            doc.prepared = Some(job);
            Ok(())
        })?;
        Ok(Eligibility::Prepared(Intent { run_id, store }))
    };
    persist().inspect_err(|_| state.local_checks_automatic.mark_unavailable())
}
pub(crate) fn reject(state: &AppState, intent: Intent) -> Result<(), Error> {
    let result = intent.store.update(|doc| {
        if doc
            .prepared
            .as_ref()
            .is_none_or(|j| j.run_id != intent.run_id)
        {
            return Err(Error);
        }
        let job = doc.prepared.take().ok_or(Error)?;
        doc.finish(
            job,
            Phase::Rejected,
            Some(Reason::PushNotConfirmed),
            History::NotAttempted,
        );
        Ok(())
    });
    if result.is_err() {
        state.local_checks_automatic.mark_unavailable()
    }
    result
}
/// Post-RPC ref observation is not a recoverable per-request Git receipt.
pub(crate) fn confirm(state: &AppState, intent: Intent) -> Result<(), Error> {
    let result = (|| {
        identity(state)?;
        let job = intent
            .store
            .read()?
            .prepared
            .filter(|j| j.run_id == intent.run_id)
            .ok_or(Error)?;
        let outcome = (|| {
            let base = state
                .git_engine
                .committed_default_branch()
                .map_err(|_| Reason::DefaultChanged)?;
            if format!("refs/heads/{base}") != job.source_ref {
                return Err(Reason::DefaultChanged);
            }
            let actual = state
                .git_engine
                .resolve_checks_ref(&job.source_ref)
                .map_err(|_| Reason::PushNotConfirmed)?;
            if actual != job.expected_commit {
                return Err(if actual == job.previous_commit {
                    Reason::PushNotConfirmed
                } else {
                    Reason::TargetChanged
                });
            }
            let policy = state
                .git_engine
                .required_checks_at_commit(&job.expected_commit)
                .map_err(|_| Reason::PolicyInvalid)?
                .ok_or(Reason::PolicyMissing)?;
            if !policy.auto_run {
                return Err(Reason::PolicyDisabled);
            }
            Ok(policy.blob_oid)
        })();
        intent.store.update(|doc| {
            if doc
                .prepared
                .as_ref()
                .is_none_or(|j| j.run_id != intent.run_id)
            {
                return Err(Error);
            }
            let mut job = doc.prepared.take().ok_or(Error)?;
            match outcome {
                Ok(blob) => {
                    job.checks_blob_oid = Some(blob);
                    doc.queue(job);
                }
                Err(reason) => {
                    let phase = if matches!(reason, Reason::TargetChanged | Reason::DefaultChanged)
                    {
                        Phase::Superseded
                    } else {
                        Phase::Rejected
                    };
                    doc.finish(job, phase, Some(reason), History::NotAttempted);
                }
            }
            Ok(())
        })?;
        identity(state)?;
        Ok(())
    })();
    if result.is_err() {
        state.local_checks_automatic.mark_unavailable()
    } else {
        state.local_checks_automatic.notify()
    }
    result
}
pub(crate) struct Service {
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<Result<(), Error>>>,
    _owner: WorktreeServiceLock,
}
impl Service {
    pub async fn shutdown(mut self) -> Result<(), std::io::Error> {
        self.shutdown.send_replace(true);
        if let Some(task) = self.task.take() {
            task.await
                .map_err(std::io::Error::other)?
                .map_err(std::io::Error::other)?
        }
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}
struct Activity {
    registry: Arc<Registry>,
    finished: bool,
}
impl Drop for Activity {
    fn drop(&mut self) {
        if !self.finished {
            self.registry.mark_unavailable()
        }
        self.registry.started.store(false, Ordering::SeqCst);
    }
}
pub(crate) async fn start(state: &AppState) -> Result<Service, std::io::Error> {
    let registry = state.local_checks_automatic.clone();
    if registry
        .started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(std::io::Error::other(
            "local automatic Checks already started",
        ));
    }
    let activity = Activity {
        registry: registry.clone(),
        finished: false,
    };
    let snapshot = state.clone();
    let (store, owner, activity) = tokio::task::spawn_blocking(move || {
        let activity = activity;
        let owner = snapshot
            .git_engine
            .lock_worktree_service()
            .map_err(|_| Error)?;
        let store = snapshot.local_checks_automatic.store(&snapshot)?;
        super::local_checks_automatic_worker::recover(&snapshot, &store)?;
        Ok::<_, Error>((store, owner, activity))
    })
    .await
    .map_err(std::io::Error::other)?
    .map_err(std::io::Error::other)?;
    registry.unavailable.store(false, Ordering::SeqCst);
    let (shutdown, receiver) = watch::channel(false);
    let snapshot = state.clone();
    let task_owner = owner.clone();
    let task = tokio::spawn(async move {
        let mut activity = activity;
        let _owner = task_owner;
        let result = super::local_checks_automatic_worker::run(snapshot, store, receiver).await;
        if result.is_ok() {
            activity.finished = true
        }
        result
    });
    registry.notify();
    Ok(Service {
        shutdown,
        task: Some(task),
        _owner: owner,
    })
}
pub(super) async fn notified(state: &AppState) {
    state.local_checks_automatic.wake.notified().await
}
