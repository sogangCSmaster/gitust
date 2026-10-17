use super::*;

const REMOTE: &str = "https://example.test/labels.git";

fn fixture(repo: &std::path::Path) -> (AppState, Router, String, AuthenticatedPrincipal) {
    init_temp_repo(repo);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("labels", "labels@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("labels", "Password!2026")
        .unwrap()
        .raw_session_token;
    let owner = store.authenticate_session(&token).unwrap();
    let state = AppState::with_auth_store(repo.to_owned(), REMOTE.into(), store).unwrap();
    (state.clone(), build_router(state), token, owner)
}

async fn recovered(
    repo: &std::path::Path,
    state: &AppState,
    token: &str,
    owner: UserId,
    original: &AgentWorktree,
) {
    let restarted =
        AppState::with_auth_store(repo.to_owned(), REMOTE.into(), state.auth_store.clone())
            .unwrap();
    assert_eq!(
        restarted
            .get_worktree_runtime(original.id)
            .unwrap()
            .created_by,
        Some(owner)
    );
    let (status, body) = call(
        &build_router(restarted),
        "GET",
        "/api/v1/swarm/worktrees".into(),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: Vec<AgentWorktree> = serde_json::from_str(&body).unwrap();
    assert_eq!(list.len(), 1);
    let wt = &list[0];
    assert_eq!(wt.id, original.id);
    assert_eq!(wt.prompt_summary, original.prompt_summary);
    assert_eq!(wt.branch, original.branch);
    assert_eq!(wt.base_branch, original.base_branch);
    assert_eq!(wt.base_commit, original.base_commit);
    assert_eq!(wt.status, WorktreeStatus::Pending);
    assert!(wt.verified_commit.is_none());
    assert!(wt.verified_checks_oid.is_none());
}

#[tokio::test]
async fn http_worktree_label_survives_restart_without_restoring_verification() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let (state, app, token, owner) = fixture(&repo);
    let prefix = "검토할 작업\n";
    // JSON escapes expand each remaining one-byte label character to six bytes on disk/wire.
    let label = format!(
        "{prefix}{}",
        "\u{0001}".repeat(MAX_PROMPT_SUMMARY_BYTES - prefix.len())
    );
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({"branch":"agent/labels/http", "prompt_summary":label})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let wt: AgentWorktree = serde_json::from_str(&body).unwrap();
    assert_eq!(wt.prompt_summary, label);

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({"branch":"agent/labels/too-long", "prompt_summary":format!("{label}x")})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(state.git_engine.list_agent_worktrees().unwrap().len(), 1);

    // Simulate ephemeral run evidence: restart may retain the label, never this authority.
    set_worktree_status(&state, wt.id, |task| {
        task.status = WorktreeStatus::Verified;
        task.verified_commit = Some(task.base_commit.clone());
        task.verified_checks_oid = Some("temporary-checks-evidence".into());
    });
    recovered(&repo, &state, &token, owner.user_id, &wt).await;
}

#[tokio::test]
async fn mcp_worktree_label_survives_restart_and_rejects_oversized_utf8() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let (state, _app, token, owner) = fixture(&repo);
    let label = "Fix file selection — 파일 전환";
    let request = |branch: &str, label: &str| {
        serde_json::json!({
            "jsonrpc":"2.0", "id":1, "method":"tools/call",
            "params":{"name":"gitust_worktree_spawn", "arguments":{
                "agent":"cursor", "branch":branch, "base_branch":"main",
                "prompt_summary":label
            }}
        })
        .to_string()
    };
    let response = handle_mcp_json_line(&state, &owner, &request("agent/labels/mcp", label))
        .await
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    let wt = state.worktrees.read().front().unwrap().clone();
    assert_eq!(wt.prompt_summary, label);
    let oversized = "가".repeat(MAX_PROMPT_SUMMARY_BYTES / 3 + 1);
    let response = handle_mcp_json_line(
        &state,
        &owner,
        &request("agent/labels/too-long", &oversized),
    )
    .await
    .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(state.git_engine.list_agent_worktrees().unwrap().len(), 1);
    recovered(&repo, &state, &token, owner.user_id, &wt).await;
}
