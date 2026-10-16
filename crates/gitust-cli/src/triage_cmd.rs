//! `gitust triage` CLI subcommands (`list`, `approve <branch>`, `discard <branch>`).

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Subcommand;
use gitust_core::RepoId;
use gitust_git::GitEngine;

#[derive(Debug, Subcommand)]
pub enum TriageSubcommands {
    /// List active agent worktrees in the repository
    List {
        /// Local repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },

    /// Fast-forward merge an `agent/*` branch into a base branch (default: `main`)
    Approve {
        /// Agent branch to approve and merge (e.g., `agent/claude/task-1`)
        branch: String,

        /// Target base branch to fast-forward (default: `main`)
        #[arg(long, default_value = "main")]
        base: String,

        /// Local repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },

    /// Discard an `agent/*` branch and its linked worktree (if present)
    Discard {
        /// Agent branch to discard (e.g., `agent/claude/task-1`)
        branch: String,

        /// Local repository path
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

pub fn run_triage_list(repo: &Path) -> Result<()> {
    let engine = GitEngine::new(repo);
    let entries = engine.list_agent_worktrees()?;
    if entries.is_empty() {
        println!("No agent worktrees.");
        return Ok(());
    }
    for e in entries {
        println!("{}  {}  {}", e.branch, e.head.0, e.worktree_dir.display());
    }
    Ok(())
}

pub fn run_triage_approve(repo: &Path, branch: &str, base: &str) -> Result<()> {
    let engine = GitEngine::new(repo);
    let _service_lock = engine.lock_worktree_service()?;
    engine.ensure_config_has_no_credentials()?;
    let clean_branch = GitEngine::validate_agent_branch_name(branch)?;
    let clean_base = GitEngine::validate_branch_name(base)?;
    let merged_sha = engine.merge_agent_branch(&clean_branch, &clean_base)?;
    println!(
        "✓ Fast-forward merged '{}' into '{}' ({})",
        clean_branch, clean_base, merged_sha.0
    );
    Ok(())
}

pub fn run_triage_discard(repo: &Path, branch: &str) -> Result<()> {
    let engine = GitEngine::new(repo);
    let _service_lock = engine.lock_worktree_service()?;
    engine.ensure_config_has_no_credentials()?;
    let clean_branch = GitEngine::validate_agent_branch_name(branch)?;
    let recovered = engine.recover_agent_worktrees(RepoId::new())?;
    if let Some(m) = recovered
        .into_iter()
        .find(|w| w.worktree.branch == clean_branch)
    {
        let branch_deleted = engine.remove_agent_worktree(
            &m.worktree_dir,
            &m.worktree.branch,
            &m.worktree.base_commit,
        )?;
        if !branch_deleted {
            engine.delete_agent_branch_ref(&clean_branch)?;
        }
        println!(
            "✓ Discarded agent worktree and branch '{}' ({})",
            clean_branch,
            m.worktree_dir.display()
        );
    } else {
        engine.delete_agent_branch_ref(&clean_branch)?;
        println!("✓ Discarded agent branch '{}'", clean_branch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitust_core::{AgentKind, WorktreeId};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .expect("spawn git");
        assert!(st.success(), "git {args:?} failed");
    }

    #[test]
    fn cli_triage_list_approve_and_discard_lifecycle() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "test@company.internal"]);
        git(&repo, &["config", "user.name", "tester"]);
        std::fs::write(repo.join("README.md"), "init\n").expect("write");
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "initial"]);

        let engine = GitEngine::new(&repo);
        let m1 = engine
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::ClaudeCode,
                "agent/claude/triage-merge",
                "main",
                "merge me".to_owned(),
            )
            .expect("materialize m1");
        std::fs::write(m1.worktree_dir.join("feature.txt"), "approved\n").expect("write");
        git(&m1.worktree_dir, &["add", "feature.txt"]);
        git(&m1.worktree_dir, &["commit", "-m", "feat: triage approve"]);

        let m2 = engine
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::CursorAgent,
                "agent/cursor/triage-discard",
                "main",
                "discard me with unmerged commit".to_owned(),
            )
            .expect("materialize m2");
        std::fs::write(m2.worktree_dir.join("unmerged.txt"), "discarded\n").expect("write");
        git(&m2.worktree_dir, &["add", "unmerged.txt"]);
        git(&m2.worktree_dir, &["commit", "-m", "feat: unmerged work"]);

        run_triage_list(&repo).expect("triage list");
        run_triage_approve(&repo, "agent/claude/triage-merge", "main").expect("triage approve");
        assert!(repo.join("feature.txt").exists());

        run_triage_discard(&repo, "agent/cursor/triage-discard").expect("triage discard");
        let remaining = engine.list_agent_worktrees().expect("list remaining");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].branch, "agent/claude/triage-merge");
        // Verify the discarded branch ref (`refs/heads/agent/cursor/triage-discard`) was deleted even though it had unmerged commits.
        assert!(engine
            .list_commits(Some("agent/cursor/triage-discard"), 1)
            .is_err());
    }
}
