use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::{json, Value};

fn git(state: &AppState, args: &[&str]) -> String {
    let output = fixture_git()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=HTTP Checks fixture",
            "-c",
            "user.email=http-checks@company.internal",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .arg("-C")
        .arg(state.git_engine.repo_path())
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn policy(state: &AppState, commands: &[&str]) -> Value {
    let repo = state.git_engine.repo_path();
    std::fs::write(
        repo.join(".gitust-checks.json"),
        json!({"version":1,"commands":commands,"auto_run":false}).to_string(),
    )
    .unwrap();
    git(state, &["add", ".gitust-checks.json"]);
    git(state, &["commit", "-qm", "committed manual HTTP checks"]);
    json!({"expected_source_ref":git(state, &["symbolic-ref","HEAD"]),
           "expected_commit":git(state, &["rev-parse","HEAD"]),"timeout_secs":10})
}
async fn request(
    app: &Router,
    token: &str,
    id: RepoId,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(if payload.is_some() { "POST" } else { "GET" })
                .uri(format!("/api/v1/repos/{}/local-check-runs", id.0))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(payload.map_or_else(Body::empty, |v| Body::from(v.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let no_store = response.headers().get(header::CACHE_CONTROL).cloned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        no_store.and_then(|v| v.to_str().ok().map(str::to_owned)),
        Some("no-store".into())
    );
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn http_default_checks_store_actual_pass_failure_and_skipped_results_without_task_authority()
{
    let (_temp, state, token, _) = fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let app = build_router(state.clone());
    for failed in [false, true] {
        let commands = if failed {
            vec!["printf expected-failure; exit 7", "printf skipped"]
        } else {
            vec!["test -f README.md && printf first", "printf second"]
        };
        let input = policy(&state, &commands);
        let refs = git(&state, &["show-ref", "--head"]);
        let head = git(&state, &["symbolic-ref", "HEAD"]);
        let worktrees = git(&state, &["worktree", "list", "--porcelain"]);
        let blob = git(&state, &["rev-parse", "HEAD:.gitust-checks.json"]);
        std::fs::write(
            state.git_engine.repo_path().join(".gitust-checks.json"),
            "dirty config",
        )
        .unwrap();
        let (status, body) = request(&app, &token, state.repo_id, Some(input.clone())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_object().unwrap().len(), 7);
        assert_eq!(body["version"], 1);
        assert_eq!(body["repo_id"], state.repo_id.0.to_string());
        assert_eq!(body["status"], if failed { "failed" } else { "passed" });
        assert_eq!(body["reason"], Value::Null);
        assert_eq!(body["cleanup"], "confirmed");
        assert_eq!(body["history"], json!({"status":"saved","reason":null}));
        let row = &body["record"];
        assert_eq!(row["source_ref"], input["expected_source_ref"]);
        assert_eq!(row["checked_commit"], input["expected_commit"]);
        assert_eq!(row["checks_blob_oid"], blob);
        assert_eq!(row["result"]["exit_code"], if failed { 7 } else { 0 });
        assert_eq!(row["result"]["steps"][0]["required"], true);
        assert_eq!(row["result"]["steps"][1]["required"], true);
        assert_eq!(
            row["result"]["steps"][1]["exit_code"],
            if failed { Value::Null } else { json!(0) }
        );
        let (status, history) = request(&app, &token, state.repo_id, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history["records"][0], *row);
        assert_eq!(git(&state, &["show-ref", "--head"]), refs);
        assert_eq!(git(&state, &["symbolic-ref", "HEAD"]), head);
        assert_eq!(git(&state, &["worktree", "list", "--porcelain"]), worktrees);
        assert_eq!(
            std::fs::read_to_string(state.git_engine.repo_path().join(".gitust-checks.json"))
                .unwrap(),
            "dirty config"
        );
        assert!(state.worktrees.read().is_empty());
        assert!(state.worktree_runtime.read().is_empty());
        let common = state.git_engine.repository_common_directory().unwrap();
        assert!(!common.join("gitust-check-runs").exists());
        assert!(!common.join("gitust-automatic-runs").exists());
        assert_eq!(
            std::fs::read_dir(common.join("gitust-active-executions"))
                .unwrap()
                .count(),
            0
        );
    }
    git(&state, &["fsck", "--full", "--strict"]);
}

#[tokio::test]
async fn http_checks_admission_keeps_primary_admin_opt_in_and_expected_snapshot_contract() {
    let (_temp, state, token, secondary) = fixture();
    let input = policy(&state, &["printf passed"]);
    let (status, body) = request(
        &build_router(state.clone()),
        &token,
        state.repo_id,
        Some(input.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, json!({"code":"local_checks_disabled"}));
    for enabled in [false, true] {
        let (status, body) = call(
            &build_router(state.clone().with_trusted_sandbox_exec(enabled)),
            "GET",
            "/api/v1/repos".into(),
            &token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let catalog: Value = serde_json::from_str(&body).unwrap();
        for row in catalog["repositories"].as_array().unwrap() {
            assert_eq!(
                row["can_run_local_checks"],
                row["id"] == state.repo_id.0.to_string() && enabled
            );
        }
    }
    let state = state.with_trusted_sandbox_exec(true);
    let app = build_router(state.clone());
    for id in [secondary, RepoId::new()] {
        assert_eq!(
            request(&app, &token, id, Some(input.clone())).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let mcp = state.auth_store.issue_oauth_session(admin.user_id).unwrap();
    assert_ne!(
        request(
            &app,
            &mcp.raw_session_token,
            state.repo_id,
            Some(input.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    for role in [EnterpriseRole::Developer, EnterpriseRole::Maintainer] {
        let name = if role == EnterpriseRole::Developer {
            "developer"
        } else {
            "maintainer"
        };
        let invite = state
            .auth_store
            .create_invite_link(
                &admin,
                &format!("{name}@company.internal"),
                role,
                vec![],
                None,
            )
            .unwrap();
        state
            .auth_store
            .redeem_invite_link(&invite.raw_token, name, "Password!2026")
            .unwrap();
        let login = state.auth_store.login(name, "Password!2026").unwrap();
        assert_eq!(
            request(
                &app,
                &login.raw_session_token,
                state.repo_id,
                Some(input.clone())
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let mut stale = input.clone();
    stale["expected_commit"] = json!("a".repeat(40));
    let (status, body) = request(&app, &token, state.repo_id, Some(stale)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, json!({"code":"local_checks_target_changed"}));
    let mut unknown = input.clone();
    unknown["command"] = json!("printf forbidden");
    assert_eq!(
        request(&app, &token, state.repo_id, Some(unknown)).await.0,
        StatusCode::BAD_REQUEST
    );
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "ordinary checks", None)
        .unwrap();
    state
        .auth_store
        .revoke_personal_access_token(&admin, pat.metadata.id)
        .unwrap();
    assert_eq!(
        request(&app, &pat.raw_token, state.repo_id, Some(input))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(!state
        .git_engine
        .repository_common_directory()
        .unwrap()
        .join("gitust-local-check-runs")
        .exists());
}

#[tokio::test]
async fn http_confirmed_checks_preserve_actual_result_when_history_is_unavailable() {
    let (_temp, state, token, _) = fixture();
    let input = policy(&state, &["printf completed"]);
    let state = state.with_trusted_sandbox_exec(true);
    let app = build_router(state.clone());
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "ordinary checks", None)
        .unwrap();
    assert_eq!(
        request(&app, &pat.raw_token, state.repo_id, Some(input.clone()))
            .await
            .0,
        StatusCode::OK
    );
    let common = state.git_engine.repository_common_directory().unwrap();
    let history = common.join("gitust-local-check-runs/history.json");
    std::fs::write(&history, "{unfinished snapshot").unwrap();
    let (status, body) = request(&app, &token, state.repo_id, Some(input)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "passed");
    assert_eq!(body["cleanup"], "confirmed");
    assert_eq!(body["record"]["result"]["stdout"], "completed");
    assert_eq!(body["history"]["status"], "failed");
    assert_eq!(body["reason"], "history_unconfirmed");
    assert_eq!(
        std::fs::read_to_string(history).unwrap(),
        "{unfinished snapshot"
    );
}
