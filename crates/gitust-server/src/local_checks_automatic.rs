//! Ordinary-branch automatic Checks queue per writable repository; credentials never enter durable jobs.
use super::{
    local_checks_automatic_model::{History, Job, Phase, Reason},
    local_checks_automatic_store::{Error, Store},
    repository_registry::ReadRepository,
    state::AppState,
};
use gitust_core::{AuthenticatedPrincipal, CommitSha, EnterpriseRole, SandboxRunId};
use gitust_git::{GitEngine, GitEngineError, ReceivePackRefUpdate, WorktreeServiceLock};
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
    pub fn store_for_repo(&self, repo: &ReadRepository) -> Result<Arc<Store>, Error> {
        identity_for_repo(repo)?;
        let mut slot = self.store.lock();
        if let Some(store) = slot.as_ref() {
            return Ok(store.clone());
        }
        let common = repo
            .engine
            .repository_common_directory()
            .map_err(|_| Error)?;
        let store = Arc::new(Store::open(&common, repo.access.repo_id)?);
        *slot = Some(store.clone());
        Ok(store)
    }
    #[cfg(test)]
    pub fn store(&self, state: &AppState) -> Result<Arc<Store>, Error> {
        let repo = state.writable_repository(state.repo_id).ok_or(Error)?;
        self.store_for_repo(&repo)
    }
    pub fn notify(&self) {
        self.wake.notify_one();
    }
}
pub(crate) fn identity_for_repo(repo: &ReadRepository) -> Result<(), Error> {
    if repo
        .engine
        .load_existing_repository_id()
        .map_err(|_| Error)?
        != repo.access.repo_id
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
    let repo = state.writable_repository(state.repo_id).ok_or(Error)?;
    prepare_for_repo(state, &repo, caller, updates)
}
pub(crate) fn prepare_for_repo(
    state: &AppState,
    repo: &ReadRepository,
    caller: &AuthenticatedPrincipal,
    updates: &[ReceivePackRefUpdate],
) -> Result<Eligibility, Error> {
    if !state.trusted_sandbox_exec
        || caller.role != EnterpriseRole::SiteAdmin
        || !(repo.is_default || repo.git_write)
    {
        return Ok(Eligibility::Ineligible);
    }
    let base = match repo.engine.committed_default_branch() {
        Ok(base) => base,
        Err(_) => {
            return Ok(Eligibility::ConsentUnproven(
                ConsentFailure::PolicyUnavailable,
            ))
        }
    };
    let mut selected = None;
    for update in updates {
        if update.is_agent_branch || update.is_deletion || update.old_oid == update.new_oid {
            continue;
        }
        let Some(short_branch) = update.refname.strip_prefix("refs/heads/") else {
            continue;
        };
        if !GitEngine::validate_outbound_branch(short_branch)
            .is_ok_and(|clean| clean == short_branch)
        {
            continue;
        }
        let actual = if update.old_oid.bytes().all(|b| b == b'0') {
            if base == short_branch {
                continue;
            }
            match repo.engine.resolve_checks_ref(&update.refname) {
                Err(GitEngineError::RevisionNotFound(_)) => {}
                Ok(_) => continue,
                Err(_) => {
                    return Ok(Eligibility::ConsentUnproven(
                        ConsentFailure::PolicyUnavailable,
                    ))
                }
            }
            match repo
                .engine
                .resolve_checks_ref(&format!("refs/heads/{base}"))
            {
                Ok(sha) => sha,
                Err(_) => {
                    return Ok(Eligibility::ConsentUnproven(
                        ConsentFailure::PolicyUnavailable,
                    ))
                }
            }
        } else {
            match repo.engine.resolve_checks_ref(&update.refname) {
                Ok(sha) if sha.0 == update.old_oid => sha,
                Ok(_) | Err(GitEngineError::RevisionNotFound(_)) => continue,
                Err(_) => {
                    return Ok(Eligibility::ConsentUnproven(
                        ConsentFailure::PolicyUnavailable,
                    ))
                }
            }
        };
        if actual.0 == update.new_oid {
            continue;
        }
        let policy = match repo.engine.required_checks_at_commit(&actual) {
            Ok(Some(policy))
                if (short_branch == base || policy.branches.is_some())
                    && policy.allows_auto_run_for_branch(short_branch) =>
            {
                policy
            }
            Ok(_) => continue,
            Err(GitEngineError::VerificationInvalid(_)) => {
                return Ok(Eligibility::ConsentUnproven(ConsentFailure::PolicyInvalid))
            }
            Err(_) => {
                return Ok(Eligibility::ConsentUnproven(
                    ConsentFailure::PolicyUnavailable,
                ))
            }
        };
        selected = Some((
            update.refname.clone(),
            actual,
            update.new_oid.clone(),
            policy,
        ));
        break;
    }
    let Some((source, actual, new_oid, policy)) = selected else {
        return Ok(Eligibility::Ineligible);
    };
    let persist = || {
        if repo
            .local_checks_automatic
            .unavailable
            .load(Ordering::SeqCst)
            || !repo.local_checks_automatic.started.load(Ordering::SeqCst)
        {
            return Err(Error);
        }
        let store = repo.local_checks_automatic.store_for_repo(repo)?;
        let job = Job::new(
            caller.user_id,
            source,
            actual,
            CommitSha(new_oid),
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
    persist().inspect_err(|_| repo.local_checks_automatic.mark_unavailable())
}
pub(crate) fn reject(state: &AppState, intent: Intent) -> Result<(), Error> {
    let repo = state.writable_repository(state.repo_id).ok_or(Error)?;
    reject_for_repo(&repo, intent)
}
pub(crate) fn reject_for_repo(repo: &ReadRepository, intent: Intent) -> Result<(), Error> {
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
        repo.local_checks_automatic.mark_unavailable()
    }
    result
}
/// Post-RPC ref observation is not a recoverable per-request Git receipt.
pub(crate) fn confirm(state: &AppState, intent: Intent) -> Result<(), Error> {
    let repo = state.writable_repository(state.repo_id).ok_or(Error)?;
    confirm_for_repo(&repo, intent)
}
pub(crate) fn confirm_for_repo(repo: &ReadRepository, intent: Intent) -> Result<(), Error> {
    let result = (|| {
        identity_for_repo(repo)?;
        let job = intent
            .store
            .read()?
            .prepared
            .filter(|j| j.run_id == intent.run_id)
            .ok_or(Error)?;
        let outcome = (|| {
            let base = repo
                .engine
                .committed_default_branch()
                .map_err(|_| Reason::DefaultChanged)?;
            let short_branch = job
                .source_ref
                .strip_prefix("refs/heads/")
                .ok_or(Reason::TargetChanged)?;
            let actual = repo
                .engine
                .resolve_checks_ref(&job.source_ref)
                .map_err(|_| Reason::PushNotConfirmed)?;
            if actual != job.expected_commit {
                return Err(if actual == job.previous_commit {
                    Reason::PushNotConfirmed
                } else {
                    Reason::TargetChanged
                });
            }
            let policy = repo
                .engine
                .required_checks_at_commit(&job.expected_commit)
                .map_err(|_| Reason::PolicyInvalid)?
                .ok_or(Reason::PolicyMissing)?;
            if !(short_branch == base || policy.branches.is_some())
                || !policy.allows_auto_run_for_branch(short_branch)
            {
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
        identity_for_repo(repo)?;
        Ok(())
    })();
    if result.is_err() {
        repo.local_checks_automatic.mark_unavailable()
    } else {
        repo.local_checks_automatic.notify()
    }
    result
}
pub(crate) struct Service {
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<Result<(), Error>>>,
    _owners: Vec<WorktreeServiceLock>,
}
impl Service {
    pub async fn shutdown(mut self) -> Result<(), std::io::Error> {
        self.shutdown.send_replace(true);
        for task in self.tasks.drain(..) {
            task.await
                .map_err(std::io::Error::other)?
                .map_err(std::io::Error::other)?;
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
    let repos: Vec<ReadRepository> = state
        .read_repositories()
        .into_iter()
        .filter(|r| r.is_default || r.git_write)
        .collect();
    let mut activities = Vec::with_capacity(repos.len());
    for repo in &repos {
        let registry = repo.local_checks_automatic.clone();
        if registry
            .started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(std::io::Error::other(
                "local automatic Checks already started",
            ));
        }
        activities.push(Activity {
            registry,
            finished: false,
        });
    }
    let snapshot = state.clone();
    let recover_repos = repos.clone();
    let (prepared, activities) = tokio::task::spawn_blocking(move || {
        let mut prepared = Vec::with_capacity(recover_repos.len());
        for repo in &recover_repos {
            let owner = repo.engine.lock_worktree_service().map_err(|_| Error)?;
            let store = repo.local_checks_automatic.store_for_repo(repo)?;
            super::local_checks_automatic_worker::recover_for_repo(&snapshot, repo, &store)?;
            prepared.push((repo.clone(), store, owner));
        }
        Ok::<_, Error>((prepared, activities))
    })
    .await
    .map_err(std::io::Error::other)?
    .map_err(std::io::Error::other)?;
    let (shutdown, receiver) = watch::channel(false);
    let mut tasks = Vec::with_capacity(prepared.len());
    let mut owners = Vec::with_capacity(prepared.len());
    for ((repo, store, owner), activity) in prepared.into_iter().zip(activities) {
        repo.local_checks_automatic
            .unavailable
            .store(false, Ordering::SeqCst);
        let snapshot = state.clone();
        let task_repo = repo.clone();
        let task_owner = owner.clone();
        let rx = receiver.clone();
        let task = tokio::spawn(async move {
            let mut activity = activity;
            let _owner = task_owner;
            let result =
                super::local_checks_automatic_worker::run_for_repo(snapshot, task_repo, store, rx)
                    .await;
            if result.is_ok() {
                activity.finished = true
            }
            result
        });
        repo.local_checks_automatic.notify();
        tasks.push(task);
        owners.push(owner);
    }
    Ok(Service {
        shutdown,
        tasks,
        _owners: owners,
    })
}
pub(super) async fn notified_for_repo(repo: &ReadRepository) {
    repo.local_checks_automatic.wake.notified().await
}
