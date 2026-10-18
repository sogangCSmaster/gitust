use anyhow::Result;
use gitust_git::{GitEngine, WorktreeServiceLock};
use gitust_sandbox::LocalSandboxExecutor;
pub(crate) use gitust_server::ephemeral_sandbox::{RunCommands, RunOutcome};
use std::path::Path;

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
    #[cfg(unix)]
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let cancel = async move {
        #[cfg(unix)]
        sigint.recv().await;
        #[cfg(not(unix))]
        {
            // fail-open-ok: either signal result cancels execution; it never reports success.
            let _ = tokio::signal::ctrl_c().await;
        }
    };
    Ok(gitust_server::ephemeral_sandbox::run_prepared_sandbox(
        engine,
        service_lock,
        base_branch,
        commands,
        keep,
        gitust_server::ephemeral_sandbox::RunContext {
            repo_id: gitust_core::RepoId::new(),
            executor: LocalSandboxExecutor::new(1).with_timeout_secs(timeout_secs),
            auth_store: None,
            admission: None,
            operation: None,
        },
        cancel,
    )
    .await?)
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
