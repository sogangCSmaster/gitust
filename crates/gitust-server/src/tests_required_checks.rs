use super::tests_verification::{commit, fixture, run};
use super::*;

fn policy(repo: &std::path::Path, commands: &[&str]) {
    std::fs::write(
        repo.join(".gitust-checks.json"),
        serde_json::json!({
            "version": 1, "commands": commands
        })
        .to_string(),
    )
    .unwrap();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["add", ".gitust-checks.json"])
        .status()
        .unwrap();
    assert!(status.success());
    commit(repo);
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
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
    String::from_utf8(out.stdout).unwrap().trim().into()
}

#[tokio::test]
async fn manual_success_cannot_skip_required_base_checks() {
    let (tmp, state, app, token, wt, _dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        &["printf required-check; exit 7", "echo must-not-run"],
    );
    let checked = state
        .git_engine
        .clean_worktree_commit(wt.id, &wt.branch)
        .unwrap();
    let (status, body) = run(&app, &token, &wt, "exit 0").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(result["exit_code"], 7, "{body}");
    assert!(result["stdout"]
        .as_str()
        .unwrap()
        .contains("required-check"));
    assert!(!result["stdout"].as_str().unwrap().contains("must-not-run"));
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    assert_eq!(
        call(
            &app,
            "POST",
            format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
            &token,
            Some(serde_json::json!({"expected_commit": checked.0}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn agent_policy_cannot_replace_base_policy_and_ordinary_exec_is_not_verification() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), &["exit 9"]);
    policy(&dir, &["true"]);
    let (status, body) = run(
        &app,
        &token,
        &wt,
        "printf '%s' \"quote's\"; exit 0 # comment",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["exit_code"],
        9
    );
    let (status, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
        &token,
        Some(serde_json::json!({"command":"true","verify":false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Pending);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn checks_success_binds_policy_and_commit_then_allows_approval() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    let repo = tmp.path().join("repo");
    policy(&repo, &["printf first", "test -f README.md; printf second"]);
    git(&dir, &["reset", "--hard", "main"]);
    commit(&dir);
    let (status, body) = run(&app, &token, &wt, "exit 0").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(result["stdout"], "firstsecond");
    assert_eq!(
        state.worktrees.read()[0].verified_checks_oid.as_deref(),
        Some(git(&repo, &["rev-parse", "main:.gitust-checks.json"]).as_str())
    );
    let verified = state.worktrees.read()[0].verified_commit.clone().unwrap();
    let (status, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &token,
        Some(serde_json::json!({"expected_commit": verified.0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), verified.0);
    git(&repo, &["fsck", "--full", "--strict"]);
    git(&dir, &["fsck", "--full", "--strict"]);
}

#[tokio::test]
async fn added_changed_or_removed_policy_invalidates_prior_verification() {
    for change in ["added", "changed", "removed"] {
        let (tmp, state, app, token, wt, dir) = fixture().await;
        let repo = tmp.path().join("repo");
        if change != "added" {
            policy(&repo, &["true"]);
            git(&dir, &["reset", "--hard", "main"]);
            commit(&dir);
        }
        assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
        let checked = state.worktrees.read()[0].verified_commit.clone().unwrap();
        if change == "removed" {
            git(&repo, &["rm", ".gitust-checks.json"]);
            commit(&repo);
        } else {
            policy(&repo, &["exit 7"]);
        }
        let before = git(&repo, &["rev-parse", "main"]);
        let (status, body) = call(
            &app,
            "POST",
            format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
            &token,
            Some(serde_json::json!({"expected_commit": checked.0})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{change}: {body}");
        assert!(body.contains("Checks changed"), "{body}");
        assert!(state.worktrees.read()[0].verified_commit.is_none());
        assert!(state.worktrees.read()[0].verified_checks_oid.is_none());
        assert_eq!(git(&repo, &["rev-parse", "main"]), before);
    }
}

#[tokio::test]
async fn invalid_policy_fails_closed_before_any_command_runs() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    let repo = tmp.path().join("repo");
    policy(&repo, &[]);
    let (status, body) = run(&app, &token, &wt, "touch should-not-run").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(!dir.join("should-not-run").exists());
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn policy_changed_while_running_cannot_create_verified_evidence() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    let repo = tmp.path().join("repo");
    policy(&repo, &["true"]);
    let request_app = app.clone();
    let request_token = token.clone();
    let request_wt = wt.clone();
    let pending = tokio::spawn(async move {
        run(
            &request_app,
            &request_token,
            &request_wt,
            "touch started; while test ! -f proceed; do sleep 0.05; done; rm started proceed",
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !dir.join("started").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("sandbox command started after policy capture");
    policy(&repo, &["exit 7"]);
    std::fs::write(dir.join("proceed"), "").unwrap();
    let (status, body) = pending.await.unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("Checks changed during"), "{body}");
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    assert!(state.worktrees.read()[0].verified_checks_oid.is_none());
}

#[tokio::test]
async fn required_checks_timeout_clears_previous_evidence_and_releases_worktree() {
    let (tmp, state, app, token, wt, _dir) = fixture().await;
    let (status, body) = run(&app, &token, &wt, "true").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Verified);
    policy(&tmp.path().join("repo"), &["sleep 10"]);
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let result = crate::worktrees::exec_worktree_sandbox_core(
        &state,
        &caller,
        wt.id.0,
        "true".into(),
        Some(1),
        true,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    assert!(state.worktrees.read()[0].verified_checks_oid.is_none());
    assert!(state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .try_lock()
        .is_ok());
    // Releasing the in-memory mutex does not authorize reuse after uncertain process cleanup.
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let before = serde_json::to_value(
        crate::run_history::read(&runtime.common_git_dir, wt.id, &wt.branch).unwrap(),
    )
    .unwrap();
    let (status, _) = run(&app, &token, &wt, "true").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(runtime.worktree_dir.exists());
    assert_eq!(
        serde_json::to_value(
            crate::run_history::read(&runtime.common_git_dir, wt.id, &wt.branch).unwrap()
        )
        .unwrap(),
        before
    );
}
