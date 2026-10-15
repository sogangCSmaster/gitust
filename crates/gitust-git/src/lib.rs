//! `gitust-git`: Git engine for Gitust.
//!
//! - Read paths (HEAD / root tree / commit history / directory tree / blob inspection) run
//!   in-process on `gitoxide` (`gix`) through a cached `gix::ThreadSafeRepository`.
//! - Agent branches are created and deleted with `gix` reference transactions: create-only
//!   (`PreviousValue::MustNotExist`), scoped to the `agent/` namespace, with reflog entries.
//! - Agent worktrees are standard Git *linked worktrees*. They share refs and objects with the main
//!   repository, so `git gc` is aware of them, agent commits are immediately visible to the main
//!   repository, and bare mirrors are supported.
//! - Inbound read-only mirror synchronization (`sync_inbound_read_only`) fetches upstream branches
//!   and tags while strictly preserving private on-premise `refs/heads/agent/*` branches.

mod ancestry;
mod browse;
mod config_guard;
mod diff;
mod mirror;
#[cfg(test)]
mod tests;
mod worktree;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gitust_core::CommitSha;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use browse::{
    CommitEntry, RepoBlobSnapshot, RepoCommitList, RepoTreeNode, RepoTreeSnapshot,
    MAX_BLOB_VIEW_BYTES, MAX_TREE_ENTRIES,
};
pub use diff::{
    DiffLineKind, RepoDiffFileSummary, RepoDiffLine, RepoDiffSnapshot, MAX_DIFF_BYTES,
    MAX_DIFF_LINES,
};
pub use mirror::InboundMirrorSyncOutcome;
pub use worktree::{ManagedWorktreeEntry, MaterializedWorktree};

/// Every branch created on behalf of an agent lives under this namespace. The sync engine keeps
/// this namespace on-premise, and the engine refuses to create or delete refs outside of it.
pub const AGENT_BRANCH_PREFIX: &str = "agent/";
/// Directory (inside the common git dir) holding agent linked worktrees of bare repositories.
pub(crate) const WORKTREES_DIR_NAME: &str = "gitust-worktrees";
/// Directory (inside the common git dir) holding per-run sandbox scratch space.
pub(crate) const SANDBOX_SCRATCH_DIR_NAME: &str = "gitust-sandbox-scratch";

#[derive(Debug, Error)]
pub enum GitEngineError {
    #[error("failed to open git repository at '{path}': {message}")]
    OpenRepo { path: String, message: String },

    #[error("failed to resolve HEAD commit: {0}")]
    HeadResolution(String),

    #[error("failed to read git tree: {0}")]
    TreeTraversal(String),

    #[error("invalid or unsafe git branch name '{0}'")]
    InvalidBranchName(String),

    #[error("invalid or unsafe git revision '{0}'")]
    InvalidRevision(String),

    #[error("git revision '{0}' not found")]
    RevisionNotFound(String),

    #[error("invalid or unsafe repository path '{0}'")]
    InvalidRepoPath(String),

    #[error("repository path '{0}' not found")]
    PathNotFound(String),

    #[error("invalid or unsafe upstream mirror URL '{0}'")]
    InvalidRemoteUrl(String),

    #[error("branch '{0}' is outside the '{AGENT_BRANCH_PREFIX}' namespace")]
    BranchNotAgentScoped(String),

    #[error("base branch '{0}' does not exist")]
    BaseBranchNotFound(String),

    #[error("branch '{0}' already exists")]
    BranchAlreadyExists(String),

    #[error("merge conflict: {0}")]
    MergeConflict(String),

    #[error("reference transaction failed: {0}")]
    RefTransaction(String),

    #[error(
        "repository config contains embedded credentials (URL userinfo or http.extraHeader); \
         agent worktrees share this config, so refusing to expose it"
    )]
    CredentialsInRepoConfig,

    #[error("`git {args}` failed: {message}")]
    GitCommand { args: String, message: String },

    #[error("path '{0}' is not a worktree managed by this engine")]
    WorktreeNotManaged(String),

    #[error("filesystem error at '{path}': {message}")]
    Io { path: String, message: String },

    #[error(
        "another gitust worktree service (`gitust serve` or `gitust mcp`) already holds the \
         repository lock at '{}'; connect via HTTP MCP (`/mcp`) instead",
        .0.display()
    )]
    WorktreeServiceBusy(PathBuf),

    #[error("git integrity check (`git fsck --full --strict`) failed at '{path}': {details}")]
    FsckFailed { path: String, details: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitTreeEntry {
    pub filename: String,
    pub oid: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoHeadSnapshot {
    pub head_sha: CommitSha,
    pub branch_name: String,
    pub root_entries: Vec<GitTreeEntry>,
    pub lookup_micros: u64,
}

/// Exclusive OS file lock guard for `<common_git_dir>/gitust-worktree-service.lock`.
///
/// Explicitly calls `File::unlock()` (`flock(fd, LOCK_UN)`) on [`Drop`] before closing the file
/// descriptor so that concurrent `fork()`/`execve()` calls in other threads never prolong the lock.
#[derive(Debug)]
pub(crate) struct WorktreeServiceLockInner(fs::File);

impl Drop for WorktreeServiceLockInner {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Debug, Clone)]
pub struct WorktreeServiceLock(#[allow(dead_code)] Arc<WorktreeServiceLockInner>);

/// Default overall ceiling for incremental `git fetch` (5 minutes), paired with 30s stall detection.
pub const DEFAULT_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// Default overall ceiling for initial mirror `git fetch` of large monorepos (30 minutes).
pub const INITIAL_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1800);

/// Git engine wrapping a shared `gix::ThreadSafeRepository` for object database reuse across
/// threads and requests.
#[derive(Debug, Clone)]
pub struct GitEngine {
    pub(crate) repo_path: PathBuf,
    pub(crate) worktree_base_dir: Option<PathBuf>,
    pub(crate) shared_repo: Arc<OnceLock<gix::ThreadSafeRepository>>,
    pub(crate) service_lock: Arc<std::sync::Mutex<std::sync::Weak<WorktreeServiceLockInner>>>,
    pub(crate) fetch_timeout: std::time::Duration,
    pub(crate) fetch_timeout_overridden: bool,
}

impl GitEngine {
    #[must_use]
    pub fn new(repo_path: impl AsRef<Path>) -> Self {
        let path = repo_path.as_ref().to_path_buf();
        let lock = OnceLock::new();
        if let Ok(repo) = gix::ThreadSafeRepository::open(&path) {
            let _ = lock.set(repo);
        }
        Self {
            repo_path: path,
            worktree_base_dir: None,
            shared_repo: Arc::new(lock),
            service_lock: Arc::new(std::sync::Mutex::new(std::sync::Weak::new())),
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
            fetch_timeout_overridden: false,
        }
    }

    /// Overrides where agent worktrees are created.
    #[must_use]
    pub fn with_worktree_base_dir(mut self, base_dir: impl AsRef<Path>) -> Self {
        self.worktree_base_dir = Some(base_dir.as_ref().to_path_buf());
        self
    }

    /// Overrides the network/command timeout ceiling for `git fetch` in inbound mirror sync.
    #[must_use]
    pub fn with_fetch_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.fetch_timeout = timeout;
        self.fetch_timeout_overridden = true;
        self
    }

    #[must_use]
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Acquires a thread-local `gix::Repository` from the shared `ThreadSafeRepository`,
    /// opening and caching it on first access.
    pub(crate) fn acquire_repo(&self) -> Result<gix::Repository, GitEngineError> {
        if let Some(shared) = self.shared_repo.get() {
            return Ok(shared.to_thread_local());
        }
        let opened = gix::ThreadSafeRepository::open(&self.repo_path).map_err(|e| {
            GitEngineError::OpenRepo {
                path: self.repo_path.display().to_string(),
                message: e.to_string(),
            }
        })?;
        let local = opened.to_thread_local();
        let _ = self.shared_repo.set(opened);
        Ok(local)
    }

    /// Validates a Git branch name against path traversal, control chars, and ref format rules.
    /// Returns the normalized short name (without `refs/heads/`).
    pub fn validate_branch_name(branch: &str) -> Result<String, GitEngineError> {
        let trimmed = branch.trim();
        let normalized = trimmed
            .strip_prefix("refs/heads/")
            .unwrap_or(trimmed)
            .trim_matches('/');

        if normalized.is_empty()
            || normalized == "@"
            || normalized == "HEAD"
            || normalized.starts_with('-')
            || normalized.starts_with("refs/")
            || normalized.contains("..")
            || normalized.contains("//")
            || normalized.contains("@{")
            || normalized.ends_with('.')
            || normalized
                .split('/')
                .any(|seg| seg.is_empty() || seg.starts_with('.') || seg.ends_with(".lock"))
            || normalized.chars().any(|c| {
                c.is_control()
                    || c.is_whitespace()
                    || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
            })
        {
            return Err(GitEngineError::InvalidBranchName(branch.to_owned()));
        }

        Ok(normalized.to_owned())
    }

    /// Validates a branch name and additionally requires it to live under `agent/<something>`.
    pub fn validate_agent_branch_name(branch: &str) -> Result<String, GitEngineError> {
        let clean = Self::validate_branch_name(branch)?;
        match clean.strip_prefix(AGENT_BRANCH_PREFIX) {
            Some(rest) if !rest.is_empty() => Ok(clean),
            _ => Err(GitEngineError::BranchNotAgentScoped(clean)),
        }
    }

    /// Inspects the repository's `HEAD` commit and root tree in-process via `gix`.
    pub fn inspect_head_zero_copy(&self) -> Result<RepoHeadSnapshot, GitEngineError> {
        let start = Instant::now();
        let repo = self.acquire_repo()?;

        let mut head = repo
            .head()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;

        let branch_name = head
            .referent_name()
            .map(|r| r.shorten().to_string())
            .unwrap_or_else(|| "detached".to_owned());

        let commit = head
            .peel_to_commit_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;

        let head_sha = CommitSha(commit.id().to_string());
        let tree = commit
            .tree()
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;

        let mut root_entries = Vec::with_capacity(32);
        for entry_res in tree.iter() {
            let entry = entry_res.map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
            root_entries.push(GitTreeEntry {
                filename: entry.filename().to_string(),
                oid: entry.oid().to_string(),
                is_dir: entry.mode().is_tree(),
            });
        }

        let lookup_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);

        Ok(RepoHeadSnapshot {
            head_sha,
            branch_name,
            root_entries,
            lookup_micros,
        })
    }

    pub(crate) fn common_git_dir(repo: &gix::Repository) -> PathBuf {
        let common = repo.common_dir();
        fs::canonicalize(common).unwrap_or_else(|_| common.to_path_buf())
    }

    pub(crate) fn worktrees_base(&self, repo: &gix::Repository, common_git_dir: &Path) -> PathBuf {
        if let Some(custom) = &self.worktree_base_dir {
            return custom.clone();
        }
        repo.work_dir()
            .map(|w| fs::canonicalize(w).unwrap_or_else(|_| w.to_path_buf()))
            .and_then(|w| {
                let name = w.file_name()?.to_string_lossy().into_owned();
                Some(w.parent()?.join(format!(".{name}.gitust-worktrees")))
            })
            .unwrap_or_else(|| common_git_dir.join(WORKTREES_DIR_NAME))
    }

    /// Acquires an exclusive non-blocking OS file lock (`gitust-worktree-service.lock`) inside
    /// the canonical common `.git` directory.
    ///
    /// Re-entrant across clones of the same `GitEngine` instance (e.g. inside a single `AppState`),
    /// while strictly rejecting any second `GitEngine` instance or external OS process.
    pub fn lock_worktree_service(&self) -> Result<WorktreeServiceLock, GitEngineError> {
        let mut slot = self.service_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = slot.upgrade() {
            return Ok(WorktreeServiceLock(existing));
        }
        let repo = self.acquire_repo()?;
        let path = Self::common_git_dir(&repo).join("gitust-worktree-service.lock");
        let lock = fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io_err(&path, &e))?;
        match lock.try_lock() {
            Ok(()) => {
                let inner = Arc::new(WorktreeServiceLockInner(lock));
                *slot = Arc::downgrade(&inner);
                Ok(WorktreeServiceLock(inner))
            }
            Err(fs::TryLockError::WouldBlock) => Err(GitEngineError::WorktreeServiceBusy(path)),
            Err(fs::TryLockError::Error(e)) => Err(io_err(&path, &e)),
        }
    }

    /// Runs `git fsck --full --strict` against a repository or worktree path (Verification Gate 7.4).
    pub fn verify_git_fsck(repo_or_worktree_path: impl AsRef<Path>) -> Result<(), GitEngineError> {
        let path = repo_or_worktree_path.as_ref();
        run_git(
            path,
            [
                OsStr::new("fsck"),
                OsStr::new("--full"),
                OsStr::new("--strict"),
            ],
        )
        .map(|_| ())
        .map_err(|e| GitEngineError::FsckFailed {
            path: path.display().to_string(),
            details: e.to_string(),
        })
    }
}

pub(crate) fn canonicalize_existing_ancestors(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        if let Ok(mut canonical_base) = fs::canonicalize(ancestor) {
            if let Ok(suffix) = path.strip_prefix(ancestor) {
                for comp in suffix.components() {
                    match comp {
                        std::path::Component::Normal(part) => canonical_base.push(part),
                        _ => return path.to_path_buf(),
                    }
                }
                return canonical_base;
            }
        }
    }
    path.to_path_buf()
}

pub(crate) fn io_err(path: &Path, e: &std::io::Error) -> GitEngineError {
    GitEngineError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

/// Runs the host `git` binary with a hardened, deterministic environment:
/// no system/global config, no hooks, no fsmonitor, no prompts, and no inherited `GIT_*` overrides.
pub(crate) fn run_git<I, S>(dir: &Path, args: I) -> Result<Vec<u8>, GitEngineError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(&args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| GitEngineError::GitCommand {
            args: display_args(&args),
            message: e.to_string(),
        })?;

    if !output.status.success() {
        return Err(GitEngineError::GitCommand {
            args: display_args(&args),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output.stdout)
}

fn display_args(args: &[std::ffi::OsString]) -> String {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ensures a committer identity exists for reflog entries, without touching any config on disk.
pub(crate) fn ensure_reflog_identity(repo: &mut gix::Repository) -> Result<(), GitEngineError> {
    if matches!(repo.committer(), Some(Ok(_))) {
        return Ok(());
    }
    let mut snapshot = repo.config_snapshot_mut();
    snapshot
        .set_value(&gix::config::tree::Committer::NAME, "gitust")
        .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;
    snapshot
        .set_value(&gix::config::tree::Committer::EMAIL, "gitust@localhost")
        .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;
    snapshot
        .commit()
        .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;
    Ok(())
}
