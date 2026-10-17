//! Branch pages describe actual local refs, independently of the default HEAD.
use super::*;
#[tokio::test]
async fn uuid_branch_page_reads_primary_and_secondary_local_heads() {
    let (_root, state, token, secondary, id) = super::tests_repository_catalog::registered_pair();
    git(
        &secondary,
        &["commit", "--allow-empty", "-qm", "distinct secondary"],
    );
    let primary = state.repo_id;
    let primary_sha = git(
        state.git_engine.repo_path(),
        &["rev-parse", "refs/heads/main"],
    );
    let secondary_sha = git(&secondary, &["rev-parse", "refs/heads/main"]);
    assert_ne!(primary_sha, secondary_sha);
    let app = build_router(state);
    for id in [primary, id] {
        let (status, body) = call(
            &app,
            "GET",
            format!("/api/v1/repos/{}/branches", id.0),
            &token,
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "branch read route must be available"
        );
        let page: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(page["branches"][0]["name"], "main");
        assert_eq!(page["branches"][0]["ref_name"], "refs/heads/main");
        assert!(page["next_cursor"].is_null());
        assert_eq!(
            page["branches"][0]["commit_sha"],
            if id == primary {
                primary_sha.as_str()
            } else {
                secondary_sha.as_str()
            }
        );
    }
}

use std::{path::Path, process::Command, sync::Arc};
fn git(path: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
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
    assert!(output.status.success(), "synthetic Git operation failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
async fn request(app: &Router, url: &str, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(url)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn branch_reads_apply_current_repo_acl_and_only_configured_ids() {
    let (_root, state, session, _secondary, id) =
        super::tests_repository_catalog::registered_pair();
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "unhosted", TeamId::new(), true)
        .unwrap();
    let invite = state
        .auth_store
        .create_invite_link(
            &admin,
            "outsider@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, "outsider", "Password!2026")
        .unwrap();
    let outsider = state
        .auth_store
        .login("outsider", "Password!2026")
        .unwrap()
        .raw_session_token;
    let app = build_router(state);
    for (id, token, expected) in [
        (id, session.as_str(), StatusCode::OK),
        (id, outsider.as_str(), StatusCode::FORBIDDEN),
        (id, "", StatusCode::UNAUTHORIZED),
        (metadata.id, session.as_str(), StatusCode::NOT_FOUND),
        (RepoId::new(), session.as_str(), StatusCode::NOT_FOUND),
    ] {
        let response = request(&app, &format!("/api/v1/repos/{}/branches", id.0), token).await;
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
#[tokio::test]
async fn branch_reads_reauthenticate_original_session_and_pat_after_git() {
    for pat in [false, true] {
        let (_root, state, session, _secondary, id) =
            super::tests_repository_catalog::registered_pair();
        let admin = state.auth_store.authenticate_session(&session).unwrap();
        let issued = state
            .auth_store
            .create_personal_access_token(&admin, "branch-read", None)
            .unwrap();
        let token = if pat {
            issued.raw_token.clone()
        } else {
            session
        };
        let revoke = token.clone();
        let store = state.auth_store.clone();
        *state.repository_read_hook.lock() = Some(Arc::new(move || {
            if pat {
                store
                    .revoke_personal_access_token(&admin, issued.metadata.id)
                    .unwrap();
            } else {
                store.revoke_session(&admin, &revoke).unwrap();
            }
        }));
        let response = request(
            &build_router(state),
            &format!("/api/v1/repos/{}/branches", id.0),
            &token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
#[tokio::test]
async fn branch_queries_reject_invalid_input_and_roundtrip_maximum_unicode_keyset_in_real_uri() {
    let (_root, state, token, path, id) = super::tests_repository_catalog::registered_pair();
    let head = git(&path, &["rev-parse", "HEAD"]);
    let names: Vec<_> = ["0", "1"]
        .map(|prefix| format!("{prefix}{}x", "é".repeat(16_383)))
        .into();
    std::fs::write(
        path.join(".git/packed-refs"),
        format!(
            "# pack-refs with: sorted\n{}",
            names
                .iter()
                .map(|name| format!("{head} refs/heads/{name}\n"))
                .collect::<String>()
        ),
    )
    .unwrap();
    let app = build_router(state);
    let route = format!("/api/v1/repos/{}/branches", id.0);
    for suffix in [
        "?limit=0",
        "?limit=201",
        "?limit=-1",
        "?limit=bad",
        "?cursor=v1.bad",
        "?extra=1",
    ] {
        let response = request(&app, &format!("{route}{suffix}"), &token).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let (status, body) = call(&app, "GET", format!("{route}?limit=1"), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let page: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(page["branches"][0]["name"], names[0]);
    let cursor = page["next_cursor"].as_str().unwrap();
    assert_eq!(cursor.len(), 43_709);
    let url = format!("{route}?limit=1&cursor={cursor}");
    assert!(url.parse::<axum::http::Uri>().is_ok());
    let (status, body) = call(&app, "GET", url, &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let page: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(page["branches"][0]["name"], names[1]);
    git(&path, &["fsck", "--full", "--strict"]);
}
#[tokio::test]
async fn branch_api_separates_empty_default_from_other_heads_and_corruption() {
    let (root, state, token, path, id) = super::tests_repository_catalog::registered_pair();
    git(&path, &["symbolic-ref", "HEAD", "refs/heads/unborn"]);
    let app = build_router(state.clone());
    let route = format!("/api/v1/repos/{}/branches", id.0);
    let (status, body) = call(&app, "GET", route.clone(), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let page: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(page["branches"][0]["name"], "main");
    std::fs::write(path.join(".git/refs/heads/broken"), "not-an-object\n").unwrap();
    let response = request(&app, &route, &token).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let empty = root.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "-q", "--bare", "-b", "main"]);
    let state = AppState::with_auth_store(
        empty,
        "https://example.invalid/unused.git".into(),
        state.auth_store.clone(),
    )
    .unwrap();
    let id = state.repo_id;
    let (status, body) = call(
        &build_router(state),
        "GET",
        format!("/api/v1/repos/{}/branches", id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(page, serde_json::json!({"branches":[],"next_cursor":null}));
}
#[test]
fn branch_fixture_initialization_ignores_inherited_git_repository_selectors() {
    const MARKER: &str = "GITUST_BRANCH_FIXTURE_CHILD";
    if let Some(root) = std::env::var_os(MARKER) {
        let root = std::path::PathBuf::from(root);
        let scenario = std::env::var("GITUST_BRANCH_FIXTURE_SCENARIO").unwrap();
        let target = root.join(&scenario);
        init_temp_repo(&target);
        assert!(
            target.join(".git").is_dir(),
            "fixture must initialize its own repository"
        );
        match scenario.as_str() {
            "ownership" => http_gate_worktree_ownership_quota_and_work_preservation(),
            "lifecycle" => http_worktree_lifecycle_spawn_exec_approve_discard(),
            _ => panic!("unexpected fixture scenario"),
        }
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("unrelated");
    std::fs::create_dir(&victim).unwrap();
    git(&victim, &["init", "-q", "-b", "main"]);
    git(&victim, &["commit", "--allow-empty", "-qm", "preserve"]);
    let before = git(&victim, &["show-ref"]);
    let head_before = git(&victim, &["rev-parse", "HEAD"]);
    let results: Vec<_> = ["ownership", "lifecycle"].into_iter().map(|scenario| {
        let mut child = Command::new(std::env::current_exe().unwrap());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                child.env_remove(key);
            }
        }
        let result=child.args(["--exact","tests::tests_repository_branches::branch_fixture_initialization_ignores_inherited_git_repository_selectors"])
            .env(MARKER,root.path()).env("GITUST_BRANCH_FIXTURE_SCENARIO",scenario)
            .env("GIT_DIR",victim.join(".git")).env("GIT_WORK_TREE",&victim).output().unwrap();
        (scenario, result.status.success())
    }).collect();
    assert!(
        results.iter().all(|(_, success)| *success),
        "fixture Git selectors: {results:?}"
    );
    assert_eq!(git(&victim, &["show-ref"]), before);
    assert_eq!(git(&victim, &["rev-parse", "HEAD"]), head_before);
    assert!(git(&victim, &["status", "--porcelain"]).is_empty());
}
