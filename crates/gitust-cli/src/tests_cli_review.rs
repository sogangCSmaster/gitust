use gitust_core::{AgentKind, RepoId, WorktreeId};
use gitust_git::GitEngine;
use std::path::Path;

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-b", "main"]);
    git(tmp.path(), &["config", "user.name", "Test"]);
    git(tmp.path(), &["config", "user.email", "test@example.test"]);
    git(
        tmp.path(),
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    tmp
}

#[tokio::test]
async fn cli_sandbox_cannot_mutate_a_repository_owned_by_another_service() {
    let tmp = repo();
    let engine = GitEngine::new(tmp.path());
    let _owner = engine.lock_worktree_service().unwrap();
    let result = crate::sandbox_cmd::run_cli_sandbox(tmp.path(), "true", false, 5).await;
    assert!(result.unwrap_err().to_string().contains("worktree service"));
    assert!(engine.list_agent_worktrees().unwrap().is_empty());
}

#[test]
fn cli_discard_is_not_blocked_by_an_unrelated_corrupt_approval() {
    let tmp = repo();
    let engine = GitEngine::new(tmp.path());
    let mut worktrees = Vec::new();
    for branch in ["agent/test/unrelated", "agent/test/discard"] {
        worktrees.push(
            engine
                .materialize_agent_worktree(
                    WorktreeId::new(),
                    RepoId::new(),
                    AgentKind::InternalBot,
                    branch,
                    "main",
                    "test",
                )
                .unwrap(),
        );
    }
    let approvals = worktrees[0].common_git_dir.join("gitust-approvals");
    std::fs::create_dir_all(&approvals).unwrap();
    let corrupt = approvals.join(format!("{}.json", worktrees[0].worktree.id.0));
    std::fs::write(&corrupt, "not json").unwrap();
    crate::triage_cmd::run_triage_discard(tmp.path(), "agent/test/discard").unwrap();
    assert!(!worktrees[1].worktree_dir.exists());
    assert!(worktrees[0].worktree_dir.exists());
    assert_eq!(std::fs::read_to_string(corrupt).unwrap(), "not json");
}
