//! `gitust-git`: Git engine for Gitust.
//!
//! - Read paths (HEAD / root tree / commit history / directory tree / blob inspection) run
//!   in-process on `gitoxide` (`gix`) through a cached `gix::ThreadSafeRepository`.
//! - Agent refs use prepared Git transactions whose expected values are checked under the
//!   actual ref locks; branch creation is create-only and deletion is agent-scoped.
//! - Agent worktrees are standard Git *linked worktrees*. They share refs and objects with the main
//!   repository, so `git gc` is aware of them, agent commits are immediately visible to the main
//!   repository, and bare mirrors are supported.
//! - Inbound read-only mirror synchronization (`sync_inbound_read_only`) fetches upstream branches
//!   and tags while strictly preserving private on-premise `refs/heads/agent/*` branches.

mod ancestry;
mod approvals;
mod branches;
mod browse;
pub use branches::{
    RepoBranch, RepoBranchPage, MAX_BRANCH_CURSOR_BYTES, MAX_BRANCH_NAME_BYTES,
    MAX_BRANCH_PAGE_BYTES,
};
mod checks;
mod config_guard;
mod default_branch;
pub use default_branch::RepositoryDefaultBranch;
mod diff;
mod execution_fence;
mod git_hosting;
mod mirror;
mod mirror_init;
mod outbound;
mod process;
mod ref_transaction;
mod repository_create;
mod repository_identity;
pub use repository_create::PreparedEmpty;
mod repository_import;
pub use repository_import::PreparedImport;
mod repository_registration;
pub use repository_registration::RegistrationReceipt;
mod service_lock;
mod smart_http;
mod ssh_receive_pack;
mod ssh_upload_pack;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_execution_fence;
#[cfg(test)]
mod tests_outbound;
#[cfg(test)]
mod tests_repository_identity;
#[cfg(all(test, unix))]
mod tests_repository_registration;
mod verification;
mod worktree;
mod worktree_commit;
mod worktree_commit_index;
mod worktree_commit_normalize;
mod worktree_commit_paths;
mod worktree_metadata;
mod worktree_origin;
mod worktree_recovery;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gitust_core::CommitSha;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use approvals::WorktreeApproval;
pub use browse::{
    CommitEntry, RepoBlobSnapshot, RepoCommitList, RepoTreeNode, RepoTreeSnapshot,
    MAX_BLOB_VIEW_BYTES, MAX_TREE_ENTRIES,
};
pub use checks::RequiredChecks;
pub use diff::{
    DiffLineKind, RepoDiffFileSummary, RepoDiffLine, RepoDiffSnapshot, MAX_DIFF_BYTES,
    MAX_DIFF_LINES,
};
pub use execution_fence::ExecutionFence;
pub use mirror::InboundMirrorSyncOutcome;
pub(crate) use process::{git_command, run_git};
pub use service_lock::WorktreeServiceLock;
use service_lock::WorktreeServiceLockInner;
pub use smart_http::{
    inspect_receive_pack_updates, ReceivePackRefUpdate, SmartHttpService,
    MAX_SMART_HTTP_BODY_BYTES, SMART_HTTP_TIMEOUT,
};
pub use ssh_receive_pack::{
    SshReceivePack, SshReceivePackCancel, SshReceivePackError, SshReceivePackOutcome,
};
pub use ssh_upload_pack::{
    SshUploadPack, SshUploadPackCancel, SshUploadPackError, SshUploadPackOutcome,
};
#[cfg(all(test, unix))]
mod tests_ssh_receive_pack;
#[cfg(all(test, unix))]
mod tests_ssh_upload_pack;
pub use worktree::{ManagedWorktreeEntry, MaterializedWorktree};
pub use worktree_commit::{PreparedWorktreeCommit, WorktreeCommit};

/// Every branch created on behalf of an agent lives under this namespace. The sync engine keeps
/// this namespace on-premise, and the engine refuses to create or delete refs outside of it.
pub const AGENT_BRANCH_PREFIX: &str = "agent/";
/// Maximum UTF-8 byte length of a managed worktree's persisted display summary.
pub const MAX_PROMPT_SUMMARY_BYTES: usize = 4096;
/// Directory (inside the common git dir) holding agent linked worktrees of bare repositories.
pub(crate) const WORKTREES_DIR_NAME: &str = "gitust-worktrees";
/// Directory (inside the common git dir) holding per-run sandbox scratch space.
pub(crate) const SANDBOX_SCRATCH_DIR_NAME: &str = "gitust-sandbox-scratch";

#[derive(Debug, Error)]
pub enum GitEngineError {
    #[error("invalid default branch request")]
    DefaultBranchInvalid,
    #[error("default branch or selected branch tip changed; refresh before retrying")]
    DefaultBranchConflict,
    #[error("default branch is unsupported or has no commit")]
    DefaultBranchUnsupported,
    #[error("repository identity: {0}")]
    RepositoryIdentity(String),
    #[error("execution lifetime fence: {0}")]
    ExecutionFence(String),
    #[error("verification invalid: {0}")]
    VerificationInvalid(String),
    #[error("worktree approval state error: {0}")]
    ApprovalState(String),
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
        "repository config contains credentials, external includes, URL rewrites, or execution/credential-file settings; \
         refusing to expose or execute unsafe shared config"
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

#[derive(Debug, Clone)]
pub struct RepoHeadIdentity {
    pub head_sha: CommitSha,
    pub branch_name: String,
}

/// The current default ref can be unborn even when other branches contain commits.
#[derive(Debug, Clone)]
pub enum RepositoryHead {
    Commit(RepoHeadIdentity),
    Unborn { branch_name: String },
}

impl RepositoryHead {
    pub fn into_parts(self) -> (String, Option<CommitSha>) {
        match self {
            Self::Commit(head) => (head.branch_name, Some(head.head_sha)),
            Self::Unborn { branch_name } => (branch_name, None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoHeadSnapshot {
    pub head_sha: CommitSha,
    pub branch_name: String,
    pub root_entries: Vec<GitTreeEntry>,
    pub lookup_micros: u64,
}

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
    pub(crate) repository_identity_init: Arc<std::sync::Mutex<()>>,
    pub(crate) fetch_timeout: std::time::Duration,
    pub(crate) fetch_timeout_overridden: bool,
}

impl GitEngine {
    #[must_use]
    pub fn new(repo_path: impl AsRef<Path>) -> Self {
        let path = repo_path.as_ref().to_path_buf();
        let lock = OnceLock::new();
        // fail-open-ok: eager warm-up only; `acquire_repo()` fails closed with `OpenRepo` if the repository is not yet initialized.
        if let Ok(repo) = gix::ThreadSafeRepository::open(&path) {
            // fail-open-ok: freshly created `OnceLock` has no concurrent writer.
            let _ = lock.set(repo);
        }
        Self {
            repo_path: path,
            worktree_base_dir: None,
            shared_repo: Arc::new(lock),
            service_lock: Arc::new(std::sync::Mutex::new(std::sync::Weak::new())),
            repository_identity_init: Arc::new(std::sync::Mutex::new(())),
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

    /// Resolve common metadata for service-owned repository-local state.
    pub fn repository_common_directory(&self) -> Result<PathBuf, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let repo = self.acquire_repo()?;
        {
            let common = Self::common_git_dir(&repo);
            std::fs::canonicalize(&common).map_err(|error| io_err(&common, &error))
        }
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
        // fail-open-ok: benign `OnceLock::set` race when two threads initialize the cache simultaneously; `local` is already opened.
        let _ = self.shared_repo.set(opened);
        Ok(local)
    }

    /// Validates a Git branch name against path traversal, control chars, and ref format rules.
    /// Returns the normalized short name (without `refs/heads/`).
    pub fn validate_branch_name(branch: &str) -> Result<String, GitEngineError> {
        let trimmed = branch.trim();
        let normalized = trimmed
            .strip_prefix("refs/heads/")
            // fail-open-ok: an optional refs/heads/ prefix is normalized before the full branch validation below.
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

    /// Inspect display identity without treating a valid unborn default ref as corruption.
    pub fn inspect_repository_head(&self) -> Result<RepositoryHead, GitEngineError> {
        let repo = self.acquire_repo()?;
        let head = repo
            .head()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
        if head.is_unborn() {
            let invalid = || GitEngineError::HeadResolution("invalid unborn default branch".into());
            let target = std::str::from_utf8(head.referent_name().ok_or_else(invalid)?.as_bstr())
                .map_err(|_| invalid())?;
            let branch = target.strip_prefix("refs/heads/").ok_or_else(invalid)?;
            if branch.len() > 32 * 1024
                || Self::validate_branch_name(branch).map_err(|_| invalid())? != branch
            {
                return Err(invalid());
            }
            return Ok(RepositoryHead::Unborn {
                branch_name: branch.to_owned(),
            });
        }
        let (branch_name, commit) = Self::peel_head_commit(head)?;
        Ok(RepositoryHead::Commit(RepoHeadIdentity {
            head_sha: CommitSha(commit.id().to_string()),
            branch_name,
        }))
    }

    /// Resolves `HEAD` without loading or traversing its root tree; unborn refs remain errors.
    pub fn inspect_head_identity(&self) -> Result<RepoHeadIdentity, GitEngineError> {
        let repo = self.acquire_repo()?;
        let (branch_name, commit) = Self::resolve_head_commit(&repo)?;
        Ok(RepoHeadIdentity {
            head_sha: CommitSha(commit.id().to_string()),
            branch_name,
        })
    }

    fn resolve_head_commit(
        repo: &gix::Repository,
    ) -> Result<(String, gix::Commit<'_>), GitEngineError> {
        let head = repo
            .head()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
        Self::peel_head_commit(head)
    }

    fn peel_head_commit(
        mut head: gix::Head<'_>,
    ) -> Result<(String, gix::Commit<'_>), GitEngineError> {
        let branch_name = head
            .referent_name()
            .map(|r| r.shorten().to_string())
            // fail-open-ok: display label for a detached HEAD only; no approval or write permission is inferred.
            .unwrap_or_else(|| "detached".to_owned());
        let commit = head
            .peel_to_commit_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
        Ok((branch_name, commit))
    }

    /// Inspects the repository's `HEAD` commit and root tree in-process via `gix`.
    pub fn inspect_head_zero_copy(&self) -> Result<RepoHeadSnapshot, GitEngineError> {
        let start = Instant::now();
        let repo = self.acquire_repo()?;
        let (branch_name, commit) = Self::resolve_head_commit(&repo)?;
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

        // fail-open-ok: saturating telemetry conversion; does not authorize access or alter Git state.
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
        // fail-open-ok: path normalization only; the gix repository remains the source of the locator and filesystem operations reject inaccessible paths.
        fs::canonicalize(common).unwrap_or_else(|_| common.to_path_buf())
    }

    pub(crate) fn worktrees_base(&self, repo: &gix::Repository, common_git_dir: &Path) -> PathBuf {
        if let Some(custom) = &self.worktree_base_dir {
            return custom.clone();
        }
        repo.work_dir()
            // fail-open-ok: path normalization only; the gix repository remains the source of the locator and filesystem operations reject inaccessible paths.
            .map(|w| fs::canonicalize(w).unwrap_or_else(|_| w.to_path_buf()))
            .and_then(|w| {
                let name = w.file_name()?.to_string_lossy().into_owned();
                Some(w.parent()?.join(format!(".{name}.gitust-worktrees")))
            })
            // fail-open-ok: bare repositories intentionally store managed checkouts beneath their common Git directory.
            .unwrap_or_else(|| common_git_dir.join(WORKTREES_DIR_NAME))
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
    // fail-open-ok: when the leaf does not exist yet, walks up ancestors below to canonicalize the deepest existing parent.
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        // fail-open-ok: probes each ancestor until the first existing directory is found.
        if let Ok(mut canonical_base) = fs::canonicalize(ancestor) {
            // fail-open-ok: `ancestor` comes from `path.ancestors()`, so `strip_prefix` is guaranteed to succeed.
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
