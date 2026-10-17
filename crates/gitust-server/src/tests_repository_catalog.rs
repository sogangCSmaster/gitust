use super::*;
use crate::repository_registry::RepositoryRegistry;
use std::sync::Arc;

fn registered_pair() -> (tempfile::TempDir, AppState, String, PathBuf, RepoId) {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    let secondary = temp.path().join("secondary");
    init_temp_repo(&primary);
    init_temp_repo(&secondary);
    let store = AuthStore::open_persistent(temp.path().join("auth/auth.json")).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&token).unwrap();
    let engine = GitEngine::new(&secondary);
    let id = engine.load_or_create_repository_id().unwrap();
    let registration = store
        .register_local_repository(&admin, id, "secondary", TeamId::new(), true, None)
        .unwrap();
    engine
        .complete_repository_registration(registration.binding)
        .unwrap();
    drop(engine);
    let mut state =
        AppState::with_auth_store(primary, "https://example.invalid/primary.git".into(), store)
            .unwrap();
    state.repository_registry =
        Some(RepositoryRegistry::load(&state, vec![secondary.clone()]).unwrap());
    (temp, state, token, secondary, id)
}

#[tokio::test]
async fn catalog_exposes_the_actual_default_repository_and_rejects_anonymous_access() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("primary");
    init_temp_repo(&path);
    let (app, token) = test_app_at(path, false);
    let (status, body) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rows = value["repositories"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["capability"], "workbench");
    assert_eq!(rows[0]["is_default"], true);
    assert_eq!(rows[0]["branch"], "main");
    assert!(!body.contains(tmp.path().to_str().unwrap()));
    assert_eq!(
        call(&app, "GET", "/api/v1/repos".into(), "invalid", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn secondary_routes_use_its_engine_and_never_accept_a_metadata_only_repository() {
    let (temp, state, token, path, id) = registered_pair();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "unhosted", TeamId::new(), true)
        .unwrap();
    let app = build_router(state.clone());
    let (status, body) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rows = value["repositories"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let secondary = rows
        .iter()
        .find(|row| row["id"] == id.0.to_string())
        .unwrap();
    assert_eq!(secondary["capability"], "browse");
    assert_eq!(secondary["is_default"], false);
    for suffix in ["commits", "tree", "blob?path=README.md", "diff"] {
        let (status, body) = call(
            &app,
            "GET",
            format!("/api/v1/repos/{}/{suffix}", id.0),
            &token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{suffix}: {body}");
        assert!(!body.contains(temp.path().to_str().unwrap()));
        assert_eq!(
            call(
                &app,
                "GET",
                format!("/api/v1/repos/{}/{suffix}", metadata.id.0),
                &token,
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    assert!(state.worktrees.read().is_empty());
    assert!(state.automatic_checks.store().is_err());
    assert!(!path.join(".git/gitust-automatic-runs").exists());
    assert_eq!(call(&app, "POST", "/api/v1/swarm/worktrees".into(), &token,
        Some(serde_json::json!({"repo_id": id,"branch":"agent/secondary-denied","prompt_summary":"denied"}))).await.0,
        StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn completed_reads_reauthenticate_the_exact_session_and_pat_before_returning_data() {
    for use_pat in [false, true] {
        for catalog in [false, true] {
            let (_temp, state, session, _path, id) = registered_pair();
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "read", None)
                .unwrap();
            let token = if use_pat {
                pat.raw_token.clone()
            } else {
                session.clone()
            };
            let store = state.auth_store.clone();
            let revoke = token.clone();
            *state.repository_read_hook.lock() = Some(Arc::new(move || {
                if use_pat {
                    store
                        .revoke_personal_access_token(&admin, pat.metadata.id)
                        .unwrap();
                } else {
                    store.revoke_session(&admin, &revoke).unwrap();
                }
            }));
            let route = if catalog {
                "/api/v1/repos".into()
            } else {
                format!("/api/v1/repos/{}/blob?path=README.md", id.0)
            };
            let (status, body) = call(&build_router(state), "GET", route, &token, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
            assert!(!body.contains("hello"));
        }
    }
}

#[test]
fn registry_rejects_aliases_duplicate_identity_and_partial_startup_releases_all_new_guards() {
    let (_temp, mut state, _token, secondary, _id) = registered_pair();
    state.repository_registry = None;
    assert!(
        RepositoryRegistry::load(&state, vec![state.git_engine.repo_path().to_owned()]).is_err()
    );
    let unregistered = secondary.parent().unwrap().join("unregistered");
    init_temp_repo(&unregistered);
    assert!(
        RepositoryRegistry::load(&state, vec![secondary.clone(), unregistered.clone()]).is_err()
    );
    assert!(
        !unregistered.join(".git/gitust-repository").exists(),
        "browse startup must not mint identity"
    );
    let primary_owner = GitEngine::new(state.git_engine.repo_path())
        .lock_worktree_service()
        .unwrap();
    let secondary_owner = GitEngine::new(&secondary).lock_worktree_service().unwrap();
    drop((primary_owner, secondary_owner));
    let copied = secondary.parent().unwrap().join("duplicate-id");
    init_temp_repo(&copied);
    let identity = copied.join(".git/gitust-repository");
    std::fs::create_dir(&identity).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o700)).unwrap();
    for leaf in ["identity.json", "registration.json"] {
        std::fs::copy(
            secondary.join(".git/gitust-repository").join(leaf),
            identity.join(leaf),
        )
        .unwrap();
    }
    assert!(RepositoryRegistry::load(&state, vec![secondary.clone(), copied]).is_err());
    assert!(RepositoryRegistry::load(&state, vec![secondary.clone(); 32]).is_err());
    let owner = RepositoryRegistry::load(&state, vec![secondary.clone()]).unwrap();
    assert!(GitEngine::new(&secondary).lock_worktree_service().is_err());
    let clone = owner.clone();
    drop(owner);
    assert!(GitEngine::new(&secondary).lock_worktree_service().is_err());
    drop(clone);
    assert!(GitEngine::new(&secondary).lock_worktree_service().is_ok());
}

#[tokio::test]
async fn secondary_only_member_has_no_default_and_private_metadata_failures_are_sanitized() {
    let (_temp, mut state, admin_token, _path, secondary_id) = registered_pair();
    let admin = state.auth_store.authenticate_session(&admin_token).unwrap();
    let primary = state
        .auth_store
        .register_local_repository(&admin, state.repo_id, "primary", TeamId::new(), true, None)
        .unwrap();
    state
        .git_engine
        .complete_repository_registration(primary.binding)
        .unwrap();
    state.repo_access = state
        .auth_store
        .configured_repository_access(state.repo_id, Some(primary.binding))
        .unwrap();
    state.repository_registry = None;
    let secondary_path = _path;
    state.repository_registry =
        Some(RepositoryRegistry::load(&state, vec![secondary_path]).unwrap());
    let team = state
        .auth_store
        .authorize_repository_access(&admin, secondary_id)
        .unwrap()
        .owner_team_id;
    let invite = state
        .auth_store
        .create_invite_link(
            &admin,
            "browser@company.internal",
            EnterpriseRole::Developer,
            vec![team],
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, "browser", "Password!2026")
        .unwrap();
    let token = state
        .auth_store
        .login("browser", "Password!2026")
        .unwrap()
        .raw_session_token;
    let app = build_router(state.clone());
    let (status, body) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["repositories"].as_array().unwrap().len(), 1);
    assert_eq!(value["repositories"][0]["id"], secondary_id.0.to_string());
    assert_eq!(value["repositories"][0]["is_default"], false);
    assert_eq!(
        call(
            &app,
            "GET",
            format!("/api/v1/repos/{}/tree", state.repo_id.0),
            &token,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let db = state.auth_store.persistence_path().unwrap().to_owned();
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        std::fs::write(&db, b"{invalid private metadata").unwrap();
    }));
    let (status, body) = call(
        &app,
        "GET",
        format!("/api/v1/repos/{}/blob?path=README.md", secondary_id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, "repository data is unavailable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_secondary_read_holds_service_ownership_until_blocking_work_finishes() {
    let (_temp, state, token, secondary, id) = registered_pair();
    let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    let release_rx = parking_lot::Mutex::new(release_rx);
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        entered.send(()).unwrap();
        release_rx
            .lock()
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
    }));
    let request = Request::builder()
        .uri(format!("/api/v1/repos/{}/tree", id.0))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let task = tokio::spawn(build_router(state).oneshot(request));
    let reached_hook = tokio::task::spawn_blocking(move || {
        entered_rx.recv_timeout(std::time::Duration::from_secs(10))
    })
    .await
    .unwrap();
    task.abort();
    let cancelled = matches!(task.await, Err(error) if error.is_cancelled());
    let retained = GitEngine::new(&secondary).lock_worktree_service().is_err();
    let released = release.send(());
    assert!(
        reached_hook.is_ok(),
        "repository read never reached the hook"
    );
    assert!(cancelled);
    assert!(
        retained,
        "cancelled blocking read released service ownership"
    );
    assert!(released.is_ok());
    let mut available = false;
    for _ in 0..100 {
        if GitEngine::new(&secondary).lock_worktree_service().is_ok() {
            available = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        available,
        "cancelled read retained service ownership after cleanup"
    );
}
