use super::*;

#[tokio::test]
async fn http_gate_public_registration_and_unauthenticated_worktrees_blocked() {
    let (app, _) = test_app();

    // 1. Public registration is strictly forbidden
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .body(Body::empty())
        .expect("build request");
    let res = app.clone().oneshot(req).await.expect("execute request");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 2. Unauthenticated worktree listing or GitHub outbound sync must be rejected (401 UNAUTHORIZED)
    let unauth_wt = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/swarm/worktrees")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(unauth_wt.status(), StatusCode::UNAUTHORIZED);

    let unauth_sync = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sync/github")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(unauth_sync.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn http_gate_invite_redemption_bola_bfla_and_instant_deactivation() {
    let (app, admin_token) = test_app();
    let team_1 = TeamId::new();
    let team_2 = TeamId::new();

    // 1. Admin creates invite for Alice (Developer in Team 1)
    let create_inv_body = serde_json::to_vec(&CreateInviteRequest {
        target_email: "alice@company.internal".to_owned(),
        assigned_role: EnterpriseRole::Developer,
        team_ids: vec![team_1],
        ttl_hours: Some(72),
    })
    .expect("serialize");

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/invites")
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create_inv_body))
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(res.status(), StatusCode::CREATED);
    let body_bytes = res.into_body().collect().await.expect("body").to_bytes();
    let issued_alice: IssuedInviteLink = serde_json::from_slice(&body_bytes).expect("parse invite");

    // 2. Alice redeems invite
    let redeem_body = serde_json::to_vec(&RedeemInviteRequest {
        raw_token: issued_alice.raw_token.clone(),
        username: "alice".to_owned(),
        password: "AlicePassword!2026".to_owned(),
    })
    .expect("serialize");

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/invites/redeem")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(redeem_body))
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(res.status(), StatusCode::CREATED);
    let body_bytes = res.into_body().collect().await.expect("body").to_bytes();
    let alice_profile: UserPublicProfile =
        serde_json::from_slice(&body_bytes).expect("parse profile");

    // 3. Alice logs in
    let login_body = serde_json::to_vec(&LoginRequest {
        identifier: "alice".to_owned(),
        password: "AlicePassword!2026".to_owned(),
    })
    .expect("serialize");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(login_body))
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(res.status(), StatusCode::OK);
    let body_bytes = res.into_body().collect().await.expect("body").to_bytes();
    let alice_login: LoginSessionResponse =
        serde_json::from_slice(&body_bytes).expect("parse login");

    // 4. BOLA: Alice tries to create a private repository for Team 2 -> 403 FORBIDDEN
    let bad_repo_body = serde_json::to_vec(&CreateTeamRepoRequest {
        name: "team2-secret".to_owned(),
        owner_team_id: team_2,
        is_private: true,
    })
    .expect("serialize");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/repos")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", alice_login.raw_session_token),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bad_repo_body))
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 5. BFLA: Developer Alice tries to trigger outbound GitHub sync without Maintainer/Admin role -> 403 FORBIDDEN
    let unapproved_sync = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/sync/github")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", alice_login.raw_session_token),
                )
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(unapproved_sync.status(), StatusCode::FORBIDDEN);

    // 5b. Fail-closed BOLA: Alice tries to spawn a worktree on an arbitrary unregistered RepoId -> 403 FORBIDDEN
    let bad_wt_body = serde_json::to_vec(&SpawnWorktreeRequest {
        repo_id: Some(RepoId::new()),
        agent: Some(AgentKind::ClaudeCode),
        branch: "agent/claude/arbitrary-repo".to_owned(),
        base_branch: None,
        prompt_summary: "Attempt unregistered repo_id".to_owned(),
    })
    .expect("serialize");
    let bad_wt_res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/swarm/worktrees")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", alice_login.raw_session_token),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bad_wt_body))
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(bad_wt_res.status(), StatusCode::FORBIDDEN);

    // 6. Admin deactivates Alice -> Alice's session token immediately fails with 403 FORBIDDEN (AccountDeactivated)
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/admin/users/{}/deactivate",
                    alice_profile.id.0
                ))
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/me")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", alice_login.raw_session_token),
                )
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("res");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn approve_worktree_conflicts_while_exec_lock_held_and_preserves_terminal_status() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);

    let auth_store = AuthStore::new_in_memory();
    auth_store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "siteadmin@company.internal",
            "AdminPassword!2026",
        )
        .expect("bootstrap initial admin");
    let login = auth_store
        .login("siteadmin", "AdminPassword!2026")
        .expect("login initial admin");
    let token = login.raw_session_token;

    let state = AppState::with_auth_store(
        repo,
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        auth_store,
    )
    .with_trusted_sandbox_exec(true);
    let app = build_router(state.clone());

    let (s, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({"branch": "agent/claude/lock-race", "prompt_summary": "race"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let wt: AgentWorktree = serde_json::from_str(&body).expect("parse wt");

    // Mark Verified first via a quick sandbox run.
    let (s, _) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
        &token,
        Some(serde_json::json!({"command": "true"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let runtime = state.get_worktree_runtime(wt.id).expect("worktree runtime");

    // Hold exec_lock to simulate an in-flight sandbox execution.
    let held_guard = runtime
        .exec_lock
        .clone()
        .try_lock_owned()
        .expect("acquire exec_lock");

    let (conflict_status, _) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(conflict_status, StatusCode::CONFLICT);

    drop(held_guard);

    // Approving a branch with zero commits ahead of `base_branch` must also fail (409).
    let (no_commit_status, _) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(no_commit_status, StatusCode::CONFLICT);

    std::fs::write(runtime.worktree_dir.join("README.md"), "race-merged\n").expect("write");
    let commit_out = std::process::Command::new("git")
        .arg("-C")
        .arg(&runtime.worktree_dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Agent",
            "-c",
            "user.email=agent@company.internal",
            "commit",
            "-q",
            "-am",
            "race commit",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git commit");
    assert!(commit_out.status.success());

    let (ok_status, _) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(ok_status, StatusCode::OK);

    // Once Merged, set_worktree_status must never overwrite the terminal status.
    set_worktree_status(&state, wt.id, |w| w.status = WorktreeStatus::Failed);
    let current_status = state
        .worktrees
        .read()
        .iter()
        .find(|w| w.id == wt.id)
        .expect("wt exists")
        .status;
    assert_eq!(current_status, WorktreeStatus::Merged);

    // Dropping an uncommitted SpawnReservation that reached Materialized must roll back the
    // on-disk worktree and release the quota slot.
    let admin_id = state
        .auth_store
        .authenticate_session(&token)
        .expect("admin")
        .user_id;
    let resv = SpawnReservation::reserve(&state, admin_id).expect("reserve");
    let m = state
        .git_engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            state.repo_id,
            AgentKind::ClaudeCode,
            "agent/claude/cancel-rollback",
            "main",
            "cancel".to_owned(),
        )
        .expect("materialize");
    let dir_to_rollback = m.worktree_dir.clone();
    *resv.lifecycle.lock() = SpawnLifecycle::Materialized {
        engine: state.git_engine.clone(),
        worktree_dir: m.worktree_dir,
        branch: m.worktree.branch,
        base_commit: m.worktree.base_commit,
    };
    drop(resv);
    for _ in 0..50 {
        if !dir_to_rollback.exists() && state.spawn_quota.lock().in_flight_total == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !dir_to_rollback.exists(),
        "cancelled spawn must remove worktree dir"
    );
    assert_eq!(state.spawn_quota.lock().in_flight_total, 0);
}

#[tokio::test]
async fn test_single_binary_webui_and_end_to_end_dogfood_onboarding_lifecycle() {
    let (app, admin_token) = test_app();

    // 1. Embedded Web UI & SPA routes serve index.html + assets with strict security headers
    for route in [
        "/",
        "/auth/accept-invite?token=gitust_inv_123",
        "/auth/reset-password?token=gitust_rst_123",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            resp.headers()
                .get(header::X_FRAME_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("DENY")
        );
        let csp = resp
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert!(csp.contains("default-src 'self'") && !csp.contains("googleapis.com"));
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("/assets/index.js") && !html.contains("fonts.googleapis.com"));
    }

    for (asset, expected_ct) in [
        ("/assets/index.js", "application/javascript; charset=utf-8"),
        ("/assets/index.css", "text/css; charset=utf-8"),
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(asset).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some(expected_ct)
        );
    }

    // 2. SiteAdmin issues a single-use invite link; Developer redeems it and signs in
    let (inv_status, inv_body) = call(
        &app,
        "POST",
        "/api/v1/admin/invites".to_owned(),
        &admin_token,
        Some(serde_json::json!({"target_email": "dogfood@company.internal", "assigned_role": "Developer", "team_ids": []})),
    ).await;
    assert_eq!(inv_status, StatusCode::CREATED);
    let issued: IssuedInviteLink = serde_json::from_str(&inv_body).unwrap();

    let (red_status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/invites/redeem".to_owned(),
        "",
        Some(serde_json::json!({"raw_token": issued.raw_token, "username": "dogfood_dev", "password": "DogfoodPassword!2026"})),
    ).await;
    assert_eq!(red_status, StatusCode::CREATED);

    let (login_status, login_body) = call(
        &app,
        "POST",
        "/api/v1/auth/login".to_owned(),
        "",
        Some(serde_json::json!({"identifier": "dogfood_dev", "password": "DogfoodPassword!2026"})),
    )
    .await;
    assert_eq!(login_status, StatusCode::OK);
    let dev_login: LoginSessionResponse = serde_json::from_str(&login_body).unwrap();
    let dev_ses = dev_login.raw_session_token;

    // 3. Developer creates a PAT and SSH key and lists them (isolated from SiteAdmin)
    let (pat_status, pat_body) = call(
        &app,
        "POST",
        "/api/v1/auth/pats".to_owned(),
        &dev_ses,
        Some(serde_json::json!({"name": "dogfood-laptop", "ttl_days": 30})),
    )
    .await;
    assert_eq!(pat_status, StatusCode::CREATED);
    let issued_pat: IssuedPersonalAccessToken = serde_json::from_str(&pat_body).unwrap();

    let (_, list_pat_body) =
        call(&app, "GET", "/api/v1/auth/pats".to_owned(), &dev_ses, None).await;
    let dev_pats: Vec<PersonalAccessTokenView> = serde_json::from_str(&list_pat_body).unwrap();
    assert_eq!(dev_pats.len(), 1);
    assert_eq!(dev_pats[0].name, "dogfood-laptop");

    let (_, admin_pats_body) = call(
        &app,
        "GET",
        "/api/v1/auth/pats".to_owned(),
        &admin_token,
        None,
    )
    .await;
    let admin_pats: Vec<PersonalAccessTokenView> = serde_json::from_str(&admin_pats_body).unwrap();
    assert!(admin_pats.is_empty());

    let (ssh_status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/ssh-keys".to_owned(),
        &dev_ses,
        Some(serde_json::json!({
            "title": "mac-studio-ed25519",
            "public_key_openssh": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl dogfood@company.internal"
        })),
    ).await;
    assert_eq!(ssh_status, StatusCode::CREATED);

    let (_, list_ssh_body) = call(
        &app,
        "GET",
        "/api/v1/auth/ssh-keys".to_owned(),
        &dev_ses,
        None,
    )
    .await;
    let dev_keys: Vec<SshPublicKeyView> = serde_json::from_str(&list_ssh_body).unwrap();
    assert_eq!(dev_keys.len(), 1);
    assert_eq!(dev_keys[0].title, "mac-studio-ed25519");

    // 4. Logout rejects PAT bearer tokens and immediately revokes the active session token
    let (pat_logout_status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/logout".to_owned(),
        &issued_pat.raw_token,
        None,
    )
    .await;
    assert_eq!(pat_logout_status, StatusCode::BAD_REQUEST);

    let (ses_logout_status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/logout".to_owned(),
        &dev_ses,
        None,
    )
    .await;
    assert_eq!(ses_logout_status, StatusCode::NO_CONTENT);

    let (after_logout_status, _) =
        call(&app, "GET", "/api/v1/auth/me".to_owned(), &dev_ses, None).await;
    assert_eq!(after_logout_status, StatusCode::UNAUTHORIZED);
}
