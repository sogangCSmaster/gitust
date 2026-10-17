use super::*;

#[tokio::test]
async fn stdio_operator_can_resume_commits_after_restart_without_transferring_ownership() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    init_temp_repo(&repo);
    let original = AppState::new(repo.clone(), "https://example.invalid/repo.git".into())
        .expect("state")
        .with_trusted_sandbox_exec(true);
    let creator = AuthenticatedPrincipal {
        user_id: UserId::new(),
        username: "earlier-local-operator".into(),
        role: EnterpriseRole::SiteAdmin,
        team_ids: vec![],
    };
    let spawn = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"gitust_worktree_spawn","arguments":{"branch":"agent/test/stdio-resume"}}});
    let spawned: serde_json::Value = serde_json::from_str(
        &handle_mcp_json_line(&original, &creator, &spawn.to_string())
            .await
            .expect("spawn response"),
    )
    .expect("JSON");
    assert_eq!(spawned["result"]["isError"], false);
    let task = original.worktrees.read()[0].clone();
    let checkout = original
        .get_worktree_runtime(task.id)
        .expect("runtime")
        .worktree_dir;
    let mut expected = task.base_commit.0;
    for number in 0..2 {
        let recovered = AppState::new(repo.clone(), "https://example.invalid/repo.git".into())
            .expect("recovery")
            .with_trusted_sandbox_exec(true);
        assert_eq!(
            recovered
                .get_worktree_runtime(task.id)
                .expect("owner")
                .created_by,
            Some(creator.user_id)
        );
        std::fs::write(
            checkout.join("README.md"),
            format!("stdio update {number}\n"),
        )
        .expect("edit");
        let request = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"gitust_worktree_commit","arguments":{"worktree_id":task.id.0,
            "expected_head":expected,"paths":["README.md"],"message":"Resume local operator task"}}});
        let outsider = AuthenticatedPrincipal {
            user_id: UserId::new(),
            ..creator.clone()
        };
        let denied: serde_json::Value = serde_json::from_str(
            &handle_mcp_json_line_with_options(&recovered, &outsider, &request.to_string(), false)
                .await
                .expect("principal response"),
        )
        .expect("JSON");
        assert_eq!(denied["result"]["isError"], true);
        let mut output = Vec::new();
        let input = tokio::io::BufReader::new(std::io::Cursor::new(request.to_string() + "\n"));
        serve_mcp_stream(recovered.clone(), input, &mut output)
            .await
            .expect("stdio stream");
        let response: serde_json::Value = serde_json::from_slice(&output).expect("JSON");
        assert_eq!(response["result"]["isError"], false, "{response}");
        let payload: serde_json::Value = serde_json::from_str(
            response["result"]["content"][0]["text"]
                .as_str()
                .expect("text"),
        )
        .expect("payload");
        assert_eq!(payload["parent"], expected);
        expected = payload["commit"].as_str().expect("commit").into();
        assert_eq!(
            recovered
                .git_engine
                .clean_worktree_commit(task.id, &task.branch)
                .expect("clean")
                .0,
            expected
        );
        assert_eq!(
            recovered
                .get_worktree_runtime(task.id)
                .expect("retained owner")
                .created_by,
            Some(creator.user_id)
        );
        assert!(recovered.worktrees.read()[0].verified_commit.is_none());
    }
    GitEngine::verify_git_fsck(&repo).expect("main fsck");
    GitEngine::verify_git_fsck(checkout).expect("worktree fsck");
}
