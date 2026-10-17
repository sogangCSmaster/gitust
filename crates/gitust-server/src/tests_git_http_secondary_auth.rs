use super::tests_git_http_secondary::{fixture, oracle, request};
use super::tests_repository_visibility::user;
use super::*;
use std::sync::Arc;

fn route(id: RepoId, post: bool) -> String {
    format!(
        "/git/{}.git/{}",
        id.0,
        if post {
            "git-upload-pack"
        } else {
            "info/refs?service=git-upload-pack"
        }
    )
}
fn upload(state: &AppState, id: RepoId) -> Body {
    let sha = state
        .read_repository(id)
        .unwrap()
        .engine
        .inspect_head_identity()
        .unwrap()
        .head_sha
        .0;
    let want = format!("want {sha}\n");
    Body::from(format!("{:04x}{want}00000009done\n", want.len() + 4))
}
#[tokio::test]
async fn secondary_reads_enforce_current_privacy_accounts_purpose_and_hosted_inventory() {
    let (temp, state, session, _path, id) = fixture();
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, id)
        .unwrap()
        .owner_team_id;
    let member = user(&state, &admin, "secondary-member", team);
    let outside = user(&state, &admin, "outside", TeamId::new());
    let oauth = state.auth_store.issue_oauth_session(admin.user_id).unwrap();
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "metadata", team, false)
        .unwrap();
    let extra = temp.path().join("unconfigured");
    init_temp_repo(&extra);
    let engine = GitEngine::new(&extra);
    let extra_id = engine.load_or_create_repository_id().unwrap();
    let binding = state
        .auth_store
        .register_local_repository(&admin, extra_id, "unconfigured", team, false, None)
        .unwrap();
    engine
        .complete_repository_registration(binding.binding)
        .unwrap();
    let app = build_router(state.clone());
    for post in [false, true] {
        let method = if post { "POST" } else { "GET" };
        for (token, expected) in [
            (&member, StatusCode::OK),
            (&outside, StatusCode::FORBIDDEN),
            (&session, StatusCode::OK),
            (&oauth.raw_session_token, StatusCode::UNAUTHORIZED),
        ] {
            let body = if post {
                upload(&state, id)
            } else {
                Body::empty()
            };
            assert_eq!(
                request(&app, method, route(id, post), token, body)
                    .await
                    .status(),
                expected
            );
        }
        let denied = request(&app, method, route(id, post), "", Body::empty()).await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert!(denied.headers().contains_key(header::WWW_AUTHENTICATE));
        for unknown in [metadata.id, extra_id, RepoId::new()] {
            assert_eq!(
                request(&app, method, route(unknown, post), &session, Body::empty())
                    .await
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
    }
    for private in [false, true] {
        assert_eq!(
            call(
                &app,
                "PATCH",
                format!("/api/v1/repos/{}/visibility", id.0),
                &session,
                Some(serde_json::json!({"expected_is_private":!private,"is_private":private}))
            )
            .await
            .0,
            StatusCode::OK
        );
        let expected = if private {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        };
        assert_eq!(
            request(&app, "GET", route(id, false), &outside, Body::empty())
                .await
                .status(),
            expected
        );
        assert_eq!(
            request(&app, "POST", route(id, true), &outside, upload(&state, id))
                .await
                .status(),
            expected
        );
        for (method, suffix) in [
            ("GET", "info/refs?service=git-receive-pack"),
            ("POST", "git-receive-pack"),
        ] {
            for token in [&session, &member, &outside] {
                assert_eq!(
                    request(
                        &app,
                        method,
                        format!("/git/{}.git/{suffix}", id.0),
                        token,
                        Body::from("0000")
                    )
                    .await
                    .status(),
                    StatusCode::NOT_FOUND
                );
            }
        }
    }
    let actor = state.auth_store.authenticate_session(&member).unwrap();
    state
        .auth_store
        .deactivate_user(&admin, actor.user_id)
        .unwrap();
    assert_eq!(
        request(&app, "GET", route(id, false), &member, Body::empty())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let before = oracle(state.git_engine.repo_path(), &["show-ref", "--head"]);
    assert_eq!(
        request(
            &app,
            "GET",
            "/git/secondary.git/info/refs?service=git-upload-pack".into(),
            &session,
            Body::empty()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        oracle(state.git_engine.repo_path(), &["show-ref", "--head"]),
        before
    );
}
struct CallbackBody {
    action: Option<Box<dyn FnOnce() + Send>>,
    data: Vec<u8>,
}
impl http_body::Body for CallbackBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if let Some(action) = self.action.take() {
            action();
            return std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes::Bytes::from(
                std::mem::take(&mut self.data),
            )))));
        }
        std::task::Poll::Ready(None)
    }
}
#[tokio::test]
async fn hosted_git_reads_reauthenticate_original_sessions_and_pats_after_body_and_git_output() {
    for pat_kind in [false, true] {
        for phase in 0..3 {
            let (_temp, state, session, _path, id) = fixture();
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "git read", None)
                .unwrap();
            let token = if pat_kind {
                pat.raw_token.clone()
            } else {
                session
            };
            let store = state.auth_store.clone();
            let original = token.clone();
            let revoke = move || {
                if pat_kind {
                    store
                        .revoke_personal_access_token(&admin, pat.metadata.id)
                        .unwrap();
                } else {
                    store.revoke_session(&admin, &original).unwrap();
                }
            };
            let post = phase != 1;
            let body = if phase == 0 {
                Body::new(CallbackBody {
                    action: Some(Box::new(revoke)),
                    data: b"0000".to_vec(),
                })
            } else {
                let once = std::sync::Mutex::new(Some(revoke));
                *state.repository_read_hook.lock() =
                    Some(Arc::new(move || once.lock().unwrap().take().unwrap()()));
                if post {
                    upload(&state, id)
                } else {
                    Body::empty()
                }
            };
            let response = request(
                &build_router(state),
                if post { "POST" } else { "GET" },
                route(id, post),
                &token,
                body,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let payload = response.into_body().collect().await.unwrap().to_bytes();
            assert!(!payload.starts_with(b"001e# service"));
        }
    }
}
#[tokio::test]
async fn hosted_git_reads_recheck_current_acl_after_body_and_completed_output() {
    for phase in 0..3 {
        let (_temp, state, session, _path, id) = fixture();
        let admin = state.auth_store.authenticate_session(&session).unwrap();
        let outside = user(&state, &admin, "acl-outsider", TeamId::new());
        let expected = state.read_repository(id).unwrap().access;
        state
            .auth_store
            .change_registered_repository_visibility(&session, admin.user_id, expected, true, false)
            .unwrap();
        let store = state.auth_store.clone();
        let revoke = move || {
            store
                .change_registered_repository_visibility(
                    &session,
                    admin.user_id,
                    expected,
                    false,
                    true,
                )
                .unwrap();
        };
        let post = phase != 1;
        let body = if phase == 0 {
            Body::new(CallbackBody {
                action: Some(Box::new(revoke)),
                data: b"0000".to_vec(),
            })
        } else {
            let once = std::sync::Mutex::new(Some(revoke));
            *state.repository_read_hook.lock() =
                Some(Arc::new(move || once.lock().unwrap().take().unwrap()()));
            if post {
                upload(&state, id)
            } else {
                Body::empty()
            }
        };
        let response = request(
            &build_router(state),
            if post { "POST" } else { "GET" },
            route(id, post),
            &outside,
            body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_hosted_read_retains_secondary_service_ownership_until_blocking_completion() {
    let (_temp, state, session, path, id) = fixture();
    let (entered_rx, release_tx) = {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let entered = std::sync::Mutex::new(Some(entered_tx));
        let release = std::sync::Mutex::new(release_rx);
        *state.repository_read_hook.lock() = Some(Arc::new(move || {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }));
        (entered_rx, release_tx)
    };
    let app = build_router(state.clone());
    let request = Request::builder()
        .uri(route(id, false))
        .header(header::AUTHORIZATION, format!("Bearer {session}"))
        .body(Body::empty())
        .unwrap();
    let task = tokio::spawn(async move { app.oneshot(request).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    drop(task);
    drop(state);
    let external = GitEngine::new(path);
    assert!(external.lock_worktree_service().is_err());
    release_tx.send(()).unwrap();
    let mut released = false;
    for _ in 0..100 {
        if let Ok(guard) = external.lock_worktree_service() {
            drop(guard);
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        released,
        "read service guard remained after operation completed"
    );
}
