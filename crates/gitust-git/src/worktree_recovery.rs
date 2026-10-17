//! Agent linked worktree listing and startup recovery (`list_agent_worktrees`, `recover_agent_worktrees`).

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use gitust_core::{AgentKind, AgentWorktree, CommitSha, RepoId, WorktreeId, WorktreeStatus};

use crate::config_guard::reject_credentials_in_config;
use crate::worktree::{ManagedWorktreeEntry, MaterializedWorktree};
use crate::worktree_metadata::read_worktree_admin_dir;
use crate::{
    canonicalize_existing_ancestors, run_git, GitEngine, GitEngineError, AGENT_BRANCH_PREFIX,
    SANDBOX_SCRATCH_DIR_NAME,
};

impl GitEngine {
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
    ///
    /// Fails closed if the repository config contains credentials or if any managed worktree has
    /// an unreadable or corrupt `.git` admin directory pointer after `git worktree prune`.
    pub fn recover_agent_worktrees(
        &self,
        repo_id: RepoId,
    ) -> Result<Vec<MaterializedWorktree>, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        reject_credentials_in_config(&common_git_dir)?;
        // fail-open-ok: best-effort stale worktree metadata prune before listing; `list_agent_worktrees` and `read_worktree_admin_dir` below fail closed on any remaining broken entry.
        let _ = run_git(&self.repo_path, ["worktree", "prune"]);
        let worktrees_base = self.worktrees_base(&repo, &common_git_dir);
        let worktrees_base = canonicalize_existing_ancestors(&worktrees_base);
        let main_workdir = repo
            .work_dir()
            // fail-open-ok: path normalization only; the gix repository remains the source of the locator and filesystem operations reject inaccessible paths.
            .map(|w| fs::canonicalize(w).unwrap_or_else(|_| w.to_path_buf()));
        let sandbox_scratch_base = common_git_dir.join(SANDBOX_SCRATCH_DIR_NAME);

        let entries = self.list_agent_worktrees()?;
        let mut recovered = Vec::with_capacity(entries.len());
        for entry in entries {
            let worktree_id = entry
                .worktree_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(WorktreeId::parse_str);
            let Some(worktree_id) = worktree_id else {
                continue;
            };
            let worktree_admin_dir = read_worktree_admin_dir(&entry.worktree_dir, &common_git_dir)?;
            let origin = crate::worktree_origin::read_or_migrate(
                &worktree_admin_dir,
                &common_git_dir,
                worktree_id,
                if entry.detached {
                    None
                } else {
                    Some(&entry.branch)
                },
            )?;
            let (branch, mut base_branch, mut base_commit) = match origin {
                Some(origin) => (origin.branch, origin.base_branch, origin.base_commit),
                None => (entry.branch, String::new(), CommitSha(String::new())),
            };
            let approval = self.load_worktree_approval(worktree_id)?;
            if let Some(ref record) = approval {
                if record.branch != branch {
                    return Err(GitEngineError::ApprovalState(
                        "approval branch does not match managed worktree".into(),
                    ));
                }
                base_branch = record.base_branch.clone();
                base_commit = record.approved_commit.clone();
            }
            let agent = infer_agent_kind(&branch);

            recovered.push(MaterializedWorktree {
                worktree: AgentWorktree {
                    id: worktree_id,
                    repo_id,
                    agent,
                    branch,
                    base_branch,
                    base_commit,
                    prompt_summary: "recovered from disk".to_owned(),
                    materialize_micros: 0,
                    status: if approval.is_some() {
                        WorktreeStatus::Merged
                    } else {
                        WorktreeStatus::Pending
                    },
                    last_sandbox_duration_ms: 0,
                    verified_commit: None,
                    verified_checks_oid: None,
                    tests_passed: 0,
                    tests_total: 0,
                    files_changed: 0,
                    additions: 0,
                    deletions: 0,
                    github_synced: false,
                    created_at: Utc::now(),
                },
                worktree_dir: entry.worktree_dir,
                approval,
                worktree_admin_dir,
                common_git_dir: common_git_dir.clone(),
                main_workdir: main_workdir.clone(),
                worktrees_base: worktrees_base.clone(),
                sandbox_scratch_base: sandbox_scratch_base.clone(),
            });
        }
        Ok(recovered)
    }
}

fn infer_agent_kind(branch: &str) -> AgentKind {
    match branch
        .strip_prefix(AGENT_BRANCH_PREFIX)
        // fail-open-ok: agent-kind display inference only; permissions never depend on this fallback.
        .unwrap_or(branch)
        .split('/')
        .next()
        // fail-open-ok: unknown agent kinds receive a display default; no access is granted here.
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
                let detached = branch.is_none();
                let resolved_branch = match branch.take() {
                    Some(b) => b
                        .strip_prefix("refs/heads/")
                        .filter(|s| s.starts_with(AGENT_BRANCH_PREFIX))
                        .map(str::to_owned),
                    None if managed_base.is_some_and(|base| canon_p.parent() == Some(base)) => {
                        // fail-open-ok: display label only; recovery separately requires a valid UUID directory name.
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
                        detached,
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
