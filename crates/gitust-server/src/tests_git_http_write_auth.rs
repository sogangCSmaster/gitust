use super::tests_git_http_secondary::{oracle, request};
use super::tests_git_http_write::{body, fixture, route};
use super::*;
use std::sync::{Arc, Mutex};

fn member(state: &AppState, token: &str, name: &str, role: EnterpriseRole, team: TeamId) -> String {
    let admin = state.auth_store.authenticate_session(token).unwrap();
    let invite = state
        .auth_store
        .create_invite_link(
            &admin,
            &format!("{name}@company.internal"),
            role,
            vec![team],
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, name, "Password!2026")
        .unwrap();
    state
        .auth_store
        .login(name, "Password!2026")
        .unwrap()
        .raw_session_token
}
#[tokio::test]
async fn ordinary_secondary_push_uses_current_roles_acl_and_never_promotes_read_only_or_unhosted() {
    let (_temp, state, session, bare, id, readonly) = fixture();
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, id)
        .unwrap()
        .owner_team_id;
    let maintainer = member(
        &state,
        &session,
        "maintainer",
        EnterpriseRole::Maintainer,
        team,
    );
    let developer = member(
        &state,
        &session,
        "developer",
        EnterpriseRole::Developer,
        team,
    );
    let outsider = member(
        &state,
        &session,
        "outsider",
        EnterpriseRole::Maintainer,
        TeamId::new(),
    );
    let mcp = state
        .auth_store
        .issue_oauth_session(admin.user_id)
        .unwrap()
        .raw_session_token;
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "metadata", team, true)
        .unwrap()
        .id;
    let app = build_router(state.clone());
    let before = oracle(&bare, &["show-ref", "--head"]);
    for (token, status) in [
        (&developer, StatusCode::FORBIDDEN),
        (&outsider, StatusCode::FORBIDDEN),
        (&mcp, StatusCode::UNAUTHORIZED),
        (&String::new(), StatusCode::UNAUTHORIZED),
    ] {
        for post in [false, true] {
            let data = if post {
                Body::from(body(&state, id, &["refs/heads/denied"]))
            } else {
                Body::empty()
            };
            let result = request(
                &app,
                if post { "POST" } else { "GET" },
                route(id, post),
                token,
                data,
            )
            .await;
            assert_eq!(result.status(), status);
            assert_eq!(result.headers()[header::CACHE_CONTROL], "no-store");
        }
    }
    for target in [readonly, metadata, RepoId::new()] {
        for post in [false, true] {
            assert_eq!(
                request(
                    &app,
                    if post { "POST" } else { "GET" },
                    route(target, post),
                    &session,
                    Body::empty()
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
    }
    assert_eq!(oracle(&bare, &["show-ref", "--head"]), before);
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &developer, None).await;
    let rows: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert!(rows["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["can_push"] == false));
    assert_eq!(
        request(
            &app,
            "POST",
            route(id, true),
            &maintainer,
            Body::from(body(&state, id, &["refs/heads/maintainer"]))
        )
        .await
        .status(),
        StatusCode::OK
    );
    oracle(&bare, &["rev-parse", "refs/heads/maintainer"]);
    let expected = state.read_repository(id).unwrap().access;
    state
        .auth_store
        .change_registered_repository_visibility(&session, admin.user_id, expected, true, false)
        .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            route(id, true),
            &outsider,
            Body::from(body(&state, id, &["refs/heads/shared-maintainer"]))
        )
        .await
        .status(),
        StatusCode::OK
    );
    oracle(&bare, &["rev-parse", "refs/heads/shared-maintainer"]);
    state
        .auth_store
        .change_registered_repository_visibility(&session, admin.user_id, expected, false, true)
        .unwrap();
    assert_eq!(
        request(&app, "GET", route(id, false), &outsider, Body::empty())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let actor = state.auth_store.authenticate_session(&maintainer).unwrap();
    state
        .auth_store
        .deactivate_user(&admin, actor.user_id)
        .unwrap();
    assert_eq!(
        request(&app, "GET", route(id, false), &maintainer, Body::empty())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}
struct CallbackBody {
    callback: Option<Box<dyn FnOnce() + Send>>,
    bytes: Vec<u8>,
}
impl http_body::Body for CallbackBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if let Some(callback) = self.callback.take() {
            callback();
            return std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes::Bytes::from(
                std::mem::take(&mut self.bytes),
            )))));
        }
        std::task::Poll::Ready(None)
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_secondary_rechecks_original_session_and_pat_after_body_lock_wait_validation_and_advertisement(
) {
    for pat_kind in [false, true] {
        for phase in 0..4 {
            let (_temp, state, session, bare, id, _) = fixture();
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "ordinary", None)
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
            let before = oracle(&bare, &["show-ref", "--head"]);
            let data = body(&state, id, &["refs/heads/revoked"]);
            let guard = if phase == 1 {
                Some(
                    state
                        .read_repository(id)
                        .unwrap()
                        .mutation
                        .clone()
                        .lock_owned()
                        .await,
                )
            } else {
                None
            };
            let mut held_revoke = None;
            let (entered, entered_rx) = tokio::sync::oneshot::channel();
            let payload = if phase == 0 {
                Body::new(CallbackBody {
                    callback: Some(Box::new(revoke)),
                    bytes: data,
                })
            } else {
                if phase == 1 {
                    let tx = Mutex::new(Some(entered));
                    *state.git_write_before_lock_hook.lock() = Some(Arc::new(move || {
                        tx.lock().unwrap().take().unwrap().send(()).unwrap()
                    }));
                    held_revoke = Some(Box::new(revoke) as Box<dyn FnOnce() + Send>);
                } else {
                    let once = Mutex::new(Some(revoke));
                    *state.repository_read_hook.lock() =
                        Some(Arc::new(move || once.lock().unwrap().take().unwrap()()));
                }
                if phase == 3 {
                    Body::empty()
                } else {
                    Body::from(data)
                }
            };
            let app = build_router(state.clone());
            let uri = route(id, phase != 3);
            let owned = token.clone();
            let task = tokio::spawn(async move {
                request(
                    &app,
                    if phase == 3 { "GET" } else { "POST" },
                    uri,
                    &owned,
                    payload,
                )
                .await
            });
            if phase == 1 {
                tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    !task.is_finished(),
                    "request must wait on its selected repository mutex"
                );
                held_revoke.take().unwrap()();
                drop(guard);
            }
            let response = task.await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(oracle(&bare, &["show-ref", "--head"]), before);
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_secondary_receive_keeps_only_its_own_mutation_and_service_holders() {
    let (_temp, state, session, bare, id, readonly) = fixture();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let entered = Mutex::new(Some(entered));
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }));
    let payload = Body::from(body(&state, id, &["refs/heads/cancelled-response"]));
    let app = build_router(state.clone());
    let uri = route(id, true);
    let task = tokio::spawn(async move { request(&app, "POST", uri, &session, payload).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(state.repo_mutation_lock.try_lock().is_ok());
    assert!(state
        .read_repository(readonly)
        .unwrap()
        .mutation
        .try_lock()
        .is_ok());
    assert!(state
        .read_repository(id)
        .unwrap()
        .mutation
        .try_lock()
        .is_err());
    task.abort();
    drop(task);
    drop(state);
    let independent = GitEngine::new(&bare);
    assert!(independent.lock_worktree_service().is_err());
    release.send(()).unwrap();
    let mut available = false;
    for _ in 0..100 {
        if let Ok(owner) = independent.lock_worktree_service() {
            drop(owner);
            available = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        available,
        "cancelled response must not leak selected service ownership"
    );
    oracle(&bare, &["rev-parse", "refs/heads/cancelled-response"]);
    oracle(&bare, &["fsck", "--full", "--strict"]);
}

#[tokio::test]
async fn primary_receive_advertisement_rechecks_original_session_and_pat_after_git() {
    let mut statuses = Vec::new();
    for pat_kind in [false, true] {
        let (_temp, state, session, _bare, _, _) = fixture();
        let admin = state.auth_store.authenticate_session(&session).unwrap();
        let pat = state
            .auth_store
            .create_personal_access_token(&admin, "advertisement", None)
            .unwrap();
        let token = if pat_kind { pat.raw_token } else { session };
        let original = token.clone();
        let store = state.auth_store.clone();
        *state.repository_read_hook.lock() = Some(Arc::new(move || {
            if pat_kind {
                store
                    .revoke_personal_access_token(&admin, pat.metadata.id)
                    .unwrap();
            } else {
                store.revoke_session(&admin, &original).unwrap();
            }
        }));
        let app = build_router(state.clone());
        let result = request(
            &app,
            "GET",
            route(state.repo_id, false),
            &token,
            Body::empty(),
        )
        .await;
        statuses.push(result.status());
    }
    assert_eq!(statuses, [StatusCode::UNAUTHORIZED; 2]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_secondary_rechecks_current_acl_after_selected_mutation_lock_wait() {
    let (_temp, state, session, bare, id, _) = fixture();
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let outsider = member(
        &state,
        &session,
        "late-outsider",
        EnterpriseRole::Maintainer,
        TeamId::new(),
    );
    let access = state.read_repository(id).unwrap().access;
    state
        .auth_store
        .change_registered_repository_visibility(&session, admin.user_id, access, true, false)
        .unwrap();
    let held = state
        .read_repository(id)
        .unwrap()
        .mutation
        .clone()
        .lock_owned()
        .await;
    let (entered, observed) = tokio::sync::oneshot::channel();
    let entered = Mutex::new(Some(entered));
    *state.git_write_before_lock_hook.lock() = Some(Arc::new(move || {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
    }));
    let before = oracle(&bare, &["show-ref", "--head"]);
    let payload = Body::from(body(&state, id, &["refs/heads/late-acl"]));
    let app = build_router(state.clone());
    let task =
        tokio::spawn(
            async move { request(&app, "POST", route(id, true), &outsider, payload).await },
        );
    tokio::time::timeout(std::time::Duration::from_secs(5), observed)
        .await
        .unwrap()
        .unwrap();
    assert!(!task.is_finished());
    state
        .auth_store
        .change_registered_repository_visibility(&session, admin.user_id, access, false, true)
        .unwrap();
    drop(held);
    assert_eq!(task.await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(oracle(&bare, &["show-ref", "--head"]), before);
}
