use super::*;
use crate::state::{set_worktree_status, SpawnLifecycle, SpawnReservation};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn test_app_at(repo: PathBuf, trusted_sandbox_exec: bool) -> (Router, String) {
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

    let state = AppState::with_auth_store(
        repo,
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        auth_store,
    )
    .with_trusted_sandbox_exec(trusted_sandbox_exec);
    (build_router(state), login.raw_session_token)
}

fn test_app() -> (Router, String) {
    // Deliberately not a repository: these tests must never touch the developer's checkout.
    test_app_at(PathBuf::from("/nonexistent/gitust-test-repo"), false)
}

/// App with one SiteAdmin and two Developers; returns (router, admin, dev_a, dev_b) tokens.
fn test_app_with_devs(repo: PathBuf) -> (Router, String, String, String) {
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "siteadmin@company.internal",
            "AdminPassword!2026",
        )
        .expect("bootstrap");
    let admin = store
        .login("siteadmin", "AdminPassword!2026")
        .expect("login");
    let admin_principal = store
        .authenticate_session(&admin.raw_session_token)
        .expect("principal");
    let mut dev_tokens = Vec::new();
    for name in ["deva", "devb"] {
        let invite = store
            .create_invite_link(
                &admin_principal,
                &format!("{name}@company.internal"),
                EnterpriseRole::Developer,
                vec![],
                None,
            )
            .expect("invite");
        store
            .redeem_invite_link(&invite.raw_token, name, "DevPassword!2026")
            .expect("redeem");
        dev_tokens.push(
            store
                .login(name, "DevPassword!2026")
                .expect("login")
                .raw_session_token,
        );
    }
    let state = AppState::with_auth_store(
        repo,
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        store,
    );
    let dev_b = dev_tokens.pop().expect("dev b");
    let dev_a = dev_tokens.pop().expect("dev a");
    (build_router(state), admin.raw_session_token, dev_a, dev_b)
}

#[tokio::test]
async fn http_gate_worktree_ownership_quota_and_work_preservation() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);
    let (app, admin, dev_a, dev_b) = test_app_with_devs(repo.clone());
    let spawn =
        |branch: String| serde_json::json!({"branch": branch, "prompt_summary": "ownership"});

    let (s, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &dev_a,
        Some(spawn("agent/a/one".into())),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let wt_a: AgentWorktree = serde_json::from_str(&body).expect("parse");
    let wt_a_uri = format!("/api/v1/swarm/worktrees/{}", wt_a.id.0);

    // Developer B cannot discard Developer A's worktree.
    let (s, _) = call(&app, "DELETE", wt_a_uri.clone(), &dev_b, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Developers cannot run sandbox commands over HTTP (SiteAdmin + trusted mode only).
    let (s, _) = call(
        &app,
        "POST",
        format!("{wt_a_uri}/sandbox"),
        &dev_a,
        Some(serde_json::json!({"command": "true"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Simulate an agent commit on A's branch: discard must keep the branch (the work).
    let worktrees = GitEngine::new(&repo).list_agent_worktrees().expect("list");
    let a_dir = &worktrees
        .iter()
        .find(|e| e.branch == "agent/a/one")
        .expect("a worktree")
        .worktree_dir;
    std::fs::write(a_dir.join("work.txt"), "agent work\n").expect("write");
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=a",
                "-c",
                "user.email=a@a",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git")
    };
    assert!(git(a_dir, &["add", "."]).status.success());
    assert!(git(a_dir, &["commit", "-qm", "work"]).status.success());

    let (s, _) = call(&app, "DELETE", wt_a_uri, &dev_a, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let kept = git(&repo, &["rev-parse", "--verify", "agent/a/one"]);
    assert!(
        kept.status.success(),
        "branch with agent commits must be kept"
    );

    // Per-user quota.
    for i in 0..MAX_WORKTREES_PER_USER {
        let (s, body) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &dev_b,
            Some(spawn(format!("agent/b/{i}"))),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
    }
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &dev_b,
        Some(spawn("agent/b/over".into())),
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    // Other users are unaffected by B's quota; admins may discard any worktree.
    let (s, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &dev_a,
        Some(spawn("agent/a/two".into())),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (_, body) = call(&app, "GET", "/api/v1/swarm/worktrees".into(), &admin, None).await;
    let list: Vec<AgentWorktree> = serde_json::from_str(&body).expect("parse");
    let b0 = list.iter().find(|w| w.branch == "agent/b/0").expect("b0");
    let (s, _) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", b0.id.0),
        &admin,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

fn init_temp_repo(dir: &std::path::Path) {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@company.internal")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@company.internal")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    std::fs::create_dir_all(dir).expect("mkdir");
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").expect("write");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
}

async fn call(
    app: &Router,
    method: &str,
    uri: String,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app
        .clone()
        .oneshot(req.body(body).expect("req"))
        .await
        .expect("res");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn http_worktree_lifecycle_spawn_exec_approve_discard() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);

    // Exec is disabled unless explicitly trusted.
    let (untrusted, token) = test_app_at(repo.clone(), false);
    let (s, body) = call(
        &untrusted,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({"branch": "agent/claude/untrusted", "prompt_summary": "t"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let wt: AgentWorktree = serde_json::from_str(&body).expect("parse");
    let (s, _) = call(
        &untrusted,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
        &token,
        Some(serde_json::json!({"command": "true"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (app, token) = test_app_at(repo.clone(), true);
    let spawn = |branch: &str| serde_json::json!({"branch": branch, "prompt_summary": "lifecycle"});

    // Startup reconciliation: the new AppState instance recovered `wt` from disk.
    let (_, recovered_body) =
        call(&app, "GET", "/api/v1/swarm/worktrees".into(), &token, None).await;
    let recovered_list: Vec<AgentWorktree> =
        serde_json::from_str(&recovered_body).expect("parse recovered");
    assert!(
        recovered_list
            .iter()
            .any(|w| w.id == wt.id && w.branch == "agent/claude/untrusted"),
        "startup must recover existing on-disk worktrees: {recovered_body}"
    );

    // Namespace, duplicate and base-branch errors (failed spawns must release quota slots).
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(spawn("main")),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(spawn("agent/claude/untrusted")),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "existing branch must not be reused"
    );
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(
            serde_json::json!({"branch": "agent/x/y", "base_branch": "nope", "prompt_summary": ""}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Spawn: response carries no server filesystem paths.
    let (s, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(spawn("agent/claude/lifecycle")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        !body.contains(&*tmp.path().to_string_lossy()),
        "path leaked: {body}"
    );
    let wt: AgentWorktree = serde_json::from_str(&body).expect("parse");
    assert_eq!(wt.status, WorktreeStatus::Pending);

    // Pending worktrees cannot be approved.
    let approve = format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0);
    let (s, _) = call(&app, "POST", approve.clone(), &token, None).await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Simulate an agent commit in the worktree checkout, then verify inside the sandbox.
    let lc_dir = GitEngine::new(&repo)
        .list_agent_worktrees()
        .expect("list")
        .into_iter()
        .find(|e| e.branch == "agent/claude/lifecycle")
        .expect("lifecycle worktree")
        .worktree_dir;
    std::fs::write(lc_dir.join("README.md"), "hello\nmerged-by-agent\n").expect("write");
    let commit_out = std::process::Command::new("git")
        .arg("-C")
        .arg(&lc_dir)
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
            "agent commit",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git commit");
    assert!(commit_out.status.success());

    // Exec runs inside the worktree and verifies the agent commit.
    let (s, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
        &token,
        Some(serde_json::json!({
            "command": "grep -q merged-by-agent README.md"
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let res: SandboxExecutionResult = serde_json::from_str(&body).expect("parse");
    assert_eq!(res.exit_code, 0, "{}", res.stderr);

    let (s, body) = call(&app, "POST", approve, &token, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        std::fs::read_to_string(repo.join("README.md"))
            .expect("read main README")
            .contains("merged-by-agent"),
        "approving worktree must merge agent commit into main"
    );

    let (sync_status, _) = call(&app, "POST", "/api/v1/sync/github".into(), &token, None).await;
    assert_eq!(sync_status, StatusCode::NOT_IMPLEMENTED);

    // Discard removes the checkout and the branch.
    let (s, _) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, body) = call(&app, "GET", "/api/v1/swarm/worktrees".into(), &token, None).await;
    assert!(!body.contains(&wt.id.0.to_string()));
    assert!(GitEngine::new(&repo)
        .list_agent_worktrees()
        .expect("list")
        .iter()
        .all(|e| e.branch != "agent/claude/lifecycle"));
}

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

    let runtime = state
        .worktree_runtime
        .read()
        .get(&wt.id)
        .expect("worktree runtime")
        .clone();

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
async fn mcp_jsonrpc_stdio_lifecycle_and_adversarial_bounds() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);

    let state = AppState::with_auth_store(
        repo.clone(),
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        AuthStore::new_in_memory(),
    )
    .with_trusted_sandbox_exec(true);
    let caller = AuthenticatedPrincipal {
        user_id: UserId::new(),
        username: "mcp-test".to_owned(),
        role: EnterpriseRole::SiteAdmin,
        team_ids: Vec::new(),
    };

    // 1. `initialize` and `notifications/initialized` (notification produces no response)
    let init_resp = handle_mcp_json_line(
            &state,
            &caller,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        )
        .await
        .expect("init response");
    let init_json: serde_json::Value = serde_json::from_str(&init_resp).expect("parse init");
    assert_eq!(init_json["id"], 1);
    assert_eq!(init_json["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init_json["result"]["serverInfo"]["name"], "gitust-mcp");

    let notif_resp = handle_mcp_json_line(
        &state,
        &caller,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    assert!(
        notif_resp.is_none(),
        "notifications must not emit a response"
    );

    // 2. `tools/list` returns the 4 tools with JSON Schemas
    let list_resp = handle_mcp_json_line(
        &state,
        &caller,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    )
    .await
    .expect("tools/list");
    let list_json: serde_json::Value = serde_json::from_str(&list_resp).expect("parse tools");
    let tools = list_json["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert_eq!(tools.len(), 4);
    assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));

    // 3. `gitust_worktree_spawn` rejects non-agent branch (`isError: true`)
    let bad_spawn = handle_mcp_json_line(
            &state,
            &caller,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"gitust_worktree_spawn","arguments":{"branch":"main"}}}"#,
        )
        .await
        .expect("bad spawn");
    let bad_spawn_json: serde_json::Value =
        serde_json::from_str(&bad_spawn).expect("parse bad spawn");
    assert_eq!(bad_spawn_json["result"]["isError"], true);

    // 4. `gitust_worktree_spawn` with auto-generated branch for Cursor agent
    let spawn_resp = handle_mcp_json_line(
            &state,
            &caller,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"gitust_worktree_spawn","arguments":{"agent":"cursor","prompt_summary":"Implement MCP"}}}"#,
        )
        .await
        .expect("spawn");
    let spawn_json: serde_json::Value = serde_json::from_str(&spawn_resp).expect("parse spawn");
    assert_eq!(spawn_json["result"]["isError"], false);
    let spawn_payload: serde_json::Value = serde_json::from_str(
        spawn_json["result"]["content"][0]["text"]
            .as_str()
            .expect("text"),
    )
    .expect("payload");
    let wt_id = spawn_payload["worktree_id"].as_str().expect("worktree_id");
    let wt_branch = spawn_payload["branch"].as_str().expect("branch");
    let wt_dir = PathBuf::from(
        spawn_payload["worktree_dir"]
            .as_str()
            .expect("worktree_dir"),
    );
    assert!(wt_branch.starts_with("agent/cursor/"));
    assert!(wt_dir.join("README.md").is_file());

    // 5. `gitust_sandbox_exec` runs inside the worktree under OS sandbox
    let exec_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "gitust_sandbox_exec",
            "arguments": {
                "worktree_id": wt_id,
                "command": "echo 'mcp-sandbox-ok' > mcp-artifact.txt && cat mcp-artifact.txt",
                "timeout_secs": 30
            }
        }
    })
    .to_string();
    let exec_resp = handle_mcp_json_line(&state, &caller, &exec_req)
        .await
        .expect("exec");
    let exec_json: serde_json::Value = serde_json::from_str(&exec_resp).expect("parse exec");
    assert_eq!(exec_json["result"]["isError"], false);
    let exec_payload: serde_json::Value = serde_json::from_str(
        exec_json["result"]["content"][0]["text"]
            .as_str()
            .expect("text"),
    )
    .expect("exec payload");
    assert_eq!(exec_payload["exit_code"], 0);
    assert_eq!(exec_payload["status"], "verified");
    assert!(exec_payload["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("mcp-sandbox-ok"));
    assert!(
        wt_dir.join("mcp-artifact.txt").is_file(),
        "sandbox write must land in worktree"
    );
    assert!(
        !repo.join("mcp-artifact.txt").exists(),
        "main working tree must remain untouched"
    );

    // 6. `gitust_worktree_list` reflects the verified worktree
    let wt_list_resp = handle_mcp_json_line(
            &state,
            &caller,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"gitust_worktree_list","arguments":{}}}"#,
        )
        .await
        .expect("wt list");
    let wt_list_json: serde_json::Value =
        serde_json::from_str(&wt_list_resp).expect("parse wt list");
    let wt_list_payload: serde_json::Value = serde_json::from_str(
        wt_list_json["result"]["content"][0]["text"]
            .as_str()
            .expect("text"),
    )
    .expect("wt list payload");
    assert_eq!(
        wt_list_payload["worktrees"][0]["worktree_id"].as_str(),
        Some(wt_id)
    );
    assert_eq!(wt_list_payload["worktrees"][0]["status"], "verified");

    // 7. `gitust_worktree_discard` removes the checkout and branch
    let discard_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "gitust_worktree_discard",
            "arguments": {
                "worktree_id": wt_id
            }
        }
    })
    .to_string();
    let discard_resp = handle_mcp_json_line(&state, &caller, &discard_req)
        .await
        .expect("discard");
    let discard_json: serde_json::Value =
        serde_json::from_str(&discard_resp).expect("parse discard");
    assert_eq!(discard_json["result"]["isError"], false);
    assert!(!wt_dir.exists(), "discarded worktree dir must be removed");

    // 8. Stream test: oversized >1 MiB line returns -32600 without crashing, followed by ping
    let oversized = format!("{}\n", "x".repeat(MAX_MCP_LINE_BYTES + 1024));
    let ping_line = "{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"ping\"}\n";
    let input_bytes = [oversized.as_bytes(), ping_line.as_bytes()].concat();
    let reader = tokio::io::BufReader::new(std::io::Cursor::new(input_bytes));
    let mut output_bytes = Vec::new();
    serve_mcp_stream(state, reader, &mut output_bytes)
        .await
        .expect("serve_mcp_stream");
    let output_str = String::from_utf8(output_bytes).expect("utf8 output");
    let lines: Vec<&str> = output_str.lines().collect();
    assert_eq!(lines.len(), 2);
    let err_line: serde_json::Value = serde_json::from_str(lines[0]).expect("err json");
    assert_eq!(err_line["error"]["code"], -32600);
    let pong_line: serde_json::Value = serde_json::from_str(lines[1]).expect("pong json");
    assert_eq!(pong_line["id"], 99);
    assert_eq!(pong_line["result"], serde_json::json!({}));
}

#[tokio::test]
async fn oauth_pkce_s256_mcp_http_lifecycle_and_deactivation_gate() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);
    let (app, admin_token) = test_app_at(repo.clone(), true);

    // 1. Unauthenticated POST /mcp returns 401 with RFC 9728 WWW-Authenticate header
    let unauth_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauth_resp.status(), StatusCode::UNAUTHORIZED);
    let www_auth = unauth_resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        www_auth.contains("resource_metadata=\"/.well-known/oauth-protected-resource\""),
        "unexpected WWW-Authenticate: {www_auth}"
    );

    // 2. OAuth Protected Resource & Authorization Server Metadata Discovery
    let pr_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-protected-resource")
                .header("host", "127.0.0.1:3000")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(pr_resp.status(), StatusCode::OK);
    let pr_meta: OAuthProtectedResourceMetadata = serde_json::from_slice(
        &pr_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse pr_meta");
    assert_eq!(pr_meta.resource, "http://127.0.0.1:3000/mcp");

    let as_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-authorization-server")
                .header("host", "127.0.0.1:3000")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(as_resp.status(), StatusCode::OK);
    let as_meta: OAuthAuthorizationServerMetadata = serde_json::from_slice(
        &as_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse as_meta");
    assert_eq!(as_meta.code_challenge_methods_supported, vec!["S256"]);

    // 3. Dynamic Client Registration (rejects unsafe redirect_uri, authority prefix confusion, and fragments)
    for bad_uri in [
        "javascript:alert(1)",
        "http://localhost.attacker.com/cb",
        "http://127.0.0.1@attacker.com/cb",
        "http://127.0.0.1:8080/cb#frag",
        "https://",
    ] {
        let bad_reg_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "client_name": "Evil Client",
                            "redirect_uris": [bad_uri]
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            bad_reg_resp.status(),
            StatusCode::BAD_REQUEST,
            "bad redirect_uri must be rejected: {bad_uri}"
        );
    }

    // Flood dynamic client registrations beyond MAX_OAUTH_CLIENTS -> built-in gitust-mcp-cli must not be evicted
    for i in 0..(MAX_OAUTH_CLIENTS + 4) {
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "client_name": format!("Temp Client {i}"),
                            "redirect_uris": ["http://127.0.0.1:54321/oauth/callback"]
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
    }

    let reg_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_name": "Claude Code MCP Client",
                        "redirect_uris": ["http://127.0.0.1:54321/oauth/callback"]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(reg_resp.status(), StatusCode::CREATED);
    let reg_client: OAuthClientRegistrationResponse = serde_json::from_slice(
        &reg_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse reg_client");

    // 4. Create a second developer user via invite so we can also test instant deactivation
    let invite_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/invites")
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateInviteRequest {
                        target_email: "mcpdev@company.internal".to_owned(),
                        assigned_role: EnterpriseRole::Developer,
                        team_ids: vec![],
                        ttl_hours: Some(24),
                    })
                    .expect("serialize invite"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(invite_resp.status(), StatusCode::CREATED);
    let issued_invite: IssuedInviteLink = serde_json::from_slice(
        &invite_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse invite");

    let redeem_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/invites/redeem")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "raw_token": issued_invite.raw_token,
                        "username": "mcpdev",
                        "password": "McpDevPassword!2026"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let dev_profile: UserPublicProfile = serde_json::from_slice(
        &redeem_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse dev_profile");

    let dev_login_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "identifier": "mcpdev",
                        "password": "McpDevPassword!2026"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let dev_login: LoginSessionResponse = serde_json::from_slice(
        &dev_login_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse dev_login");

    // 5. OAuth Authorize with PKCE S256 (first test wrong verifier rejection, then valid exchange)
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk-gitust-mcp-2026";
    let challenge = pkce_s256_challenge(verifier);

    // Built-in client_id = "gitust-mcp-cli" is still pinned and usable after registration flood
    let builtin_authz_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", dev_login.raw_session_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_id": "gitust-mcp-cli",
                        "redirect_uri": "http://127.0.0.1/callback",
                        "response_type": "code",
                        "code_challenge": challenge,
                        "code_challenge_method": "S256"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(builtin_authz_resp.status(), StatusCode::OK);

    let authz_resp_1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", dev_login.raw_session_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_id": reg_client.client_id,
                        "redirect_uri": "http://127.0.0.1:54321/oauth/callback",
                        "response_type": "code",
                        "code_challenge": challenge,
                        "code_challenge_method": "S256",
                        "state": "xyz-state-1"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(authz_resp_1.status(), StatusCode::OK);
    let authz_1: OAuthAuthorizeResponse = serde_json::from_slice(
        &authz_resp_1
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse authz_1");

    // Wrong PKCE code_verifier is rejected and consumes the single-use code
    let bad_tok_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "grant_type": "authorization_code",
                        "code": authz_1.code,
                        "client_id": reg_client.client_id,
                        "redirect_uri": "http://127.0.0.1:54321/oauth/callback",
                        "code_verifier": "wrong_verifier_that_is_at_least_43_chars_long_1234567890"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(bad_tok_resp.status(), StatusCode::BAD_REQUEST);

    // Issue a fresh authorization code and exchange via application/x-www-form-urlencoded
    let authz_resp_2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", dev_login.raw_session_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_id": reg_client.client_id,
                        "redirect_uri": "http://127.0.0.1:54321/oauth/callback",
                        "response_type": "code",
                        "code_challenge": challenge,
                        "code_challenge_method": "S256"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(authz_resp_2.status(), StatusCode::OK);
    let authz_2: OAuthAuthorizeResponse = serde_json::from_slice(
        &authz_resp_2
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse authz_2");

    let form_body = format!(
        "grant_type=authorization_code&code={}&client_id={}&redirect_uri={}&code_verifier={}",
        authz_2.code,
        reg_client.client_id,
        "http%3A%2F%2F127.0.0.1%3A54321%2Foauth%2Fcallback",
        verifier
    );
    let tok_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form_body.clone()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(tok_resp.status(), StatusCode::OK);
    let oauth_token: OAuthTokenResponse = serde_json::from_slice(
        &tok_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse oauth_token");
    assert_eq!(oauth_token.token_type, "Bearer");
    assert!(oauth_token.access_token.starts_with("gitust_ses_"));

    // Replaying the same authorization code must fail (single-use)
    let replay_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form_body))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(replay_resp.status(), StatusCode::BAD_REQUEST);

    // 6. Use the OAuth 2.0 Bearer token against Streamable HTTP `POST /mcp`
    let mcp_init_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", oauth_token.access_token),
                    )
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
    assert_eq!(mcp_init_resp.status(), StatusCode::OK);

    // Notification returns 202 Accepted
    let mcp_notif_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", oauth_token.access_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(mcp_notif_resp.status(), StatusCode::ACCEPTED);

    // Spawn worktree over HTTP MCP -> must NOT leak host `worktree_dir`
    let mcp_spawn_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", oauth_token.access_token),
                    )
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gitust_worktree_spawn","arguments":{"agent":"claude_code","prompt_summary":"HTTP OAuth MCP test"}}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
    assert_eq!(mcp_spawn_resp.status(), StatusCode::OK);
    let spawn_rpc: serde_json::Value = serde_json::from_slice(
        &mcp_spawn_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse spawn_rpc");
    let spawn_payload: serde_json::Value = serde_json::from_str(
        spawn_rpc["result"]["content"][0]["text"]
            .as_str()
            .expect("text"),
    )
    .expect("parse spawn_payload");
    assert!(
        spawn_payload.get("worktree_dir").is_none(),
        "HTTP MCP response must not leak host filesystem worktree_dir"
    );
    let wt_id = spawn_payload["worktree_id"].as_str().expect("wt_id");

    // Discard worktree over HTTP MCP
    let mcp_discard_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", oauth_token.access_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "tools/call",
                        "params": {
                            "name": "gitust_worktree_discard",
                            "arguments": { "worktree_id": wt_id }
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(mcp_discard_resp.status(), StatusCode::OK);

    // 7. Deactivating the user immediately revokes the OAuth-issued MCP Bearer token (403 Forbidden)
    let deact_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/admin/users/{}/deactivate",
                    dev_profile.id.0
                ))
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(deact_resp.status(), StatusCode::OK);

    let revoked_mcp_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", oauth_token.access_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(revoked_mcp_resp.status(), StatusCode::FORBIDDEN);

    // 8. Slow-loris body TOCTOU regression test:
    // Create a second developer, start a POST /mcp request whose body stream pauses on first poll,
    // deactivate the developer mid-stream, then release the body bytes (`gitust_worktree_spawn`).
    // Post-body re-authentication must reject with 403 Forbidden and create zero worktrees.
    let invite2_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/invites")
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateInviteRequest {
                        target_email: "slowloris@company.internal".to_owned(),
                        assigned_role: EnterpriseRole::Developer,
                        team_ids: vec![],
                        ttl_hours: Some(24),
                    })
                    .expect("serialize invite2"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let issued_invite2: IssuedInviteLink = serde_json::from_slice(
        &invite2_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse invite2");

    let redeem2_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/invites/redeem")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "raw_token": issued_invite2.raw_token,
                        "username": "slowdev",
                        "password": "SlowDevPassword!2026"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let slow_profile: UserPublicProfile = serde_json::from_slice(
        &redeem2_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse slow_profile");

    let slow_login_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "identifier": "slowdev",
                        "password": "SlowDevPassword!2026"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let slow_login: LoginSessionResponse = serde_json::from_slice(
        &slow_login_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse slow_login");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("local_addr");
    let app_for_slow = app.clone();
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app_for_slow.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });

    let spawn_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "tools/call",
        "params": {
            "name": "gitust_worktree_spawn",
            "arguments": { "branch": "agent/claude/slow-loris-bypass" }
        }
    })
    .to_string();

    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect slow stream");
    let headers_part = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        slow_login.raw_session_token,
        spawn_body.len(),
        &spawn_body[..1]
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, headers_part.as_bytes())
        .await
        .expect("write headers + 1 byte");
    tokio::io::AsyncWriteExt::flush(&mut stream)
        .await
        .expect("flush headers");

    // Give the server handler time to pass initial header auth and suspend in to_bytes().await
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    // Admin deactivates slowdev while body read is suspended at .await
    let deact2_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/admin/users/{}/deactivate",
                    slow_profile.id.0
                ))
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(deact2_resp.status(), StatusCode::OK);

    // Send the rest of the mutation payload after deactivation
    tokio::io::AsyncWriteExt::write_all(&mut stream, &spawn_body.as_bytes()[1..])
        .await
        .expect("write remaining body");
    tokio::io::AsyncWriteExt::flush(&mut stream)
        .await
        .expect("flush remaining body");

    let mut raw_http_resp = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut raw_http_resp)
        .await
        .expect("read slow response");
    server_task.abort();
    assert!(
        raw_http_resp.starts_with("HTTP/1.1 403 Forbidden"),
        "mid-body deactivated account must be rejected with 403 Forbidden, got: {raw_http_resp}"
    );
}
