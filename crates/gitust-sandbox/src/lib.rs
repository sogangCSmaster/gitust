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
//! - **Processes**: each command runs in its own process group, which is killed when its lead
//!   process exits, times out, or the future is dropped. Check sequences share files and scratch
//!   directories, but cannot rely on a previous command keeping background processes alive.

mod cleanup;
mod command;
mod execution;
mod linux_policy;
mod mcp;
mod policy;
#[cfg(test)]
mod tests;
#[cfg(all(test, unix))]
mod tests_late_hidden;
#[cfg(test)]
mod tests_policy_failclosed;
#[cfg(test)]
mod tests_required_commands;
#[cfg(test)]
mod tests_steps;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gitust_core::{SandboxRunId, WorktreeId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

pub use mcp::{builtin_mcp_tools, McpToolDescriptor};

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

    #[error("sandbox cleanup could not be confirmed: {message}")]
    CleanupIncomplete { message: String },
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
            "gitust-active-executions",
            "gitust-repository",
            "gitust-approvals",
            "gitust-check-runs",
            "gitust-local-check-runs",
            "gitust-automatic-runs",
            ".gitust",
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
pub struct SandboxCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxCommandResult {
    pub command: String,
    pub required: bool,
    /// None means this command was skipped after an earlier failure.
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    /// Older saved results omit output; skipped commands never have output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<SandboxCommandOutput>,
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
    #[serde(default)]
    pub steps: Vec<SandboxCommandResult>,
    pub backend: SandboxBackend,
    pub network_isolated: bool,
}

/// Kills the run's process group and removes its scratch dir, including when the execution
/// future is cancelled. The shared scratch base is never removed (concurrent runs use it).
struct SandboxCleanupGuard {
    pid: Option<u32>,
    run_dir: Option<PathBuf>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for SandboxCleanupGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(pid) = self.pid.take() {
            kill_process_group(pid);
        }
        if let Some(run_dir) = self.run_dir.take() {
            cleanup_scratch(run_dir);
        }
    }
}

/// Drop may run on a Tokio worker after cancellation; directory walks belong off that worker.
fn cleanup_scratch(run_dir: PathBuf) {
    // fail-open-ok: a missing runtime uses synchronous cleanup as a last-resort fallback.
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn_blocking(move || {
            // fail-open-ok: best-effort cleanup of a cancelled or failed run.
            let _ = fs::remove_dir_all(run_dir);
        });
    } else {
        // fail-open-ok: best-effort cleanup when no async runtime is available.
        let _ = fs::remove_dir_all(run_dir);
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
}

struct PipeDrainGuard(tokio::task::JoinHandle<(Vec<u8>, bool)>);

impl Drop for PipeDrainGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn await_pipe_drain(
    task: tokio::task::JoinHandle<(Vec<u8>, bool)>,
    partial: &Arc<std::sync::Mutex<Vec<u8>>>,
) -> (Vec<u8>, bool) {
    let mut task = PipeDrainGuard(task);
    match tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, &mut task.0).await {
        Ok(Ok(res)) => res,
        Ok(Err(_)) | Err(_) => {
            task.0.abort();
            // fail-open-ok: poisoned output buffers hold diagnostic bytes only; truncated remains true.
            let captured = std::mem::take(&mut *partial.lock().unwrap_or_else(|e| e.into_inner()));
            (captured, true)
        }
    }
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    partial: Arc<std::sync::Mutex<Vec<u8>>>,
    cap: usize,
) -> (Vec<u8>, bool) {
    let Some(mut p) = pipe else {
        return (Vec::new(), false);
    };
    let mut chunk = [0_u8; 8192];
    let mut truncated = false;
    loop {
        match p.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                // fail-open-ok: output buffer poison recovery cannot authorize or change sandbox policy.
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
    // fail-open-ok: output buffer poison recovery returns captured diagnostics only.
    let bytes = std::mem::take(&mut *partial.lock().unwrap_or_else(|e| e.into_inner()));
    (bytes, truncated)
}

/// Drop is best effort; only the checked shutdown path may report a completed command.
fn kill_process_group(pid: u32) {
    // fail-open-ok: Drop cannot report cleanup errors; it never publishes a successful execution result.
    let _ = cleanup::signal_group(pid);
}
