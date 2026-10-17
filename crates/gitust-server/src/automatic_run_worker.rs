use super::*;

fn current(state: &AppState, generation: u64) -> bool {
    state.automatic_checks.inner.lock().generation == generation
}
async fn terminal(
    state: &AppState,
    store: Arc<Store>,
    wt: gitust_core::AgentWorktree,
    run_id: SandboxRunId,
    outcome: (Phase, Option<Reason>),
    lease: (Arc<tokio::sync::OwnedMutexGuard<()>>, WorktreeServiceLock),
) -> Result<(), Failure> {
    let registry = state.automatic_checks.clone();
    let (phase, reason) = outcome;
    let id = wt.id;
    let result = tokio::task::spawn_blocking(move || {
        // Preserve the existing owner across quarantine: only diagnostics are finalized here.
        let _lease = lease;
        store.update(id, &wt.branch, &wt.base_branch, |doc| {
            if doc.running.as_ref().is_some_and(|j| j.run_id == run_id) {
                let job = doc.running.take().ok_or_else(unavailable)?;
                match reason {
                    Some(reason) => doc.finish_reason(job, phase, reason),
                    None => doc.finish(job, phase),
                };
            } else if doc.queued.as_ref().is_some_and(|j| j.run_id == run_id) {
                let job = doc.queued.take().ok_or_else(unavailable)?;
                match reason {
                    Some(reason) => doc.finish_reason(job, phase, reason),
                    None => doc.finish(job, phase),
                };
            } else {
                return Err(unavailable());
            }
            prune_local(&registry, id, doc);
            Ok(())
        })
    })
    .await
    .map_err(crate::state::internal_err)?;
    if result.is_err() {
        state
            .automatic_checks
            .unavailable
            .store(true, Ordering::SeqCst);
    }
    result
}
pub(super) async fn run(
    state: AppState,
    id: WorktreeId,
    generation: u64,
    token: uuid::Uuid,
    mut shutdown: watch::Receiver<()>,
) {
    let result = run_worker(&state, id, generation, token, &mut shutdown).await;
    if result.is_err() {
        state
            .automatic_checks
            .unavailable
            .store(true, Ordering::SeqCst);
        set_worktree_status(&state, id, |w| w.status = WorktreeStatus::Failed);
    }
    let mut inner = state.automatic_checks.inner.lock();
    if inner.generation == generation
        && inner
            .pending
            .get(&id)
            .is_some_and(|(owner, _)| *owner == token)
    {
        inner.pending.remove(&id);
    }
}
async fn run_worker(
    state: &AppState,
    id: WorktreeId,
    generation: u64,
    token: uuid::Uuid,
    shutdown: &mut watch::Receiver<()>,
) -> Result<(), Failure> {
    let store = state.automatic_checks.store()?;
    loop {
        if !current(state, generation) {
            return Ok(());
        }
        let revision = match state.automatic_checks.inner.lock().pending.get(&id) {
            Some((owner, revision)) if *owner == token => *revision,
            _ => return Ok(()),
        };
        let Some(runtime) = state.get_worktree_runtime(id) else {
            return Ok(());
        };
        let guard = tokio::select! { biased;
            // fail-open-ok: any notification stops dispatch without consuming a durable queued job.
            _ = shutdown.changed() => return Ok(()),
            guard = runtime.exec_lock.clone().lock_owned() => guard,
        };
        let guard = Arc::new(guard);
        let snapshot = state.clone();
        let disk = store.clone();
        let operation_guard = guard.clone();
        let target = tokio::task::spawn_blocking(move || {
            let _guard = operation_guard;
            let owner = snapshot
                .git_engine
                .lock_worktree_service()
                .map_err(map_git_err)?;
            let wt = snapshot
                .worktrees
                .read()
                .iter()
                .find(|w| w.id == id)
                .cloned();
            let Some(wt) = wt else {
                return Ok((None, owner));
            };
            let target = disk.update(id, &wt.branch, &wt.base_branch, |doc| {
                if doc.running.is_some() {
                    return Err(unavailable());
                }
                let Some(mut job) = doc.queued.take() else {
                    return Ok(None);
                };
                let allowed = (|| {
                    let local = job.origin == Origin::LocalOperator;
                    let caller = if local {
                        let live = snapshot
                            .automatic_checks
                            .inner
                            .lock()
                            .local_actors
                            .get(&id)
                            .and_then(|actors| actors.get(&job.run_id))
                            .cloned()
                            .ok_or(Reason::OperatorContextEnded)?;
                        snapshot
                            .auth_store
                            .verify_active_principal(&live)
                            .map_err(|_| Reason::AuthorityUnavailable)?
                    } else {
                        snapshot
                            .auth_store
                            .authorize_automatic_checks_actor(job.user_id)
                            .map_err(|_| Reason::AuthorityUnavailable)?
                    };
                    check_repo_bola(
                        &snapshot.auth_store,
                        snapshot.repo_id,
                        snapshot.repo_access,
                        &caller,
                        wt.repo_id,
                    )
                    .map_err(|_| Reason::AuthorityUnavailable)?;
                    if !snapshot.trusted_sandbox_exec {
                        return Err(Reason::ExecutionDisabled);
                    }
                    if !local && runtime.created_by != Some(caller.user_id) {
                        return Err(Reason::AuthorityUnavailable);
                    }
                    if wt.status != WorktreeStatus::Pending {
                        return Err(Reason::StateChanged);
                    }
                    let sha = snapshot
                        .git_engine
                        .clean_worktree_commit(id, &wt.branch)
                        .map_err(|_| Reason::TargetUnavailable)?;
                    if sha != job.expected {
                        return Err(Reason::TargetChanged);
                    }
                    let checks = snapshot
                        .git_engine
                        .required_checks(&wt.base_branch)
                        .map_err(|_| Reason::PolicyUnavailable)?;
                    if !checks.is_some_and(|c| c.auto_run && c.blob_oid == job.checks_oid) {
                        return Err(Reason::PolicyChanged);
                    }
                    Ok(caller)
                })();
                match allowed {
                    Ok(caller) => {
                        job.transition(Phase::Running);
                        doc.running = Some(job.clone());
                        Ok(Some((wt.clone(), job, caller)))
                    }
                    Err(reason) => {
                        doc.finish_reason(job, Phase::Rejected, reason);
                        prune_local(&snapshot.automatic_checks, id, doc);
                        if wt.status == WorktreeStatus::Pending {
                            set_worktree_status(&snapshot, id, |w| {
                                w.status = WorktreeStatus::Failed
                            });
                        }
                        Ok(None)
                    }
                }
            })?;
            Ok::<_, Failure>((target, owner))
        })
        .await
        .map_err(crate::state::internal_err)??;
        let (target, owner) = target;
        let Some((wt, job, caller)) = target else {
            let mut inner = state.automatic_checks.inner.lock();
            if inner
                .pending
                .get(&id)
                .is_some_and(|(owner, n)| *owner == token && *n == revision)
            {
                inner.pending.remove(&id);
                return Ok(());
            }
            continue;
        };
        let run_id = job.run_id;
        let outcome = tokio::select! { biased;
            // fail-open-ok: cancellation drops execution first; the unresolved fence remains authoritative.
            _ = shutdown.changed() => (Phase::Cancelled, Some(Reason::ServiceStopped)),
            result = crate::worktrees::execution::automatic(state, &caller, id.0, job, guard.clone()) => {
                match result {
                    Ok((res, _, _)) if res.exit_code == 0 => (Phase::Completed, None),
                    Ok(_) => (Phase::Failed, Some(Reason::CommandFailed)),
                    Err((StatusCode::REQUEST_TIMEOUT, _)) => (Phase::TimedOut, Some(Reason::TimedOut)),
                    Err((StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED, _)) => (Phase::Failed, Some(Reason::AuthorityUnavailable)),
                    Err((StatusCode::CONFLICT, _)) => (Phase::Failed, Some(Reason::ValidationFailed)),
                    Err(_) => (Phase::Failed, Some(Reason::ExecutionFailed)),
                }
            }
        };
        terminal(state, store.clone(), wt, run_id, outcome, (guard, owner)).await?;
        if outcome.0 == Phase::Cancelled {
            return Ok(());
        }
    }
}
