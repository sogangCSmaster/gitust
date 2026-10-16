//! Agent linked worktree listing and startup recovery (`list_agent_worktrees`, `recover_agent_worktrees`).

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use gitust_core::{AgentKind, AgentWorktree, CommitSha, RepoId, WorktreeId, WorktreeStatus};

use crate::config_guard::reject_credentials_in_config;
use crate::worktree::{read_worktree_admin_dir, ManagedWorktreeEntry, MaterializedWorktree};
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
            .map(|w| fs::canonicalize(w).unwrap_or_else(|_| w.to_path_buf()));
        let sandbox_scratch_base = common_git_dir.join(SANDBOX_SCRATCH_DIR_NAME);

        let entries = self.list_agent_worktrees()?;
        let mut recovered = Vec::with_capacity(entries.len());
        for entry in entries {
            let worktree_admin_dir = read_worktree_admin_dir(&entry.worktree_dir)?;
            let worktree_id = entry
                .worktree_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(WorktreeId::parse_str)
                .unwrap_or_else(WorktreeId::new);
            let (mut base_branch, mut base_commit) =
                read_branch_creation_reflog(&common_git_dir, &entry.branch);
            let approval = self.load_worktree_approval(worktree_id)?;
            if let Some(ref record) = approval {
                if record.branch != entry.branch {
                    return Err(GitEngineError::ApprovalState(
                        "approval branch does not match managed worktree".into(),
                    ));
                }
                base_branch = record.base_branch.clone();
                base_commit = record.approved_commit.clone();
            }
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
                    status: if approval.is_some() {
                        WorktreeStatus::Merged
                    } else {
                        WorktreeStatus::Pending
                    },
                    last_sandbox_duration_ms: 0,
                    verified_commit: None,
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

/// Reads the initial reflog line of `refs/heads/<branch>` to recover `base_branch` and
/// `base_commit` across restarts. If the reflog is absent, returns an empty `base_commit` so
/// [`GitEngine::remove_agent_worktree`] preserves the branch on discard.
fn read_branch_creation_reflog(common_git_dir: &Path, branch: &str) -> (String, CommitSha) {
    use std::io::{BufRead, BufReader};
    let default = || ("main".to_owned(), CommitSha(String::new()));
    // fail-open-ok: returning empty `base_commit` is fail-safe (causes `remove_agent_worktree` to preserve the branch rather than deleting it).
    let Ok(clean) = GitEngine::validate_agent_branch_name(branch) else {
        return default();
    };
    let log_path = common_git_dir.join("logs/refs/heads").join(&clean);
    // fail-open-ok: missing reflog falls back to empty `base_commit`, which safely preserves the branch on discard.
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
