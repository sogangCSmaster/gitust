use super::tests_repository_catalog::registered_pair;
use super::tests_repository_visibility::{fixture, user};
use super::*;
use std::sync::Arc;

#[tokio::test]
async fn visibility_denies_unsupported_contexts_unknown_fields_and_oversized_bodies() {
    let (_temp, state, token, second) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "unhosted", TeamId::new(), true)
        .unwrap();
    let unconfigured = state
        .auth_store
        .register_local_repository(
            &admin,
            RepoId::new(),
            "registered-unconfigured",
            TeamId::new(),
            true,
            None,
        )
        .unwrap();
    let member = user(&state, &admin, "member", metadata.owner_team_id);
    let app = build_router(state.clone());
    let target = format!("/api/v1/repos/{}/visibility", second.0);
    for (uri, credential, body, expected) in [
        (
            target.clone(),
            token.as_str(),
            serde_json::json!({"expected_is_private":true,"is_private":false,"name":"unexpected"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            target.clone(),
            token.as_str(),
            serde_json::json!({"is_private":false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            target.clone(),
            token.as_str(),
            serde_json::json!({"expected_is_private":true,"is_private":"false"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            target.clone(),
            token.as_str(),
            serde_json::json!({"expected_is_private":true,"is_private":false,"padding":"x".repeat(4096)}),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            target.clone(),
            member.as_str(),
            serde_json::json!({"expected_is_private":true,"is_private":false}),
            StatusCode::FORBIDDEN,
        ),
        (
            target.clone(),
            "invalid",
            serde_json::json!({"expected_is_private":true,"is_private":false}),
            StatusCode::UNAUTHORIZED,
        ),
        (
            format!("/api/v1/repos/{}/visibility", metadata.id.0),
            token.as_str(),
            serde_json::json!({"expected_is_private":true,"is_private":false}),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/v1/repos/{}/visibility", unconfigured.metadata.id.0),
            token.as_str(),
            serde_json::json!({"expected_is_private":true,"is_private":false}),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/v1/repos/{}/visibility", RepoId::new().0),
            token.as_str(),
            serde_json::json!({"expected_is_private":true,"is_private":false}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    assert!(
        state
            .auth_store
            .authorize_repository_access(&admin, second)
            .unwrap()
            .is_private
    );
    let (_temp, legacy, credential, _, _) = registered_pair();
    let (_, catalog) = call(
        &build_router(legacy.clone()),
        "GET",
        "/api/v1/repos".into(),
        &credential,
        None,
    )
    .await;
    let catalog: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    let row = catalog["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == legacy.repo_id.0.to_string())
        .unwrap();
    assert!(row.get("is_private").is_none());
    assert_eq!(row["can_change_visibility"], false);
    assert_eq!(
        call(
            &build_router(legacy.clone()),
            "PATCH",
            format!("/api/v1/repos/{}/visibility", legacy.repo_id.0),
            &credential,
            Some(serde_json::json!({"expected_is_private":true,"is_private":false}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

struct RevokeBody {
    action: Option<Box<dyn FnOnce() + Send>>,
}
impl http_body::Body for RevokeBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if let Some(action) = self.action.take() {
            action();
            return std::task::Poll::Ready(Some(Ok(http_body::Frame::data(
                bytes::Bytes::from_static(br#"{"expected_is_private":true,"is_private":false}"#),
            ))));
        }
        std::task::Poll::Ready(None)
    }
}
#[tokio::test]
async fn visibility_rechecks_sessions_and_pats_after_body_and_before_durable_mutation() {
    for use_pat in [false, true] {
        for after_body in [false, true] {
            let (_temp, state, session, id) = fixture();
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "visibility", None)
                .unwrap();
            let credential = if use_pat {
                pat.raw_token.clone()
            } else {
                session.clone()
            };
            let store = state.auth_store.clone();
            let revoke = credential.clone();
            let action = move || {
                if use_pat {
                    store
                        .revoke_personal_access_token(&admin, pat.metadata.id)
                        .unwrap();
                } else {
                    store.revoke_session(&admin, &revoke).unwrap();
                }
            };
            let body = if after_body {
                let action = std::sync::Mutex::new(Some(action));
                *state.repository_read_hook.lock() = Some(Arc::new(move || {
                    action.lock().unwrap().take().unwrap()();
                }));
                Body::from(r#"{"expected_is_private":true,"is_private":false}"#)
            } else {
                Body::new(RevokeBody {
                    action: Some(Box::new(action)),
                })
            };
            let response = build_router(state.clone())
                .oneshot(
                    Request::builder()
                        .method("PATCH")
                        .uri(format!("/api/v1/repos/{}/visibility", id.0))
                        .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                        .body(body)
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let next = state.auth_store.login("admin", "Password!2026").unwrap();
            let admin = state
                .auth_store
                .authenticate_session(&next.raw_session_token)
                .unwrap();
            assert!(
                state
                    .auth_store
                    .authorize_repository_access(&admin, id)
                    .unwrap()
                    .is_private
            );
        }
    }
}
