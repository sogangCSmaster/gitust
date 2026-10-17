use super::*;

async fn mcp_call(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let request = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":name,"arguments":arguments}}).to_string();
    serde_json::from_str(
        &handle_mcp_json_line_with_options(state, caller, &request, false)
            .await
            .expect("MCP response"),
    )
    .expect("JSON response")
}

#[tokio::test]
async fn mcp_cannot_remove_human_approval_but_operator_http_can() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    init_temp_repo(&repo);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "SyntheticPassword!2026")
        .expect("bootstrap");
    let login = store
        .login("admin", "SyntheticPassword!2026")
        .expect("login");
    let caller = store
        .authenticate_session(&login.raw_session_token)
        .expect("principal");
    let state = AppState::with_auth_store(
        repo.clone(),
        "https://example.invalid/repo.git".into(),
        store,
    )
    .expect("state");
    let spawned = mcp_call(
        &state,
        &caller,
        "gitust_worktree_spawn",
        serde_json::json!({"branch":"agent/review/approved", "base_branch":"main"}),
    )
    .await;
    assert_eq!(spawned["result"]["isError"], false);
    let worktree = state
        .worktrees
        .read()
        .front()
        .cloned()
        .expect("spawned worktree");
    let runtime = state.get_worktree_runtime(worktree.id).expect("runtime");
    std::fs::write(
        runtime.worktree_dir.join("review.txt"),
        "synthetic change\n",
    )
    .expect("write");
    for args in [
        vec!["add", "review.txt"],
        vec!["commit", "-q", "-m", "synthetic change"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&runtime.worktree_dir)
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Review")
            .env("GIT_AUTHOR_EMAIL", "review@example.invalid")
            .env("GIT_COMMITTER_NAME", "Review")
            .env("GIT_COMMITTER_EMAIL", "review@example.invalid")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    state
        .git_engine
        .approve_managed_worktree(worktree.id, &worktree.branch, &worktree.base_branch)
        .expect("human approval");
    let state = AppState::with_auth_store(
        repo,
        "https://example.invalid/repo.git".into(),
        state.auth_store.clone(),
    )
    .expect("recover approval");
    assert_eq!(
        state.worktrees.read().front().expect("recovered").status,
        WorktreeStatus::Merged
    );
    let approval_path = runtime
        .common_git_dir
        .join("gitust-approvals")
        .join(format!("{}.json", worktree.id.0));
    let approval = std::fs::read(&approval_path).expect("approval receipt");
    let history_dir = runtime.common_git_dir.join("gitust-check-runs");
    std::fs::create_dir_all(&history_dir).expect("history directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&history_dir, std::fs::Permissions::from_mode(0o700))
            .expect("private history directory");
    }
    let history_path = history_dir.join(format!("{}.json", worktree.id.0));
    std::fs::write(&history_path, b"synthetic diagnostics").expect("history sentinel");

    let rejected = mcp_call(
        &state,
        &caller,
        "gitust_worktree_discard",
        serde_json::json!({"worktree_id":worktree.id.0}),
    )
    .await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        std::fs::read(&approval_path).expect("approval preserved"),
        approval
    );
    assert_eq!(
        std::fs::read(&history_path).expect("history preserved"),
        b"synthetic diagnostics"
    );
    assert!(runtime.worktree_dir.is_dir());
    assert_eq!(state.worktrees.read().len(), 1);

    let app = build_router(state.clone());
    let (status, body) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", worktree.id.0),
        &login.raw_session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!approval_path.exists());
    assert!(!history_path.exists());
    assert!(!runtime.worktree_dir.exists());
}

#[tokio::test]
async fn mcp_owner_can_discard_an_unmerged_worktree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    init_temp_repo(&repo);
    let state = AppState::new(repo, "https://example.invalid/repo.git".into()).expect("state");
    let caller = AuthenticatedPrincipal {
        user_id: UserId::new(),
        username: "agent-owner".into(),
        role: EnterpriseRole::Developer,
        team_ids: vec![],
    };
    assert_eq!(
        mcp_call(
            &state,
            &caller,
            "gitust_worktree_spawn",
            serde_json::json!({"branch":"agent/review/unmerged", "base_branch":"main"})
        )
        .await["result"]["isError"],
        false
    );
    let worktree = state
        .worktrees
        .read()
        .front()
        .cloned()
        .expect("spawned worktree");
    let dir = state
        .get_worktree_runtime(worktree.id)
        .expect("runtime")
        .worktree_dir;
    assert_eq!(
        mcp_call(
            &state,
            &caller,
            "gitust_worktree_discard",
            serde_json::json!({"worktree_id":worktree.id.0})
        )
        .await["result"]["isError"],
        false
    );
    assert!(!dir.exists());
    assert!(state.worktrees.read().is_empty());
}
