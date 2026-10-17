use super::*;
#[path = "tests_repository_default_auth.rs"]
mod authentication;
#[path = "tests_repository_default_secondary.rs"]
mod secondary;
use std::{path::Path, process::Command};
pub(super) fn git(path: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let out = command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@company.internal")
        .env("GIT_COMMITTER_EMAIL", "fixture@company.internal")
        .output()
        .unwrap();
    assert!(out.status.success(), "synthetic Git operation failed");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
pub(super) fn fixture() -> (tempfile::TempDir, AppState, String) {
    let temp = tempfile::tempdir().unwrap();
    let seed = temp.path().join("seed");
    init_temp_repo(&seed);
    git(&seed, &["branch", "topic"]);
    git(&seed, &["checkout", "-q", "topic"]);
    std::fs::write(seed.join("topic.txt"), "topic\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-qm", "topic"]);
    let bare = temp.path().join("bare");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&token).unwrap();
    let engine = GitEngine::new(&bare);
    let id = engine.load_or_create_repository_id().unwrap();
    let row = store
        .register_local_repository(&admin, id, "primary", TeamId::new(), true, None)
        .unwrap();
    engine
        .complete_repository_registration(row.binding)
        .unwrap();
    let state = AppState::with_engine_and_auth_store(
        engine,
        "https://example.invalid/repo.git".into(),
        store,
    )
    .unwrap();
    (temp, state, token)
}
pub(super) fn payload(state: &AppState) -> serde_json::Value {
    serde_json::json!({"target_ref":"refs/heads/topic","expected_current_ref":"refs/heads/main",
        "expected_target_commit":git(state.git_engine.repo_path(), &["rev-parse","refs/heads/topic"])})
}
#[tokio::test]
async fn bare_primary_default_changes_actual_head_and_preserves_all_branch_tips() {
    let (_temp, state, token) = fixture();
    let refs = git(state.git_engine.repo_path(), &["show-ref"]);
    let receipt = std::fs::read(
        state
            .git_engine
            .repository_common_directory()
            .unwrap()
            .join("gitust-repository/registration.json"),
    )
    .unwrap();
    let body = payload(&state);
    let app = build_router(state.clone());
    let (status, text) = call(
        &app,
        "PATCH",
        format!("/api/v1/repos/{}/default-branch", state.repo_id.0),
        &token,
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"id":state.repo_id,"branch":"topic","head_commit":body["expected_target_commit"]})
    );
    assert_eq!(
        git(state.git_engine.repo_path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/topic"
    );
    assert_eq!(git(state.git_engine.repo_path(), &["show-ref"]), refs);
    assert_eq!(
        std::fs::read(
            state
                .git_engine
                .repository_common_directory()
                .unwrap()
                .join("gitust-repository/registration.json")
        )
        .unwrap(),
        receipt
    );
    git(
        state.git_engine.repo_path(),
        &["fsck", "--full", "--strict"],
    );
}
#[tokio::test]
async fn default_change_denies_stale_invalid_unknown_secondary_legacy_and_nonbare_contexts() {
    let (_temp, state, token) = fixture();
    let app = build_router(state.clone());
    let before = git(state.git_engine.repo_path(), &["symbolic-ref", "HEAD"]);
    let url = format!("/api/v1/repos/{}/default-branch", state.repo_id.0);
    for (key, value, status) in [
        (
            "expected_current_ref",
            "refs/heads/stale",
            StatusCode::CONFLICT,
        ),
        (
            "expected_target_commit",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            StatusCode::CONFLICT,
        ),
        ("target_ref", "refs/heads/missing", StatusCode::CONFLICT),
        (
            "target_ref",
            "refs/heads/agent/private",
            StatusCode::BAD_REQUEST,
        ),
        ("target_ref", "refs/tags/topic", StatusCode::BAD_REQUEST),
        ("unknown", "extra", StatusCode::BAD_REQUEST),
    ] {
        let mut body = payload(&state);
        body[key] = value.into();
        assert_eq!(
            call(&app, "PATCH", url.clone(), &token, Some(body)).await.0,
            status
        );
        assert_eq!(
            git(state.git_engine.repo_path(), &["symbolic-ref", "HEAD"]),
            before
        );
    }
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let unhosted = state
        .auth_store
        .create_team_repository(&admin, "metadata", TeamId::new(), true)
        .unwrap();
    for id in [RepoId::new(), unhosted.id] {
        assert_eq!(
            call(
                &app,
                "PATCH",
                format!("/api/v1/repos/{}/default-branch", id.0),
                &token,
                Some(payload(&state))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(&app, "PATCH", url.clone(), "invalid", Some(payload(&state)))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let oversized = serde_json::json!({"target_ref":"x".repeat(256*1024),"expected_current_ref":"refs/heads/main","expected_target_commit":"a".repeat(40)});
    assert_eq!(
        call(&app, "PATCH", url, &token, Some(oversized)).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let (temp, mut normal, normal_token, secondary, id) =
        super::tests_repository_catalog::registered_pair();
    let normal_admin = normal
        .auth_store
        .authenticate_session(&normal_token)
        .unwrap();
    let row = normal
        .auth_store
        .register_local_repository(
            &normal_admin,
            normal.repo_id,
            "primary",
            TeamId::new(),
            true,
            None,
        )
        .unwrap();
    normal
        .git_engine
        .complete_repository_registration(row.binding)
        .unwrap();
    normal.repo_access = normal
        .auth_store
        .configured_repository_access(normal.repo_id, Some(row.binding))
        .unwrap();
    normal.repository_registry = None;
    normal.repository_registry = Some(
        crate::repository_registry::RepositoryRegistry::load(&normal, vec![secondary]).unwrap(),
    );
    let normal_app = build_router(normal.clone());
    let body = serde_json::json!({"target_ref":"refs/heads/main","expected_current_ref":"refs/heads/main","expected_target_commit":git(normal.git_engine.repo_path(), &["rev-parse","HEAD"])});
    for (repo_id, status) in [
        (normal.repo_id, StatusCode::CONFLICT),
        (id, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            call(
                &normal_app,
                "PATCH",
                format!("/api/v1/repos/{}/default-branch", repo_id.0),
                &normal_token,
                Some(body.clone())
            )
            .await
            .0,
            status
        );
    }
    let (_, catalog) = call(
        &normal_app,
        "GET",
        "/api/v1/repos".into(),
        &normal_token,
        None,
    )
    .await;
    let catalog: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert!(catalog["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["can_change_default_branch"] == false));
    let legacy = temp.path().join("legacy");
    init_temp_repo(&legacy);
    let (legacy_app, legacy_token) = test_app_at(legacy, false);
    let (_, raw) = call(
        &legacy_app,
        "GET",
        "/api/v1/repo".into(),
        &legacy_token,
        None,
    )
    .await;
    let id = serde_json::from_str::<serde_json::Value>(&raw).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        call(
            &legacy_app,
            "PATCH",
            format!("/api/v1/repos/{id}/default-branch"),
            &legacy_token,
            Some(body)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn default_change_is_admin_only_and_catalog_hint_is_not_authority() {
    let (_temp, state, token) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, state.repo_id)
        .unwrap()
        .owner_team_id;
    for (index, role) in [EnterpriseRole::Developer, EnterpriseRole::Maintainer]
        .into_iter()
        .enumerate()
    {
        let invite = state
            .auth_store
            .create_invite_link(
                &admin,
                &format!("member{index}@company.internal"),
                role,
                vec![team],
                None,
            )
            .unwrap();
        state
            .auth_store
            .redeem_invite_link(
                &invite.raw_token,
                &format!("member{index}"),
                "Password!2026",
            )
            .unwrap();
        let session = state
            .auth_store
            .login(&format!("member{index}"), "Password!2026")
            .unwrap()
            .raw_session_token;
        let app = build_router(state.clone());
        assert_eq!(
            call(
                &app,
                "PATCH",
                format!("/api/v1/repos/{}/default-branch", state.repo_id.0),
                &session,
                Some(payload(&state))
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let (_, body) = call(&app, "GET", "/api/v1/repos".into(), &session, None).await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["repositories"][0]
                ["can_change_default_branch"],
            false
        );
    }
    let (_, body) = call(
        &build_router(state.clone()),
        "GET",
        "/api/v1/repos".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["repositories"][0]
            ["can_change_default_branch"],
        true
    );
}
#[tokio::test]
async fn omitted_http_and_mcp_bases_follow_current_committed_default_and_explicit_bases_stay_fixed()
{
    let (_temp, state, token) = fixture();
    let app = build_router(state.clone());
    git(
        state.git_engine.repo_path(),
        &["symbolic-ref", "HEAD", "refs/heads/topic"],
    );
    let tip = git(state.git_engine.repo_path(), &["rev-parse", "topic"]);
    let (_, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({"branch":"agent/http-default","prompt_summary":"actual default"})),
    )
    .await;
    let row: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(row["base_branch"], "topic");
    assert_eq!(row["base_commit"], tip);
    let (_,body)=call(&app,"POST","/api/v1/swarm/worktrees".into(),&token,Some(serde_json::json!({"branch":"agent/http-explicit","base_branch":"main","prompt_summary":"explicit"}))).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["base_branch"],
        "main"
    );
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"gitust_worktree_spawn","arguments":{"branch":"agent/mcp-default"}}});
    let raw = handle_mcp_json_line(&state, &caller, &request.to_string())
        .await
        .unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(envelope["result"]["isError"], false);
    let row: serde_json::Value =
        serde_json::from_str(envelope["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(row["base_branch"], "topic");
    for head in ["refs/heads/unborn", &tip] {
        if head.starts_with("refs/") {
            git(
                state.git_engine.repo_path(),
                &["symbolic-ref", "HEAD", head],
            );
        } else {
            std::fs::write(
                state.git_engine.repo_path().join("HEAD"),
                format!("{head}\n"),
            )
            .unwrap();
        }
        let count = state.worktrees.read().len();
        assert_ne!(
            call(
                &app,
                "POST",
                "/api/v1/swarm/worktrees".into(),
                &token,
                Some(serde_json::json!({"branch":"agent/denied","prompt_summary":"no fallback"}))
            )
            .await
            .0,
            StatusCode::OK
        );
        let raw = handle_mcp_json_line(
            &state,
            &caller,
            &request
                .to_string()
                .replace("agent/mcp-default", "agent/mcp-denied"),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()["result"]["isError"],
            true
        );
        assert_eq!(state.worktrees.read().len(), count);
    }
}
