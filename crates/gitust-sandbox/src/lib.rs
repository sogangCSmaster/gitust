//! `gitust-sandbox`: local OS-level sandbox for running agent commands in a worktree.
//!
//! # Scope (read before relying on this crate)
//!
//! This is the **local / trusted single-user tier**, modelled on OS sandboxes used by local coding
//! agents (macOS Seatbelt, Linux Bubblewrap). It protects the developer's own machine from
//! accidental damage and casual exfiltration by an agent. It is **not** a multi-tenant security
//! boundary; server-side multi-tenant execution belongs to the microVM tier (microsandbox).
//!
//! # Policy
//!
//! - **Fail closed**: if no isolation backend is available, execution is refused.
//! - **Writes**: only the worktree checkout and a per-run scratch dir. The worktree's `.git`
//!   pointer file and the shared git dir are read-only, so the agent can run `git status`,
//!   `git diff` and `git log`, but commits are made host-side by Gitust.
//! - **Reads**: host home directories, temp directories, external volumes, the main repository
//!   working tree, and sibling agent worktrees are hidden. Rules are applied shallow-to-deep so a
//!   deeper rule (e.g. the own worktree) overrides a shallower one (e.g. the hidden worktree base).
//!   System paths (`/usr`, `/bin`, ...) stay readable so toolchains work.
//! - **Network**: denied unless explicitly allowed.
//! - **Environment**: cleared; `HOME`/`TMPDIR` point to the per-run scratch dir.
//! - **Processes**: the command runs in its own process group, which is killed when the lead
//!   process exits, times out, or the future is dropped.

mod command;
mod mcp;
mod policy;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_required_commands;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gitust_core::{SandboxRunId, WorktreeId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;

pub use mcp::{builtin_mcp_tools, McpToolDescriptor};
use policy::{build_sanitized_path, prepare_run};

/// Maximum bytes captured per stream (`stdout` / `stderr`).
pub(crate) const MAX_SANDBOX_STREAM_BYTES: u64 = 1024 * 1024;
/// Default wall-clock execution timeout for a sandbox run.
const DEFAULT_SANDBOX_TIMEOUT_SECS: u64 = 60;
/// How long to keep draining output after the lead process exited and its group was killed.
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("no sandbox isolation backend available: {0}")]
    IsolationUnavailable(String),

    #[error("path '{0}' cannot be expressed safely in a sandbox policy")]
    UnsupportedPath(String),

    #[error("failed to prepare sandbox at '{path}': {message}")]
    Prepare { path: String, message: String },

    #[error("sandbox execution failed to spawn command '{command}': {message}")]
    SpawnFailed { command: String, message: String },

    #[error("sandbox concurrency semaphore closed")]
    ConcurrencyLimiterClosed,

    #[error("sandbox command '{command}' timed out after {timeout_secs}s")]
    TimedOut { command: String, timeout_secs: u64 },
}

/// Isolation backend actually used for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    MacosSeatbelt,
    LinuxBubblewrap,
}

impl SandboxBackend {
    /// Detects the isolation backend for this host, failing closed if none is available.
    pub fn detect() -> Result<(Self, PathBuf), SandboxError> {
        #[cfg(target_os = "macos")]
        {
            let p = PathBuf::from("/usr/bin/sandbox-exec");
            if p.is_file() {
                return Ok((Self::MacosSeatbelt, p));
            }
            Err(SandboxError::IsolationUnavailable(
                "/usr/bin/sandbox-exec not found".to_owned(),
            ))
        }
        #[cfg(target_os = "linux")]
        {
            for candidate in ["/usr/bin/bwrap", "/usr/local/bin/bwrap"] {
                let p = PathBuf::from(candidate);
                if p.is_file() {
                    return Ok((Self::LinuxBubblewrap, p));
                }
            }
            Err(SandboxError::IsolationUnavailable(
                "bubblewrap (bwrap) not found; install it to run sandboxed commands".to_owned(),
            ))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Err(SandboxError::IsolationUnavailable(
                "unsupported operating system".to_owned(),
            ))
        }
    }
}

/// Access granted to a path (and everything below it) inside the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathAccess {
    /// Contents cannot be read or written.
    Hidden,
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRule {
    pub path: PathBuf,
    pub access: PathAccess,
}

impl PathRule {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, access: PathAccess) -> Self {
        Self {
            path: path.into(),
            access,
        }
    }
}

/// What a sandbox run may see and modify.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// Working directory of the command.
    pub workdir: PathBuf,
    /// Parent of per-run scratch dirs (`HOME`/`TMPDIR`); hidden except for the own run.
    pub scratch_base: PathBuf,
    /// Path rules on top of the built-in host rules.
    pub rules: Vec<PathRule>,
}

impl SandboxSpec {
    /// Policy for a Git linked worktree.
    #[must_use]
    pub fn for_linked_worktree(
        worktree_dir: &Path,
        worktree_admin_dir: &Path,
        common_git_dir: &Path,
        main_workdir: Option<&Path>,
        worktrees_base: &Path,
        scratch_base: &Path,
    ) -> Self {
        let mut rules = Vec::with_capacity(16);
        if let Some(main) = main_workdir {
            rules.push(PathRule::new(main, PathAccess::Hidden));
        }
        rules.push(PathRule::new(common_git_dir, PathAccess::ReadOnly));
        for private in [
            "index",
            "logs",
            "refs/stash",
            "COMMIT_EDITMSG",
            "MERGE_MSG",
            "SQUASH_MSG",
            "ORIG_HEAD",
            "FETCH_HEAD",
            "config.worktree",
            "worktrees",
            "gitust-worktree-service.lock",
            "gitust-approvals",
        ] {
            rules.push(PathRule::new(
                common_git_dir.join(private),
                PathAccess::Hidden,
            ));
        }
        rules.push(PathRule::new(worktree_admin_dir, PathAccess::ReadOnly));
        rules.push(PathRule::new(worktrees_base, PathAccess::Hidden));
        rules.push(PathRule::new(worktree_dir, PathAccess::ReadWrite));
        rules.push(PathRule::new(
            worktree_dir.join(".git"),
            PathAccess::ReadOnly,
        ));
        Self {
            workdir: worktree_dir.to_path_buf(),
            scratch_base: scratch_base.to_path_buf(),
            rules,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxExecutionResult {
    pub run_id: SandboxRunId,
    pub worktree_id: WorktreeId,
    pub command: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub output_truncated: bool,
    pub backend: SandboxBackend,
    pub network_isolated: bool,
}

/// Kills the run's process group and removes its scratch dir, including when the execution
/// future is cancelled. The shared scratch base is never removed (concurrent runs use it).
struct SandboxCleanupGuard {
    pid: Option<u32>,
    run_dir: PathBuf,
    cancelled: Arc<AtomicBool>,
}

impl Drop for SandboxCleanupGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(pid) = self.pid.take() {
            kill_process_group(pid);
        }
        // fail-open-ok: best-effort per-run scratch dir removal in Drop.
        let _ = fs::remove_dir_all(&self.run_dir);
    }
}

/// Host OS-level sandbox executor (macOS Seatbelt / Linux Bubblewrap) for local single-user
/// execution.
#[derive(Debug, Clone)]
pub struct LocalSandboxExecutor {
    pub max_concurrent_sandboxes: usize,
    pub timeout_secs: u64,
    pub allow_network: bool,
    semaphore: Arc<Semaphore>,
}

/// Compatibility alias for [`LocalSandboxExecutor`].
pub type MicrosandboxExecutor = LocalSandboxExecutor;

impl Default for LocalSandboxExecutor {
    fn default() -> Self {
        Self::new(4)
    }
}

impl LocalSandboxExecutor {
    #[must_use]
    pub fn new(max_concurrent_sandboxes: usize) -> Self {
        let permits = max_concurrent_sandboxes.max(1);
        Self {
            max_concurrent_sandboxes: permits,
            timeout_secs: DEFAULT_SANDBOX_TIMEOUT_SECS,
            allow_network: false,
            semaphore: Arc::new(Semaphore::new(permits)),
        }
    }

    #[must_use]
    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs.max(1);
        self
    }

    /// Runs `command_str` with `/bin/sh -c` inside the sandbox described by `spec`.
    pub async fn execute(
        &self,
        worktree_id: WorktreeId,
        spec: &SandboxSpec,
        command_str: &str,
    ) -> Result<SandboxExecutionResult, SandboxError> {
        self.execute_with_checks(worktree_id, spec, command_str, &[])
            .await
    }

    /// Runs each required check in its own shell after the requested command succeeds.
    /// One process group, execution deadline, and output cap cover the entire sequence.
    pub async fn execute_with_checks(
        &self,
        worktree_id: WorktreeId,
        spec: &SandboxSpec,
        requested: &str,
        required: &[String],
    ) -> Result<SandboxExecutionResult, SandboxError> {
        let execution = command::ShellCommand::new(requested, required);
        let command_str = execution.display;
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| SandboxError::ConcurrencyLimiterClosed)?;

        let start = Instant::now();
        let run_id = SandboxRunId::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cleanup_guard = SandboxCleanupGuard {
            pid: None,
            run_dir: spec.scratch_base.join(run_id.0.to_string()),
            cancelled: Arc::clone(&cancelled),
        };
        let spec_owned = spec.clone();
        let allow_network = self.allow_network;
        let prepared = tokio::task::spawn_blocking(move || {
            let anticipated = spec_owned.scratch_base.join(run_id.0.to_string());
            let res = prepare_run(&spec_owned, run_id, &execution.script, allow_network).map(
                |mut prepared| {
                    prepared
                        .args
                        .extend(execution.args.into_iter().map(Into::into));
                    prepared
                },
            );
            if res.is_err() || cancelled.load(Ordering::SeqCst) {
                // fail-open-ok: best-effort cleanup of anticipated scratch dir on prepare failure or cancellation.
                let _ = fs::remove_dir_all(&anticipated);
                // fail-open-ok: if `prepare_run` succeeded just as cancellation arrived, clean up the canonical `run_dir` too.
                if let Ok(ref p) = res {
                    // fail-open-ok: best-effort cleanup of canonical `run_dir` on cancellation.
                    let _ = fs::remove_dir_all(&p.run_dir);
                }
            }
            res
        })
        .await
        .map_err(|e| SandboxError::Prepare {
            path: spec.workdir.display().to_string(),
            message: e.to_string(),
        })??;

        cleanup_guard.run_dir = prepared.run_dir.clone();

        let mut cmd = Command::new(&prepared.program);
        cmd.args(&prepared.args)
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
            .envs(prepared.toolchain_env.iter().map(|(k, v)| (*k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|e| SandboxError::SpawnFailed {
            command: command_str.to_owned(),
            message: e.to_string(),
        })?;
        cleanup_guard.pid = child.id();

        let stdout_partial = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stderr_partial = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stdout_task = tokio::spawn(read_capped(
            child.stdout.take(),
            Arc::clone(&stdout_partial),
        ));
        let stderr_task = tokio::spawn(read_capped(
            child.stderr.take(),
            Arc::clone(&stderr_partial),
        ));

        let timeout_duration = Duration::from_secs(self.timeout_secs.max(1));
        let wait_outcome = tokio::time::timeout(timeout_duration, child.wait()).await;

        if let Some(pid) = cleanup_guard.pid.take() {
            kill_process_group(pid);
        }

        let status = match wait_outcome {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                stdout_task.abort();
                stderr_task.abort();
                return Err(SandboxError::SpawnFailed {
                    command: command_str.to_owned(),
                    message: e.to_string(),
                });
            }
            Err(_) => {
                stdout_task.abort();
                stderr_task.abort();
                return Err(SandboxError::TimedOut {
                    command: command_str.to_owned(),
                    timeout_secs: self.timeout_secs,
                });
            }
        };

        let (stdout_bytes, stdout_truncated) = await_pipe_drain(stdout_task, &stdout_partial).await;
        let (stderr_bytes, stderr_truncated) = await_pipe_drain(stderr_task, &stderr_partial).await;

        drop(cleanup_guard);

        Ok(SandboxExecutionResult {
            run_id,
            worktree_id,
            command: command_str.to_owned(),
            exit_code: status.code().unwrap_or(-1),
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
            output_truncated: stdout_truncated || stderr_truncated,
            backend: prepared.backend,
            network_isolated: !self.allow_network,
        })
    }
}

async fn await_pipe_drain(
    task: tokio::task::JoinHandle<(Vec<u8>, bool)>,
    partial: &Arc<std::sync::Mutex<Vec<u8>>>,
) -> (Vec<u8>, bool) {
    match tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, task).await {
        Ok(Ok(res)) => res,
        Ok(Err(_)) | Err(_) => {
            let captured = std::mem::take(&mut *partial.lock().unwrap_or_else(|e| e.into_inner()));
            (captured, true)
        }
    }
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    partial: Arc<std::sync::Mutex<Vec<u8>>>,
) -> (Vec<u8>, bool) {
    let Some(mut p) = pipe else {
        return (Vec::new(), false);
    };
    let cap = usize::try_from(MAX_SANDBOX_STREAM_BYTES).unwrap_or(1024 * 1024);
    let mut chunk = [0_u8; 8192];
    let mut truncated = false;
    loop {
        match p.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                let mut guard = partial.lock().unwrap_or_else(|e| e.into_inner());
                let remaining = cap.saturating_sub(guard.len());
                if remaining > 0 {
                    let take = n.min(remaining);
                    guard.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    let bytes = std::mem::take(&mut *partial.lock().unwrap_or_else(|e| e.into_inner()));
    (bytes, truncated)
}

/// Sends `SIGKILL` to the process group created via `process_group(0)`.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    // fail-open-ok: OS pid fits in positive `i32` on Unix (`pid_t`); guard against invalid `u32::MAX`.
    if let Ok(pgid) = i32::try_from(pid) {
        if pgid > 1 {
            // SAFETY: `killpg` has no memory-safety preconditions; `pgid` is the group id of the
            // child we spawned as a group leader.
            unsafe {
                // fail-open-ok: `ESRCH` when the process group already exited naturally is expected.
                let _ = libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}
