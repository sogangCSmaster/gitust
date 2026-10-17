//! Observe the namespace after host-side metadata appears, rather than inspecting policy text.

use std::{fs, path::Path, process::Command, time::Duration};

use gitust_core::{AgentKind, RepoId, WorktreeId};
use gitust_git::GitEngine;

use crate::{LocalSandboxExecutor, SandboxSpec};

fn git(path: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Sandbox fixture")
        .env("GIT_AUTHOR_EMAIL", "sandbox@example.invalid")
        .env("GIT_COMMITTER_NAME", "Sandbox fixture")
        .env("GIT_COMMITTER_EMAIL", "sandbox@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[tokio::test]
async fn late_host_metadata_and_symlink_aliases_stay_hidden_while_git_and_writes_work() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("source.txt"), "tracked fixture\n").unwrap();
    git(&repo, &["add", "source.txt"]);
    git(&repo, &["commit", "-qm", "initial fixture"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let engine = GitEngine::new(&repo);
    let materialized = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::ClaudeCode,
            "agent/claude/late-hidden",
            "main",
            "late hidden metadata regression",
        )
        .unwrap();
    let common = &materialized.common_git_dir;
    let history = common.join("gitust-check-runs/private.json");
    let config = common.join("config.worktree");
    let stash = common.join("refs/stash");
    for private in [&history, &config, &stash] {
        assert!(
            !private.exists(),
            "fixture must start before private metadata exists"
        );
    }
    let relative_alias = common.join("history-alias");
    let absolute_alias = common.join("absolute-history-alias");
    let file_alias = common.join("stash-alias");
    std::os::unix::fs::symlink("gitust-check-runs/private.json", &relative_alias).unwrap();
    std::os::unix::fs::symlink(&history, &absolute_alias).unwrap();
    std::os::unix::fs::symlink("refs/stash", &file_alias).unwrap();
    let spec = SandboxSpec::for_linked_worktree(
        &materialized.worktree_dir,
        &materialized.worktree_admin_dir,
        common,
        materialized.main_workdir.as_deref(),
        &materialized.worktrees_base,
        &materialized.sandbox_scratch_base,
    );
    let targets = [
        &history,
        &config,
        &stash,
        &relative_alias,
        &absolute_alias,
        &file_alias,
    ]
    .map(|path| quote(path))
    .join(" ");
    let command = format!(
        "git log -1 --format=%H && git status --short && \
         printf ready > .probe-ready && \
         while [ ! -f .probe-release ]; do sleep 0.01; done; \
         for private in {targets}; do \
           if cat \"$private\" 2>/dev/null; then exit 91; fi; \
           if (printf forbidden > \"$private\") 2>/dev/null; then exit 92; fi; \
         done; \
         printf editable > allowed.txt && printf scratch > \"$TMPDIR/allowed\" && \
         test \"$(cat \"$TMPDIR/allowed\")\" = scratch && \
         git log -1 --format=%H && git status --short && printf late-private-denied"
    );
    let executor = LocalSandboxExecutor::new(1).with_timeout_secs(15);
    let id = materialized.worktree.id;
    let mut executions = tokio::task::JoinSet::new();
    executions.spawn(async move { executor.execute(id, &spec, &command).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !materialized.worktree_dir.join(".probe-ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("sandbox must start before host metadata is created");
    fs::create_dir_all(history.parent().unwrap()).unwrap();
    fs::write(&history, "LATE_PRIVATE_HISTORY").unwrap();
    fs::write(&config, "[gitust-test]\nprivate = LATE_PRIVATE_CONFIG\n").unwrap();
    fs::write(&stash, &head).unwrap();
    fs::write(materialized.worktree_dir.join(".probe-release"), "release").unwrap();
    let result = executions.join_next().await.unwrap().unwrap().unwrap();
    assert_eq!(
        result.exit_code, 0,
        "stdout: {}; stderr: {}",
        result.stdout, result.stderr
    );
    assert!(result.stdout.contains("late-private-denied"));
    assert!(!result.stdout.contains("LATE_PRIVATE_"));
    assert!(result.stdout.matches(head.trim()).count() >= 2);
    assert_eq!(
        fs::read_to_string(&history).unwrap(),
        "LATE_PRIVATE_HISTORY"
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "[gitust-test]\nprivate = LATE_PRIVATE_CONFIG\n"
    );
    assert_eq!(fs::read_to_string(&stash).unwrap(), head);
    assert_eq!(
        fs::read_to_string(materialized.worktree_dir.join("allowed.txt")).unwrap(),
        "editable"
    );
}
