use super::tests_verification::{fixture, run};
use super::*;

#[tokio::test]
async fn completed_runs_survive_restart_without_restoring_approval_authority() {
    let (tmp, state, app, token, wt, _) = fixture().await;
    let (status, body) = run(&app, &token, &wt, "printf durable-result; exit 3").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    let recovered = AppState::with_auth_store(
        tmp.path().join("repo"),
        "https://example.test/repo.git".into(),
        state.auth_store.clone(),
    )
    .unwrap();
    assert_eq!(
        recovered.worktrees.read()[0].status,
        WorktreeStatus::Pending
    );
    assert!(recovered.worktrees.read()[0].verified_commit.is_none());
    let app = build_router(recovered);
    let (status, body) = call(
        &app,
        "GET",
        format!("/api/v1/swarm/worktrees/{}/runs", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let records: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(records[0]["result"]["run_id"], result["run_id"]);
    assert_eq!(records[0]["result"]["stdout"], "durable-result");
    assert_eq!(
        records[0]["result"]["steps"][0]["output"]["stdout"],
        "durable-result"
    );
    assert_eq!(records[0]["result"]["exit_code"], 3);
    assert_eq!(records[0]["verification_requested"], true);
    assert!(records[0]["checked_commit"].is_string());
    let (status, _) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &token,
        Some(serde_json::json!({"expected_commit": records[0]["checked_commit"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn step_output_is_bounded_with_json_escaping_and_legacy_history_still_loads() {
    let (_tmp, state, app, token, wt, _) = fixture().await;
    let (_, body) = run(&app, &token, &wt, "true").await;
    let mut result: SandboxExecutionResult = serde_json::from_str(&body).unwrap();
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let path = runtime
        .common_git_dir
        .join("gitust-check-runs")
        .join(format!("{}.json", wt.id.0));
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for step in legacy["records"][0]["result"]["steps"]
        .as_array_mut()
        .unwrap()
    {
        step.as_object_mut().unwrap().remove("output");
    }
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(crate::run_history::read(&runtime.common_git_dir, wt.id, &wt.branch).is_ok());
    result.command = "\0".repeat(4096);
    result.stdout = "\0".repeat(16384);
    result.stderr = "\0".repeat(16384);
    let mut step = result.steps[0].clone();
    step.command = result.command.clone();
    step.output = Some(gitust_sandbox::SandboxCommandOutput {
        stdout: "한".repeat(1000),
        stderr: "\0".repeat(4096),
        truncated: false,
    });
    result.steps = vec![step; 9];
    let user = state
        .auth_store
        .authenticate_session(&token)
        .unwrap()
        .user_id;
    for _ in 0..5 {
        let record = crate::run_history::RunRecord::new(user, false, None, None, result.clone());
        crate::run_history::append(&runtime.common_git_dir, wt.id, &wt.branch, record).unwrap();
    }
    assert!(std::fs::metadata(&path).unwrap().len() < 3 * 1024 * 1024);
    let records = crate::run_history::read(&runtime.common_git_dir, wt.id, &wt.branch).unwrap();
    let json = serde_json::to_value(records).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 5);
    assert_eq!(json[0]["history_truncated"], true);
    let saved = &json[0]["result"]["steps"][0]["output"];
    assert_eq!(saved["stdout"].as_str().unwrap().len(), 1023);
    assert_eq!(saved["stderr"].as_str().unwrap().len(), 1024);
    assert_eq!(
        saved["truncated"], false,
        "history truncation is distinct from execution truncation"
    );
    assert_eq!(result.steps[0].output.as_ref().unwrap().stdout.len(), 3000);
}

#[tokio::test]
async fn history_storage_failure_does_not_publish_verified_state() {
    let (_tmp, state, app, token, wt, _) = fixture().await;
    let dir = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .sandbox_spec
        .scratch_base
        .parent()
        .unwrap()
        .join("gitust-check-runs");
    std::fs::write(dir, "not a directory").unwrap();
    let (status, _) = run(&app, &token, &wt, "true").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn keeps_five_newest_runs_and_discard_removes_history() {
    let (_tmp, state, app, token, wt, _) = fixture().await;
    for index in 0..6 {
        let (status, body) = run(&app, &token, &wt, &format!("printf run-{index}")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let url = format!("/api/v1/swarm/worktrees/{}/runs", wt.id.0);
    let (status, body) = call(&app, "GET", url, &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let records: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 5);
    assert_eq!(records[0]["result"]["stdout"], "run-5");
    assert_eq!(records[4]["result"]["stdout"], "run-1");
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let path = runtime
        .common_git_dir
        .join("gitust-check-runs")
        .join(format!("{}.json", wt.id.0));
    assert!(path.is_file());
    let (status, body) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!path.exists());
}

#[tokio::test]
async fn restricts_history_to_active_admin_and_rejects_corruption() {
    let (_tmp, state, app, token, wt, _) = fixture().await;
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let invite = state
        .auth_store
        .create_invite_link(
            &admin,
            "reader@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, "reader", "Password!2026")
        .unwrap();
    let reader = state.auth_store.login("reader", "Password!2026").unwrap();
    let url = format!("/api/v1/swarm/worktrees/{}/runs", wt.id.0);
    assert_eq!(
        call(&app, "GET", url.clone(), &reader.raw_session_token, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", url.clone(), "invalid", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let path = runtime
        .common_git_dir
        .join("gitust-check-runs")
        .join(format!("{}.json", wt.id.0));
    std::fs::write(&path, "not valid JSON").unwrap();
    assert_eq!(
        call(&app, "GET", url, &token, None).await.0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        run(&app, &token, &wt, "true").await.0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "not valid JSON");
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn history_bounds_utf8_output_without_changing_the_execution_response() {
    let (_tmp, state, app, token, wt, _) = fixture().await;
    let (status, body) = run(&app, &token, &wt, "printf complete").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut result: SandboxExecutionResult = serde_json::from_str(&body).unwrap();
    result.stdout = "한".repeat(6000);
    let original = result.stdout.clone();
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let record = crate::run_history::RunRecord::new(
        state
            .auth_store
            .authenticate_session(&token)
            .unwrap()
            .user_id,
        false,
        None,
        None,
        result.clone(),
    );
    crate::run_history::append(&runtime.common_git_dir, wt.id, &wt.branch, record).unwrap();
    let records = crate::run_history::read(&runtime.common_git_dir, wt.id, &wt.branch).unwrap();
    let json = serde_json::to_value(&records).unwrap();
    assert_eq!(json[0]["history_truncated"], true);
    let stored = json[0]["result"]["stdout"].as_str().unwrap();
    assert!(stored.len() <= 16384 && stored.len() > 16380);
    assert!(original.starts_with(stored));
    assert_eq!(result.stdout, original);
}

#[tokio::test]
async fn discard_checks_history_cleanup_before_removing_the_checkout() {
    let (_tmp, state, app, token, wt, dir) = fixture().await;
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let path = runtime
        .common_git_dir
        .join("gitust-check-runs")
        .join(format!("{}.json", wt.id.0));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let (status, _) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        dir.exists(),
        "failed history cleanup must leave the checkout available for recovery"
    );
    assert!(state.get_worktree_runtime(wt.id).is_some());
}
