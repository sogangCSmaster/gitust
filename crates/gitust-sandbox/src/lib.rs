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
//!
//! # Known limitations
//!
//! - macOS: a process that calls `setsid()` leaves the process group and survives the run (it
//!   remains sandboxed with the same file/network policy). Linux Bubblewrap runs the command in a
//!   PID namespace (`--unshare-all --die-with-parent`), which does contain such processes.
//! - macOS Seatbelt profiles start from `(allow default)`: Mach IPC and other non-file,
//!   non-network operations are not restricted.
//! - Reads are a denylist: paths outside user homes, temp dirs and external volumes (e.g.
//!   `/srv`, `/opt`, other repositories there) stay readable, and hidden paths are only
//!   protected against reading contents, not against `stat`/existence checks.
//! - The shared object database is readable (see [`SandboxSpec::for_linked_worktree`]), and so
//!   is the shared repository config (repositories with credentials in it are refused).
//! - The agent can create symlinks in its worktree pointing anywhere. Host-side code must never
//!   follow worktree symlinks when reading files (read from Git objects or use `O_NOFOLLOW`).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gitust_core::{SandboxRunId, WorktreeId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;

/// Maximum bytes captured per stream (`stdout` / `stderr`).
const MAX_SANDBOX_STREAM_BYTES: u64 = 1024 * 1024;
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
    ///
    /// - The checkout is writable; its `.git` pointer file is read-only.
    /// - The shared git dir is read-only so `git status/diff/log` work, but main-repository
    ///   working state inside it is hidden: the main index (staged files), the loose
    ///   `refs/stash` ref, reflogs, message files, and every other worktree's admin dir.
    ///   `packed-refs` stays readable, so after `git gc` / `git pack-refs --all` the stash commit
    ///   id is visible there.
    /// - The main working tree and sibling worktree checkouts are hidden.
    ///
    /// Limitation: the object database stays readable. Any blob that was ever written to it
    /// (including staged or stashed content that is no longer referenced) can be found by
    /// listing `objects/`. Isolating that requires a per-sandbox object store.
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
            "worktrees",
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
    pub backend: SandboxBackend,
    pub network_isolated: bool,
}

/// Kills the run's process group and removes its scratch dir, including when the execution
/// future is cancelled. The shared scratch base is never removed (concurrent runs use it).
struct SandboxCleanupGuard {
    pid: Option<u32>,
    run_dir: PathBuf,
}

impl Drop for SandboxCleanupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            kill_process_group(pid);
        }
        let _ = fs::remove_dir_all(&self.run_dir);
    }
}

/// Everything resolved on a blocking thread before spawning.
struct PreparedRun {
    backend: SandboxBackend,
    program: PathBuf,
    args: Vec<OsString>,
    workdir: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    run_dir: PathBuf,
    /// Toolchain variables (`RUSTUP_HOME`, `CARGO_HOME`, ...).
    toolchain_env: Vec<(&'static str, OsString)>,
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
///
/// Note: this executor uses the host OS sandbox (macOS Seatbelt / Linux Bubblewrap), not a
/// microVM.
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
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| SandboxError::ConcurrencyLimiterClosed)?;

        let start = Instant::now();
        let run_id = SandboxRunId::new();
        let spec_owned = spec.clone();
        let command_owned = command_str.to_owned();
        let allow_network = self.allow_network;
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_run(&spec_owned, run_id, &command_owned, allow_network)
        })
        .await
        .map_err(|e| SandboxError::Prepare {
            path: spec.workdir.display().to_string(),
            message: e.to_string(),
        })??;

        let mut cleanup_guard = SandboxCleanupGuard {
            pid: None,
            run_dir: prepared.run_dir.clone(),
        };

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

        let stdout_task = tokio::spawn(read_capped(child.stdout.take()));
        let stderr_task = tokio::spawn(read_capped(child.stderr.take()));

        let timeout_duration = Duration::from_secs(self.timeout_secs.max(1));
        let wait_outcome = tokio::time::timeout(timeout_duration, child.wait()).await;

        // Kill the group as soon as the lead process is gone so background children cannot keep
        // the output pipes open.
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

        let stdout_bytes = tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, stdout_task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        let stderr_bytes = tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, stderr_task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();

        drop(cleanup_guard);

        Ok(SandboxExecutionResult {
            run_id,
            worktree_id,
            command: command_str.to_owned(),
            exit_code: status.code().unwrap_or(-1),
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
            backend: prepared.backend,
            network_isolated: !self.allow_network,
        })
    }
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(p) = pipe {
        let _ = p.take(MAX_SANDBOX_STREAM_BYTES).read_to_end(&mut buf).await;
    }
    buf
}

fn prepare_err(path: &Path, e: &std::io::Error) -> SandboxError {
    SandboxError::Prepare {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

fn prepare_run(
    spec: &SandboxSpec,
    run_id: SandboxRunId,
    command_str: &str,
    allow_network: bool,
) -> Result<PreparedRun, SandboxError> {
    let (backend, program) = SandboxBackend::detect()?;

    let workdir = fs::canonicalize(&spec.workdir).map_err(|e| prepare_err(&spec.workdir, &e))?;
    let run_dir = spec.scratch_base.join(run_id.0.to_string());
    let home = run_dir.join("home");
    let tmp = run_dir.join("tmp");
    for dir in [&home, &tmp] {
        fs::create_dir_all(dir).map_err(|e| prepare_err(dir, &e))?;
    }
    let scratch_base =
        fs::canonicalize(&spec.scratch_base).map_err(|e| prepare_err(&spec.scratch_base, &e))?;
    let run_dir = fs::canonicalize(&run_dir).map_err(|e| prepare_err(&run_dir, &e))?;

    let toolchain_env = prepare_toolchain_env(&run_dir)?;

    let mut rules = host_rules();
    rules.extend(spec.rules.iter().cloned());
    rules.push(PathRule::new(&scratch_base, PathAccess::Hidden));
    rules.push(PathRule::new(&run_dir, PathAccess::ReadWrite));
    let rules = resolve_rules(rules)?;

    let args = match backend {
        SandboxBackend::MacosSeatbelt => {
            let profile = seatbelt_profile(&rules, allow_network)?;
            vec![
                OsString::from("-p"),
                OsString::from(profile),
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(command_str),
            ]
        }
        SandboxBackend::LinuxBubblewrap => bwrap_args(&rules, &workdir, command_str, allow_network),
    };

    Ok(PreparedRun {
        backend,
        program,
        args,
        workdir,
        home: run_dir.join("home"),
        tmp: run_dir.join("tmp"),
        run_dir,
        toolchain_env,
    })
}

/// Points rustup/cargo at the host toolchain without exposing the host home:
/// - `RUSTUP_HOME` is the host `~/.rustup` (read-only in the policy).
/// - `CARGO_HOME` is a per-run writable dir (cargo writes lock/cache files there) whose
///   `registry/` and `git/` link to the host's read-only copies.
/// - Network is denied, so cargo runs offline. Crates not already extracted in the host
///   registry cannot be extracted (read-only) and will fail to build.
fn prepare_toolchain_env(run_dir: &Path) -> Result<Vec<(&'static str, OsString)>, SandboxError> {
    let mut env = vec![("CARGO_NET_OFFLINE", OsString::from("true"))];
    let Some(host_home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(env);
    };
    let rustup_home = host_home.join(".rustup");
    if rustup_home.is_dir() {
        env.push(("RUSTUP_HOME", rustup_home.into_os_string()));
    }
    let cargo_home = run_dir.join("cargo");
    fs::create_dir_all(&cargo_home).map_err(|e| prepare_err(&cargo_home, &e))?;
    #[cfg(unix)]
    for sub in ["registry", "git"] {
        let host_dir = host_home.join(".cargo").join(sub);
        if host_dir.is_dir() {
            let link = cargo_home.join(sub);
            std::os::unix::fs::symlink(&host_dir, &link).map_err(|e| prepare_err(&link, &e))?;
        }
    }
    env.push(("CARGO_HOME", cargo_home.into_os_string()));
    Ok(env)
}

/// Built-in host rules: hide user homes, temp dirs and external volumes; expose the Rust
/// toolchain read-only.
fn host_rules() -> Vec<PathRule> {
    #[cfg(target_os = "macos")]
    let hidden: &[&str] = &[
        "/Users",
        "/System/Volumes/Data/Users",
        "/Volumes",
        "/private/tmp",
        "/private/var/tmp",
        "/private/var/folders",
        "/private/etc/master.passwd",
    ];
    #[cfg(not(target_os = "macos"))]
    let hidden: &[&str] = &[
        "/home",
        "/root",
        "/tmp",
        "/var/tmp",
        "/run/user",
        "/mnt",
        "/media",
    ];

    let mut rules: Vec<PathRule> = hidden
        .iter()
        .map(|p| PathRule::new(*p, PathAccess::Hidden))
        .collect();

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        rules.push(PathRule::new(&home, PathAccess::Hidden));
        for sub in [".rustup", ".cargo/bin", ".cargo/registry", ".cargo/git"] {
            rules.push(PathRule::new(home.join(sub), PathAccess::ReadOnly));
        }
    }
    rules
}

/// Canonicalizes rule paths (dropping ones that do not exist) and orders them shallow-to-deep,
/// so that deeper rules take precedence. Equal depth keeps insertion order (later wins).
fn resolve_rules(rules: Vec<PathRule>) -> Result<Vec<PathRule>, SandboxError> {
    let mut resolved = Vec::with_capacity(rules.len());
    for rule in rules {
        let Ok(path) = fs::canonicalize(&rule.path) else {
            continue;
        };
        let lossy = path.to_string_lossy();
        if path.to_str().is_none() || lossy.chars().any(char::is_control) {
            return Err(SandboxError::UnsupportedPath(lossy.into_owned()));
        }
        resolved.push(PathRule::new(path, rule.access));
    }
    resolved.sort_by_key(|r| r.path.components().count());
    Ok(resolved)
}

/// Builds a Seatbelt profile. Seatbelt applies the last matching rule, so emitting rules in
/// shallow-to-deep order makes deeper rules win.
fn seatbelt_profile(rules: &[PathRule], allow_network: bool) -> Result<String, SandboxError> {
    let mut profile = String::with_capacity(256 + rules.len() * 96);
    profile.push_str("(version 1)\n(allow default)\n");
    if !allow_network {
        profile.push_str("(deny network*)\n");
    }
    profile.push_str("(deny file-write*)\n(allow file-write* (subpath \"/dev\"))\n");
    for rule in rules {
        let p = seatbelt_quote(&rule.path)?;
        match rule.access {
            PathAccess::Hidden => {
                profile.push_str(&format!("(deny file-read-data (subpath {p}))\n"));
                profile.push_str(&format!("(deny file-write* (subpath {p}))\n"));
            }
            PathAccess::ReadOnly => {
                profile.push_str(&format!("(allow file-read-data (subpath {p}))\n"));
                profile.push_str(&format!("(deny file-write* (subpath {p}))\n"));
            }
            PathAccess::ReadWrite => {
                profile.push_str(&format!("(allow file-read-data (subpath {p}))\n"));
                profile.push_str(&format!("(allow file-write* (subpath {p}))\n"));
            }
        }
    }
    Ok(profile)
}

fn seatbelt_quote(path: &Path) -> Result<String, SandboxError> {
    let s = path
        .to_str()
        .ok_or_else(|| SandboxError::UnsupportedPath(path.to_string_lossy().into_owned()))?;
    if s.chars().any(char::is_control) {
        return Err(SandboxError::UnsupportedPath(s.to_owned()));
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Builds Bubblewrap arguments: read-only root, private `/dev` + `/proc`, all namespaces
/// unshared (network shared only when allowed), then path rules shallow-to-deep (later mounts
/// shadow earlier ones).
fn bwrap_args(
    rules: &[PathRule],
    workdir: &Path,
    command_str: &str,
    allow_network: bool,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    if allow_network {
        args.push("--share-net".into());
    }
    for rule in rules {
        let p = rule.path.as_os_str().to_owned();
        match rule.access {
            PathAccess::Hidden if rule.path.is_dir() => {
                args.extend(["--tmpfs".into(), p]);
            }
            PathAccess::Hidden => {
                args.extend(["--ro-bind".into(), "/dev/null".into(), p]);
            }
            PathAccess::ReadOnly => {
                args.extend(["--ro-bind".into(), p.clone(), p]);
            }
            PathAccess::ReadWrite => {
                args.extend(["--bind".into(), p.clone(), p]);
            }
        }
    }
    args.extend([
        "--chdir".into(),
        workdir.as_os_str().to_owned(),
        "/bin/sh".into(),
        "-c".into(),
        command_str.into(),
    ]);
    args
}

/// Sends `SIGKILL` to the process group created via `process_group(0)`.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    if let Ok(pgid) = i32::try_from(pid) {
        if pgid > 1 {
            // SAFETY: `killpg` has no memory-safety preconditions; `pgid` is the group id of the
            // child we spawned as a group leader.
            unsafe {
                let _ = libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

fn build_sanitized_path() -> String {
    let mut dirs = vec![
        "/usr/local/bin".to_owned(),
        "/opt/homebrew/bin".to_owned(),
        "/usr/bin".to_owned(),
        "/bin".to_owned(),
        "/usr/sbin".to_owned(),
        "/sbin".to_owned(),
    ];
    if let Ok(host_home) = std::env::var("HOME") {
        let cargo_bin = Path::new(&host_home).join(".cargo").join("bin");
        if cargo_bin.is_dir() {
            dirs.insert(0, cargo_bin.display().to_string());
        }
    }
    dirs.join(":")
}

// ============================================================================
// Built-in Model Context Protocol (MCP) Tool Definitions (Replaces container-use)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
}

#[must_use]
pub fn builtin_mcp_tools() -> Vec<McpToolDescriptor> {
    vec![
        McpToolDescriptor {
            name: "gitust_worktree_spawn",
            description: "Create a new agent/ branch and a Git linked worktree for an AI coding agent without touching the main working tree.",
        },
        McpToolDescriptor {
            name: "gitust_sandbox_exec",
            description: "Run build, test, or lint commands inside the agent worktree under an OS sandbox (no network, writes confined to the worktree).",
        },
        McpToolDescriptor {
            name: "gitust_worktree_merge_and_sync",
            description: "Merge a verified agent worktree into main and sync approved commits with the upstream GitHub mirror.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitust_core::{AgentKind, RepoId};
    use gitust_git::{GitEngine, MaterializedWorktree};
    use std::io::{Read, Write};
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@company.internal")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@company.internal")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn spec_for(m: &MaterializedWorktree) -> SandboxSpec {
        SandboxSpec::for_linked_worktree(
            &m.worktree_dir,
            &m.worktree_admin_dir,
            &m.common_git_dir,
            m.main_workdir.as_deref(),
            &m.worktrees_base,
            &m.sandbox_scratch_base,
        )
    }

    /// Serves one minimal HTTP response per connection on 127.0.0.1; returns the port.
    fn spawn_local_http_server() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });
        port
    }

    #[tokio::test]
    async fn gate_7_4_sandbox_isolation() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_dir = tmp.path().join("repo");
        let secret_dir = tmp.path().join("host-secrets");
        fs::create_dir_all(&repo_dir).expect("mkdir");
        fs::create_dir_all(&secret_dir).expect("mkdir");
        let secret_file = secret_dir.join("prod.key");
        fs::write(&secret_file, "TOP_SECRET_ON_PREM_KEY_999").expect("write");
        let outside_target = secret_dir.join("pwned.txt");

        git(&repo_dir, &["init", "-q", "-b", "main"]);
        fs::write(repo_dir.join("app.rs"), "fn main() {}\n").expect("write");
        git(&repo_dir, &["add", "."]);
        git(&repo_dir, &["commit", "-q", "-m", "initial"]);
        // Main-repo state that must not be readable from the sandbox:
        // a stashed untracked secret, a staged secret, and an uncommitted file.
        fs::write(repo_dir.join(".env"), "STASHED_SECRET_1").expect("write");
        git(&repo_dir, &["stash", "push", "-q", "-u", "-m", "wip"]);
        fs::write(repo_dir.join("staged.env"), "STAGED_SECRET_2").expect("write");
        git(&repo_dir, &["add", "staged.env"]);
        fs::write(repo_dir.join("MAIN_ONLY.txt"), "MAIN_WORKTREE_CONTENT").expect("write");

        let engine = GitEngine::new(&repo_dir);
        let mk = |branch: &str| {
            engine
                .materialize_agent_worktree(
                    WorktreeId::new(),
                    RepoId::new(),
                    AgentKind::ClaudeCode,
                    branch,
                    "main",
                    "sandbox gate",
                )
                .expect("materialize")
        };
        let m = mk("agent/claude/sandbox-gate");
        let sibling = mk("agent/cursor/sibling");
        fs::write(sibling.worktree_dir.join("SIBLING.txt"), "SIBLING_CONTENT").expect("write");
        let spec = spec_for(&m);
        let exec = LocalSandboxExecutor::new(4).with_timeout_secs(10);
        let run = |cmd: String| {
            let exec = exec.clone();
            let spec = spec.clone();
            let id = m.worktree.id;
            async move { exec.execute(id, &spec, &cmd).await.expect("execute") }
        };

        // Allowed: read/write inside the worktree, read-only git commands.
        let ok = run("cat app.rs && echo '// edit' >> app.rs && git status --short".into()).await;
        assert_eq!(ok.exit_code, 0, "stderr: {}", ok.stderr);
        assert!(ok.stdout.contains("fn main()"));
        assert!(
            ok.stdout.contains("app.rs"),
            "git status must work: {}",
            ok.stdout
        );
        assert!(ok.network_isolated);

        // Scratch dir cleaned up after the run.
        assert!(
            !m.sandbox_scratch_base
                .join(ok.run_id.0.to_string())
                .exists(),
            "per-run scratch dir must be removed"
        );

        // Denied: committing (shared git dir is read-only) and rewriting the .git pointer.
        let commit = run("git -c user.name=a -c user.email=a@a commit -qam x".into()).await;
        assert_ne!(
            commit.exit_code, 0,
            "commit must be blocked inside the sandbox"
        );
        let gitfile = run("echo 'gitdir: /etc' > .git".into()).await;
        assert_ne!(gitfile.exit_code, 0, ".git pointer must be read-only");

        // Denied: reading the main working tree, sibling worktrees and host secrets.
        for (path, marker) in [
            (repo_dir.join("MAIN_ONLY.txt"), "MAIN_WORKTREE_CONTENT"),
            (sibling.worktree_dir.join("SIBLING.txt"), "SIBLING_CONTENT"),
            (secret_file.clone(), "TOP_SECRET_ON_PREM_KEY_999"),
        ] {
            let res = run(format!("cat '{}'", path.display())).await;
            assert_ne!(res.exit_code, 0, "read of {} must fail", path.display());
            assert!(!res.stdout.contains(marker), "{} leaked", path.display());
        }

        // Denied: reading main-repo staged/stashed content through the shared git dir.
        let common = m.common_git_dir.display().to_string();
        for probe in [
            format!("git --git-dir='{common}' ls-files -s staged.env"),
            format!("cat '{common}/index'"),
            format!("git --git-dir='{common}' show stash@{{0}}^3:.env"),
            format!("cat '{common}/refs/stash'"),
            format!("cat '{common}/logs/HEAD'"),
            format!("ls '{common}/worktrees'"),
            format!("cat '{}/index'", sibling.worktree_admin_dir.display()),
        ] {
            let res = run(probe.clone()).await;
            assert!(
                res.exit_code != 0 || res.stdout.trim().is_empty(),
                "probe must not succeed: {probe}\nstdout: {}",
                res.stdout
            );
            assert!(!res.stdout.contains("SECRET"), "{probe} leaked a secret");
        }

        // Documented behavior: after `git pack-refs --all`, `packed-refs` remains readable so
        // `git status` and `git log` keep working on packed branches, while `logs/` stays hidden.
        git(&repo_dir, &["pack-refs", "--all"]);
        let packed = run(format!(
            "git status --short && test -r '{common}/packed-refs'"
        ))
        .await;
        assert_eq!(packed.exit_code, 0, "stderr: {}", packed.stderr);
        let logs_still_hidden = run(format!("cat '{common}/logs/HEAD'")).await;
        assert_ne!(logs_still_hidden.exit_code, 0);

        // Denied: writing outside the worktree.
        let write = run(format!("echo pwned > '{}'", outside_target.display())).await;
        assert_ne!(write.exit_code, 0);
        assert!(!outside_target.exists());

        // Environment: no host variables, HOME is the per-run scratch dir.
        std::env::set_var("GITUST_TEST_HOST_SECRET_TOKEN", "SUPER_SENSITIVE_777");
        let env = run("env; echo HOME=$HOME".into()).await;
        assert_eq!(env.exit_code, 0);
        assert!(!env.stdout.contains("SUPER_SENSITIVE_777"));
        // `sandbox_scratch_base` is derived from the canonical common git dir.
        assert!(
            env.stdout
                .contains(&format!("HOME={}", m.sandbox_scratch_base.display())),
            "HOME must be under the scratch base: {}",
            env.stdout
        );

        // Network: a local HTTP server is reachable from the host but not from the sandbox.
        let port = spawn_local_http_server();
        let curl = format!("curl -s --max-time 2 http://127.0.0.1:{port}/");
        let host = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&curl)
            .output()
            .expect("host curl");
        assert_eq!(
            String::from_utf8_lossy(&host.stdout),
            "ok",
            "sanity: server must be reachable without sandbox"
        );
        let net = run(curl).await;
        assert_ne!(net.exit_code, 0, "network must be blocked");
        assert!(!net.stdout.contains("ok"));

        // Background children do not keep the run alive.
        let started = Instant::now();
        let bg = run("(sleep 30 &) ; echo lead-exited".into()).await;
        assert_eq!(bg.exit_code, 0);
        assert!(bg.stdout.contains("lead-exited"));
        assert!(started.elapsed() < Duration::from_secs(5));

        // Timeouts.
        let fast = LocalSandboxExecutor::new(1).with_timeout_secs(1);
        let timed_out = fast.execute(m.worktree.id, &spec, "sleep 30").await;
        assert!(matches!(timed_out, Err(SandboxError::TimedOut { .. })));
    }

    /// The host Rust toolchain must work inside the sandbox (offline build of a tiny crate).
    #[tokio::test]
    async fn gate_rust_toolchain_works_in_sandbox() {
        let has_rustup = std::env::var_os("HOME")
            .map(|h| Path::new(&h).join(".rustup").is_dir())
            .unwrap_or(false);
        if !has_rustup {
            eprintln!("skipping: host has no ~/.rustup");
            return;
        }
        let tmp = TempDir::new().expect("tempdir");
        let repo_dir = tmp.path().join("crate");
        fs::create_dir_all(repo_dir.join("src")).expect("mkdir");
        git(&repo_dir, &["init", "-q", "-b", "main"]);
        fs::write(
            repo_dir.join("Cargo.toml"),
            "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write");
        fs::write(repo_dir.join("src/lib.rs"), "pub fn one() -> u8 { 1 }\n").expect("write");
        git(&repo_dir, &["add", "."]);
        git(&repo_dir, &["commit", "-q", "-m", "tiny crate"]);

        let m = GitEngine::new(&repo_dir)
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::ClaudeCode,
                "agent/claude/cargo",
                "main",
                "cargo",
            )
            .expect("materialize");
        let res = LocalSandboxExecutor::new(1)
            .with_timeout_secs(120)
            .execute(
                m.worktree.id,
                &spec_for(&m),
                "cargo build --offline -q && cargo --version",
            )
            .await
            .expect("execute");
        assert_eq!(
            res.exit_code, 0,
            "stdout: {}\nstderr: {}",
            res.stdout, res.stderr
        );
        assert!(res.stdout.contains("cargo "), "{}", res.stdout);
    }

    #[test]
    fn rules_are_ordered_shallow_to_deep() {
        let tmp = TempDir::new().expect("tempdir");
        let deep = tmp.path().join("a/b/c");
        fs::create_dir_all(&deep).expect("mkdir");
        let rules = resolve_rules(vec![
            PathRule::new(&deep, PathAccess::ReadWrite),
            PathRule::new(tmp.path().join("a"), PathAccess::Hidden),
            PathRule::new(tmp.path().join("missing"), PathAccess::ReadOnly),
        ])
        .expect("resolve");
        assert_eq!(rules.len(), 2, "missing paths are dropped");
        assert_eq!(rules[0].access, PathAccess::Hidden);
        assert_eq!(rules[1].access, PathAccess::ReadWrite);
    }

    #[test]
    fn seatbelt_quote_rejects_control_chars() {
        assert!(seatbelt_quote(Path::new("/tmp/a\nb")).is_err());
        assert_eq!(
            seatbelt_quote(Path::new("/tmp/a\"b")).expect("quote"),
            "\"/tmp/a\\\"b\""
        );
    }

    #[test]
    fn bwrap_args_orders_mounts_and_network_flags() {
        let tmp = TempDir::new().expect("tempdir");
        let hidden_dir = tmp.path().join("hidden-dir");
        let hidden_file = tmp.path().join("hidden-file");
        let ro_dir = tmp.path().join("ro");
        let rw_dir = tmp.path().join("rw");
        fs::create_dir_all(&hidden_dir).expect("mkdir");
        fs::write(&hidden_file, "secret").expect("write");
        fs::create_dir_all(&ro_dir).expect("mkdir");
        fs::create_dir_all(&rw_dir).expect("mkdir");

        let rules = vec![
            PathRule::new(&hidden_dir, PathAccess::Hidden),
            PathRule::new(&hidden_file, PathAccess::Hidden),
            PathRule::new(&ro_dir, PathAccess::ReadOnly),
            PathRule::new(&rw_dir, PathAccess::ReadWrite),
        ];
        let offline: Vec<String> = bwrap_args(&rules, &rw_dir, "echo hi", false)
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(!offline.iter().any(|a| a == "--share-net"));
        assert!(offline
            .windows(2)
            .any(|w| w[0] == "--tmpfs" && w[1] == hidden_dir.to_string_lossy()));
        assert!(offline.windows(3).any(|w| w[0] == "--ro-bind"
            && w[1] == "/dev/null"
            && w[2] == hidden_file.to_string_lossy()));
        assert!(offline.windows(3).any(|w| w[0] == "--ro-bind"
            && w[1] == ro_dir.to_string_lossy()
            && w[2] == ro_dir.to_string_lossy()));
        assert!(offline.windows(3).any(|w| w[0] == "--bind"
            && w[1] == rw_dir.to_string_lossy()
            && w[2] == rw_dir.to_string_lossy()));

        let online: Vec<String> = bwrap_args(&rules, &rw_dir, "echo hi", true)
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(online.iter().any(|a| a == "--share-net"));
    }
}
