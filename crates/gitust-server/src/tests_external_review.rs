use super::tests_verification::{commit, fixture, run};
use super::*;
use crate::oauth::pkce_s256_challenge;

const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ0123456789";
const CLIENT: &str = "gitust-mcp-cli";
const REDIRECT: &str = "http://127.0.0.1/callback";

fn add_user(state: &AppState, admin: &str, name: &str, role: EnterpriseRole) -> (UserId, String) {
    let caller = state.auth_store.authenticate_session(admin).unwrap();
    let invite = state
        .auth_store
        .create_invite_link(
            &caller,
            &format!("{name}@company.internal"),
            role,
            vec![],
            None,
        )
        .unwrap();
    let user = state
        .auth_store
        .redeem_invite_link(&invite.raw_token, name, "Password!2026")
        .unwrap();
    let token = state
        .auth_store
        .login(name, "Password!2026")
        .unwrap()
        .raw_session_token;
    (user.id, token)
}

async fn spawn(app: &Router, token: &str, branch: &str) -> AgentWorktree {
    let (status, body) = call(
        app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        token,
        Some(serde_json::json!({"branch":branch, "prompt_summary":"Review regression"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str(&body).unwrap()
}

fn authorization(scope: &str) -> serde_json::Value {
    serde_json::json!({"response_type":"code", "client_id":CLIENT, "redirect_uri":REDIRECT,
        "code_challenge":pkce_s256_challenge(VERIFIER), "code_challenge_method":"S256", "scope":scope})
}

async fn code(app: &Router, token: &str) -> String {
    let (status, body) = call(
        app,
        "POST",
        "/oauth/authorize".into(),
        token,
        Some(authorization("mcp")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str::<serde_json::Value>(&body).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn exchange(code: &str) -> serde_json::Value {
    serde_json::json!({"grant_type":"authorization_code", "code":code, "client_id":CLIENT,
        "redirect_uri":REDIRECT, "code_verifier":VERIFIER})
}

#[tokio::test]
async fn spawn_rejects_a_busy_repository_before_creating_a_branch() {
    let (tmp, state, app, token, _wt, _dir) = fixture().await;
    let _lock = state.repo_mutation_lock.lock().await;
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({"branch":"agent/review/busy", "prompt_summary":"Must not create"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(tmp.path().join("repo"))
        .args(["show-ref", "--verify", "refs/heads/agent/review/busy"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(state.worktrees.read().len(), 1);
}

#[tokio::test]
async fn merged_worktrees_preserve_approval_against_owner_and_mcp_discard() {
    let (tmp, state, app, admin, _wt, _dir) = fixture().await;
    let (owner, dev) = add_user(&state, &admin, "owner", EnterpriseRole::Developer);
    let wt = spawn(&app, &dev, "agent/review/merged").await;
    let dir = state.get_worktree_runtime(wt.id).unwrap().worktree_dir;
    commit(&dir);
    assert_eq!(run(&app, &admin, &wt, "true").await.0, StatusCode::OK);
    let checked = state
        .worktrees
        .read()
        .iter()
        .find(|entry| entry.id == wt.id)
        .unwrap()
        .verified_commit
        .clone()
        .unwrap();
    let (status, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0),
        &admin,
        Some(serde_json::json!({"expected_commit": checked.0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let approval = tmp
        .path()
        .join("repo/.git/gitust-approvals")
        .join(format!("{}.json", wt.id.0));
    let expected = std::fs::read(&approval).unwrap();
    let (status, body) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", wt.id.0),
        &dev,
        None,
    )
    .await;
    assert!(
        matches!(status, StatusCode::CONFLICT | StatusCode::FORBIDDEN),
        "{body}"
    );
    assert_eq!(std::fs::read(&approval).unwrap(), expected);
    let mcp = state
        .auth_store
        .issue_oauth_session(owner)
        .unwrap()
        .raw_session_token;
    let (status, body) = call(&app, "POST", "/mcp".into(), &mcp, Some(serde_json::json!({
        "jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"gitust_worktree_discard", "arguments":{"worktree_id":wt.id.0}}
    }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(result["result"]["isError"], true, "{body}");
    assert_eq!(std::fs::read(&approval).unwrap(), expected);
    assert!(dir.exists());
    let (status, body) = call(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", wt.id.0),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!approval.exists());
    assert!(!dir.exists());
}

#[tokio::test]
async fn deactivation_during_a_sandbox_run_cannot_mint_verified_evidence() {
    let (_tmp, state, app, admin, wt, dir) = fixture().await;
    let (_, second) = add_user(&state, &admin, "secondadmin", EnterpriseRole::SiteAdmin);
    let original = state.auth_store.authenticate_session(&admin).unwrap();
    let second = state.auth_store.authenticate_session(&second).unwrap();
    let pending = tokio::spawn(async move {
        run(
            &app,
            &admin,
            &wt,
            "touch started; while test ! -f proceed; do sleep 0.02; done; rm started proceed",
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !dir.join("started").exists() {
            assert!(
                !pending.is_finished(),
                "sandbox exited before reaching the file gate"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("sandbox reached the file gate");
    state
        .auth_store
        .deactivate_user(&second, original.user_id)
        .unwrap();
    std::fs::write(dir.join("proceed"), "").unwrap();
    let (status, body) = pending.await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let tasks = state.worktrees.read();
    assert_eq!(tasks[0].status, WorktreeStatus::Failed);
    assert!(tasks[0].verified_commit.is_none());
    assert!(tasks[0].verified_checks_oid.is_none());
}

#[tokio::test]
async fn every_recognized_code_exchange_attempt_consumes_the_code() {
    let (_tmp, _state, app, admin, _wt, _dir) = fixture().await;
    for invalid in ["verifier", "grant"] {
        let code = code(&app, &admin).await;
        let mut request = exchange(&code);
        if invalid == "verifier" {
            request["code_verifier"] = serde_json::json!("short");
        } else {
            request["grant_type"] = serde_json::json!("password");
        }
        assert_eq!(
            call(&app, "POST", "/oauth/token".into(), "", Some(request))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let (status, body) = call(
            &app,
            "POST",
            "/oauth/token".into(),
            "",
            Some(exchange(&code)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{invalid}: consumed code was accepted again"
        );
        assert!(body.contains("invalid_grant"), "{body}");
    }
}

#[tokio::test]
async fn saturated_dynamic_registration_keeps_existing_clients_and_rejects_new_ones() {
    let (_tmp, state, app, admin, _wt, _dir) = fixture().await;
    for n in 1..crate::state::MAX_OAUTH_CLIENTS {
        let (status, body) = call(&app, "POST", "/oauth/register".into(), "", Some(serde_json::json!({
            "client_name":format!("client-{n}"), "redirect_uris":["https://example.test/callback"]
        }))).await;
        assert_eq!(status, StatusCode::CREATED, "{n}: {body}");
    }
    let (status, body) = call(
        &app,
        "POST",
        "/oauth/register".into(),
        "",
        Some(serde_json::json!({
            "client_name":"overflow", "redirect_uris":["https://example.test/callback"]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(
        state
            .oauth_mcp
            .lock()
            .get_client(CLIENT)
            .unwrap()
            .redirect_uris[0],
        REDIRECT
    );
    assert!(!code(&app, &admin).await.is_empty());
}

#[tokio::test]
async fn mcp_token_can_list_tools_and_logout_but_cannot_access_general_apis() {
    let (_tmp, state, app, admin, _wt, _dir) = fixture().await;
    let user = state
        .auth_store
        .authenticate_session(&admin)
        .unwrap()
        .user_id;
    let token = state
        .auth_store
        .issue_oauth_session(user)
        .unwrap()
        .raw_session_token;
    assert_eq!(
        call(&app, "GET", "/api/v1/mcp/tools".into(), &token, None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/auth/me".into(), &token, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "POST", "/api/v1/auth/logout".into(), &token, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/mcp/tools".into(), &token, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn revoked_authorizing_credentials_cannot_be_exchanged_for_new_mcp_sessions() {
    for invalidation in ["logout", "password-reset", "revoke-pat"] {
        let (_tmp, state, app, admin, _wt, _dir) = fixture().await;
        let principal = state.auth_store.authenticate_session(&admin).unwrap();
        let pat = state
            .auth_store
            .create_personal_access_token(&principal, "Authorize MCP", None)
            .unwrap();
        let bearer = if invalidation == "revoke-pat" {
            &pat.raw_token
        } else {
            &admin
        };
        let code = code(&app, bearer).await;
        match invalidation {
            "logout" => state.auth_store.revoke_session(&principal, &admin).unwrap(),
            "password-reset" => {
                let reset = state
                    .auth_store
                    .create_password_reset_link(&principal, principal.user_id, None)
                    .unwrap();
                state
                    .auth_store
                    .redeem_password_reset_link(&reset.raw_token, "UpdatedPassword!2026")
                    .unwrap();
            }
            _ => {
                state
                    .auth_store
                    .revoke_personal_access_token(&principal, pat.metadata.id)
                    .unwrap();
            }
        }
        let (status, body) = call(
            &app,
            "POST",
            "/oauth/token".into(),
            "",
            Some(exchange(&code)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{invalidation}: revoked credential minted a new token"
        );
        assert!(body.contains("invalid_grant"), "{body}");
    }
}

#[tokio::test]
async fn authorization_rejects_unknown_scopes_and_registration_rejects_malformed_redirects() {
    let (_tmp, _state, app, admin, _wt, _dir) = fixture().await;
    for scope in ["admin", "mcp admin", "", &"m".repeat(4096)] {
        let (status, body) = call(
            &app,
            "POST",
            "/oauth/authorize".into(),
            &admin,
            Some(authorization(scope)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("invalid_scope"), "{body}");
    }
    for redirect in [
        "123://example.test/callback",
        "blob://example.test/callback",
        "ws://example.test/callback",
        "https://:8443/callback",
        "https://-bad.example/callback",
        "http://example.test/callback",
    ] {
        let (status, body) = call(
            &app,
            "POST",
            "/oauth/register".into(),
            "",
            Some(serde_json::json!({
                "client_name":"Bad redirect", "redirect_uris":[redirect]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{redirect}: {body}");
    }
    for redirect in [
        "https://example.test/callback",
        "http://127.0.0.1:8472/callback",
        "cursor://anysphere.cursor-mcp/oauth/callback",
    ] {
        let (status, body) = call(
            &app,
            "POST",
            "/oauth/register".into(),
            "",
            Some(serde_json::json!({
                "client_name":"Valid redirect", "redirect_uris":[redirect]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{redirect}: {body}");
    }
}

#[tokio::test]
async fn ownership_survives_restart_and_completed_worktrees_do_not_use_active_user_quota() {
    let (tmp, state, app, admin, _wt, _dir) = fixture().await;
    let (owner, dev) = add_user(&state, &admin, "restoredowner", EnterpriseRole::Developer);
    let wt = spawn(&app, &dev, "agent/review/recovered-owner").await;
    let restarted = AppState::with_auth_store(
        tmp.path().join("repo"),
        "https://example.test/repo.git".into(),
        state.auth_store.clone(),
    )
    .unwrap();
    assert_eq!(
        restarted.get_worktree_runtime(wt.id).unwrap().created_by,
        Some(owner)
    );
    let recovered = build_router(restarted.clone());
    let (status, body) = call(
        &recovered,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{}", wt.id.0),
        &dev,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    for n in 0..crate::state::MAX_WORKTREES_PER_USER {
        spawn(&recovered, &dev, &format!("agent/review/completed-{n}")).await;
    }
    let (status, body) = call(&recovered, "POST", "/api/v1/swarm/worktrees".into(), &dev,
        Some(serde_json::json!({"branch":"agent/review/over-active-quota", "prompt_summary":"Must reject while active"}))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    // Quota classification uses completed domain state; real merging is covered by the approval test.
    for task in restarted.worktrees.write().iter_mut() {
        task.status = WorktreeStatus::Merged;
    }
    let final_wt = spawn(&recovered, &dev, "agent/review/after-completion").await;
    assert_eq!(final_wt.status, WorktreeStatus::Pending);
}

#[tokio::test]
async fn custom_persistent_auth_database_and_lock_are_hidden_from_real_sandbox_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);
    let auth_dir = tmp.path().join("custom-credentials");
    let auth_db = auth_dir.join("private-auth.json");
    let store = AuthStore::open_persistent(&auth_db).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let state = AppState::with_auth_store(repo, "https://example.test/repo.git".into(), store)
        .unwrap()
        .with_trusted_sandbox_exec(true);
    let app = build_router(state.clone());
    let wt = spawn(&app, &token, "agent/review/custom-auth").await;
    // Temp directories are normally hidden by host policy. Reopen this fixture parent
    // read-only to model a service installed under a readable host path such as /var/lib.
    // A sibling control read proves denial comes from the custom auth policy itself.
    state
        .worktree_runtime
        .write()
        .get_mut(&wt.id)
        .unwrap()
        .sandbox_spec
        .rules
        .push(gitust_sandbox::PathRule::new(
            tmp.path(),
            gitust_sandbox::PathAccess::ReadOnly,
        ));
    let public_marker = tmp.path().join("public-host-marker");
    std::fs::write(&public_marker, "public-marker").unwrap();
    let lock = auth_dir.join("private-auth.json.lock");
    let temporary_snapshot = auth_dir.join("atomic-snapshot-marker");
    std::fs::write(&temporary_snapshot, "test-marker").unwrap();
    assert!(auth_db.is_file() && lock.is_file());
    let public_path = format!(
        "'{}'",
        public_marker.display().to_string().replace('\'', "'\\''")
    );
    let mut command =
        format!("test -f README.md && /bin/cat {public_path} >/dev/null 2>&1 || exit 30;");
    for (index, path) in [&auth_db, &lock, &temporary_snapshot].iter().enumerate() {
        let quoted = format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
        command.push_str(&format!(
            " if /bin/cat {quoted} >/dev/null 2>&1; then exit {}; fi;",
            31 + index
        ));
    }
    let (status, body) = run(&app, &token, &wt, &command).await;
    assert_eq!(status, StatusCode::OK, "sandbox request failed");
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        result["exit_code"], 0,
        "sandbox could read a private auth path"
    );
    assert_eq!(result["stdout"], "");
}
