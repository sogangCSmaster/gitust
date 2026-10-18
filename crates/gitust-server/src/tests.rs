use super::*;
use crate::state::{set_worktree_status, SpawnLifecycle, SpawnReservation};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[path = "tests_invite_credentials.rs"]
mod tests_invite_credentials;
#[path = "tests_repository_group_invite_auth.rs"]
mod tests_repository_group_invite_auth;
#[path = "tests_repository_group_invites.rs"]
mod tests_repository_group_invites;

#[path = "tests_approval_target.rs"]
mod tests_approval_target;
#[path = "tests_auth.rs"]
mod tests_auth;
#[path = "tests_automatic_http_revocation.rs"]
mod tests_automatic_http_revocation;
#[path = "tests_automatic_store.rs"]
mod tests_automatic_store;
#[path = "tests_check_steps.rs"]
mod tests_check_steps;
#[path = "tests_external_review.rs"]
mod tests_external_review;
#[path = "tests_git_http.rs"]
mod tests_git_http;
#[path = "tests_mcp.rs"]
mod tests_mcp;
#[path = "tests_mcp_commit.rs"]
mod tests_mcp_commit;
#[path = "tests_mcp_commit_protected.rs"]
mod tests_mcp_commit_protected;
#[path = "tests_mcp_commit_stdio.rs"]
mod tests_mcp_commit_stdio;
#[path = "tests_mcp_discard.rs"]
mod tests_mcp_discard;
#[path = "tests_oauth.rs"]
mod tests_oauth;
#[path = "tests_outbound.rs"]
mod tests_outbound;
#[path = "tests_repository_default.rs"]
mod tests_repository_default;
#[path = "tests_repository_summary.rs"]
mod tests_repository_summary;
#[path = "tests_repository_unborn.rs"]
mod tests_repository_unborn;
#[path = "tests_required_checks.rs"]
mod tests_required_checks;
#[path = "tests_run_history.rs"]
mod tests_run_history;
#[path = "tests_verification.rs"]
mod tests_verification;
#[path = "tests_worktree_labels.rs"]
mod tests_worktree_labels;

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
    .expect("recover server state")
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
    )
    .expect("recover server state");
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

fn fixture_git() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

fn init_temp_repo(dir: &std::path::Path) {
    let git = |args: &[&str]| {
        let out = fixture_git()
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
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
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
#[path = "tests_automatic_checks.rs"]
mod tests_automatic_checks;
#[path = "tests_automatic_git_http.rs"]
mod tests_automatic_git_http;
#[path = "tests_webui_assets.rs"]
mod webui_assets;

#[path = "tests_automatic_intents.rs"]
mod tests_automatic_intents;
#[path = "tests_automatic_restart.rs"]
mod tests_automatic_restart;

#[path = "tests_repository_automatic_runs.rs"]
mod tests_repository_automatic_runs;
#[path = "tests_repository_registration.rs"]
mod tests_repository_registration;

#[path = "tests_repository_catalog.rs"]
mod tests_repository_catalog;

#[path = "tests_repository_local_checks.rs"]
mod tests_repository_local_checks;

#[path = "tests_repository_rename.rs"]
mod tests_repository_rename;

#[path = "tests_repository_read_credentials.rs"]
mod tests_repository_read_credentials;

#[path = "tests_sync_credentials.rs"]
mod tests_sync_credentials;

#[path = "tests_repository_branches.rs"]
mod tests_repository_branches;

#[path = "tests_repository_visibility.rs"]
mod tests_repository_visibility;
#[path = "tests_repository_visibility_auth.rs"]
mod tests_repository_visibility_auth;

#[path = "tests_git_http_secondary.rs"]
mod tests_git_http_secondary;
#[path = "tests_git_http_secondary_auth.rs"]
mod tests_git_http_secondary_auth;

#[path = "tests_credential_mutations.rs"]
mod tests_credential_mutations;

#[path = "tests_repository_group_members.rs"]
mod tests_repository_group_members;

#[path = "tests_repository_group_members_auth.rs"]
mod tests_repository_group_members_auth;

#[path = "tests_git_http_write.rs"]
mod tests_git_http_write;
#[path = "tests_git_http_write_auth.rs"]
mod tests_git_http_write_auth;

#[path = "tests_repository_local_checks_exec.rs"]
mod tests_repository_local_checks_exec;
