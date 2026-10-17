use super::*;
use std::sync::{Arc, Mutex};
struct RevokeBody {
    data: Option<Vec<u8>>,
    revoke: Option<Box<dyn FnOnce() + Send>>,
}
impl http_body::Body for RevokeBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let Some(data) = self.data.take() else {
            return std::task::Poll::Ready(None);
        };
        if let Some(revoke) = self.revoke.take() {
            revoke()
        }
        std::task::Poll::Ready(Some(Ok(http_body::Frame::data(data.into()))))
    }
}
#[tokio::test]
async fn original_session_and_pat_are_rechecked_after_body_validation_and_head_lock_wait() {
    for use_pat in [false, true] {
        for phase in ["body", "validation", "head-lock"] {
            let (_temp, state, session) = fixture();
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "default", None)
                .unwrap();
            let token = if use_pat {
                pat.raw_token.clone()
            } else {
                session
            };
            let store = state.auth_store.clone();
            let credential = token.clone();
            let revoke = move || {
                if use_pat {
                    store
                        .revoke_personal_access_token(&admin, pat.metadata.id)
                        .unwrap();
                } else {
                    store.revoke_session(&admin, &credential).unwrap();
                }
            };
            let original = std::fs::read(state.git_engine.repo_path().join("HEAD")).unwrap();
            let body = serde_json::to_vec(&payload(&state)).unwrap();
            let writer = Arc::new(Mutex::new(None));
            let body = if phase == "body" {
                Body::new(RevokeBody {
                    data: Some(body),
                    revoke: Some(Box::new(revoke)),
                })
            } else {
                if phase == "validation" {
                    *state.repository_read_hook.lock() = Some(Arc::new(revoke));
                } else {
                    let path = state.git_engine.repo_path().to_path_buf();
                    let writer = writer.clone();
                    let revoke = Arc::new(revoke);
                    *state.repository_read_hook.lock() = Some(Arc::new(move || {
                        std::fs::write(path.join("HEAD.lock"), "ref: refs/heads/main\n").unwrap();
                        let path = path.clone();
                        let revoke = revoke.clone();
                        *writer.lock().unwrap() = Some(std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                            revoke();
                            std::fs::rename(path.join("HEAD.lock"), path.join("HEAD")).unwrap();
                        }));
                    }));
                }
                Body::from(body)
            };
            let response = build_router(state.clone())
                .oneshot(
                    Request::builder()
                        .method("PATCH")
                        .uri(format!("/api/v1/repos/{}/default-branch", state.repo_id.0))
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(body)
                        .unwrap(),
                )
                .await
                .unwrap();
            if let Some(writer) = writer.lock().unwrap().take() {
                writer.join().unwrap();
            }
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "phase {phase}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                std::fs::read(state.git_engine.repo_path().join("HEAD")).unwrap(),
                original
            );
        }
    }
}
#[tokio::test]
async fn repo_mutation_busy_and_unresolved_fences_fail_without_touching_head() {
    let (_temp, state, token) = fixture();
    let app = build_router(state.clone());
    let body = payload(&state);
    let guard = state.repo_mutation_lock.clone().lock_owned().await;
    assert_eq!(
        call(
            &app,
            "PATCH",
            format!("/api/v1/repos/{}/default-branch", state.repo_id.0),
            &token,
            Some(body.clone())
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    drop(guard);
    // A service lease is not replaced by an HTTP request; an unresolved fence must deny the mutation.
    let wt = state
        .git_engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            state.repo_id,
            gitust_core::AgentKind::CodexSwarm,
            "agent/fence",
            "main",
            "fence",
        )
        .unwrap();
    let fence = state
        .git_engine
        .begin_execution_fence(&wt.worktree)
        .unwrap();
    drop(fence);
    assert_ne!(
        call(
            &app,
            "PATCH",
            format!("/api/v1/repos/{}/default-branch", state.repo_id.0),
            &token,
            Some(body)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        git(state.git_engine.repo_path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
}
#[tokio::test]
async fn default_change_rejects_mismatched_pinned_registration_without_exposing_storage() {
    let (_temp, mut state, token) = fixture();
    let before = std::fs::read(state.git_engine.repo_path().join("HEAD")).unwrap();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let other = state
        .auth_store
        .register_local_repository(&admin, RepoId::new(), "other", TeamId::new(), true, None)
        .unwrap();
    state.repo_access.registration = Some(other.binding);
    let body = payload(&state);
    let response = build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/repos/{}/default-branch", state.repo_id.0))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let data = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(data.as_ref(), b"repository data is unavailable");
    assert_eq!(
        std::fs::read(state.git_engine.repo_path().join("HEAD")).unwrap(),
        before
    );
}
