use super::*;
use serde_json::{json, Value};

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    remote: PathBuf,
    state: AppState,
    token: String,
    owner: AuthenticatedPrincipal,
    worktree: AgentWorktree,
    dir: PathBuf,
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    let out = cmd
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git oracle");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("git UTF-8")
        .trim()
        .into()
}

fn request(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":name,"arguments":arguments}})
}

async fn rpc(state: &AppState, caller: &AuthenticatedPrincipal, args: Value) -> Value {
    serde_json::from_str(
        &handle_mcp_json_line_with_options(
            state,
            caller,
            &request("gitust_worktree_commit", args).to_string(),
            false,
        )
        .await
        .expect("MCP response"),
    )
    .expect("RPC JSON")
}

fn payload(response: &Value) -> Value {
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool response text"),
    )
    .expect("tool payload")
}

fn args(f: &Fixture) -> Value {
    json!({"worktree_id": f.worktree.id.0, "expected_head":git(&f.dir, &["rev-parse","HEAD"]),
        "paths":["README.md"], "message":"Commit managed worktree changes"})
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let remote = temp.path().join("upstream.git");
    init_temp_repo(&repo);
    std::fs::write(repo.join("delete-me.txt"), "remove after task\n").expect("file");
    std::fs::write(
        repo.join(".gitust-checks.json"),
        json!({"version":1,"commands":["test -f README.md"]}).to_string(),
    )
    .expect("Checks policy");
    git(&repo, &["add", "delete-me.txt", ".gitust-checks.json"]);
    git(&repo, &["commit", "-qm", "Task fixture"]);
    std::fs::create_dir(&remote).expect("remote dir");
    git(&remote, &["init", "--bare", "-q", "-b", "main"]);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .expect("bootstrap");
    let token = store
        .login("admin", "Password!2026")
        .expect("login")
        .raw_session_token;
    let owner = store.authenticate_session(&token).expect("principal");
    let state =
        AppState::with_auth_store(repo.clone(), remote.to_str().expect("remote").into(), store)
            .expect("state")
            .with_trusted_sandbox_exec(true);
    let spawned = handle_mcp_json_line(
        &state,
        &owner,
        &request(
            "gitust_worktree_spawn",
            json!({"branch":"agent/test/mcp-commit", "base_branch":"main"}),
        )
        .to_string(),
    )
    .await
    .expect("spawn response");
    let spawned: Value = serde_json::from_str(&spawned).expect("spawn JSON");
    assert_eq!(spawned["result"]["isError"], false, "{spawned}");
    let worktree = state.worktrees.read().front().cloned().expect("worktree");
    let dir = state
        .get_worktree_runtime(worktree.id)
        .expect("runtime")
        .worktree_dir;
    Fixture {
        _temp: temp,
        repo,
        remote,
        state,
        token,
        owner,
        worktree,
        dir,
    }
}

async fn verify(f: &Fixture) {
    let app = build_router(f.state.clone());
    let (status, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/sandbox", f.worktree.id.0),
        &f.token,
        Some(json!({"verify":true,"command":"test -f README.md"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(f.state.worktrees.read()[0].status, WorktreeStatus::Verified);
    assert!(f.state.worktrees.read()[0].verified_checks_oid.is_some());
}

#[tokio::test]
async fn mcp_commit_http_oauth_edit_verify_approve_publish_exact_content() {
    let f = fixture().await;
    let oauth = f
        .state
        .auth_store
        .issue_oauth_session(f.owner.user_id)
        .expect("MCP OAuth");
    let app = build_router(f.state.clone());
    let base = git(&f.dir, &["rev-parse", "HEAD"]);
    let (status, text) = call(&app, "POST", "/mcp".into(), &oauth.raw_session_token,
        Some(request("gitust_sandbox_exec", json!({"worktree_id":f.worktree.id.0,
            "command":"printf 'new content\n' > 'file [x].txt'; printf '#!/bin/sh\nprintf executable\n' > check.sh; chmod +x check.sh; rm delete-me.txt"})))).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let executed: Value = serde_json::from_str(&text).expect("execute JSON");
    assert_eq!(executed["result"]["isError"], false, "{text}");
    assert_eq!(git(&f.dir, &["rev-parse", "HEAD"]), base);
    let (status, text) = call(
        &app,
        "POST",
        "/mcp".into(),
        &oauth.raw_session_token,
        Some(request(
            "gitust_worktree_commit",
            json!({"worktree_id":f.worktree.id.0,
            "expected_head":base,"paths":["file [x].txt","check.sh","delete-me.txt"],
            "message":"Add literal file and executable, remove obsolete input"}),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let response: Value = serde_json::from_str(&text).expect("commit JSON");
    assert_eq!(response["result"]["isError"], false, "{text}");
    let committed = payload(&response);
    let sha = committed["commit"].as_str().expect("commit SHA");
    assert_eq!(committed["worktree_id"], json!(f.worktree.id.0));
    assert_eq!(committed["branch"], f.worktree.branch);
    assert_eq!(committed["parent"], base);
    assert_eq!(committed["status"], "pending");
    assert_eq!(committed["verification_required"], true);
    assert!(committed.get("worktree_dir").is_none());
    assert_eq!(git(&f.dir, &["rev-parse", "HEAD"]), sha);
    assert_eq!(git(&f.dir, &["rev-parse", "HEAD^"]), base);
    assert_eq!(git(&f.dir, &["show", "HEAD:file [x].txt"]), "new content");
    assert!(git(&f.dir, &["ls-tree", "HEAD", "check.sh"]).starts_with("100755 "));
    assert!(!git(&f.dir, &["ls-tree", "--name-only", "HEAD"]).contains("delete-me.txt"));
    assert!(git(&f.dir, &["status", "--porcelain"]).is_empty());
    assert_eq!(git(&f.repo, &["rev-parse", "main"]), base);
    assert!(f.state.worktrees.read()[0].verified_commit.is_none());
    let approve_uri = format!("/api/v1/swarm/worktrees/{}/approve", f.worktree.id.0);
    assert_eq!(
        call(
            &app,
            "POST",
            approve_uri.clone(),
            &f.token,
            Some(json!({"expected_commit": sha})),
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "commit alone cannot authorize approval"
    );
    verify(&f).await;
    assert_eq!(
        f.state.worktrees.read()[0]
            .verified_commit
            .as_ref()
            .expect("verified")
            .0,
        sha
    );
    assert_eq!(
        call(
            &app,
            "POST",
            approve_uri.clone(),
            &oauth.raw_session_token,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED,
        "MCP credential cannot grant human approval"
    );
    let (status, text) = call(
        &app,
        "POST",
        approve_uri,
        &f.token,
        Some(json!({"expected_commit": sha})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let publish_uri = format!("/api/v1/swarm/worktrees/{}/publish", f.worktree.id.0);
    assert_eq!(
        call(
            &app,
            "POST",
            publish_uri.clone(),
            &oauth.raw_session_token,
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, text) = call(&app, "POST", publish_uri, &f.token, Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        git(&f.remote, &["show-ref"]),
        format!("{sha} refs/heads/main")
    );
    for dir in [&f.repo, &f.dir, &f.remote] {
        git(dir, &["fsck", "--strict"]);
    }
}

#[tokio::test]
async fn mcp_commit_rejects_bad_inputs_and_busy_operations_without_ref_mutation() {
    let f = fixture().await;
    std::fs::write(f.dir.join("README.md"), "new content\n").expect("edit");
    let before = git(&f.dir, &["rev-parse", "HEAD"]);
    let snapshot = git(&f.dir, &["status", "--porcelain"]);
    let valid = args(&f);
    for (key, value) in [
        ("expected_head", json!("0".repeat(40))),
        ("expected_head", json!("HEAD")),
        ("paths", json!([])),
        ("paths", json!(["../README.md"])),
        ("paths", json!([".git/config"])),
        ("message", json!("")),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        let result = rpc(&f.state, &f.owner, invalid).await;
        assert_eq!(result["result"]["isError"], true, "{key}: {result}");
        assert_eq!(git(&f.dir, &["rev-parse", "HEAD"]), before);
        assert_eq!(git(&f.dir, &["status", "--porcelain"]), snapshot);
    }
    let runtime = f
        .state
        .get_worktree_runtime(f.worktree.id)
        .expect("runtime");
    let worktree_guard = runtime.exec_lock.lock().await;
    let result = rpc(&f.state, &f.owner, valid.clone()).await;
    assert_eq!(payload(&result)["http_status"], 409, "{result}");
    drop(worktree_guard);
    let repo_guard = f.state.repo_mutation_lock.lock().await;
    let result = rpc(&f.state, &f.owner, valid).await;
    assert_eq!(payload(&result)["http_status"], 409, "{result}");
    drop(repo_guard);
    assert_eq!(git(&f.dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(git(&f.dir, &["status", "--porcelain"]), snapshot);
    git(&f.repo, &["fsck", "--strict"]);
}

#[tokio::test]
async fn mcp_commit_enforces_trusted_mode_role_owner_and_terminal_state() {
    let f = fixture().await;
    std::fs::write(f.dir.join("README.md"), "new content\n").expect("edit");
    let valid = args(&f);
    let before = git(&f.dir, &["rev-parse", "HEAD"]);
    let disabled = f.state.clone().with_trusted_sandbox_exec(false);
    let result = rpc(&disabled, &f.owner, valid.clone()).await;
    assert_eq!(payload(&result)["http_status"], 403, "{result}");
    for role in [EnterpriseRole::Developer, EnterpriseRole::SiteAdmin] {
        let username = if role == EnterpriseRole::Developer {
            "developer"
        } else {
            "otheradmin"
        };
        let invite = f
            .state
            .auth_store
            .create_invite_link(
                &f.owner,
                &format!("{username}@company.internal"),
                role,
                vec![],
                None,
            )
            .expect("invite");
        f.state
            .auth_store
            .redeem_invite_link(&invite.raw_token, username, "Password!2026")
            .expect("redeem");
        let login = f
            .state
            .auth_store
            .login(username, "Password!2026")
            .expect("login");
        let principal = f
            .state
            .auth_store
            .authenticate_session(&login.raw_session_token)
            .expect("principal");
        if role == EnterpriseRole::Developer {
            f.state
                .worktree_runtime
                .write()
                .get_mut(&f.worktree.id)
                .expect("runtime")
                .created_by = Some(principal.user_id);
        }
        let result = rpc(&f.state, &principal, valid.clone()).await;
        assert_eq!(payload(&result)["http_status"], 403, "{role:?}: {result}");
        f.state
            .worktree_runtime
            .write()
            .get_mut(&f.worktree.id)
            .expect("runtime")
            .created_by = Some(f.owner.user_id);
    }
    set_worktree_status(&f.state, f.worktree.id, |wt| {
        wt.status = WorktreeStatus::Merged
    });
    let result = rpc(&f.state, &f.owner, valid).await;
    assert_eq!(payload(&result)["http_status"], 409, "{result}");
    assert_eq!(git(&f.dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(git(&f.repo, &["rev-parse", "main"]), before);
    git(&f.repo, &["fsck", "--strict"]);
}

#[tokio::test]
async fn mcp_commit_clears_old_verification_on_success_and_failed_attempt() {
    let f = fixture().await;
    verify(&f).await;
    std::fs::write(f.dir.join("README.md"), "new content\n").expect("edit");
    let response = rpc(&f.state, &f.owner, args(&f)).await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(f.state.worktrees.read()[0].status, WorktreeStatus::Pending);
    assert!(f.state.worktrees.read()[0].verified_commit.is_none());
    assert!(f.state.worktrees.read()[0].verified_checks_oid.is_none());
    verify(&f).await;
    let mut stale = args(&f);
    stale["expected_head"] = json!("0".repeat(40));
    let response = rpc(&f.state, &f.owner, stale).await;
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_ne!(f.state.worktrees.read()[0].status, WorktreeStatus::Verified);
    assert!(f.state.worktrees.read()[0].verified_commit.is_none());
    assert!(f.state.worktrees.read()[0].verified_checks_oid.is_none());
}

#[tokio::test]
async fn mcp_commit_http_reauthenticates_after_body_stream() {
    let f = fixture().await;
    std::fs::write(f.dir.join("README.md"), "pending change\n").expect("edit");
    let before = git(&f.dir, &["rev-parse", "HEAD"]);
    let oauth = f
        .state
        .auth_store
        .issue_oauth_session(f.owner.user_id)
        .expect("MCP token");
    let body = RevokeOnBody {
        store: f.state.auth_store.clone(),
        principal: f.owner.clone(),
        token: oauth.raw_session_token.clone(),
        data: Some(request("gitust_worktree_commit", args(&f)).to_string()),
    };
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", oauth.raw_session_token),
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::new(body))
        .expect("streamed request");
    let response = build_router(f.state.clone())
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(git(&f.dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(git(&f.repo, &["rev-parse", "main"]), before);
    assert!(git(&f.dir, &["status", "--porcelain"]).contains("README.md"));
}

struct RevokeOnBody {
    store: AuthStore,
    principal: AuthenticatedPrincipal,
    token: String,
    data: Option<String>,
}

impl http_body::Body for RevokeOnBody {
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
            .revoke_session(&self.principal, &self.token)
            .expect("revoke before body finishes");
        std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes::Bytes::from(data)))))
    }
}
