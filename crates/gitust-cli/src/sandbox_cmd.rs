use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use gitust_core::{AgentKind, RepoId, WorktreeId};
use gitust_git::{GitEngine, MaterializedWorktree, WorktreeServiceLock};
use gitust_sandbox::{LocalSandboxExecutor, SandboxSpec};

struct CliWorktreeGuard {
    engine: GitEngine,
    _service_lock: WorktreeServiceLock,
    materialized: Arc<Mutex<Option<MaterializedWorktree>>>,
    keep: bool,
    cancelled: Arc<AtomicBool>,
}

impl CliWorktreeGuard {
    async fn finish(self) {
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
            return;
        }
        let engine = self.engine.clone();
        let service_lock = self._service_lock.clone();
        match tokio::task::spawn_blocking(move || {
            let _held_service = service_lock;
            engine.remove_agent_worktree(
                &m.worktree_dir,
                &m.worktree.branch,
                &m.worktree.base_commit,
            )
        })
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => eprintln!("[gitust sandbox] warning: cleanup failed: {e}"),
            Err(e) => eprintln!("[gitust sandbox] warning: cleanup task failed: {e}"),
        }
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
    let base_branch = head.branch_name.clone();
    let spawn_service_lock = service_lock.clone();
    let m = tokio::task::spawn_blocking(move || {
        let _held_service = spawn_service_lock;
        let res = engine_for_spawn.materialize_agent_worktree(
            worktree_id,
            RepoId::new(),
            AgentKind::InternalBot,
            &branch,
            &base_branch,
            "CLI sandbox command",
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
        // fail-open-ok: records materialized worktree in `CliWorktreeGuard` slot when `res` is `Ok`; `res??` propagates `Err` on line 143.
        } else if let Ok(ref m) = res {
            *slot = Some(m.clone());
        }
        res
    })
    .await??;
    let spec = SandboxSpec::for_linked_worktree(
        &m.worktree_dir,
        &m.worktree_admin_dir,
        &m.common_git_dir,
        m.main_workdir.as_deref(),
        &m.worktrees_base,
        &m.sandbox_scratch_base,
    );

    let executor = LocalSandboxExecutor::new(1).with_timeout_secs(timeout_secs);
    // Biased select checks for a pending Ctrl-C (including one received during checkout) before
    // polling execution; dropping the execution future kills the sandboxed process group, and
    // `CliWorktreeGuard` always cleans up the worktree.
    #[cfg(unix)]
    let outcome = tokio::select! {
        biased;
        // fail-open-ok: an interrupt always cancels execution and enters checkout cleanup; no success is inferred.
        _ = sigint.recv() => None,
        res = executor.execute(m.worktree.id, &spec, cmd) => Some(res),
    };
    #[cfg(not(unix))]
    let outcome = tokio::select! {
        res = executor.execute(m.worktree.id, &spec, cmd) => Some(res),
        // fail-open-ok: signal completion cancels execution and cleans up instead of authorizing the worktree.
        _ = tokio::signal::ctrl_c() => None,
    };

    let Some(outcome) = outcome else {
        guard.finish().await;
        eprintln!("[gitust sandbox] interrupted");
        return Ok(130);
    };
    let materialize_micros = m.worktree.materialize_micros;
    guard.finish().await;

    let res = outcome?;
    if !res.stdout.is_empty() {
        print!("{}", res.stdout);
    }
    if !res.stderr.is_empty() {
        eprint!("{}", res.stderr);
    }
    eprintln!(
        "[gitust sandbox] backend={:?} worktree_setup={}µs run={}ms exit={}",
        res.backend, materialize_micros, res.duration_ms, res.exit_code
    );
    Ok(res.exit_code)
}
