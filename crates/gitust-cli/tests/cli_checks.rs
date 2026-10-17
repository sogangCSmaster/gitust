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

fn history(repo: &Path, run_id: Option<&str>) -> Output {
    let mut cli = command(env!("CARGO_BIN_EXE_gitust"));
    cli.args(["sandbox", "checks-history", "--repo"]).arg(repo);
    if let Some(id) = run_id {
        cli.args(["--run-id", id]);
    }
    cli.output().unwrap()
}

fn record(repo: &Path, sha: &str) -> Output {
    command(env!("CARGO_BIN_EXE_gitust"))
        .args(["sandbox", "checks", "--record", "--repo"])
        .arg(repo)
        .args(["--expected-commit", sha, "--timeout", "10"])
        .output()
        .unwrap()
}

#[test]
fn explicit_history_preserves_results_and_reports_storage_failure_separately() {
    let (_temp, repo, sha) = fixture(Some(
        r#"{"version":1,"commands":["printf recorded"],"auto_run":false}"#,
    ));
    let legacy = checks(&repo, &sha);
    assert_eq!(legacy.status.code(), Some(0));
    let legacy: Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(legacy.as_object().unwrap().len(), 8);
    assert_eq!(legacy["version"], 1);
    assert!(!repo.join(".git/gitust-repository").exists());
    assert!(!repo.join(".git/gitust-local-check-runs").exists());

    let saved = record(&repo, &sha);
    assert_eq!(saved.status.code(), Some(0));
    let saved: Value = serde_json::from_slice(&saved.stdout).unwrap();
    assert_eq!(saved.as_object().unwrap().len(), 9);
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["status"], "passed");
    assert_eq!(saved["history"]["status"], "saved");
    assert_eq!(saved["history"]["reason"], Value::Null);
    assert_eq!(saved["history"]["run_id"], saved["result"]["run_id"]);
    let run_id = saved["history"]["run_id"].as_str().unwrap();
    let listed = history(&repo, None);
    assert_eq!(listed.status.code(), Some(0));
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed.as_object().unwrap().len(), 5);
    assert_eq!(listed["status"], "ok");
    assert_eq!(listed["repo_id"], saved["history"]["repo_id"]);
    assert_eq!(listed["records"].as_array().unwrap().len(), 1);
    assert_eq!(listed["records"][0]["result"], saved["result"]);
    assert_eq!(listed["records"][0]["checked_commit"], sha);
    let found = history(&repo, Some(run_id));
    assert_eq!(found.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<Value>(&found.stdout).unwrap()["records"],
        listed["records"]
    );
    let absent = history(&repo, Some("11111111-1111-4111-8111-111111111111"));
    assert_eq!(absent.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&absent.stdout).unwrap()["status"],
        "not_found"
    );

    let store = repo.join(".git/gitust-local-check-runs/history.json");
    std::fs::write(&store, "incomplete checkpoint").unwrap();
    let failed_save = record(&repo, &sha);
    assert_eq!(failed_save.status.code(), Some(1));
    let failed_save: Value = serde_json::from_slice(&failed_save.stdout).unwrap();
    assert_eq!(failed_save["status"], "passed");
    assert_eq!(failed_save["result"]["exit_code"], 0);
    assert_eq!(failed_save["cleanup"], "confirmed");
    assert_eq!(failed_save["history"]["status"], "failed");
    assert_eq!(failed_save["history"]["reason"], "invalid_storage");
    assert_eq!(
        std::fs::read_to_string(store).unwrap(),
        "incomplete checkpoint"
    );
    let unavailable = history(&repo, None);
    assert_eq!(unavailable.status.code(), Some(2));
    let unavailable: Value = serde_json::from_slice(&unavailable.stdout).unwrap();
    assert_eq!(unavailable["status"], "rejected");
    assert_eq!(unavailable["reason"], "invalid_storage");
    assert_eq!(unavailable["records"], json!([]));

    std::fs::write(
        repo.join(".gitust-checks.json"),
        r#"{"version":1,"commands":["printf failed; exit 7"]}"#,
    )
    .unwrap();
    git(&repo, &["add", ".gitust-checks.json"]);
    git(&repo, &["commit", "-qm", "failed diagnostic"]);
    let current = git(&repo, &["rev-parse", "HEAD"]);
    let failed_run = record(&repo, &current);
    assert_eq!(failed_run.status.code(), Some(7));
    let failed_run: Value = serde_json::from_slice(&failed_run.stdout).unwrap();
    assert_eq!(failed_run["status"], "failed");
    assert_eq!(failed_run["result"]["exit_code"], 7);
    assert_eq!(failed_run["history"]["status"], "failed");
    let rejected = record(&repo, &sha);
    assert_eq!(rejected.status.code(), Some(2));
    let rejected: Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(rejected["history"]["status"], "not_attempted");
    assert_eq!(rejected["history"]["repo_id"], Value::Null);
}

#[test]
fn history_query_never_initializes_a_repository_identity() {
    let (_temp, repo, _sha) = fixture(None);
    let absent = history(&repo, None);
    assert_eq!(absent.status.code(), Some(2));
    let absent: Value = serde_json::from_slice(&absent.stdout).unwrap();
    assert_eq!(absent["status"], "rejected");
    assert_eq!(absent["reason"], "repository_unavailable");
    assert_eq!(absent["repo_id"], Value::Null);
    assert_eq!(absent["records"], json!([]));
    assert!(!repo.join(".git/gitust-repository").exists());
    let id = GitEngine::new(&repo)
        .load_or_create_repository_id()
        .unwrap();
    let empty = history(&repo, None);
    assert_eq!(empty.status.code(), Some(0));
    let empty: Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(empty["status"], "ok");
    assert_eq!(empty["repo_id"], id.0.to_string());
    assert_eq!(empty["records"], json!([]));
    assert!(!repo.join(".git/gitust-local-check-runs").exists());
}

fn selected_fixture(failed: bool) -> (tempfile::TempDir, std::path::PathBuf, String, String) {
    let (temp, repo, main) = fixture(Some(r#"{"version":1,"commands":["printf main-policy"]}"#));
    git(&repo, &["checkout", "-qb", "feature/checks"]);
    std::fs::write(repo.join("marker.txt"), "selected\n").unwrap();
    let commands = if failed {
        vec!["printf selected-failed; exit 7", "printf must-not-run"]
    } else {
        vec![
            "test \"$(cat marker.txt)\" = selected && printf selected-policy",
            "printf selected-second",
        ]
    };
    std::fs::write(
        repo.join(".gitust-checks.json"),
        json!({"version":1,"commands":commands,"auto_run":false}).to_string(),
    )
    .unwrap();
    git(&repo, &["add", "marker.txt", ".gitust-checks.json"]);
    git(
        &repo,
        &[
            "commit",
            "-qm",
            "different selected branch policy and content",
        ],
    );
    let selected = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &["tag", "-a", "selected-tag", "-m", "preserved annotated tag"],
    );
    git(&repo, &["checkout", "-q", "main"]);
    (temp, repo, main, selected)
}
fn selected_checks(repo: &Path, sha: &str, source: &str, save: bool) -> Output {
    let mut cli = command(env!("CARGO_BIN_EXE_gitust"));
    cli.args(["sandbox", "checks", "--repo"]).arg(repo).args([
        "--expected-commit",
        sha,
        "--timeout",
        "10",
        "--ref",
        source,
    ]);
    if save {
        cli.arg("--record");
    }
    cli.output().unwrap()
}

#[test]
fn explicit_ordinary_ref_uses_selected_commit_without_changing_head_or_existing_tasks() {
    for bare in [false, true] {
        let (temp, source, main, selected) = selected_fixture(false);
        let repo = if bare {
            let bare = temp.path().join("repository.git");
            git(&source, &["clone", "--bare", ".", bare.to_str().unwrap()]);
            bare
        } else {
            source
        };
        let blob = git(
            &repo,
            &["rev-parse", "refs/heads/feature/checks:.gitust-checks.json"],
        );
        let engine = GitEngine::new(&repo);
        let task = engine
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::InternalBot,
                "agent/preserved-selection",
                "main",
                "Preserved selection task",
            )
            .unwrap();
        let origin_path = task.worktree_admin_dir.join("gitust-origin.json");
        let origin = std::fs::read(&origin_path).unwrap();
        drop(engine);
        if !bare {
            std::fs::write(repo.join("marker.txt"), "uncommitted source\n").unwrap();
            std::fs::write(repo.join(".gitust-checks.json"), "not committed policy").unwrap();
        }
        let head = git(&repo, &["symbolic-ref", "HEAD"]);
        let refs = git(&repo, &["show-ref"]);
        let worktrees = git(&repo, &["worktree", "list", "--porcelain"]);
        let dirty = (!bare).then(|| git(&repo, &["status", "--porcelain"]));
        for save in [false, true] {
            let output = selected_checks(&repo, &selected, "refs/heads/feature/checks", save);
            assert_eq!(output.status.code(), Some(0));
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report.as_object().unwrap().len(), if save { 9 } else { 8 });
            assert_eq!(report["version"], if save { 2 } else { 1 });
            assert_eq!(report["status"], "passed");
            assert_eq!(report["source_ref"], "refs/heads/feature/checks");
            assert_eq!(report["checked_commit"], selected);
            assert_eq!(report["checks_blob_oid"], blob);
            assert_eq!(report["cleanup"], "confirmed");
            assert_eq!(report["result"]["stdout"], "selected-policyselected-second");
            assert!(report["result"]["steps"]
                .as_array()
                .unwrap()
                .iter()
                .all(|step| step["required"] == true));
            if save {
                assert_eq!(report["history"]["status"], "saved");
                let found = history(&repo, report["result"]["run_id"].as_str());
                assert_eq!(found.status.code(), Some(0));
                let found: Value = serde_json::from_slice(&found.stdout).unwrap();
                assert_eq!(found["records"][0]["source_ref"], report["source_ref"]);
                assert_eq!(found["records"][0]["checked_commit"], selected);
                assert_eq!(found["records"][0]["checks_blob_oid"], blob);
                assert_eq!(found["records"][0]["result"], report["result"]);
            }
            assert_eq!(git(&repo, &["symbolic-ref", "HEAD"]), head);
            assert_eq!(git(&repo, &["rev-parse", "HEAD"]), main);
            assert_eq!(git(&repo, &["show-ref"]), refs);
            assert_eq!(git(&repo, &["worktree", "list", "--porcelain"]), worktrees);
            assert_eq!(std::fs::read(&origin_path).unwrap(), origin);
            if !bare {
                assert_eq!(
                    git(&repo, &["status", "--porcelain"]),
                    dirty.as_ref().unwrap().as_str()
                );
            }
        }
        git(&repo, &["fsck", "--full", "--strict"]);
    }
}

#[test]
fn explicit_ordinary_ref_reports_failed_steps_and_rejects_stale_missing_or_invalid_sources() {
    let (_temp, repo, main, selected) = selected_fixture(true);
    let refs = git(&repo, &["show-ref"]);
    let output = selected_checks(&repo, &selected, "refs/heads/feature/checks", true);
    assert_eq!(output.status.code(), Some(7));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["source_ref"], "refs/heads/feature/checks");
    assert_eq!(report["checked_commit"], selected);
    assert_eq!(report["status"], "failed");
    assert_eq!(report["history"]["status"], "saved");
    assert_eq!(report["result"]["steps"][0]["exit_code"], 7);
    assert_eq!(report["result"]["steps"][1]["exit_code"], Value::Null);
    let common = repo.join(".git");
    let saved = std::fs::read(common.join("gitust-local-check-runs/history.json")).unwrap();
    for (source, expected, reason) in [
        (
            "refs/heads/feature/checks",
            main.as_str(),
            "revision_mismatch",
        ),
        (
            "refs/heads/missing",
            selected.as_str(),
            "source_ref_unavailable",
        ),
        ("feature/checks", selected.as_str(), "invalid_source_ref"),
        (
            "refs/tags/selected-tag",
            selected.as_str(),
            "invalid_source_ref",
        ),
        (
            "refs/heads/Agent/example",
            selected.as_str(),
            "invalid_source_ref",
        ),
    ] {
        let output = selected_checks(&repo, expected, source, true);
        assert_eq!(output.status.code(), Some(2));
        let rejected: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rejected["status"], "rejected");
        assert_eq!(rejected["reason"], reason);
        assert_eq!(rejected["result"], Value::Null);
        assert_eq!(rejected["cleanup"], "not_created");
        assert_eq!(rejected["history"]["status"], "not_attempted");
        assert_eq!(
            std::fs::read(common.join("gitust-local-check-runs/history.json")).unwrap(),
            saved
        );
    }
    let missing_value = command(env!("CARGO_BIN_EXE_gitust"))
        .args(["sandbox", "checks", "--repo"])
        .arg(&repo)
        .args(["--expected-commit", selected.as_str(), "--ref"])
        .output()
        .unwrap();
    assert_eq!(missing_value.status.code(), Some(2));
    assert!(missing_value.stdout.is_empty());
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), main);
    assert_eq!(git(&repo, &["show-ref"]), refs);
    git(&repo, &["fsck", "--full", "--strict"]);
}
