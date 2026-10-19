use super::tests_pull_requests::api_json;
use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::json;

#[tokio::test]
async fn hosted_repository_ownership_survives_creation_and_recovery() {
    let (_temp, state, token, _) = fixture();
    let app = build_router(state.clone());
    let (status, created) = api_json(
        &app,
        "POST",
        "/api/v1/admin/repos".into(),
        &token,
        Some(json!({"name": "owned-online"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = RepoId(uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap());
    let path = state
        .read_repository(id)
        .unwrap()
        .engine
        .repo_path()
        .to_path_buf();
    assert!(matches!(
        GitEngine::new(&path).lock_worktree_service(),
        Err(gitust_git::GitEngineError::WorktreeServiceBusy(_))
    ));
    let primary = state.git_engine.repo_path().to_path_buf();
    let auth = state.auth_store.clone();
    drop(app);
    drop(state);
    let reopened =
        AppState::with_engine_and_auth_store(GitEngine::new(primary), String::new(), auth).unwrap();
    assert!(reopened.read_repository(id).is_some());
    assert!(matches!(
        GitEngine::new(&path).lock_worktree_service(),
        Err(gitust_git::GitEngineError::WorktreeServiceBusy(_))
    ));
    drop(reopened);
    assert!(GitEngine::new(path).lock_worktree_service().is_ok());
}

#[tokio::test]
async fn invalid_settings_fields_leave_all_existing_settings_unchanged() {
    let (_temp, state, token, _) = fixture();
    let app = build_router(state.clone());
    let uri = format!("/api/v1/repos/{}/settings", state.repo_id.0);
    let (status, before) = api_json(&app, "GET", uri.clone(), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    for payload in [
        json!({"name": "renamed-first", "is_private": false, "description": "x".repeat(2049)}),
        json!({"name": "renamed-second", "is_private": false, "default_branch": "missing-target"}),
        json!({"name": "renamed-third", "is_private": false, "default_branch": "invalid..branch"}),
    ] {
        let (status, failed) = api_json(&app, "PATCH", uri.clone(), &token, Some(payload)).await;
        assert!(!status.is_success(), "{failed}");
        let (status, after) = api_json(&app, "GET", uri.clone(), &token, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            after, before,
            "rejected settings must not change earlier fields"
        );
    }
}

#[tokio::test]
async fn settings_write_failure_explicitly_reports_prior_changes() {
    let (_temp, state, token, _) = fixture();
    let app = build_router(state.clone());
    let uri = format!("/api/v1/repos/{}/settings", state.repo_id.0);
    let settings_dir = state
        .git_engine
        .repository_common_directory()
        .unwrap()
        .join("gitust-repository");
    std::fs::create_dir(settings_dir.join("settings.json.tmp")).unwrap();
    let (status, failed) = api_json(
        &app,
        "PATCH",
        uri.clone(),
        &token,
        Some(json!({"name": "persisted-name", "description": "new description"})),
    )
    .await;
    assert!(!status.is_success());
    assert!(failed.to_string().contains("partially applied"), "{failed}");
    assert!(failed.to_string().contains("refresh"), "{failed}");
    let (status, after) = api_json(&app, "GET", uri, &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["name"], "persisted-name");
    assert!(after["description"].is_null());
}

#[tokio::test]
async fn settings_recheck_original_session_and_pat_before_each_publication() {
    for use_pat in [false, true] {
        for name_change in [false, true] {
            let (_temp, state, session, _) = fixture();
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "settings", None)
                .unwrap();
            let credential = if use_pat {
                pat.raw_token
            } else {
                session.clone()
            };
            let auth = state.auth_store.clone();
            let revoke = credential.clone();
            *state.repository_read_hook.lock() = Some(std::sync::Arc::new(move || {
                if use_pat {
                    auth.revoke_personal_access_token(&admin, pat.metadata.id)
                        .unwrap();
                } else {
                    auth.revoke_session(&admin, &revoke).unwrap();
                }
            }));
            let before = state.read_repository(state.repo_id).unwrap();
            let uri = format!("/api/v1/repos/{}/settings", state.repo_id.0);
            let payload = if name_change {
                json!({"name":"must-not-publish"})
            } else {
                json!({"description":"must-not-publish", "archived":true})
            };
            let (status, result) = api_json(
                &build_router(state.clone()),
                "PATCH",
                uri.clone(),
                &credential,
                Some(payload),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{result}");
            let fresh = state.auth_store.login("admin", "Password!2026").unwrap();
            let (status, after) = api_json(
                &build_router(state.clone()),
                "GET",
                uri,
                &fresh.raw_session_token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(after["name"], "primary");
            assert_eq!(after["archived"], before.archived);
            assert!(after["description"].is_null());
            let dir = state
                .git_engine
                .repository_common_directory()
                .unwrap()
                .join("gitust-repository");
            assert!(!dir.join("settings.json").exists());
            assert!(!dir.join("settings.json.tmp").exists());
        }
    }
}
