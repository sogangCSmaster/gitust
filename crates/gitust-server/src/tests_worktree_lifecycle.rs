use super::*;

#[tokio::test]
pub(super) async fn http_gate_worktree_ownership_quota_and_work_preservation() {
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
        Some(serde_json::json!({"verify":true,"command": "true"})),
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
        fixture_git()
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

#[tokio::test]
pub(super) async fn http_worktree_lifecycle_spawn_exec_approve_discard() {
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
        Some(serde_json::json!({"verify":true,"command": "true"})),
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
    let (s, _) = call(
        &app,
        "POST",
        approve.clone(),
        &token,
        Some(serde_json::json!({"expected_commit": wt.base_commit.0})),
    )
    .await;
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
    let commit_out = fixture_git()
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
            "command": "grep -q merged-by-agent README.md", "verify": true
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let res: SandboxExecutionResult = serde_json::from_str(&body).expect("parse");
    assert_eq!(res.exit_code, 0, "{}", res.stderr);
    let verification: serde_json::Value = serde_json::from_str(&body).expect("verification");
    let checked = verification["checked_commit"]
        .as_str()
        .expect("checked SHA");

    let (s, body) = call(
        &app,
        "POST",
        approve,
        &token,
        Some(serde_json::json!({"expected_commit": checked})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        std::fs::read_to_string(repo.join("README.md"))
            .expect("read main README")
            .contains("merged-by-agent"),
        "approving worktree must merge agent commit into main"
    );

    let (sync_status, _) = call(
        &app,
        "POST",
        "/api/v1/sync/github".into(),
        &token,
        Some(serde_json::json!({
            "github_remote_url": "https://evil.example.com/ssrf.git"
        })),
    )
    .await;
    assert_eq!(sync_status, StatusCode::BAD_REQUEST);

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
