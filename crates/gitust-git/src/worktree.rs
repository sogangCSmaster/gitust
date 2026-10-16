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
use crate::worktree_metadata::read_worktree_admin_dir;
use crate::{
    canonicalize_existing_ancestors, ensure_reflog_identity, io_err, run_git, GitEngine,
    GitEngineError, AGENT_BRANCH_PREFIX, SANDBOX_SCRATCH_DIR_NAME,
};

/// A linked worktree created for an agent, plus the paths a sandbox needs to confine it.
#[derive(Debug, Clone)]
pub struct MaterializedWorktree {
    pub worktree: AgentWorktree,
    pub approval: Option<crate::WorktreeApproval>,
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
            .and_then(|_| read_worktree_admin_dir(&worktree_dir, &common_git_dir));
        let worktree_admin_dir = match checkout {
            Ok(dir) => dir,
            Err(e) => {
                // fail-open-ok: best-effort rollback cleanup after `git worktree add` failure before returning `Err(e)`.
                let _ = run_git(
                    &self.repo_path,
                    [
                        OsStr::new("worktree"),
                        OsStr::new("remove"),
                        OsStr::new("--force"),
                        worktree_dir.as_os_str(),
                    ],
                );
                // fail-open-ok: best-effort directory removal during checkout failure rollback.
                let _ = fs::remove_dir_all(&worktree_dir);
                // fail-open-ok: best-effort worktree prune during checkout failure rollback.
                let _ = run_git(
                    &self.repo_path,
                    [OsStr::new("worktree"), OsStr::new("prune")],
                );
                // fail-open-ok: best-effort CAS branch deletion during checkout failure rollback.
                let _ = delete_branch_ref(
                    &repo,
                    &full_branch,
                    PreviousValue::MustExistAndMatch(Target::Object(base_id)),
                );
                return Err(e);
            }
        };

        // fail-open-ok: path normalization only; the gix repository remains the source of the locator and filesystem operations reject inaccessible paths.
        let worktree_dir = fs::canonicalize(&worktree_dir).unwrap_or(worktree_dir);
        // fail-open-ok: path normalization only; the gix repository remains the source of the locator and filesystem operations reject inaccessible paths.
        let worktrees_base = fs::canonicalize(&worktrees_base).unwrap_or(worktrees_base);
        let main_workdir = repo
            .work_dir()
            // fail-open-ok: path normalization only; the gix repository remains the source of the locator and filesystem operations reject inaccessible paths.
            .map(|w| fs::canonicalize(w).unwrap_or_else(|_| w.to_path_buf()));
        // fail-open-ok: saturating telemetry conversion; does not authorize access or alter Git state.
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
                verified_commit: None,
                verified_checks_oid: None,
                last_sandbox_duration_ms: 0,
                tests_passed: 0,
                tests_total: 0,
                files_changed: 0,
                additions: 0,
                deletions: 0,
                github_synced: false,
                created_at: Utc::now(),
            },
            approval: None,
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
        // fail-open-ok: a missing/unreadable path is not a symlink; the canonical parent check
        // below still rejects anything outside the managed worktrees base.
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
        if let Some(id) = target
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(WorktreeId::parse_str)
        {
            self.remove_worktree_approval(id)?;
        }
        // fail-open-ok: removes the shared worktrees base dir only when empty; non-empty when sibling worktrees exist.
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

    /// Deletes an `agent/` branch reference (`refs/heads/agent/...`) when it is not currently
    /// checked out in a worktree.
    pub fn delete_agent_branch_ref(&self, branch: &str) -> Result<(), GitEngineError> {
        let clean = Self::validate_agent_branch_name(branch)?;
        let _service_lock = self.lock_worktree_service()?;
        let mut repo = self.acquire_repo()?;
        ensure_reflog_identity(&mut repo)?;
        let (main_checked_out, linked_checked_out) =
            crate::ancestry::list_checked_out_branches(&self.repo_path, repo.work_dir().is_some())?;
        if main_checked_out.as_deref() == Some(clean.as_str())
            || linked_checked_out.contains(clean.as_str())
        {
            return Err(GitEngineError::MergeConflict(format!(
                "cannot delete agent branch '{clean}' while it is checked out in a worktree"
            )));
        }
        let full = format!("refs/heads/{clean}");
        let tip = repo
            .find_reference(full.as_str())
            .map_err(|_| GitEngineError::BaseBranchNotFound(clean))?
            .peel_to_id_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .detach();
        delete_branch_ref(
            &repo,
            &full,
            PreviousValue::MustExistAndMatch(Target::Object(tip)),
        )
    }

    /// Merges (fast-forwards) a verified `agent/` branch into `base_branch` (`refs/heads/<base>`)
    /// using an atomic `gix` CAS reference transaction, and updates the main working tree if it is
    /// non-bare and currently checked out on `base_branch`.
    pub fn merge_agent_branch(
        &self,
        branch: &str,
        base_branch: &str,
    ) -> Result<CommitSha, GitEngineError> {
        self.merge_agent_branch_at(branch, base_branch, None)
    }

    pub(crate) fn merge_agent_branch_at(
        &self,
        branch: &str,
        base_branch: &str,
        expected: Option<&CommitSha>,
    ) -> Result<CommitSha, GitEngineError> {
        let clean_branch = Self::validate_agent_branch_name(branch)?;
        let clean_base = Self::validate_branch_name(base_branch)?;
        let _service_lock = self.lock_worktree_service()?;
        self.ensure_config_has_no_credentials()?;

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
        if expected.is_some_and(|sha| sha.0 != agent_hex) {
            return Err(GitEngineError::VerificationInvalid(
                "branch changed since verification; run verification again".into(),
            ));
        }
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

        let (main_checked_out, linked_checked_out) =
            crate::ancestry::list_checked_out_branches(&self.repo_path, repo.work_dir().is_some())?;
        if linked_checked_out.contains(clean_base.as_str()) {
            return Err(GitEngineError::MergeConflict(format!(
                "base branch '{clean_base}' is currently checked out in a linked worktree; cannot fast-forward"
            )));
        }
        let head_on_base = main_checked_out.as_deref() == Some(clean_base.as_str());

        if head_on_base {
            crate::verification::inspect_index_flags(&self.repo_path)?;
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
        // fail-open-ok: normalizes an optional refs/heads/ prefix; the required agent namespace is checked immediately below.
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
