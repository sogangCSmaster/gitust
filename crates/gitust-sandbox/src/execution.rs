//! Host-observed command steps sharing one sandbox preparation and resource budget.

use std::fs;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gitust_core::{SandboxRunId, WorktreeId};
use tokio::process::Command;

use crate::policy::{build_sanitized_path, prepare_run, PreparedRun};
use crate::{
    await_pipe_drain, cleanup, command, read_capped, LocalSandboxExecutor, SandboxCleanupGuard,
    SandboxCommandOutput, SandboxCommandResult, SandboxError, SandboxExecutionResult, SandboxSpec,
    MAX_SANDBOX_STREAM_BYTES,
};

impl LocalSandboxExecutor {
    /// Commands run in independent process groups; files, HOME and TMPDIR are shared.
    /// A sequence has one concurrency permit, execution deadline and per-stream output cap.
    pub async fn execute_with_checks(
        &self,
        worktree_id: WorktreeId,
        spec: &SandboxSpec,
        requested: &str,
        required: &[String],
    ) -> Result<SandboxExecutionResult, SandboxError> {
        self.execute_check_run(None, worktree_id, spec, requested, required)
            .await
    }

    /// Labels a preallocated attempt; the durable dispatcher owns duplicate-execution policy.
    /// Scratch always gets a fresh nonce, so reusing a label cannot share or remove another run.
    pub async fn execute_with_checks_for_run(
        &self,
        run_id: SandboxRunId,
        worktree_id: WorktreeId,
        spec: &SandboxSpec,
        requested: &str,
        required: &[String],
    ) -> Result<SandboxExecutionResult, SandboxError> {
        if run_id.0.is_nil() {
            return Err(SandboxError::Prepare {
                path: spec.workdir.display().to_string(),
                message: "execution attempt ID must not be nil".into(),
            });
        }
        self.execute_check_run(Some(run_id), worktree_id, spec, requested, required)
            .await
    }

    async fn execute_check_run(
        &self,
        attempt: Option<SandboxRunId>,
        worktree_id: WorktreeId,
        spec: &SandboxSpec,
        requested: &str,
        required: &[String],
    ) -> Result<SandboxExecutionResult, SandboxError> {
        let command_str = command::display(requested, required);
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| SandboxError::ConcurrencyLimiterClosed)?;
        let start = Instant::now();
        let scratch_id = SandboxRunId::new();
        let run_id = match attempt {
            Some(id) => id,
            None => scratch_id,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cleanup = SandboxCleanupGuard {
            pid: None,
            run_dir: Some(spec.scratch_base.join(scratch_id.0.to_string())),
            cancelled: Arc::clone(&cancelled),
        };
        let spec_owned = spec.clone();
        let allow_network = self.allow_network;
        let prepared = tokio::task::spawn_blocking(move || {
            let anticipated = spec_owned.scratch_base.join(scratch_id.0.to_string());
            let res = prepare_run(&spec_owned, scratch_id, "", allow_network);
            if res.is_err() || cancelled.load(Ordering::SeqCst) {
                // fail-open-ok: best-effort scratch cleanup on preparation failure/cancellation.
                let _ = fs::remove_dir_all(&anticipated);
                // fail-open-ok: a cancelled successful preparation only needs scratch cleanup.
                if let Ok(ref prepared) = res {
                    // fail-open-ok: cleanup cannot authorize or run a command.
                    let _ = fs::remove_dir_all(&prepared.run_dir);
                }
            }
            res
        })
        .await
        .map_err(|error| SandboxError::Prepare {
            path: spec.workdir.display().to_string(),
            message: error.to_string(),
        })??;
        cleanup.run_dir = Some(prepared.run_dir.clone());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(self.timeout_secs.max(1));
        let mut steps: Vec<_> = std::iter::once(requested)
            .chain(required.iter().map(String::as_str))
            .enumerate()
            .map(|(index, command)| SandboxCommandResult {
                command: command.to_owned(),
                required: index != 0,
                exit_code: None,
                duration_ms: None,
                output: None,
            })
            .collect();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut output_truncated = false;
        let mut exit_code = 0;
        // fail-open-ok: numeric fallback preserves the fixed per-stream output cap.
        let cap = usize::try_from(MAX_SANDBOX_STREAM_BYTES).unwrap_or(1024 * 1024);
        for step in &mut steps {
            if tokio::time::Instant::now() >= deadline {
                return Err(SandboxError::TimedOut {
                    command: command_str,
                    timeout_secs: self.timeout_secs,
                });
            }
            let step_start = Instant::now();
            let outcome = tokio::time::timeout_at(
                deadline,
                run_command(
                    &prepared,
                    &step.command,
                    &mut cleanup,
                    cap.saturating_sub(stdout.len()),
                    cap.saturating_sub(stderr.len()),
                ),
            )
            .await
            .map_err(|_| SandboxError::TimedOut {
                command: command_str.clone(),
                timeout_secs: self.timeout_secs,
            })??;
            step.exit_code = Some(outcome.exit_code);
            let elapsed_ms = step_start.elapsed().as_millis();
            // fail-open-ok: display-only elapsed time saturates rather than overflowing.
            let duration_ms = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
            step.duration_ms = Some(duration_ms);
            step.output = Some(SandboxCommandOutput {
                stdout: String::from_utf8_lossy(&outcome.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&outcome.stderr).into_owned(),
                truncated: outcome.truncated,
            });
            stdout.extend_from_slice(&outcome.stdout);
            stderr.extend_from_slice(&outcome.stderr);
            output_truncated |= outcome.truncated;
            exit_code = outcome.exit_code;
            if exit_code != 0 {
                break;
            }
        }
        if let Some(run_dir) = cleanup.run_dir.as_ref() {
            let run_dir = run_dir.clone();
            tokio::task::spawn_blocking(move || fs::remove_dir_all(run_dir))
                .await
                .map_err(|_| SandboxError::CleanupIncomplete {
                    message: "scratch cleanup did not complete".into(),
                })?
                .map_err(|_| SandboxError::CleanupIncomplete {
                    message: "scratch cleanup failed".into(),
                })?;
            cleanup.run_dir = None;
        }
        Ok(SandboxExecutionResult {
            run_id,
            worktree_id,
            command: command_str,
            exit_code,
            // fail-open-ok: display-only elapsed time saturates rather than overflowing.
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            output_truncated,
            steps,
            backend: prepared.backend,
            network_isolated: !allow_network,
        })
    }
}

struct CommandOutcome {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

/// Every pipe reader is aborted on cancellation, even while waiting for the child.
struct Readers {
    stdout: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>,
    stderr: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>,
}
impl Drop for Readers {
    fn drop(&mut self) {
        for reader in [&self.stdout, &self.stderr].into_iter().flatten() {
            reader.abort();
        }
    }
}

async fn run_command(
    prepared: &PreparedRun,
    command: &str,
    cleanup: &mut SandboxCleanupGuard,
    stdout_cap: usize,
    stderr_cap: usize,
) -> Result<CommandOutcome, SandboxError> {
    let mut args = prepared.args.clone();
    let Some(script) = args.last_mut() else {
        return Err(SandboxError::Prepare {
            path: prepared.workdir.display().to_string(),
            message: "sandbox command arguments are missing".into(),
        });
    };
    *script = command.into();
    let mut cmd = Command::new(&prepared.program);
    cmd.args(args)
        .current_dir(&prepared.workdir)
        .env_clear()
        .env("PATH", build_sanitized_path())
        .env("HOME", &prepared.home)
        .env("TMPDIR", &prepared.tmp)
        .env("TEMP", &prepared.tmp)
        .env("TMP", &prepared.tmp)
        .env("USER", "gitust-sandbox")
        .env("LOGNAME", "gitust-sandbox")
        .env("LANG", "C.UTF-8")
        .env("TERM", "xterm-256color")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GITUST_SANDBOX_ISOLATED", "1")
        .envs(
            prepared
                .toolchain_env
                .iter()
                .map(|(key, value)| (*key, value)),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|error| SandboxError::SpawnFailed {
        command: command.into(),
        message: error.to_string(),
    })?;
    cleanup.pid = child.id();
    let stdout_partial = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_partial = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut readers = Readers {
        stdout: Some(tokio::spawn(read_capped(
            child.stdout.take(),
            Arc::clone(&stdout_partial),
            stdout_cap,
        ))),
        stderr: Some(tokio::spawn(read_capped(
            child.stderr.take(),
            Arc::clone(&stderr_partial),
            stderr_cap,
        ))),
    };
    let status = child.wait().await;
    if let Some(pid) = cleanup.pid {
        cleanup::signal_group(pid)?;
        // The signal completed; cancellation during observation must not signal a reused PGID.
        cleanup.pid = None;
        cleanup::wait_group_stopped(pid).await?;
    }
    let status = status.map_err(|error| SandboxError::SpawnFailed {
        command: command.into(),
        message: error.to_string(),
    })?;
    let stdout_task = readers
        .stdout
        .take()
        .ok_or_else(|| SandboxError::SpawnFailed {
            command: command.into(),
            message: "stdout reader missing".into(),
        })?;
    let (stdout, stdout_truncated) = await_pipe_drain(stdout_task, &stdout_partial).await;
    let stderr_task = readers
        .stderr
        .take()
        .ok_or_else(|| SandboxError::SpawnFailed {
            command: command.into(),
            message: "stderr reader missing".into(),
        })?;
    let (stderr, stderr_truncated) = await_pipe_drain(stderr_task, &stderr_partial).await;
    Ok(CommandOutcome {
        // fail-open-ok: signal termination is an explicit failure, never a successful status.
        exit_code: status.code().unwrap_or(-1),
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated,
    })
}
