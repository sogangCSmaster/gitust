use super::tests_verification::{fixture, run};
use super::*;

#[tokio::test]
async fn approval_binds_the_commit_observed_by_the_human_client() {
    let (_temp, state, app, token_a, wt, dir) = fixture().await;
    let (status, result) = run(&app, &token_a, &wt, "true").await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let observed_a = state.worktrees.read()[0].verified_commit.clone().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result).unwrap()["checked_commit"],
        observed_a.0
    );
    let main_before = state.git_engine.inspect_head_zero_copy().unwrap().head_sha;
    let token_b = state
        .auth_store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let caller_b = state.auth_store.authenticate_session(&token_b).unwrap();
    std::fs::write(dir.join("README.md"), "second client change\n").unwrap();
    let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"gitust_worktree_commit","arguments":{"worktree_id":wt.id.0,
        "expected_head":observed_a.0,"paths":["README.md"],"message":"Second client commit"}}});
    let response =
        handle_mcp_json_line_with_options(&state, &caller_b, &request.to_string(), false)
            .await
            .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap()["result"]["isError"],
        false
    );
    assert_eq!(run(&app, &token_b, &wt, "true").await.0, StatusCode::OK);
    let observed_b = state.worktrees.read()[0].verified_commit.clone().unwrap();
    assert_ne!(observed_a, observed_b);
    let uri = format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0);
    let (status, body) = call(
        &app,
        "POST",
        uri.clone(),
        &token_a,
        Some(serde_json::json!({"expected_commit":observed_a.0})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        state.git_engine.inspect_head_zero_copy().unwrap().head_sha,
        main_before
    );
    assert!(state
        .git_engine
        .load_worktree_approval(wt.id)
        .unwrap()
        .is_none());
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Verified);
    assert_eq!(
        state.worktrees.read()[0].verified_commit.as_ref(),
        Some(&observed_b)
    );
    let (status, body) = call(
        &app,
        "POST",
        uri,
        &token_a,
        Some(serde_json::json!({"expected_commit":observed_b.0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        state.git_engine.inspect_head_zero_copy().unwrap().head_sha,
        observed_b
    );
    GitEngine::verify_git_fsck(state.git_engine.repo_path()).unwrap();
}

#[tokio::test]
async fn approval_requires_a_bounded_explicit_commit_precondition() {
    let (_temp, state, app, token, wt, _dir) = fixture().await;
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    let checked = state.worktrees.read()[0].verified_commit.clone().unwrap();
    let uri = format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0);
    for body in [
        None,
        Some(serde_json::json!({})),
        Some(serde_json::json!({"expected_commit":"main"})),
        Some(serde_json::json!({"expected_commit":checked.0,"ignored":true})),
    ] {
        let (status, response) = call(&app, "POST", uri.clone(), &token, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    }
    let (status, _) = call(
        &app,
        "POST",
        uri.clone(),
        &token,
        Some(serde_json::json!({"expected_commit":"a".repeat(2048)})),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        state.worktrees.read()[0].verified_commit.as_ref(),
        Some(&checked)
    );
    assert!(state
        .git_engine
        .load_worktree_approval(wt.id)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn approval_reauthenticates_after_reading_its_commit_precondition() {
    let (_temp, state, app, token, wt, _dir) = fixture().await;
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    let checked = state.worktrees.read()[0].verified_commit.clone().unwrap();
    let before = state.git_engine.inspect_head_zero_copy().unwrap().head_sha;
    let body = RevokeOnRead {
        store: state.auth_store.clone(),
        caller: state.auth_store.authenticate_session(&token).unwrap(),
        token: token.clone(),
        data: Some(serde_json::to_vec(&serde_json::json!({"expected_commit":checked.0})).unwrap()),
    };
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::new(body))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        state.git_engine.inspect_head_zero_copy().unwrap().head_sha,
        before
    );
    assert!(state
        .git_engine
        .load_worktree_approval(wt.id)
        .unwrap()
        .is_none());
}

struct RevokeOnRead {
    store: AuthStore,
    caller: AuthenticatedPrincipal,
    token: String,
    data: Option<Vec<u8>>,
}

impl http_body::Body for RevokeOnRead {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let Some(data) = self.data.take() else {
            return std::task::Poll::Ready(None);
        };
        self.store
            .revoke_session(&self.caller, &self.token)
            .unwrap();
        std::task::Poll::Ready(Some(Ok(http_body::Frame::data(data.into()))))
    }
}
