use super::tests_repository_visibility::{fixture, user};
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[tokio::test]
async fn group_member_contexts_roles_and_strict_requests_fail_closed() {
    let (_temp, state, token, id) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let row = state
        .auth_store
        .authorize_repository_access(&admin, id)
        .unwrap();
    let member = user(&state, &admin, "member", row.owner_team_id);
    let mcp = state
        .auth_store
        .issue_oauth_session(admin.user_id)
        .unwrap()
        .raw_session_token;
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "metadata", TeamId::new(), true)
        .unwrap();
    let registered = state
        .auth_store
        .register_local_repository(
            &admin,
            RepoId::new(),
            "unconfigured",
            TeamId::new(),
            true,
            None,
        )
        .unwrap();
    let app = build_router(state.clone());
    for (path, credential, status) in [
        (
            format!("/api/v1/repos/{}/group-members?username=admin", id.0),
            "invalid".to_owned(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            format!("/api/v1/repos/{}/group-members?username=admin", id.0),
            member.clone(),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/api/v1/repos/{}/group-members?username=admin", id.0),
            mcp,
            StatusCode::UNAUTHORIZED,
        ),
        (
            format!(
                "/api/v1/repos/{}/group-members?username=admin",
                metadata.id.0
            ),
            token.clone(),
            StatusCode::NOT_FOUND,
        ),
        (
            format!(
                "/api/v1/repos/{}/group-members?username=admin",
                registered.metadata.id.0
            ),
            token.clone(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    assert!(
        serde_json::from_str::<serde_json::Value>(&catalog).unwrap()["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["can_manage_group_members"] == true)
    );
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &member, None).await;
    assert!(
        serde_json::from_str::<serde_json::Value>(&catalog).unwrap()["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["can_manage_group_members"] == false)
    );
    for body in [
        serde_json::json!({"expected_membership_version":"0","is_member":true,"team_ids":[]}),
        serde_json::json!({"expected_membership_version":"0","is_member":true,"padding":"x".repeat(4096)}),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!(
                        "/api/v1/repos/{}/group-members/{}",
                        id.0, admin.user_id.0
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
        ));
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let target = state.auth_store.authenticate_session(&member).unwrap();
    state
        .auth_store
        .deactivate_user(&admin, target.user_id)
        .unwrap();
    let (status, body) = call(
        &app,
        "GET",
        format!("/api/v1/repos/{}/group-members?username=member", id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["status"] == "suspended");
    let path = format!("/api/v1/repos/{}/group-members/{}", id.0, target.user_id.0);
    let (status, body) = call(
        &app,
        "PATCH",
        path.clone(),
        &token,
        Some(serde_json::json!({"expected_membership_version":"0","is_member":true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["code"]
            == "group_member_inactive"
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            path,
            &token,
            Some(serde_json::json!({"expected_membership_version":"0","is_member":false}))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_temp, legacy, token, _, _) = super::tests_repository_catalog::registered_pair();
    assert_eq!(
        call(
            &build_router(legacy.clone()),
            "GET",
            format!(
                "/api/v1/repos/{}/group-members?username=admin",
                legacy.repo_id.0
            ),
            &token,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn group_member_reads_and_locked_writes_recheck_exact_original_credentials() {
    for use_pat in [false, true] {
        for write in [false, true] {
            let (temp, state, session, id) = fixture();
            let store = &state.auth_store;
            let admin = store.authenticate_session(&session).unwrap();
            let pat = store
                .create_personal_access_token(&admin, "origin", None)
                .unwrap();
            let credential = if use_pat {
                pat.raw_token.clone()
            } else {
                session.clone()
            };
            let db = store.persistence_path().unwrap().to_owned();
            // A second legitimate auth writer supplies a fully persisted revoked checkpoint.
            // No token/hash is fabricated or printed, and the original fixture is self-owned.
            let copy = temp.path().join("revoked-auth.json");
            std::fs::copy(&db, &copy).unwrap();
            let revoked = AuthStore::open_persistent(&copy).unwrap();
            if use_pat {
                revoked
                    .revoke_personal_access_token(&admin, pat.metadata.id)
                    .unwrap();
            } else {
                revoked.revoke_session(&admin, &session).unwrap();
            }
            let bytes = std::fs::read(&copy).unwrap();
            let expected_bytes = bytes.clone();
            let completed = Arc::new(AtomicBool::new(false));
            let held = Arc::new(AtomicBool::new(false));
            let worker = Arc::new(Mutex::new(None));
            let (done, locked, join) = (completed.clone(), held.clone(), worker.clone());
            let hook_db = db.clone();
            *state.repository_read_hook.lock() = Some(Arc::new(move || {
                let db = &hook_db;
                let mut name = db.as_os_str().to_owned();
                name.push(".lock");
                let lock = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(std::path::PathBuf::from(name))
                    .unwrap();
                lock.lock().unwrap();
                let (tx, rx) = std::sync::mpsc::sync_channel(0);
                let db = db.clone();
                let bytes = bytes.clone();
                let done = done.clone();
                let held = locked.clone();
                *join.lock().unwrap() = Some(std::thread::spawn(move || {
                    tx.send(()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    assert!(
                        !done.load(Ordering::SeqCst),
                        "request must remain pending while auth lock is held"
                    );
                    let replacement = db.with_extension("revoked");
                    std::fs::write(&replacement, &bytes).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(
                            &replacement,
                            std::fs::Permissions::from_mode(0o600),
                        )
                        .unwrap();
                    }
                    std::fs::File::open(&replacement)
                        .unwrap()
                        .sync_all()
                        .unwrap();
                    std::fs::rename(replacement, &db).unwrap();
                    std::fs::File::open(db.parent().unwrap())
                        .unwrap()
                        .sync_all()
                        .unwrap();
                    held.store(true, Ordering::SeqCst);
                    lock.unlock().unwrap();
                }));
                rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
            }));
            if !write {
                // Publish the legitimate revoked checkpoint before the read transaction begins.
                *state.repository_read_hook.lock() = Some(Arc::new({
                    let db = db.clone();
                    let bytes = expected_bytes.clone();
                    move || {
                        std::fs::write(&db, &bytes).unwrap();
                    }
                }));
            }
            let path = if write {
                format!("/api/v1/repos/{}/group-members/{}", id.0, admin.user_id.0)
            } else {
                format!("/api/v1/repos/{}/group-members?username=admin", id.0)
            };
            let (status, _) = call(
                &build_router(state.clone()),
                if write { "PATCH" } else { "GET" },
                path,
                &credential,
                if write {
                    Some(serde_json::json!({"expected_membership_version":"0","is_member":true}))
                } else {
                    None
                },
            )
            .await;
            completed.store(true, Ordering::SeqCst);
            if write {
                worker.lock().unwrap().take().unwrap().join().unwrap();
                assert!(held.load(Ordering::SeqCst));
            }
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(std::fs::read(&db).unwrap() == expected_bytes);
        }
    }
}

#[tokio::test]
async fn cancelled_group_lookup_retains_service_ownership_until_blocking_completion() {
    let (_temp, state, token, secondary, id) = super::tests_repository_catalog::registered_pair();
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
        .uri(format!(
            "/api/v1/repos/{}/group-members?username=admin",
            id.0
        ))
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
