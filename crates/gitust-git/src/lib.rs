//! `gitust-git`: Git engine for Gitust.
//!
//! - Read paths (HEAD / root tree inspection) run in-process on `gitoxide` (`gix`) through a cached
//!   `gix::ThreadSafeRepository`.
//! - Agent branches are created and deleted with `gix` reference transactions: create-only
//!   (`PreviousValue::MustNotExist`), scoped to the `agent/` namespace, with reflog entries.
//! - Agent worktrees are standard Git *linked worktrees*. They share refs and objects with the main
//!   repository, so `git gc` is aware of them, agent commits are immediately visible to the main
//!   repository, and bare mirrors are supported.
//!
//! Temporary exception to the pure-Rust goal: `gix` 0.70 exposes no public linked-worktree
//! creation/checkout API, so worktree add/remove/list shell out to the `git` binary (which is
//! already required for the `git fsck` verification gate). Replace once `gix` grows the API.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use chrono::Utc;
use gitust_core::{AgentKind, AgentWorktree, CommitSha, RepoId, WorktreeId, WorktreeStatus};
use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};
use gix::refs::{FullName, Target};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Every branch created on behalf of an agent lives under this namespace. The sync engine keeps
/// this namespace on-premise, and the engine refuses to create or delete refs outside of it.
pub const AGENT_BRANCH_PREFIX: &str = "agent/";
/// Directory (inside the common git dir) holding agent linked worktrees of bare repositories.
const WORKTREES_DIR_NAME: &str = "gitust-worktrees";
/// Directory (inside the common git dir) holding per-run sandbox scratch space.
const SANDBOX_SCRATCH_DIR_NAME: &str = "gitust-sandbox-scratch";

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

    #[error("branch '{0}' is outside the '{AGENT_BRANCH_PREFIX}' namespace")]
    BranchNotAgentScoped(String),

    #[error("base branch '{0}' does not exist")]
    BaseBranchNotFound(String),

    #[error("branch '{0}' already exists")]
    BranchAlreadyExists(String),

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

    #[error("git integrity check (`git fsck --full --strict`) failed at '{path}': {details}")]
    FsckFailed { path: String, details: String },
}

/// A linked worktree created for an agent, plus the paths a sandbox needs to confine it.
#[derive(Debug, Clone)]
pub struct MaterializedWorktree {
    pub worktree: AgentWorktree,
    /// Checkout directory of the linked worktree (sandbox read-write root).
    pub worktree_dir: PathBuf,
    /// This worktree's admin dir inside the shared git dir (`<common>/worktrees/<name>`).
    pub worktree_admin_dir: PathBuf,
    /// Shared `.git` directory (sandbox read-only: objects, refs, config).
    pub common_git_dir: PathBuf,
    /// Main repository working tree, `None` for bare repositories (sandbox hidden).
    pub main_workdir: Option<PathBuf>,
    /// Parent directory of all agent worktrees (sandbox hidden, except the own worktree).
    pub worktrees_base: PathBuf,
    /// Parent directory of per-run sandbox scratch dirs.
    pub sandbox_scratch_base: PathBuf,
}

/// An agent worktree discovered on disk via `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedWorktreeEntry {
    pub worktree_dir: PathBuf,
    pub branch: String,
    pub head: CommitSha,
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

/// Git engine wrapping a shared `gix::ThreadSafeRepository` for object database reuse across
/// threads and requests.
#[derive(Debug, Clone)]
pub struct GitEngine {
    repo_path: PathBuf,
    worktree_base_dir: Option<PathBuf>,
    shared_repo: Arc<OnceLock<gix::ThreadSafeRepository>>,
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
        }
    }

    /// Overrides where agent worktrees are created.
    ///
    /// Default: for a repository with a working tree at `<parent>/<name>`, a sibling directory
    /// `<parent>/.<name>.gitust-worktrees`; for a bare repository, `<git dir>/gitust-worktrees`.
    /// Worktrees are deliberately kept outside the main working tree: tools such as cargo, npm
    /// and rustfmt search parent directories for configuration and would otherwise pick up (or
    /// fail on) the main repository's files.
    #[must_use]
    pub fn with_worktree_base_dir(mut self, base_dir: impl AsRef<Path>) -> Self {
        self.worktree_base_dir = Some(base_dir.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Acquires a thread-local `gix::Repository` from the shared `ThreadSafeRepository`,
    /// opening and caching it on first access.
    fn acquire_repo(&self) -> Result<gix::Repository, GitEngineError> {
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

    fn common_git_dir(repo: &gix::Repository) -> PathBuf {
        let common = repo.common_dir();
        fs::canonicalize(common).unwrap_or_else(|_| common.to_path_buf())
    }

    fn worktrees_base(&self, repo: &gix::Repository, common_git_dir: &Path) -> PathBuf {
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

    /// Creates a new `agent/` branch from `base_branch` and checks it out as a Git linked worktree.
    ///
    /// - The branch is created with a `gix` transaction that fails if it already exists, so
    ///   existing branches can never be overwritten.
    /// - The worktree is a clean checkout of the base commit; uncommitted or untracked files of
    ///   the main working tree are never copied.
    /// - Refuses to run when the shared repository config holds embedded credentials, because the
    ///   sandboxed agent can read that config.
    pub fn materialize_agent_worktree(
        &self,
        worktree_id: WorktreeId,
        repo_id: RepoId,
        agent: AgentKind,
        branch: &str,
        base_branch: &str,
        prompt_summary: impl Into<String>,
    ) -> Result<MaterializedWorktree, GitEngineError> {
        let start = Instant::now();
        let clean_branch = Self::validate_agent_branch_name(branch)?;
        let clean_base = Self::validate_branch_name(base_branch)?;

        let mut repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        reject_credentials_in_config(&common_git_dir)?;
        ensure_reflog_identity(&mut repo)?;

        let base_ref = format!("refs/heads/{clean_base}");
        let base_id = repo
            .find_reference(base_ref.as_str())
            .map_err(|_| GitEngineError::BaseBranchNotFound(clean_base.clone()))?
            .peel_to_id_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .detach();

        let full_branch = format!("refs/heads/{clean_branch}");
        let branch_exists = repo
            .try_find_reference(full_branch.as_str())
            .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?
            .is_some();
        if branch_exists {
            return Err(GitEngineError::BranchAlreadyExists(clean_branch));
        }
        if let Err(e) = repo.reference(
            full_branch.as_str(),
            base_id,
            PreviousValue::MustNotExist,
            format!("gitust: create agent branch from {clean_base}"),
        ) {
            // A concurrent creator won the race between the pre-check and the transaction.
            if matches!(repo.try_find_reference(full_branch.as_str()), Ok(Some(_))) {
                return Err(GitEngineError::BranchAlreadyExists(clean_branch));
            }
            return Err(GitEngineError::RefTransaction(e.to_string()));
        }

        let worktrees_base = self.worktrees_base(&repo, &common_git_dir);
        let worktree_dir = worktrees_base.join(worktree_id.0.to_string());
        let checkout = fs::create_dir_all(&worktrees_base)
            .map_err(|e| io_err(&worktrees_base, &e))
            .and_then(|()| {
                run_git(
                    &self.repo_path,
                    [
                        OsStr::new("worktree"),
                        OsStr::new("add"),
                        OsStr::new("--quiet"),
                        worktree_dir.as_os_str(),
                        OsStr::new(clean_branch.as_str()),
                    ],
                )
            })
            .and_then(|_| read_worktree_admin_dir(&worktree_dir));
        let worktree_admin_dir = match checkout {
            Ok(dir) => dir,
            Err(e) => {
                let _ = run_git(
                    &self.repo_path,
                    [
                        OsStr::new("worktree"),
                        OsStr::new("remove"),
                        OsStr::new("--force"),
                        worktree_dir.as_os_str(),
                    ],
                );
                let _ = fs::remove_dir_all(&worktree_dir);
                let _ = run_git(
                    &self.repo_path,
                    [OsStr::new("worktree"), OsStr::new("prune")],
                );
                // Roll back the branch only if it still points where we created it.
                let _ = delete_branch_ref(
                    &repo,
                    &full_branch,
                    PreviousValue::MustExistAndMatch(Target::Object(base_id)),
                );
                return Err(e);
            }
        };

        let worktree_dir = fs::canonicalize(&worktree_dir).unwrap_or(worktree_dir);
        let worktrees_base = fs::canonicalize(&worktrees_base).unwrap_or(worktrees_base);
        let main_workdir = repo
            .work_dir()
            .map(|w| fs::canonicalize(w).unwrap_or_else(|_| w.to_path_buf()));
        let materialize_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);

        Ok(MaterializedWorktree {
            worktree: AgentWorktree {
                id: worktree_id,
                repo_id,
                agent,
                branch: clean_branch,
                base_branch: clean_base,
                base_commit: CommitSha(base_id.to_string()),
                prompt_summary: prompt_summary.into(),
                materialize_micros,
                status: WorktreeStatus::Pending,
                last_sandbox_duration_ms: 0,
                tests_passed: 0,
                tests_total: 0,
                files_changed: 0,
                additions: 0,
                deletions: 0,
                github_synced: false,
                created_at: Utc::now(),
            },
            worktree_dir,
            worktree_admin_dir,
            sandbox_scratch_base: common_git_dir.join(SANDBOX_SCRATCH_DIR_NAME),
            common_git_dir,
            main_workdir,
            worktrees_base,
        })
    }

    /// Fails closed if the shared repository config currently holds credentials. Call before
    /// every sandbox run: the config can change after a worktree was created.
    pub fn ensure_config_has_no_credentials(&self) -> Result<(), GitEngineError> {
        let repo = self.acquire_repo()?;
        reject_credentials_in_config(&Self::common_git_dir(&repo))
    }

    /// Removes an agent linked worktree checkout (discarding uncommitted changes).
    ///
    /// The `agent/` branch is deleted only if it still points at `unchanged_from` (i.e. the agent
    /// produced no commits); otherwise it is kept so committed work is never destroyed.
    /// Returns whether the branch was deleted. Refuses paths outside the worktree base directory.
    pub fn remove_agent_worktree(
        &self,
        worktree_dir: &Path,
        branch: &str,
        unchanged_from: &CommitSha,
    ) -> Result<bool, GitEngineError> {
        let clean_branch = Self::validate_agent_branch_name(branch)?;
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        let base = self.worktrees_base(&repo, &common_git_dir);
        let base = fs::canonicalize(&base).unwrap_or(base);
        let target = fs::canonicalize(worktree_dir).unwrap_or_else(|_| worktree_dir.to_path_buf());
        if target.parent() != Some(base.as_path()) {
            return Err(GitEngineError::WorktreeNotManaged(
                worktree_dir.display().to_string(),
            ));
        }

        if target.exists() {
            run_git(
                &self.repo_path,
                [
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    OsStr::new("--force"),
                    target.as_os_str(),
                ],
            )?;
        }
        run_git(
            &self.repo_path,
            [OsStr::new("worktree"), OsStr::new("prune")],
        )?;
        // Best effort: only succeeds when no other worktree remains (`git worktree add`
        // recreates missing parent directories, so a concurrent create is unaffected).
        let _ = fs::remove_dir(&base);

        let full_branch = format!("refs/heads/{clean_branch}");
        let Some(mut current) = repo
            .try_find_reference(full_branch.as_str())
            .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?
        else {
            return Ok(false);
        };
        let tip = current
            .peel_to_id_in_place()
            .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?
            .detach();
        if tip.to_string() != unchanged_from.0 {
            return Ok(false);
        }
        delete_branch_ref(
            &repo,
            &full_branch,
            PreviousValue::MustExistAndMatch(Target::Object(tip)),
        )?;
        Ok(true)
    }

    /// Lists agent worktrees managed by this engine (linked worktrees under the worktree base
    /// directory whose branch is in the `agent/` namespace).
    pub fn list_agent_worktrees(&self) -> Result<Vec<ManagedWorktreeEntry>, GitEngineError> {
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        let base = self.worktrees_base(&repo, &common_git_dir);
        let base = fs::canonicalize(&base).unwrap_or(base);

        let stdout = run_git(
            &self.repo_path,
            [
                OsStr::new("worktree"),
                OsStr::new("list"),
                OsStr::new("--porcelain"),
                OsStr::new("-z"),
            ],
        )?;
        Ok(parse_worktree_list(&stdout)
            .into_iter()
            .filter(|e| e.worktree_dir.parent() == Some(base.as_path()))
            .collect())
    }

    /// Reconstructs [`MaterializedWorktree`] descriptors for existing managed `agent/` worktrees
    /// on disk (e.g. across server restarts), pruning stale Git worktree admin entries first.
    pub fn recover_agent_worktrees(
        &self,
        repo_id: RepoId,
    ) -> Result<Vec<MaterializedWorktree>, GitEngineError> {
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        let _ = run_git(
            &self.repo_path,
            [OsStr::new("worktree"), OsStr::new("prune")],
        );
        let worktrees_base = self.worktrees_base(&repo, &common_git_dir);
        let worktrees_base = fs::canonicalize(&worktrees_base).unwrap_or(worktrees_base);
        let main_workdir = repo
            .work_dir()
            .map(|w| fs::canonicalize(w).unwrap_or_else(|_| w.to_path_buf()));
        let sandbox_scratch_base = common_git_dir.join(SANDBOX_SCRATCH_DIR_NAME);

        let entries = self.list_agent_worktrees()?;
        let mut recovered = Vec::with_capacity(entries.len());
        for entry in entries {
            let Ok(worktree_admin_dir) = read_worktree_admin_dir(&entry.worktree_dir) else {
                continue;
            };
            let worktree_id = entry
                .worktree_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(WorktreeId::parse_str)
                .unwrap_or_else(WorktreeId::new);
            let (base_branch, base_commit) =
                read_branch_creation_reflog(&common_git_dir, &entry.branch);
            let agent = infer_agent_kind(&entry.branch);

            recovered.push(MaterializedWorktree {
                worktree: AgentWorktree {
                    id: worktree_id,
                    repo_id,
                    agent,
                    branch: entry.branch,
                    base_branch,
                    base_commit,
                    prompt_summary: "recovered from disk".to_owned(),
                    materialize_micros: 0,
                    status: WorktreeStatus::Pending,
                    last_sandbox_duration_ms: 0,
                    tests_passed: 0,
                    tests_total: 0,
                    files_changed: 0,
                    additions: 0,
                    deletions: 0,
                    github_synced: false,
                    created_at: Utc::now(),
                },
                worktree_dir: entry.worktree_dir,
                worktree_admin_dir,
                common_git_dir: common_git_dir.clone(),
                main_workdir: main_workdir.clone(),
                worktrees_base: worktrees_base.clone(),
                sandbox_scratch_base: sandbox_scratch_base.clone(),
            });
        }
        Ok(recovered)
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

fn io_err(path: &Path, e: &std::io::Error) -> GitEngineError {
    GitEngineError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

/// Runs the host `git` binary with a hardened, deterministic environment:
/// no system/global config, no hooks, no fsmonitor, no prompts, and no inherited `GIT_*` overrides.
fn run_git<I, S>(dir: &Path, args: I) -> Result<Vec<u8>, GitEngineError>
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
fn ensure_reflog_identity(repo: &mut gix::Repository) -> Result<(), GitEngineError> {
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

/// Deletes an `agent/` branch ref (with reflog) using a `gix` transaction.
fn delete_branch_ref(
    repo: &gix::Repository,
    full_branch: &str,
    expected: PreviousValue,
) -> Result<(), GitEngineError> {
    let short = full_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(full_branch);
    if !short.starts_with(AGENT_BRANCH_PREFIX) {
        return Err(GitEngineError::BranchNotAgentScoped(short.to_owned()));
    }
    let name: FullName = full_branch
        .try_into()
        .map_err(|_| GitEngineError::InvalidBranchName(full_branch.to_owned()))?;
    repo.edit_reference(RefEdit {
        change: Change::Delete {
            expected,
            log: RefLog::AndReference,
        },
        name,
        deref: false,
    })
    .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;
    Ok(())
}

/// Fails closed if the shared repository config contains credentials an agent could read:
/// userinfo in `http(s)://` URLs (`https://user:token@host`, `https://token@host`) or any
/// `http.extraHeader` setting.
///
/// Limitation: `include.path` / `includeIf` targets are not followed.
fn reject_credentials_in_config(common_git_dir: &Path) -> Result<(), GitEngineError> {
    let config_path = common_git_dir.join("config");
    let text = match fs::read_to_string(&config_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_err(&config_path, &e)),
    };
    if config_contains_credentials(&text) {
        return Err(GitEngineError::CredentialsInRepoConfig);
    }
    Ok(())
}

/// Resolves `<worktree>/.git` (`gitdir: <common>/worktrees/<name>`) to the admin dir path.
fn read_worktree_admin_dir(worktree_dir: &Path) -> Result<PathBuf, GitEngineError> {
    let pointer = worktree_dir.join(".git");
    let text = fs::read_to_string(&pointer).map_err(|e| io_err(&pointer, &e))?;
    let raw = text
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| GitEngineError::Io {
            path: pointer.display().to_string(),
            message: "not a linked worktree pointer".to_owned(),
        })?;
    let path = Path::new(raw);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree_dir.join(path)
    };
    fs::canonicalize(&path).map_err(|e| io_err(&path, &e))
}

fn config_contains_credentials(text: &str) -> bool {
    for raw in text.lines() {
        let line = raw.trim().to_ascii_lowercase();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.contains("extraheader") {
            return true;
        }
        // `credential.helper = store [--file ...]` keeps plaintext credentials on disk.
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "helper" && value.trim().starts_with("store") {
                return true;
            }
        }
        for scheme in ["http://", "https://"] {
            let mut rest = line.as_str();
            while let Some(idx) = rest.find(scheme) {
                let after = &rest[idx + scheme.len()..];
                let authority_end = after
                    .find(|c: char| c == '/' || c == '"' || c.is_whitespace())
                    .unwrap_or(after.len());
                if after[..authority_end].contains('@') {
                    return true;
                }
                rest = &after[authority_end..];
            }
        }
    }
    false
}

/// Reads the initial reflog line of `refs/heads/<branch>` to recover `base_branch` and
/// `base_commit` across restarts. If the reflog is absent, returns an empty `base_commit` so
/// [`GitEngine::remove_agent_worktree`] preserves the branch on discard.
fn read_branch_creation_reflog(common_git_dir: &Path, branch: &str) -> (String, CommitSha) {
    use std::io::{BufRead, BufReader};
    let default = || ("main".to_owned(), CommitSha(String::new()));
    let Ok(clean) = GitEngine::validate_agent_branch_name(branch) else {
        return default();
    };
    let log_path = common_git_dir.join("logs/refs/heads").join(&clean);
    let Ok(file) = fs::File::open(log_path) else {
        return default();
    };
    let mut first_line = String::new();
    if BufReader::new(file).read_line(&mut first_line).is_err() || first_line.is_empty() {
        return default();
    }
    let first_line = first_line.trim_end_matches(['\r', '\n']);
    let (meta, msg) = first_line.split_once('\t').unwrap_or((first_line, ""));
    let mut parts = meta.split_whitespace();
    let (Some(old_oid), Some(new_oid)) = (parts.next(), parts.next()) else {
        return default();
    };
    if old_oid.is_empty()
        || !old_oid.chars().all(|c| c == '0')
        || new_oid.is_empty()
        || !new_oid.chars().all(|c| c.is_ascii_hexdigit())
    {
        return default();
    }
    let base_branch = msg
        .trim()
        .strip_prefix("gitust: create agent branch from ")
        .filter(|b| !b.is_empty())
        .unwrap_or("main")
        .to_owned();
    (base_branch, CommitSha(new_oid.to_owned()))
}

fn infer_agent_kind(branch: &str) -> AgentKind {
    let rest = branch.strip_prefix(AGENT_BRANCH_PREFIX).unwrap_or(branch);
    let seg = rest.split('/').next().unwrap_or("");
    if seg.eq_ignore_ascii_case("cursor") {
        AgentKind::CursorAgent
    } else if seg.eq_ignore_ascii_case("codex") {
        AgentKind::CodexSwarm
    } else if seg.eq_ignore_ascii_case("cli") || seg.eq_ignore_ascii_case("bot") {
        AgentKind::InternalBot
    } else {
        AgentKind::ClaudeCode
    }
}

/// Parses `git worktree list --porcelain -z`, keeping only entries on `agent/` branches.
fn parse_worktree_list(stdout: &[u8]) -> Vec<ManagedWorktreeEntry> {
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut head: Option<String> = None;
    let mut branch: Option<String> = None;

    let mut flush =
        |path: &mut Option<PathBuf>, head: &mut Option<String>, branch: &mut Option<String>| {
            if let (Some(p), Some(h), Some(b)) = (path.take(), head.take(), branch.take()) {
                if let Some(short) = b.strip_prefix("refs/heads/") {
                    if short.starts_with(AGENT_BRANCH_PREFIX) {
                        entries.push(ManagedWorktreeEntry {
                            worktree_dir: fs::canonicalize(&p).unwrap_or(p),
                            branch: short.to_owned(),
                            head: CommitSha(h),
                        });
                    }
                }
            }
            *path = None;
            *head = None;
            *branch = None;
        };

    for field in stdout.split(|b| *b == 0) {
        let field = String::from_utf8_lossy(field);
        if field.is_empty() {
            flush(&mut path, &mut head, &mut branch);
        } else if let Some(p) = field.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(h) = field.strip_prefix("HEAD ") {
            head = Some(h.to_owned());
        } else if let Some(b) = field.strip_prefix("branch ") {
            branch = Some(b.to_owned());
        }
    }
    flush(&mut path, &mut head, &mut branch);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Gitust Test")
            .env("GIT_AUTHOR_EMAIL", "test@company.internal")
            .env("GIT_COMMITTER_NAME", "Gitust Test")
            .env("GIT_COMMITTER_EMAIL", "test@company.internal")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir.join("src")).expect("mkdir");
        git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").expect("write");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "initial commit"]);
    }

    fn materialize(
        engine: &GitEngine,
        branch: &str,
    ) -> Result<MaterializedWorktree, GitEngineError> {
        engine.materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::ClaudeCode,
            branch,
            "main",
            "test",
        )
    }

    #[test]
    fn gate_agent_worktree_is_clean_checkout_create_only_and_shares_refs() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);
        let base_commit = git(&repo_dir, &["rev-parse", "main"]);
        git(&repo_dir, &["branch", "release/1.0"]);

        // Dirty main working tree: uncommitted change + untracked secret.
        fs::write(
            repo_dir.join("src/lib.rs"),
            "pub fn answer() -> u32 { 0 }\n",
        )
        .expect("write");
        fs::write(repo_dir.join(".env"), "SECRET=1\n").expect("write");

        let engine = GitEngine::new(&repo_dir);

        // Namespace and traversal guards.
        assert!(matches!(
            materialize(&engine, "release/1.0"),
            Err(GitEngineError::BranchNotAgentScoped(_))
        ));
        assert!(matches!(
            materialize(&engine, "agent/../../etc/passwd"),
            Err(GitEngineError::InvalidBranchName(_))
        ));
        assert!(matches!(
            materialize(&engine, "agent/"),
            Err(GitEngineError::InvalidBranchName(_) | GitEngineError::BranchNotAgentScoped(_))
        ));
        assert_eq!(git(&repo_dir, &["rev-parse", "release/1.0"]), base_commit);

        let m = materialize(&engine, "agent/claude/task-1.2").expect("materialize");
        assert_eq!(m.worktree.status, WorktreeStatus::Pending);
        assert_eq!(m.worktree.base_commit.0, base_commit);

        // Clean checkout of the base commit: no uncommitted changes, no untracked files.
        assert_eq!(
            fs::read_to_string(m.worktree_dir.join("src/lib.rs")).expect("read"),
            "pub fn answer() -> u32 { 42 }\n"
        );
        assert!(!m.worktree_dir.join(".env").exists());

        // Create-only: the same branch cannot be created twice.
        assert!(matches!(
            materialize(&engine, "agent/claude/task-1.2"),
            Err(GitEngineError::BranchAlreadyExists(_))
        ));

        // Reflog written for the agent branch.
        assert!(repo_dir
            .join(".git/logs/refs/heads/agent/claude/task-1.2")
            .is_file());

        // Edits in the worktree do not touch the main working tree; commits share refs/objects.
        fs::write(m.worktree_dir.join("src/agent.rs"), "// agent\n").expect("write");
        git(&m.worktree_dir, &["add", "."]);
        git(&m.worktree_dir, &["commit", "-q", "-m", "agent change"]);
        assert!(!repo_dir.join("src/agent.rs").exists());
        let wt_head = git(&m.worktree_dir, &["rev-parse", "HEAD"]);
        assert_eq!(
            git(&repo_dir, &["rev-parse", "agent/claude/task-1.2"]),
            wt_head
        );

        // GC in the main repo is worktree-aware: nothing the worktree needs is pruned.
        git(&repo_dir, &["gc", "-q", "--prune=now"]);
        GitEngine::verify_git_fsck(&m.worktree_dir).expect("worktree fsck after gc");
        GitEngine::verify_git_fsck(&repo_dir).expect("main fsck after gc");

        // Listing and removal.
        let listed = engine.list_agent_worktrees().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].worktree_dir, m.worktree_dir);
        assert_eq!(listed[0].branch, "agent/claude/task-1.2");

        assert!(matches!(
            engine.remove_agent_worktree(
                &repo_dir,
                "agent/claude/task-1.2",
                &m.worktree.base_commit
            ),
            Err(GitEngineError::WorktreeNotManaged(_))
        ));
        // The branch has an agent commit: the checkout goes, the branch (the work) stays.
        let deleted = engine
            .remove_agent_worktree(
                &m.worktree_dir,
                "agent/claude/task-1.2",
                &m.worktree.base_commit,
            )
            .expect("remove");
        assert!(!deleted);
        assert!(!m.worktree_dir.exists());
        assert!(engine.list_agent_worktrees().expect("list").is_empty());
        assert_eq!(
            git(&repo_dir, &["rev-parse", "agent/claude/task-1.2"]),
            wt_head
        );

        // An untouched worktree: both checkout and branch are removed.
        let untouched = materialize(&engine, "agent/claude/untouched").expect("materialize");
        let deleted = engine
            .remove_agent_worktree(
                &untouched.worktree_dir,
                "agent/claude/untouched",
                &untouched.worktree.base_commit,
            )
            .expect("remove");
        assert!(deleted);
        let branches = git(&repo_dir, &["branch", "--list", "agent/claude/untouched"]);
        assert!(
            branches.is_empty(),
            "untouched branch must be deleted: {branches}"
        );
        // Removing again is a no-op, not an error.
        assert!(!engine
            .remove_agent_worktree(
                &untouched.worktree_dir,
                "agent/claude/untouched",
                &untouched.worktree.base_commit,
            )
            .expect("idempotent remove"));
        GitEngine::verify_git_fsck(&repo_dir).expect("main fsck after remove");
    }

    #[test]
    fn gate_agent_worktree_from_bare_mirror() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src-repo");
        init_repo(&src);
        let bare = tmp.path().join("mirror.git");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().expect("utf8"),
                bare.to_str().expect("utf8"),
            ],
        );

        let engine = GitEngine::new(&bare);
        let m = materialize(&engine, "agent/cursor/bare").expect("materialize from bare");
        assert!(m.main_workdir.is_none());
        assert_eq!(
            fs::read_to_string(m.worktree_dir.join("src/lib.rs")).expect("read"),
            "pub fn answer() -> u32 { 42 }\n"
        );
        GitEngine::verify_git_fsck(&m.worktree_dir).expect("fsck");
    }

    #[test]
    fn recover_agent_worktrees_reconstructs_id_base_commit_and_prunes_stale() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);
        let engine = GitEngine::new(&repo_dir);

        let untouched = materialize(&engine, "agent/claude/untouched").expect("materialize");
        let committed = materialize(&engine, "agent/cursor/committed").expect("materialize");
        let stale = materialize(&engine, "agent/codex/stale").expect("materialize");
        fs::write(committed.worktree_dir.join("src/new.rs"), "// work\n").expect("write");
        git(&committed.worktree_dir, &["add", "."]);
        git(&committed.worktree_dir, &["commit", "-q", "-m", "work"]);
        // Simulate an ungraceful crash where a worktree dir was deleted externally.
        fs::remove_dir_all(&stale.worktree_dir).expect("rm stale");

        let recovered = GitEngine::new(&repo_dir)
            .recover_agent_worktrees(RepoId::new())
            .expect("recover");
        assert_eq!(recovered.len(), 2);

        let rec_untouched = recovered
            .iter()
            .find(|w| w.worktree.branch == "agent/claude/untouched")
            .expect("untouched");
        assert_eq!(rec_untouched.worktree.id, untouched.worktree.id);
        assert_eq!(
            rec_untouched.worktree.base_commit,
            untouched.worktree.base_commit
        );
        assert_eq!(rec_untouched.worktree.base_branch, "main");
        assert_eq!(rec_untouched.worktree.agent, AgentKind::ClaudeCode);

        let rec_committed = recovered
            .iter()
            .find(|w| w.worktree.branch == "agent/cursor/committed")
            .expect("committed");
        assert_eq!(rec_committed.worktree.id, committed.worktree.id);
        assert_eq!(
            rec_committed.worktree.base_commit,
            committed.worktree.base_commit
        );
        assert_eq!(rec_committed.worktree.agent, AgentKind::CursorAgent);
    }

    #[test]
    fn gate_refuses_repos_with_credentials_in_config() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);
        git(
            &repo_dir,
            &[
                "remote",
                "add",
                "origin",
                "https://x-access-token:ghp_secret@github.com/acme/app.git",
            ],
        );
        let engine = GitEngine::new(&repo_dir);
        assert!(matches!(
            materialize(&engine, "agent/claude/creds"),
            Err(GitEngineError::CredentialsInRepoConfig)
        ));
        // Nothing was created.
        assert!(git(&repo_dir, &["branch", "--list", "agent/*"]).is_empty());
    }

    #[test]
    fn credential_detection_cases() {
        assert!(config_contains_credentials(
            "[remote \"o\"]\n\turl = https://user:pw@host/r.git\n"
        ));
        assert!(config_contains_credentials(
            "[remote \"o\"]\n\turl = https://ghp_token@github.com/r.git\n"
        ));
        assert!(config_contains_credentials(
            "[http]\n\textraHeader = Authorization: Bearer x\n"
        ));
        assert!(config_contains_credentials(
            "[url \"https://tok@github.com/\"]\n\tinsteadOf = https://github.com/\n"
        ));
        assert!(config_contains_credentials(
            "[credential]\n\thelper = store --file .git/creds\n"
        ));
        assert!(!config_contains_credentials(
            "[credential]\n\thelper = osxkeychain\n"
        ));
        assert!(!config_contains_credentials(
            "[remote \"o\"]\n\turl = https://github.com/acme/r.git\n"
        ));
        assert!(!config_contains_credentials(
            "[remote \"o\"]\n\turl = git@github.com:acme/r.git\n"
        ));
        assert!(!config_contains_credentials(
            "[remote \"o\"]\n\turl = ssh://git@github.com/acme/r.git\n"
        ));
    }

    #[test]
    fn gate_7_4_git_integrity_clone_commit_push_fetch_roundtrip_and_fsck() {
        let tmp = TempDir::new().expect("create tempdir");
        let server_repo = tmp.path().join("server.git");
        let seed_client = tmp.path().join("seed-client");
        let clone_a = tmp.path().join("client-a");
        let clone_b = tmp.path().join("client-b");

        fs::create_dir_all(&server_repo).expect("mkdir server");
        fs::create_dir_all(&seed_client).expect("mkdir seed");

        git(&server_repo, &["init", "-q", "--bare", "-b", "main"]);

        git(&seed_client, &["init", "-q", "-b", "main"]);
        fs::write(seed_client.join("README.md"), "# Gitust Integrity Gate\n").expect("write");
        git(&seed_client, &["add", "README.md"]);
        git(&seed_client, &["commit", "-q", "-m", "seed commit"]);
        git(
            &seed_client,
            &["remote", "add", "origin", server_repo.to_str().expect("p")],
        );
        git(&seed_client, &["push", "-q", "-u", "origin", "main"]);

        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                server_repo.to_str().expect("p"),
                clone_a.to_str().expect("p"),
            ],
        );
        fs::create_dir_all(clone_a.join("crates")).expect("mkdir crates");
        fs::write(clone_a.join("crates/engine.rs"), "pub const A: u64 = 1;\n").expect("write");
        git(&clone_a, &["add", "."]);
        git(&clone_a, &["commit", "-q", "-m", "feat: add engine module"]);
        git(&clone_a, &["push", "-q", "origin", "main"]);

        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                server_repo.to_str().expect("p"),
                clone_b.to_str().expect("p"),
            ],
        );
        fs::write(
            clone_a.join("crates/engine.rs"),
            "pub const A: u64 = 1;\npub const B: bool = true;\n",
        )
        .expect("update");
        git(&clone_a, &["commit", "-q", "-am", "feat: second change"]);
        git(&clone_a, &["push", "-q", "origin", "main"]);

        git(&clone_b, &["fetch", "-q", "origin", "main"]);
        git(&clone_b, &["merge", "-q", "--ff-only", "origin/main"]);

        let server_head = GitEngine::new(&server_repo)
            .inspect_head_zero_copy()
            .expect("inspect server head");
        let client_b_head = GitEngine::new(&clone_b)
            .inspect_head_zero_copy()
            .expect("inspect client_b head");

        assert_eq!(server_head.head_sha, client_b_head.head_sha);
        assert!(server_head
            .root_entries
            .iter()
            .any(|e| e.filename == "crates" && e.is_dir));

        GitEngine::verify_git_fsck(&server_repo).expect("server git fsck");
        GitEngine::verify_git_fsck(&clone_a).expect("client A git fsck");
        GitEngine::verify_git_fsck(&clone_b).expect("client B git fsck");
    }

    /// Benchmark: `cargo test -p gitust-git --release -- --ignored --nocapture bench_`
    #[test]
    #[ignore = "benchmark; run explicitly"]
    fn bench_materialize_agent_worktree() {
        for file_count in [10_usize, 1_000, 10_000] {
            let tmp = TempDir::new().expect("tempdir");
            let repo_dir = tmp.path().join("repo");
            fs::create_dir_all(&repo_dir).expect("mkdir");
            git(&repo_dir, &["init", "-q", "-b", "main"]);
            for i in 0..file_count {
                let dir = repo_dir.join(format!("d{}", i % 100));
                fs::create_dir_all(&dir).expect("mkdir");
                fs::write(
                    dir.join(format!("f{i}.rs")),
                    format!("pub const X: usize = {i};\n"),
                )
                .expect("write");
            }
            git(&repo_dir, &["add", "."]);
            git(&repo_dir, &["commit", "-q", "-m", "bench"]);

            let engine = GitEngine::new(&repo_dir);
            let mut samples = Vec::new();
            for run in 0..5 {
                let m = materialize(&engine, &format!("agent/bench/r{run}")).expect("materialize");
                samples.push(m.worktree.materialize_micros);
            }
            samples.sort_unstable();
            println!(
                "files={file_count:>6} materialize_micros min={} median={} max={}",
                samples[0], samples[2], samples[4]
            );
        }
    }
}
