use super::*;

#[tokio::test]
async fn outbound_requires_approval_privilege_and_preserves_exact_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let remote = tmp.path().join("remote.git");
    init_temp_repo(&repo);
    std::fs::create_dir(&remote).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Gitust Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Gitust Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&remote, &["init", "--bare", "-b", "main"]);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let login = store.login("admin", "Password!2026").unwrap();
    let principal = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    let mcp_token = store.issue_oauth_session(principal.user_id).unwrap();
    let invite = store
        .create_invite_link(
            &principal,
            "dev@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    store
        .redeem_invite_link(&invite.raw_token, "dev", "Password!2026")
        .unwrap();
    let dev = store.login("dev", "Password!2026").unwrap();
    let state =
        AppState::with_auth_store(repo.clone(), remote.to_str().unwrap().into(), store.clone())
            .expect("recover server state");
    let app = build_router(state.clone());
    let (status, text) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &login.raw_session_token,
        Some(serde_json::json!({"branch":"agent/test/outbound", "prompt_summary":"outbound"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let wt: AgentWorktree = serde_json::from_str(&text).unwrap();
    let uri = format!("/api/v1/swarm/worktrees/{}/publish", wt.id.0);
    let request = || Some(serde_json::json!({}));
    assert_eq!(
        call(
            &app,
            "POST",
            uri.clone(),
            &login.raw_session_token,
            request()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    git(
        &runtime.worktree_dir,
        &["commit", "--allow-empty", "-m", "approved"],
    );
    let verified = state
        .git_engine
        .clean_worktree_commit(wt.id, &wt.branch)
        .unwrap();
    set_worktree_status(&state, wt.id, |w| {
        w.status = WorktreeStatus::Verified;
        w.verified_commit = Some(verified.clone());
    });
    let approved_uri = format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0);
    assert_eq!(
        call(
            &app,
            "POST",
            approved_uri.clone(),
            &mcp_token.raw_session_token,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (s, body) = call(
        &app,
        "POST",
        approved_uri,
        &login.raw_session_token,
        Some(serde_json::json!({"expected_commit": verified.0})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let approved: AgentWorktree = serde_json::from_str(&body).unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            uri.clone(),
            &mcp_token.raw_session_token,
            request()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "later unapproved change"],
    );
    drop(app);
    drop(state);
    let state =
        AppState::with_auth_store(repo.clone(), remote.to_str().unwrap().into(), store.clone())
            .unwrap();
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Merged);
    assert_eq!(state.worktrees.read()[0].base_commit, approved.base_commit);
    assert!(!state.worktrees.read()[0].github_synced);
    let app = build_router(state.clone());
    let new_sha = git(&repo, &["rev-parse", "main"]);
    let command = format!(
        "{} {new_sha} refs/heads/{}\0report-status\n",
        approved.base_commit.0, wt.branch
    );
    let push = Request::builder()
        .method("POST")
        .uri("/git/default.git/git-receive-pack")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", login.raw_session_token),
        )
        .body(Body::from(format!(
            "{:04x}{command}0000",
            command.len() + 4
        )))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(push).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "POST", uri.clone(), &dev.raw_session_token, request())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            uri.clone(),
            &login.raw_session_token,
            Some(serde_json::json!({"github_remote_url":"file:///tmp/other"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let guard = state.outbound_sync_lock.clone().lock_owned().await;
    assert_eq!(
        call(
            &app,
            "POST",
            uri.clone(),
            &login.raw_session_token,
            request()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    drop(guard);
    let unavailable = tmp.path().join("unavailable.git");
    std::fs::create_dir(&unavailable).unwrap();
    let mut failed_state = state.clone();
    failed_state.sync_config.github_remote_url = unavailable.to_str().unwrap().into();
    let failed_app = build_router(failed_state);
    assert_eq!(
        call(
            &failed_app,
            "POST",
            uri.clone(),
            &login.raw_session_token,
            request()
        )
        .await
        .0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Merged);
    assert!(!state.worktrees.read()[0].github_synced);
    let (s, body) = call(
        &app,
        "POST",
        uri.clone(),
        &login.raw_session_token,
        request(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        serde_json::from_str::<AgentWorktree>(&body)
            .unwrap()
            .github_synced
    );
    assert_eq!(
        git(&remote, &["show-ref"]),
        format!("{} refs/heads/main", approved.base_commit.0)
    );
    let restarted =
        AppState::with_auth_store(repo.clone(), remote.to_str().unwrap().into(), store.clone())
            .unwrap();
    assert!(restarted.worktrees.read()[0].github_synced);
    let other_destination = AppState::with_auth_store(
        repo.clone(),
        unavailable.to_str().unwrap().into(),
        store.clone(),
    )
    .unwrap();
    assert!(!other_destination.worktrees.read()[0].github_synced);
    let receipt_path = repo
        .join(".git/gitust-approvals")
        .join(format!("{}.json", wt.id.0));
    let receipt = std::fs::read(&receipt_path).unwrap();
    std::fs::write(&receipt_path, "corrupt").unwrap();
    assert!(AppState::with_auth_store(
        repo.clone(),
        remote.to_str().unwrap().into(),
        store.clone()
    )
    .is_err());
    std::fs::write(&receipt_path, receipt).unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            uri.clone(),
            &login.raw_session_token,
            request()
        )
        .await
        .0,
        StatusCode::OK
    );
    let streamed = Request::builder()
        .method("POST")
        .uri(&uri)
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", login.raw_session_token),
        )
        .body(Body::new(RevokeDuringBody {
            store,
            principal,
            token: login.raw_session_token.clone(),
            sent: false,
        }))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(streamed).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "POST", uri, &login.raw_session_token, request())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    GitEngine::verify_git_fsck(&remote).unwrap();
}

// Deterministic body boundary: revocation happens only after the handler's initial auth.
struct RevokeDuringBody {
    store: AuthStore,
    principal: AuthenticatedPrincipal,
    token: String,
    sent: bool,
}

impl http_body::Body for RevokeDuringBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if self.sent {
            return std::task::Poll::Ready(None);
        }
        self.store
            .revoke_session(&self.principal, &self.token)
            .unwrap();
        self.sent = true;
        std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes::Bytes::from_static(
            b"{}",
        )))))
    }
}
