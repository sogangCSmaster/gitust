use super::tests_repository_visibility::{fixture, user};
use super::*;
use std::sync::Arc;

fn body(id: gitust_core::InviteId) -> serde_json::Value {
    serde_json::json!({"invite_id":id,"target_email":"invited@company.internal","assigned_role":"Developer"})
}
#[tokio::test]
async fn group_invites_reject_unsupported_contexts_and_never_accept_client_chosen_groups() {
    let (_temp, state, token, secondary) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, secondary)
        .unwrap()
        .owner_team_id;
    let member = user(&state, &admin, "member", team);
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "metadata", TeamId::new(), true)
        .unwrap();
    let unconfigured = state
        .auth_store
        .register_local_repository(&admin, RepoId::new(), "unconfigured", team, true, None)
        .unwrap();
    let id = gitust_core::InviteId::new();
    let uri = format!("/api/v1/repos/{}/group-invites", secondary.0);
    let mcp = state
        .auth_store
        .issue_oauth_session(admin.user_id)
        .unwrap()
        .raw_session_token;
    let invitation = state
        .auth_store
        .create_invite_link(
            &admin,
            "inactive-admin@company.internal",
            EnterpriseRole::SiteAdmin,
            vec![],
            None,
        )
        .unwrap();
    let inactive = state
        .auth_store
        .redeem_invite_link(&invitation.raw_token, "inactive-admin", "Password!2026")
        .unwrap();
    let inactive_session = state
        .auth_store
        .login("inactive-admin", "Password!2026")
        .unwrap();
    let inactive_principal = state
        .auth_store
        .authenticate_session(&inactive_session.raw_session_token)
        .unwrap();
    let inactive_pat = state
        .auth_store
        .create_personal_access_token(&inactive_principal, "inactive", None)
        .unwrap();
    state
        .auth_store
        .deactivate_user(&admin, inactive.id)
        .unwrap();
    let before = state.auth_store.list_invites(&admin).unwrap();
    let app = build_router(state.clone());
    let mut extra = body(id);
    extra["team_ids"] = serde_json::json!([team]);
    let mut oversized = body(id);
    oversized["padding"] = serde_json::json!("x".repeat(4096));
    let mut badrole = body(id);
    badrole["assigned_role"] = serde_json::json!("Owner");
    for (path, credential, payload, status) in [
        (uri.clone(), mcp, body(id), StatusCode::UNAUTHORIZED),
        (
            uri.clone(),
            inactive_pat.raw_token,
            body(id),
            StatusCode::FORBIDDEN,
        ),
        (uri.clone(), token.clone(), extra, StatusCode::BAD_REQUEST),
        (
            uri.clone(),
            token.clone(),
            oversized,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (uri.clone(), token.clone(), badrole, StatusCode::BAD_REQUEST),
        (
            uri.clone(),
            token.clone(),
            body(gitust_core::InviteId(uuid::Uuid::nil())),
            StatusCode::BAD_REQUEST,
        ),
        (
            uri.clone(),
            "invalid".into(),
            body(id),
            StatusCode::UNAUTHORIZED,
        ),
        (uri.clone(), member.clone(), body(id), StatusCode::FORBIDDEN),
        (
            format!("/api/v1/repos/{}/group-invites", metadata.id.0),
            token.clone(),
            body(id),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/v1/repos/{}/group-invites", unconfigured.metadata.id.0),
            token.clone(),
            body(id),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/v1/repos/{}/group-invites", RepoId::new().0),
            token.clone(),
            body(id),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    assert_eq!(state.auth_store.list_invites(&admin).unwrap(), before);
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    let value: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert!(value["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["can_invite_to_group"] == true));
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &member, None).await;
    let value: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert!(value["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["can_invite_to_group"] == false));
    let (_temp, legacy, session, _, _) = super::tests_repository_catalog::registered_pair();
    let uri = format!("/api/v1/repos/{}/group-invites", legacy.repo_id.0);
    assert_eq!(
        call(&build_router(legacy), "POST", uri, &session, Some(body(id)))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

struct PendingBody {
    action: Option<Box<dyn FnOnce() + Send>>,
    bytes: Option<bytes::Bytes>,
}
impl http_body::Body for PendingBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if let Some(action) = self.action.take() {
            action();
        }
        std::task::Poll::Ready(
            self.bytes
                .take()
                .map(|bytes| Ok(http_body::Frame::data(bytes))),
        )
    }
}
#[tokio::test]
async fn group_invites_reauthenticate_original_session_and_pat_after_body_and_preparation() {
    for use_pat in [false, true] {
        for in_body in [false, true] {
            let (_temp, state, session, id) = fixture();
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "invite", None)
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
            let bytes = bytes::Bytes::from(body(gitust_core::InviteId::new()).to_string());
            let request_body = if in_body {
                Body::new(PendingBody {
                    action: Some(Box::new(action)),
                    bytes: Some(bytes),
                })
            } else {
                *state.repository_read_hook.lock() = Some(Arc::new(action));
                Body::from(bytes)
            };
            let response = build_router(state.clone())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/v1/repos/{}/group-invites", id.0))
                        .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                        .body(request_body)
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(state
                .auth_store
                .list_invites(
                    &state
                        .auth_store
                        .authenticate_session(
                            &state
                                .auth_store
                                .login("admin", "Password!2026")
                                .unwrap()
                                .raw_session_token
                        )
                        .unwrap()
                )
                .unwrap()
                .is_empty());
        }
    }
}
