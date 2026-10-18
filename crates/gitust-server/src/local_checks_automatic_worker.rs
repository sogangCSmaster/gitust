//! Dispatch only durable queued work; uncertain attempts never become retries.
use super::{
    ephemeral_sandbox::{run_prepared_sandbox, RunCommands, RunContext, RunError},
    local_checks_automatic::{identity, notified},
    local_checks_automatic_model::{History, Job, Phase, Reason},
    local_checks_automatic_store::{Error, Store},
    local_checks_history::{LocalChecksHistoryStore, LocalChecksRecord},
    state::{check_repo_bola, AppState},
};
use gitust_core::SandboxRunId;
use gitust_git::{RequiredChecks, WorktreeServiceLock};
use gitust_sandbox::SandboxError;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::{watch, OwnedMutexGuard};

fn admit(state: &AppState, job: &Job) -> Result<(String, RequiredChecks), Reason> {
    if state
        .local_checks_automatic
        .unavailable
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(Reason::RepositoryUnavailable);
    }
    identity(state).map_err(|_| Reason::RepositoryUnavailable)?;
    let caller = state
        .auth_store
        .authorize_automatic_checks_actor(job.actor_id)
        .map_err(|_| Reason::AuthorityUnavailable)?;
    check_repo_bola(
        &state.auth_store,
        state.repo_id,
        state.repo_access,
        &caller,
        state.repo_id,
    )
    .map_err(|_| Reason::AuthorityUnavailable)?;
    if !state.trusted_sandbox_exec {
        return Err(Reason::ExecutionDisabled);
    }
    let base = state
        .git_engine
        .committed_default_branch()
        .map_err(|_| Reason::DefaultChanged)?;
    if format!("refs/heads/{base}") != job.source_ref {
        return Err(Reason::DefaultChanged);
    }
    if state
        .git_engine
        .resolve_checks_ref(&job.source_ref)
        .map_err(|_| Reason::TargetChanged)?
        != job.expected_commit
    {
        return Err(Reason::TargetChanged);
    }
    let checks = state
        .git_engine
        .required_checks_at_commit(&job.expected_commit)
        .map_err(|_| Reason::PolicyInvalid)?
        .ok_or(Reason::PolicyMissing)?;
    if !checks.auto_run {
        return Err(Reason::PolicyDisabled);
    }
    if job.checks_blob_oid.as_ref() != Some(&checks.blob_oid) {
        return Err(Reason::PolicyInvalid);
    }
    state
        .git_engine
        .ensure_config_has_no_credentials()
        .map_err(|_| Reason::RepositoryUnavailable)?;
    Ok((base, checks))
}
fn no_execution_phase(reason: Reason) -> Phase {
    if matches!(reason, Reason::DefaultChanged | Reason::TargetChanged) {
        Phase::Superseded
    } else {
        Phase::Rejected
    }
}
pub(crate) fn recover(state: &AppState, store: &Store) -> Result<(), Error> {
    identity(state)?;
    let doc = store.read()?;
    if doc.prepared.is_none()
        && doc.running.is_none()
        && (state.trusted_sandbox_exec || doc.queued.is_none())
    {
        return Ok(());
    }
    let common = state
        .git_engine
        .repository_common_directory()
        .map_err(|_| Error)?;
    let history = LocalChecksHistoryStore::open(&common, state.repo_id).map_err(|_| Error)?;
    store.update(|doc| {
        if let Some(job) = doc.prepared.take() {
            doc.finish(
                job,
                Phase::Interrupted,
                Some(Reason::AcceptanceUnconfirmed),
                History::NotAttempted,
            )
        }
        if let Some(job) = doc.running.take() {
            match history.get(job.run_id).map_err(|_| Error)? {
                Some(record) => {
                    if record.source_ref != job.source_ref
                        || record.checked_commit != job.expected_commit
                        || job.checks_blob_oid.as_ref() != Some(&record.checks_blob_oid)
                    {
                        return Err(Error);
                    }
                    history.append(record.clone()).map_err(|_| Error)?;
                    let (phase, reason) = if record.result.exit_code == 0 {
                        (Phase::Completed, None)
                    } else {
                        (Phase::Failed, Some(Reason::CommandFailed))
                    };
                    doc.finish(job, phase, reason, History::Saved);
                }
                None => doc.finish(
                    job,
                    Phase::Interrupted,
                    Some(Reason::UnconfirmedExecution),
                    History::NotAttempted,
                ),
            }
        }
        if !state.trusted_sandbox_exec {
            if let Some(job) = doc.queued.take() {
                doc.finish(
                    job,
                    Phase::Rejected,
                    Some(Reason::ExecutionDisabled),
                    History::NotAttempted,
                )
            }
        }
        Ok(())
    })?;
    identity(state)
}
struct Target {
    job: Job,
    base: String,
    checks: RequiredChecks,
    owner: WorktreeServiceLock,
}
enum Claim {
    Empty,
    Retired,
    Run(Box<Target>),
}
fn claim(state: &AppState, store: &Store) -> Result<Claim, Error> {
    identity(state)?;
    let owner = state
        .git_engine
        .lock_worktree_service()
        .map_err(|_| Error)?;
    let doc = store.read()?;
    if doc.running.is_some() {
        return Err(Error);
    }
    let Some(job) = doc.queued else {
        return Ok(Claim::Empty);
    };
    let allowed = admit(state, &job);
    let claimed = store.update(|doc| {
        if doc.queued.as_ref().is_none_or(|j| j.run_id != job.run_id) {
            return Ok(false);
        }
        let mut selected = doc.queued.take().ok_or(Error)?;
        match &allowed {
            Ok(_) => {
                selected.transition(Phase::Running, None, History::NotAttempted);
                doc.running = Some(selected);
            }
            Err(reason) => doc.finish(
                selected,
                no_execution_phase(*reason),
                Some(*reason),
                History::NotAttempted,
            ),
        }
        Ok(true)
    })?;
    if !claimed {
        return Ok(Claim::Retired);
    }
    match allowed {
        Ok((base, checks)) => Ok(Claim::Run(Box::new(Target {
            job,
            base,
            checks,
            owner,
        }))),
        Err(_) => Ok(Claim::Retired),
    }
}
async fn terminal(
    state: AppState,
    store: Arc<Store>,
    run_id: SandboxRunId,
    outcome: (Phase, Option<Reason>, History),
    owner: WorktreeServiceLock,
    operation: Arc<OwnedMutexGuard<()>>,
) -> Result<(), Error> {
    let (phase, reason, history) = outcome;
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        let _operation = operation;
        let _registry = state.repository_registry.clone();
        identity(&state)?;
        store.update(|doc| {
            if doc.running.as_ref().is_none_or(|j| j.run_id != run_id) {
                return Err(Error);
            }
            let job = doc.running.take().ok_or(Error)?;
            doc.finish(job, phase, reason, history);
            Ok(())
        })?;
        identity(&state)
    })
    .await
    .map_err(|_| Error)?
}
async fn dispatch(
    state: &AppState,
    store: Arc<Store>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<bool, Error> {
    let operation = tokio::select! {biased;
        _=shutdown.changed()=>return Ok(false),
        operation=state.local_checks_lock.clone().lock_owned()=>Arc::new(operation),
    };
    let snapshot = state.clone();
    let disk = store.clone();
    let held = operation.clone();
    let target = tokio::task::spawn_blocking(move || {
        let _operation = held;
        claim(&snapshot, &disk)
    })
    .await
    .map_err(|_| Error)??;
    let target = match target {
        Claim::Empty => return Ok(false),
        Claim::Retired => return Ok(true),
        Claim::Run(target) => target,
    };
    let job = target.job;
    let denied = Arc::new(Mutex::new(None));
    let denial = denied.clone();
    let snapshot = state.clone();
    let expected = job.clone();
    let disk = store.clone();
    let held = operation.clone();
    let owner = target.owner.clone();
    let admission = Box::pin(async move {
        let result = tokio::task::spawn_blocking(move || {
            let _owner = owner;
            let _operation = held;
            let current = disk.read().map_err(|_| Reason::RepositoryUnavailable)?;
            if current
                .running
                .as_ref()
                .is_none_or(|j| j.run_id != expected.run_id)
            {
                return Err(Reason::RepositoryUnavailable);
            }
            admit(&snapshot, &expected).map(|_| ())
        })
        .await;
        match result {
            Ok(Ok(())) => Ok(()),
            other => {
                *denial.lock() = Some(match other {
                    Ok(Err(reason)) => reason,
                    _ => Reason::RepositoryUnavailable,
                });
                Err(SandboxError::AdmissionDenied)
            }
        }
    });
    let outcome = run_prepared_sandbox(
        state.git_engine.clone(),
        target.owner.clone(),
        target.base,
        RunCommands::Checks {
            expected: job.expected_commit.clone(),
            commands: target.checks.commands,
        },
        false,
        RunContext {
            repo_id: state.repo_id,
            executor: state.sandbox_executor.clone(),
            auth_store: Some(state.auth_store.clone()),
            admission: Some(admission),
            operation: Some(operation.clone()),
            run_id: Some(job.run_id),
        },
        async {
            let _changed = shutdown.changed().await;
        },
    )
    .await;
    let mut uncertain = false;
    let (phase, reason, history) = match outcome {
        Ok(outcome)
            if outcome.cleanup_confirmed && outcome.fence_confirmed && !outcome.interrupted =>
        {
            let result = outcome.result.ok_or(Error)?;
            if result.run_id != job.run_id {
                return Err(Error);
            }
            let record = LocalChecksRecord::new(
                job.source_ref.clone(),
                job.expected_commit.clone(),
                job.checks_blob_oid.clone().ok_or(Error)?,
                result,
            )
            .map_err(|_| Error)?;
            let passed = record.result.exit_code == 0;
            let saved = super::repository_local_checks_result::save(
                state.git_engine.clone(),
                target.owner.clone(),
                operation.clone(),
                state.repo_id,
                record,
            )
            .await;
            let history = if saved.status == "saved" {
                History::Saved
            } else {
                History::Failed
            };
            (
                if passed {
                    Phase::Completed
                } else {
                    Phase::Failed
                },
                if history == History::Failed {
                    Some(Reason::HistoryUnconfirmed)
                } else if passed {
                    None
                } else {
                    Some(Reason::CommandFailed)
                },
                history,
            )
        }
        Err(RunError::TargetChanged) => (
            Phase::Superseded,
            Some(Reason::TargetChanged),
            History::NotAttempted,
        ),
        Err(RunError::AdmissionDenied) => {
            let reason = denied.lock().take().ok_or(Error)?;
            (
                no_execution_phase(reason),
                Some(reason),
                History::NotAttempted,
            )
        }
        _ => {
            uncertain = true;
            (
                Phase::Interrupted,
                Some(if *shutdown.borrow() {
                    Reason::ServiceStopped
                } else {
                    Reason::UnconfirmedExecution
                }),
                History::NotAttempted,
            )
        }
    };
    terminal(
        state.clone(),
        store,
        job.run_id,
        (phase, reason, history),
        target.owner,
        operation,
    )
    .await?;
    if uncertain {
        return Err(Error);
    }
    Ok(true)
}
pub(crate) async fn run(
    state: AppState,
    store: Arc<Store>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Error> {
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        tokio::select! {biased;
            _=shutdown.changed()=>return Ok(()),
            _=notified(&state)=>{},
        }
        while !*shutdown.borrow() && dispatch(&state, store.clone(), &mut shutdown).await? {}
    }
}
