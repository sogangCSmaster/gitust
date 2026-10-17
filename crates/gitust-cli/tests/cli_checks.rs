use gitust_core::{AgentKind, RepoId, WorktreeId};
use gitust_git::GitEngine;
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Output};

fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}
fn git(repo: &Path, args: &[&str]) -> String {
    let output = command("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "synthetic Git fixture operation failed"
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn fixture(config: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "Checks Fixture"]);
    git(&repo, &["config", "user.email", "checks@company.internal"]);
    std::fs::write(repo.join("marker.txt"), "committed\n").unwrap();
    if let Some(config) = config {
        std::fs::write(repo.join(".gitust-checks.json"), config).unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "committed Checks fixture"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    (temp, repo, sha)
}
fn checks(repo: &Path, sha: &str) -> Output {
    command(env!("CARGO_BIN_EXE_gitust"))
        .args(["sandbox", "checks", "--repo"])
        .arg(repo)
        .args(["--expected-commit", sha, "--timeout", "10"])
        .output()
        .unwrap()
}

#[test]
fn committed_checks_run_all_required_steps_and_preserve_source_and_existing_task() {
    for failed in [false, true] {
        let commands = if failed {
            vec!["printf failed; exit 7", "printf skipped"]
        } else {
            vec![
                "test \"$(cat marker.txt)\" = committed; printf first",
                "printf second",
            ]
        };
        let config = json!({"version":1,"commands":commands,"auto_run":failed}).to_string();
        let (_temp, repo, sha) = fixture(Some(&config));
        let blob = git(&repo, &["rev-parse", "HEAD:.gitust-checks.json"]);
        let engine = GitEngine::new(&repo);
        let task = engine
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::InternalBot,
                "agent/existing",
                "main",
                "Preserved task",
            )
            .unwrap();
        let origin = std::fs::read(task.worktree_admin_dir.join("gitust-origin.json")).unwrap();
        drop(engine);
        std::fs::write(
            repo.join(".gitust-checks.json"),
            "uncommitted configuration is not policy",
        )
        .unwrap();
        std::fs::write(repo.join("marker.txt"), "uncommitted\n").unwrap();
        let refs = git(&repo, &["show-ref"]);
        let worktrees = git(&repo, &["worktree", "list", "--porcelain"]);
        let status = git(&repo, &["status", "--porcelain"]);
        let output = checks(&repo, &sha);
        assert_eq!(output.status.code(), Some(if failed { 7 } else { 0 }));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["version"], 1);
        assert_eq!(report["source_ref"], "refs/heads/main");
        assert_eq!(report["checked_commit"], sha);
        assert_eq!(report["checks_blob_oid"], blob);
        assert_eq!(report["status"], if failed { "failed" } else { "passed" });
        assert_eq!(report["reason"], Value::Null);
        assert_eq!(report["cleanup"], "confirmed");
        let steps = report["result"]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), commands.len());
        assert!(steps.iter().all(|step| step["required"] == true));
        assert_eq!(steps[0]["command"], commands[0]);
        assert_eq!(steps[1]["command"], commands[1]);
        assert_eq!(steps[0]["exit_code"], if failed { 7 } else { 0 });
        assert_eq!(
            steps[1]["exit_code"],
            if failed { Value::Null } else { json!(0) }
        );
        assert_eq!(git(&repo, &["show-ref"]), refs);
        assert_eq!(git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert_eq!(git(&repo, &["worktree", "list", "--porcelain"]), worktrees);
        assert_eq!(git(&repo, &["status", "--porcelain"]), status);
        assert_eq!(
            std::fs::read(task.worktree_admin_dir.join("gitust-origin.json")).unwrap(),
            origin
        );
        for private in [
            "gitust-approvals",
            "gitust-check-runs",
            "gitust-automatic-runs",
        ] {
            assert!(!repo.join(".git").join(private).exists());
        }
        let engine = GitEngine::new(&repo);
        assert!(engine.lock_worktree_service().is_ok());
        assert_eq!(engine.list_agent_worktrees().unwrap().len(), 1);
        git(&repo, &["fsck", "--full", "--strict"]);
    }
}

#[test]
fn rejected_checks_do_not_create_an_execution_or_change_refs() {
    for (config, mismatch, reason) in [
        (None, false, "checks_missing"),
        (Some("not JSON"), false, "checks_invalid"),
        (
            Some(r#"{"version":1,"commands":["printf must-not-run"]}"#),
            true,
            "revision_mismatch",
        ),
    ] {
        let (_temp, repo, sha) = fixture(config);
        let refs = git(&repo, &["show-ref"]);
        let output = checks(
            &repo,
            if mismatch {
                "1111111111111111111111111111111111111111"
            } else {
                &sha
            },
        );
        assert_eq!(output.status.code(), Some(2));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], "rejected");
        assert_eq!(report["reason"], reason);
        assert_eq!(report["result"], Value::Null);
        assert_eq!(report["cleanup"], "not_created");
        assert_eq!(git(&repo, &["show-ref"]), refs);
        assert!(GitEngine::new(&repo)
            .list_agent_worktrees()
            .unwrap()
            .is_empty());
    }
}
