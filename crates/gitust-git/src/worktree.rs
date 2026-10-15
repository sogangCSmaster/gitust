//! Agent linked worktree lifecycle (`materialize_agent_worktree`, `remove_agent_worktree`,
//! `list_agent_worktrees`, `recover_agent_worktrees`, and `merge_agent_branch`).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use gitust_core::{AgentKind, AgentWorktree, CommitSha, RepoId, WorktreeId, WorktreeStatus};
use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};
use gix::refs::{FullName, Target};
use serde::{Deserialize, Serialize};

use crate::config_guard::reject_credentials_in_config;
use crate::{
    canonicalize_existing_ancestors, ensure_reflog_identity, io_err, run_git, GitEngine,
    GitEngineError, AGENT_BRANCH_PREFIX, SANDBOX_SCRATCH_DIR_NAME,
};

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

impl GitEngine {
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
        if fs::symlink_metadata(worktree_dir)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(GitEngineError::WorktreeNotManaged(
                worktree_dir.display().to_string(),
            ));
        }
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        let base = self.worktrees_base(&repo, &common_git_dir);
        let base = canonicalize_existing_ancestors(&base);
        let target = canonicalize_existing_ancestors(worktree_dir);
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

    /// Lists agent worktrees managed by this engine under the worktree base directory.
    pub fn list_agent_worktrees(&self) -> Result<Vec<ManagedWorktreeEntry>, GitEngineError> {
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        let base = canonicalize_existing_ancestors(&self.worktrees_base(&repo, &common_git_dir));

        let stdout = run_git(&self.repo_path, ["worktree", "list", "--porcelain", "-z"])?;
        Ok(parse_worktree_list(&stdout, Some(&base))
            .into_iter()
            .filter(|e| e.worktree_dir.parent() == Some(base.as_path()))
            .collect())
    }

    /// Reconstructs [`MaterializedWorktree`] descriptors for existing managed `agent/` worktrees.
    pub fn recover_agent_worktrees(
        &self,
        repo_id: RepoId,
    ) -> Result<Vec<MaterializedWorktree>, GitEngineError> {
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        reject_credentials_in_config(&common_git_dir)?;
        let _ = run_git(&self.repo_path, ["worktree", "prune"]);
        let worktrees_base = self.worktrees_base(&repo, &common_git_dir);
        let worktrees_base = canonicalize_existing_ancestors(&worktrees_base);
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

    /// Merges (fast-forwards) a verified `agent/` branch into `base_branch` (`refs/heads/<base>`)
    /// using an atomic `gix` CAS reference transaction, and updates the main working tree if it is
    /// non-bare and currently checked out on `base_branch`.
    pub fn merge_agent_branch(
        &self,
        branch: &str,
        base_branch: &str,
    ) -> Result<CommitSha, GitEngineError> {
        let clean_branch = Self::validate_agent_branch_name(branch)?;
        let clean_base = Self::validate_branch_name(base_branch)?;
        let _service_lock = self.lock_worktree_service()?;

        let mut repo = self.acquire_repo()?;
        ensure_reflog_identity(&mut repo)?;

        let full_agent = format!("refs/heads/{clean_branch}");
        let agent_id = repo
            .find_reference(full_agent.as_str())
            .map_err(|_| GitEngineError::BaseBranchNotFound(clean_branch.clone()))?
            .peel_to_id_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .detach();

        let full_base = format!("refs/heads/{clean_base}");
        let base_id = repo
            .find_reference(full_base.as_str())
            .map_err(|_| GitEngineError::BaseBranchNotFound(clean_base.clone()))?
            .peel_to_id_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .detach();

        let agent_hex = agent_id.to_string();
        if agent_id == base_id {
            return Err(GitEngineError::MergeConflict(format!(
                "agent branch '{clean_branch}' has no commits ahead of '{clean_base}' to merge"
            )));
        }

        let base_hex = base_id.to_string();
        run_git(
            &self.repo_path,
            [
                OsStr::new("merge-base"),
                OsStr::new("--is-ancestor"),
                OsStr::new(base_hex.as_str()),
                OsStr::new(agent_hex.as_str()),
            ],
        )
        .map_err(|_| {
            GitEngineError::MergeConflict(format!(
                "base branch '{clean_base}' has diverged from '{clean_branch}'; fast-forward not possible"
            ))
        })?;

        let head_on_base = repo.work_dir().is_some()
            && repo
                .head_name()
                .ok()
                .flatten()
                .is_some_and(|n| n.as_bstr() == full_base.as_bytes());

        if head_on_base {
            let status_out = run_git(
                &self.repo_path,
                [
                    OsStr::new("status"),
                    OsStr::new("--porcelain"),
                    OsStr::new("--untracked-files=no"),
                ],
            )?;
            if !status_out.iter().all(u8::is_ascii_whitespace) {
                return Err(GitEngineError::MergeConflict(format!(
                    "main working tree has uncommitted changes on '{clean_base}'; cannot fast-forward"
                )));
            }
            run_git(
                &self.repo_path,
                [
                    OsStr::new("merge"),
                    OsStr::new("--ff-only"),
                    OsStr::new("--quiet"),
                    OsStr::new(agent_hex.as_str()),
                ],
            )
            .map_err(|_| {
                GitEngineError::MergeConflict(format!(
                    "cannot fast-forward '{clean_base}' to '{clean_branch}'"
                ))
            })?;
        } else {
            repo.reference(
                full_base.as_str(),
                agent_id,
                PreviousValue::MustExistAndMatch(Target::Object(base_id)),
                format!("gitust: fast-forward {clean_base} to {clean_branch}"),
            )
            .map_err(|_| {
                GitEngineError::MergeConflict(format!(
                    "base branch '{clean_base}' changed concurrently; fast-forward aborted"
                ))
            })?;
        }

        Ok(CommitSha(agent_hex))
    }
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
    };
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
    match branch
        .strip_prefix(AGENT_BRANCH_PREFIX)
        .unwrap_or(branch)
        .split('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "cursor" => AgentKind::CursorAgent,
        "codex" => AgentKind::CodexSwarm,
        "cli" | "bot" => AgentKind::InternalBot,
        _ => AgentKind::ClaudeCode,
    }
}

/// Parses `git worktree list --porcelain -z`, keeping entries on `agent/` branches or detached
/// worktrees located directly inside `managed_base`.
fn parse_worktree_list(stdout: &[u8], managed_base: Option<&Path>) -> Vec<ManagedWorktreeEntry> {
    let mut entries = Vec::new();
    let (mut path, mut head, mut branch) = (None, None, None);

    let mut flush =
        |path: &mut Option<PathBuf>, head: &mut Option<String>, branch: &mut Option<String>| {
            if let (Some(p), Some(h)) = (path.take(), head.take()) {
                let canon_p = canonicalize_existing_ancestors(&p);
                let resolved_branch = match branch.take() {
                    Some(b) => b
                        .strip_prefix("refs/heads/")
                        .filter(|s| s.starts_with(AGENT_BRANCH_PREFIX))
                        .map(str::to_owned),
                    None if managed_base.is_some_and(|base| canon_p.parent() == Some(base)) => {
                        let leaf = canon_p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        let clean: String = leaf
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                            .collect();
                        let suffix = if clean.is_empty() { "worktree" } else { &clean };
                        Some(format!("{AGENT_BRANCH_PREFIX}detached/{suffix}"))
                    }
                    None => None,
                };
                if let Some(short) = resolved_branch {
                    entries.push(ManagedWorktreeEntry {
                        worktree_dir: canon_p,
                        branch: short,
                        head: CommitSha(h),
                    });
                }
            }
            (*path, *head, *branch) = (None, None, None);
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
