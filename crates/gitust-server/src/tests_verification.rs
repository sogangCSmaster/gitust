use super::*;

fn commit(dir: &std::path::Path) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-am",
            "new revision",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn fixture() -> (
    tempfile::TempDir,
    AppState,
    Router,
    String,
    AgentWorktree,
    PathBuf,
) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let state = AppState::with_auth_store(repo, "https://example.test/repo.git".into(), store)
        .unwrap()
        .with_trusted_sandbox_exec(true);
    let app = build_router(state.clone());
    let (status, body) = call(&app, "POST", "/api/v1/swarm/worktrees".into(), &token,
        Some(serde_json::json!({"branch":"agent/test/verification", "prompt_summary":"verify exact revision"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let wt: AgentWorktree = serde_json::from_str(&body).unwrap();
    let dir = state.get_worktree_runtime(wt.id).unwrap().worktree_dir;
    commit(&dir);
    (tmp, state, app, token, wt, dir)
}

async fn run(app: &Router, token: &str, wt: &AgentWorktree, command: &str) -> (StatusCode, String) {
    call(
        app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
        token,
        Some(serde_json::json!({"verify":true,"command":command})),
    )
    .await
}

#[tokio::test]
async fn approval_rejects_local_commit_after_successful_verification() {
    let (_tmp, state, app, token, wt, dir) = fixture().await;
    let (status, body) = run(&app, &token, &wt, "test -f README.md").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Verified);
    assert!(state.worktrees.read()[0].verified_commit.is_some());
    std::fs::write(dir.join("README.md"), "not verified\n").unwrap();
    commit(&dir);
    let (status, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_ne!(state.worktrees.read()[0].status, WorktreeStatus::Verified);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    // A new verification of the new commit permits approval; the stale result cannot.
    assert_eq!(
        run(&app, &token, &wt, "test -f README.md").await.0,
        StatusCode::OK
    );
    let checked = state.worktrees.read()[0].verified_commit.clone().unwrap();
    let (status, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let merged: AgentWorktree = serde_json::from_str(&body).unwrap();
    assert_eq!(merged.base_commit, checked);
}

#[tokio::test]
async fn successful_command_cannot_verify_modified_checkout() {
    let (_tmp, state, app, token, wt, _dir) = fixture().await;
    let (status, body) = run(&app, &token, &wt, "printf changed > README.md").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn dirty_checkout_and_failed_rerun_never_retain_approval_authority() {
    let (_tmp, state, app, token, wt, dir) = fixture().await;
    std::fs::write(dir.join("untracked.txt"), "must commit first").unwrap();
    let (status, body) = run(&app, &token, &wt, "touch should-not-run").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(!dir.join("should-not-run").exists());
    std::fs::remove_file(dir.join("untracked.txt")).unwrap();
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    assert!(state.worktrees.read()[0].verified_commit.is_some());
    let (status, body) = run(&app, &token, &wt, "echo failure >&2; exit 7").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["exit_code"],
        7
    );
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    std::fs::write(dir.join("README.md"), "dirty after verification").unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
            &token,
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn restart_does_not_infer_verification_from_a_clean_branch() {
    let (tmp, state, app, token, wt, _dir) = fixture().await;
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    let restarted = AppState::with_auth_store(
        tmp.path().join("repo"),
        "https://example.test/repo.git".into(),
        state.auth_store.clone(),
    )
    .unwrap();
    assert_eq!(
        restarted.worktrees.read()[0].status,
        WorktreeStatus::Pending
    );
    assert!(restarted.worktrees.read()[0].verified_commit.is_none());
    let app = build_router(restarted);
    assert_eq!(
        call(
            &app,
            "POST",
            format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
            &token,
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn index_hidden_changes_cannot_receive_verification() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let (_tmp, state, app, token, wt, dir) = fixture().await;
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["update-index", flag, "README.md"])
            .output()
            .unwrap();
        assert!(output.status.success());
        std::fs::write(dir.join("README.md"), "index hides this change\n").unwrap();
        let (status, body) = run(&app, &token, &wt, "true").await;
        assert_eq!(status, StatusCode::CONFLICT, "{flag}: {body}");
        assert!(state.worktrees.read()[0].verified_commit.is_none());
    }
}
