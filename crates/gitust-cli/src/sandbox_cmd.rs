use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use gitust_core::{AgentKind, CommitSha, RepoId, WorktreeId};
use gitust_git::{GitEngine, MaterializedWorktree, WorktreeServiceLock};
use gitust_sandbox::{LocalSandboxExecutor, SandboxExecutionResult, SandboxSpec};

pub(crate) enum RunCommands {
    Command(String),
    Checks {
        expected: CommitSha,
        commands: Vec<String>,
    },
}
pub(crate) struct RunOutcome {
    pub result: Option<SandboxExecutionResult>,
    pub cleanup_confirmed: bool,
    pub fence_confirmed: bool,
    pub interrupted: bool,
    materialize_micros: u64,
}

struct CliWorktreeGuard {
    engine: GitEngine,
    _service_lock: WorktreeServiceLock,
    materialized: Arc<Mutex<Option<MaterializedWorktree>>>,
    keep: bool,
    cancelled: Arc<AtomicBool>,
}

impl CliWorktreeGuard {
    async fn finish(self) -> Result<bool> {
        self.cancelled.store(true, Ordering::SeqCst);
        let taken = self
            .materialized
            .lock()
            // fail-open-ok: recover the cleanup-only worktree slot after poisoning so an existing checkout is still removed.
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(m) = taken else {
            return Ok(true);
        };
        if self.keep {
            eprintln!(
                "[gitust sandbox] kept worktree {} on branch {}",
                m.worktree_dir.display(),
                m.worktree.branch
            );
            return Ok(true);
        }
        let engine = self.engine.clone();
        let service_lock = self._service_lock.clone();
        tokio::task::spawn_blocking(move || {
            let _held_service = service_lock;
            engine.remove_agent_worktree(
                &m.worktree_dir,
                &m.worktree.branch,
                &m.worktree.base_commit,
            )?;
            Ok::<_, anyhow::Error>(
                !m.worktree_dir.try_exists()?
                    && engine.resolve_branch_oid(&m.worktree.branch)?.is_none(),
            )
        })
        .await?
    }
}

impl Drop for CliWorktreeGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let taken = self
            .materialized
            .lock()
            // fail-open-ok: recover the cleanup-only worktree slot after poisoning so an existing checkout is still removed.
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(m) = taken else {
            return;
        };
        if self.keep {
            eprintln!(
                "[gitust sandbox] kept worktree {} on branch {}",
                m.worktree_dir.display(),
                m.worktree.branch
            );
        } else if let Err(e) = self.engine.remove_agent_worktree(
            &m.worktree_dir,
            &m.worktree.branch,
            &m.worktree.base_commit,
        ) {
            eprintln!("[gitust sandbox] warning: cleanup failed: {e}");
        }
    }
}

/// Creates an ephemeral agent worktree from the current branch, runs `cmd` in the sandbox, and
/// removes the worktree (and its branch) afterwards unless `keep` is set.
pub(crate) async fn run_cli_sandbox(
    repo: &Path,
    cmd: &str,
    keep: bool,
    timeout_secs: u64,
) -> Result<i32> {
    let engine = GitEngine::new(repo);
    let setup_engine = engine.clone();
    let (head, service_lock) = tokio::task::spawn_blocking(move || {
        let lock = setup_engine.lock_worktree_service()?;
        setup_engine.ensure_config_has_no_credentials()?;
        Ok::<_, gitust_git::GitEngineError>((setup_engine.inspect_head_zero_copy()?, lock))
    })
    .await??;
    if head.branch_name == "detached" {
        anyhow::bail!("HEAD is detached; check out a branch first");
    }

    let outcome = run_prepared_sandbox(
        engine,
        service_lock,
        head.branch_name,
        RunCommands::Command(cmd.to_owned()),
        keep,
        timeout_secs,
    )
    .await?;
    if outcome.interrupted {
        eprintln!("[gitust sandbox] interrupted");
        return Ok(130);
    }
    if !outcome.fence_confirmed {
        anyhow::bail!(
            "sandbox execution completion is unconfirmed; explicit recovery may be required"
        );
    }
    if !outcome.cleanup_confirmed {
        eprintln!("[gitust sandbox] warning: cleanup is unconfirmed");
    }
    let res = outcome
        .result
        .ok_or_else(|| anyhow::anyhow!("sandbox result is unavailable"))?;
    if !res.stdout.is_empty() {
        print!("{}", res.stdout);
    }
    if !res.stderr.is_empty() {
        eprint!("{}", res.stderr);
    }
    eprintln!(
        "[gitust sandbox] backend={:?} worktree_setup={}µs run={}ms exit={}",
        res.backend, outcome.materialize_micros, res.duration_ms, res.exit_code
    );
    Ok(res.exit_code)
}

pub(crate) async fn run_prepared_sandbox(
    engine: GitEngine,
    service_lock: WorktreeServiceLock,
    base_branch: String,
    commands: RunCommands,
    keep: bool,
    timeout_secs: u64,
) -> Result<RunOutcome> {
    // Register the OS interrupt handler before materializing the worktree so Ctrl-C during
    // checkout does not terminate the process mid-checkout without cleaning up.
    #[cfg(unix)]
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

    let cancelled = Arc::new(AtomicBool::new(false));
    let materialized_slot = Arc::new(Mutex::new(None));
    let guard = CliWorktreeGuard {
        engine: engine.clone(),
        _service_lock: service_lock.clone(),
        materialized: Arc::clone(&materialized_slot),
        keep,
        cancelled: Arc::clone(&cancelled),
    };

    let worktree_id = WorktreeId::new();
    let id_str = worktree_id.0.to_string();
    let branch = format!("agent/cli/sandbox-{}", &id_str[..8]);
    let engine_for_spawn = engine.clone();
    let label = match &commands {
        RunCommands::Command(_) => "CLI sandbox command",
        RunCommands::Checks { .. } => "Repository Checks",
    };
    let spawn_service_lock = service_lock.clone();
    let m = tokio::task::spawn_blocking(move || {
        let _held_service = spawn_service_lock;
        let res = engine_for_spawn.materialize_agent_worktree(
            worktree_id,
            RepoId::new(),
            AgentKind::InternalBot,
            &branch,
            &base_branch,
            label,
        );
        // fail-open-ok: poison recovery retains the cleanup slot; materialization errors are still returned to the caller.
        let mut slot = materialized_slot.lock().unwrap_or_else(|e| e.into_inner());
        if cancelled.load(Ordering::SeqCst) {
            drop(slot);
            // fail-open-ok: rolls back the newly materialized worktree if Ctrl-C arrived during checkout; `res` is still returned below.
            if let Ok(ref m) = res {
                if !keep {
                    // fail-open-ok: best-effort worktree removal on Ctrl-C cancellation.
                    let _ = engine_for_spawn.remove_agent_worktree(
                        &m.worktree_dir,
                        &m.worktree.branch,
                        &m.worktree.base_commit,
                    );
                }
            }
        // fail-open-ok: records a successful checkout in the cleanup guard; materialization errors are still propagated to the caller.
        } else if let Ok(ref m) = res {
            *slot = Some(m.clone());
        }
        res
    })
    .await??;
    if let RunCommands::Checks { expected, .. } = &commands {
        let check_engine = engine.clone();
        let checkout = m.worktree.clone();
        let expected = expected.clone();
        let check_service = service_lock.clone();
        tokio::task::spawn_blocking(move || {
            let _service = check_service;
            if checkout.base_commit != expected
                || check_engine.clean_worktree_commit(checkout.id, &checkout.branch)? != expected
            {
                anyhow::bail!("Checks checkout does not match the expected commit");
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
    }
    let spec = SandboxSpec::for_linked_worktree(
        &m.worktree_dir,
        &m.worktree_admin_dir,
        &m.common_git_dir,
        m.main_workdir.as_deref(),
        &m.worktrees_base,
        &m.sandbox_scratch_base,
    );
    let fence_engine = engine.clone();
    let target = m.worktree.clone();
    let execution_guard = tokio::task::spawn_blocking(move || {
        let fence = fence_engine.begin_execution_fence(&target)?;
        Ok::<_, gitust_git::GitEngineError>((fence, guard))
    })
    .await??;

    let executor = LocalSandboxExecutor::new(1).with_timeout_secs(timeout_secs);
    let execute = async {
        match &commands {
            RunCommands::Command(command) => executor.execute(m.worktree.id, &spec, command).await,
            RunCommands::Checks { commands, .. } => {
                executor
                    .execute_required_checks(m.worktree.id, &spec, commands)
                    .await
            }
        }
    };
    // Biased select checks for a pending Ctrl-C (including one received during checkout) before
    // polling execution; dropping the execution future kills the sandboxed process group, and
    // An uncertain interruption retains the execution fence and preserves the checkout.
    #[cfg(unix)]
    let outcome = tokio::select! {
        biased;
        // fail-open-ok: an interrupt always cancels execution and enters checkout cleanup; no success is inferred.
        _ = sigint.recv() => None,
        res = execute => Some(res),
    };
    #[cfg(not(unix))]
    let outcome = tokio::select! {
        res = execute => Some(res),
        // fail-open-ok: signal completion cancels execution and cleans up instead of authorizing the worktree.
        _ = tokio::signal::ctrl_c() => None,
    };

    let Some(outcome) = outcome else {
        let (fence, guard) = execution_guard;
        drop(fence);
        let cleanup_confirmed = guard.finish().await.is_ok_and(|confirmed| confirmed);
        return Ok(RunOutcome {
            result: None,
            cleanup_confirmed,
            fence_confirmed: false,
            interrupted: true,
            materialize_micros: m.worktree.materialize_micros,
        });
    };
    let (fence, guard) = execution_guard;
    let materialize_micros = m.worktree.materialize_micros;
    let (res, fence_confirmed) = match outcome {
        Ok(res) => {
            let confirmed = tokio::task::spawn_blocking(move || fence.complete())
                .await
                .is_ok_and(|result| result.is_ok());
            (res, confirmed)
        }
        Err(error) => {
            drop(fence);
            if !guard.finish().await.is_ok_and(|confirmed| confirmed) {
                eprintln!("[gitust sandbox] warning: cleanup is unconfirmed");
            }
            return Err(error.into());
        }
    };
    let cleanup_confirmed = guard.finish().await.is_ok_and(|confirmed| confirmed);
    Ok(RunOutcome {
        result: Some(res),
        cleanup_confirmed,
        fence_confirmed,
        interrupted: false,
        materialize_micros,
    })
}

pub(crate) async fn recover_cli_sandbox(repo: &Path, acknowledged: bool) -> Result<()> {
    if !acknowledged {
        anyhow::bail!("stop every prior sandbox command before explicitly acknowledging recovery");
    }
    let engine = GitEngine::new(repo);
    let count = tokio::task::spawn_blocking(move || {
        engine.recover_execution_fences_after_operator_confirmation()
    })
    .await??;
    println!("Cleared {count} interrupted execution fences after operator confirmation. Reverify pending work before approval.");
    Ok(())
}
