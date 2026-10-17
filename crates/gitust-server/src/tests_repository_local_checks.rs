use super::tests_repository_visibility::fixture;
use super::*;
use crate::local_checks_history::{LocalChecksHistoryStore, LocalChecksRecord};
use gitust_core::CommitSha;
use gitust_sandbox::{SandboxBackend, SandboxCommandOutput, SandboxCommandResult};
use serde_json::{json, Value};

fn route(id: RepoId) -> String {
    format!("/api/v1/repos/{}/local-check-runs", id.0)
}
async fn read(app: &Router, token: &str, id: RepoId) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(route(id))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()));
    (status, value)
}
fn record(failed: bool) -> LocalChecksRecord {
    let result = SandboxExecutionResult {
        run_id: gitust_core::SandboxRunId::new(),
        worktree_id: WorktreeId::new(),
        command: "first && second".into(),
        exit_code: if failed { 7 } else { 0 },
        duration_ms: 3,
        stdout: "stored command output".into(),
        stderr: String::new(),
        output_truncated: false,
        steps: ["first", "second"]
            .into_iter()
            .enumerate()
            .map(|(index, command)| {
                let skipped = failed && index == 1;
                SandboxCommandResult {
                    command: command.into(),
                    required: true,
                    exit_code: (!skipped).then_some(if failed { 7 } else { 0 }),
                    duration_ms: (!skipped).then_some(1),
                    output: (!skipped).then(|| SandboxCommandOutput {
                        stdout: "stored command output".into(),
                        stderr: String::new(),
                        truncated: false,
                    }),
                }
            })
            .collect(),
        backend: SandboxBackend::MacosSeatbelt,
        network_isolated: true,
    };
    LocalChecksRecord::new(
        "refs/heads/main".into(),
        CommitSha("a".repeat(40)),
        "b".repeat(40),
        result,
    )
    .unwrap()
}

#[tokio::test]
async fn primary_local_checks_returns_exact_saved_results_without_enabling_execution_or_mutating_history(
) {
    let (_temp, state, token, _) = fixture();
    assert!(!state.trusted_sandbox_exec);
    let common = state.git_engine.repository_common_directory().unwrap();
    let app = build_router(state.clone());
    let (status, empty) = read(&app, &token, state.repo_id).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(
        empty,
        json!({"version":1,"repo_id":state.repo_id,"records":[]})
    );
    assert!(!common.join("gitust-local-check-runs").exists());
    let owner = state.git_engine.lock_worktree_service().unwrap();
    let store = LocalChecksHistoryStore::open(&common, state.repo_id).unwrap();
    let mut saved = Vec::new();
    for index in 0..6 {
        let item = record(index == 5);
        store.append(item.clone()).unwrap();
        saved.push(item);
    }
    let snapshot = std::fs::read(common.join("gitust-local-check-runs/history.json")).unwrap();
    let principal = state.auth_store.authenticate_session(&token).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&principal, "read local Checks", None)
        .unwrap();
    for credential in [&token, &pat.raw_token] {
        let (status, actual) = read(&app, credential, state.repo_id).await;
        assert_eq!(status, StatusCode::OK, "{actual}");
        assert_eq!(
            actual,
            json!({"version":1,"repo_id":state.repo_id,"records":saved[1..].iter().rev().collect::<Vec<_>>()})
        );
        assert_eq!(
            std::fs::read(common.join("gitust-local-check-runs/history.json")).unwrap(),
            snapshot
        );
    }
    drop(owner);
    assert!(!common.join("gitust-check-runs").exists());
    assert!(!common.join("gitust-automatic-runs").exists());
    assert!(state.worktrees.read().is_empty());
}

#[tokio::test]
async fn local_checks_requires_current_general_admin_credentials_and_actual_repository_access() {
    let (_temp, state, token, _) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let app = build_router(state.clone());
    assert_eq!(
        read(&app, "invalid", state.repo_id).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mcp = state.auth_store.issue_oauth_session(admin.user_id).unwrap();
    assert_eq!(
        read(&app, &mcp.raw_session_token, state.repo_id).await.0,
        StatusCode::UNAUTHORIZED
    );
    for (name, role) in [
        ("developer", EnterpriseRole::Developer),
        ("maintainer", EnterpriseRole::Maintainer),
    ] {
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
            read(&app, &login.raw_session_token, state.repo_id).await.0,
            StatusCode::FORBIDDEN
        );
        let member = state
            .auth_store
            .authenticate_session(&login.raw_session_token)
            .unwrap();
        state
            .auth_store
            .deactivate_user(&admin, member.user_id)
            .unwrap();
        assert_ne!(
            read(&app, &login.raw_session_token, state.repo_id).await.0,
            StatusCode::OK
        );
    }
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "revoked read", None)
        .unwrap();
    state
        .auth_store
        .revoke_personal_access_token(&admin, pat.metadata.id)
        .unwrap();
    assert_eq!(
        read(&app, &pat.raw_token, state.repo_id).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mut mismatch = state.clone();
    mismatch.repo_access.repo_id = RepoId::new();
    assert_eq!(
        read(&build_router(mismatch), &token, state.repo_id).await.0,
        StatusCode::FORBIDDEN
    );
    state.auth_store.revoke_session(&admin, &token).unwrap();
    assert_eq!(
        read(&app, &token, state.repo_id).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn secondary_is_not_exposed_and_invalid_saved_or_repository_identity_is_an_error_not_empty_history(
) {
    let (_temp, state, token, secondary) = fixture();
    let app = build_router(state.clone());
    for id in [secondary, RepoId::new()] {
        assert_eq!(read(&app, &token, id).await.0, StatusCode::NOT_FOUND);
    }
    let common = state.git_engine.repository_common_directory().unwrap();
    let owner = state.git_engine.lock_worktree_service().unwrap();
    LocalChecksHistoryStore::open(&common, state.repo_id)
        .unwrap()
        .append(record(false))
        .unwrap();
    let path = common.join("gitust-local-check-runs/history.json");
    let snapshot = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{unfinished JSON").unwrap();
    let (status, body) = read(&app, &token, state.repo_id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, "repository data is unavailable");
    assert_eq!(std::fs::read(&path).unwrap(), b"{unfinished JSON");
    std::fs::write(&path, snapshot).unwrap();
    let identity = common.join("gitust-repository/identity.json");
    std::fs::write(
        &identity,
        serde_json::to_vec(&json!({"version":1,"repo_id":RepoId::new()})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        read(&app, &token, state.repo_id).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(owner);
}
