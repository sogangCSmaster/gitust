use super::*;
use crate::state::MAX_MCP_LINE_BYTES;

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
async fn http_gate_7_4_inbound_mirror_sync_and_repo_code_commit_diff_browsing() {
    use gitust_git::{RepoBlobSnapshot, RepoCommitList, RepoDiffSnapshot, RepoTreeSnapshot};

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let upstream = tmp.path().join("upstream-github");
    let mirror = tmp.path().join("gitust-mirror");
    init_temp_repo(&upstream);
    init_temp_repo(&mirror);

    std::fs::create_dir_all(upstream.join("src")).expect("mkdir src");
    std::fs::write(
        upstream.join("src/lib.rs"),
        "pub fn speed() -> &'static str {\n    \"blazing\"\n}\n",
    )
    .expect("write src/lib.rs");
    std::fs::write(upstream.join("README.md"), "hello\n synced from upstream\n").expect("write");
    for args in [
        &["add", "."][..],
        &["commit", "-q", "-m", "feat: add speed module"],
        &["branch", "release/v1"],
        &["tag", "v1.0.0"],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&upstream)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Upstream")
            .env("GIT_AUTHOR_EMAIL", "up@company.internal")
            .env("GIT_COMMITTER_NAME", "Upstream")
            .env("GIT_COMMITTER_EMAIL", "up@company.internal")
            .output()
            .expect("git upstream");
        assert!(out.status.success());
    }

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

    let upstream_url = upstream.to_string_lossy().into_owned();
    let state = AppState::with_auth_store(mirror.clone(), upstream_url.clone(), auth_store);
    let app = build_router(state);

    let (s, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({
            "branch": "agent/claude/keep-private",
            "prompt_summary": "local experiment"
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");

    for uri in [
        "/api/v1/repo/commits",
        "/api/v1/repo/tree",
        "/api/v1/repo/blob?path=README.md",
        "/api/v1/repo/diff",
    ] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "uri={uri}");
    }

    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&upstream)
        .args(["branch", "agent/upstream/should-be-filtered", "HEAD"])
        .output()
        .expect("git upstream agent branch");
    assert!(out.status.success());

    let (s, sync_body) = call(&app, "POST", "/api/v1/sync/github".into(), &token, None).await;
    assert_eq!(s, StatusCode::OK, "{sync_body}");
    let sync_report: gitust_sync::SyncStatusReport =
        serde_json::from_str(&sync_body).expect("parse sync report");
    assert_eq!(sync_report.private_agent_branches_filtered, 1);
    assert_eq!(
        sync_report
            .last_synced_commit
            .as_ref()
            .expect("last_synced_commit")
            .0
            .len(),
        40
    );

    let upstream_refs = GitEngine::new(&upstream)
        .list_non_agent_refs()
        .expect("upstream refs");
    let mirror_refs = GitEngine::new(&mirror)
        .list_non_agent_refs()
        .expect("mirror refs");
    assert_eq!(mirror_refs, upstream_refs);

    let (s, commits_body) = call(
        &app,
        "GET",
        "/api/v1/repo/commits?rev=main&limit=10".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{commits_body}");
    let commits: RepoCommitList = serde_json::from_str(&commits_body).expect("parse commits");
    assert_eq!(commits.commits.len(), 2);
    assert_eq!(commits.commits[0].summary, "feat: add speed module");

    let (s, tree_body) = call(
        &app,
        "GET",
        "/api/v1/repo/tree?rev=main&recursive=true".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{tree_body}");
    let tree: RepoTreeSnapshot = serde_json::from_str(&tree_body).expect("parse tree");
    assert!(tree.entries.iter().any(|e| e.path == "src/lib.rs"));

    let (s, blob_body) = call(
        &app,
        "GET",
        "/api/v1/repo/blob?rev=main&path=src/lib.rs".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{blob_body}");
    let blob: RepoBlobSnapshot = serde_json::from_str(&blob_body).expect("parse blob");
    assert!(!blob.is_binary);
    assert!(blob
        .content
        .as_deref()
        .unwrap_or_default()
        .contains("blazing"));

    let (s, diff_body) = call(
        &app,
        "GET",
        "/api/v1/repo/diff?base=HEAD~1&head=main".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{diff_body}");
    let diff: RepoDiffSnapshot = serde_json::from_str(&diff_body).expect("parse diff");
    assert!(diff.files.iter().any(|f| f.path == "src/lib.rs"));
    assert!(diff.additions >= 3);

    let (s, _) = call(
        &app,
        "GET",
        "/api/v1/repo/commits?rev=--upload-pack=evil".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, _) = call(
        &app,
        "GET",
        "/api/v1/repo/blob?rev=main&path=../secret".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, _) = call(
        &app,
        "GET",
        "/api/v1/repo/blob?rev=main&path=nonexistent.rs".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oauth_mcp_slow_loris_mid_body_deactivation_toctou_gate() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);
    let (app, admin_token) = test_app_at(repo, true);

    let (slow_profile, slow_login) = super::tests_oauth::create_and_login_dev(
        &app,
        &admin_token,
        "slowdev",
        "slowloris@company.internal",
        "SlowDevPassword!2026",
    )
    .await;

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

    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    let deact2_resp = app
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
