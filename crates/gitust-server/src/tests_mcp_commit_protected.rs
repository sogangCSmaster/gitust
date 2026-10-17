use super::*;

#[tokio::test]
async fn mcp_commit_cannot_read_auth_state_hidden_from_its_execution_sandbox() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_temp_repo(&repo);
    let outside = temp.path().join("private/auth-db.json");
    let store = AuthStore::open_persistent(&outside).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "SyntheticPassword!2026")
        .unwrap();
    let session = store.login("admin", "SyntheticPassword!2026").unwrap();
    let caller = store
        .authenticate_session(&session.raw_session_token)
        .unwrap();
    let engine = GitEngine::new(&repo);
    let task = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::CodexSwarm,
            "agent/test/protected-source",
            "main",
            "protected source regression",
        )
        .unwrap();
    crate::worktree_owners::persist(&task, caller.user_id).unwrap();
    let private = task.worktree_dir.join("private-auth");
    std::fs::create_dir(&private).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let relocated = private.join("auth-db.json");
    std::fs::copy(&outside, &relocated).unwrap();
    let store = AuthStore::open_persistent(&relocated).unwrap();
    let state = AppState::with_auth_store(
        repo.clone(),
        "https://example.invalid/repo.git".into(),
        store,
    )
    .unwrap()
    .with_trusted_sandbox_exec(true);
    let app = build_router(state.clone());
    std::fs::hard_link(&relocated, task.worktree_dir.join("auth-alias.json")).unwrap();
    let complex = format!("{}leaf", "a/".repeat(1000));
    let mut denied_paths = vec![
        ("private-auth/auth-db.json", 403),
        ("private-auth/auth-db.json.lock", 403),
        ("auth-alias.json", 403),
        (complex.as_str(), 400),
    ];
    if task.worktree_dir.join("PRIVATE-AUTH/auth-db.json").exists() {
        denied_paths.push(("PRIVATE-AUTH/auth-db.json", 403));
    }
    for (path, expected_status) in denied_paths {
        let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"gitust_worktree_commit","arguments":{"worktree_id":task.worktree.id.0,
            "expected_head":task.worktree.base_commit.0,"paths":[path],"message":"Reject disallowed commit input"}}});
        let (status, text) = call(
            &app,
            "POST",
            "/mcp".into(),
            &session.raw_session_token,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let response: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            response["result"]["isError"], true,
            "disallowed source was accepted"
        );
        let payload: serde_json::Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(payload["http_status"], expected_status);
        assert_eq!(
            GitEngine::new(&task.worktree_dir)
                .inspect_head_zero_copy()
                .unwrap()
                .head_sha,
            task.worktree.base_commit,
        );
    }
    std::fs::write(
        task.worktree_dir.join("README.md"),
        "permitted source edit\n",
    )
    .unwrap();
    let body = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"gitust_worktree_commit","arguments":{"worktree_id":task.worktree.id.0,
        "expected_head":task.worktree.base_commit.0,"paths":["README.md"],"message":"Commit an allowed source"}}});
    let (_, text) = call(
        &app,
        "POST",
        "/mcp".into(),
        &session.raw_session_token,
        Some(body),
    )
    .await;
    let response: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(response["result"]["isError"], false, "{text}");
    GitEngine::verify_git_fsck(&repo).unwrap();
}
