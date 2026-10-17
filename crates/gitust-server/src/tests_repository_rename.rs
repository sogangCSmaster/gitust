use super::tests_repository_catalog::registered_pair;
use super::*;
use crate::repository_registry::RepositoryRegistry;
use std::sync::Arc;
fn fixture() -> (tempfile::TempDir, AppState, String, RepoId) {
    let (temp, mut state, token, secondary, id) = registered_pair();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let row = state
        .auth_store
        .register_local_repository(&admin, state.repo_id, "primary", TeamId::new(), true, None)
        .unwrap();
    state
        .git_engine
        .complete_repository_registration(row.binding)
        .unwrap();
    state.repo_access = state
        .auth_store
        .configured_repository_access(state.repo_id, Some(row.binding))
        .unwrap();
    state.repository_registry = None;
    state.repository_registry = Some(RepositoryRegistry::load(&state, vec![secondary]).unwrap());
    (temp, state, token, id)
}
#[tokio::test]
async fn configured_primary_and_secondary_rename_only_the_display_name_and_report_capability() {
    let (_temp, state, token, secondary_id) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "rename", None)
        .unwrap();
    let app = build_router(state.clone());
    for (id, credential) in [
        (state.repo_id, token.as_str()),
        (secondary_id, pat.raw_token.as_str()),
    ] {
        let before = state
            .auth_store
            .authorize_repository_access(&admin, id)
            .unwrap();
        let context = state.read_repository(id).unwrap();
        let head = context.engine.inspect_head_identity().unwrap().head_sha;
        let path = context
            .engine
            .repository_common_directory()
            .unwrap()
            .join("gitust-repository/registration.json");
        let receipt = std::fs::read(&path).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/v1/repos/{}", id.0))
                    .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":" renamed "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"id":id,"name":"renamed"})
        );
        let mut expected = before;
        expected.name = "renamed".into();
        assert_eq!(
            state
                .auth_store
                .authorize_repository_access(&admin, id)
                .unwrap(),
            expected
        );
        assert_eq!(
            context.engine.inspect_head_identity().unwrap().head_sha,
            head
        );
        assert_eq!(std::fs::read(path).unwrap(), receipt);
    }
    let (summary_status, summary_body) =
        call(&app, "GET", "/api/v1/repo".into(), &token, None).await;
    assert_eq!(summary_status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&summary_body).unwrap()["name"],
        "renamed"
    );
    let (status, body) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(rows["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["can_rename"] == true && row["name"] == "renamed"));
}
#[tokio::test]
async fn rename_rejects_nonadmin_legacy_unhosted_invalid_and_oversized_requests() {
    let (_temp, state, token, secondary_id) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, secondary_id)
        .unwrap()
        .owner_team_id;
    let invite = state
        .auth_store
        .create_invite_link(
            &admin,
            "member@company.internal",
            EnterpriseRole::Developer,
            vec![team],
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, "member", "Password!2026")
        .unwrap();
    let member = state
        .auth_store
        .login("member", "Password!2026")
        .unwrap()
        .raw_session_token;
    let metadata = state
        .auth_store
        .create_team_repository(&admin, "metadata", team, true)
        .unwrap();
    let app = build_router(state.clone());
    for (id, credential, payload, status) in [
        (
            secondary_id,
            member.as_str(),
            serde_json::json!({"name":"no"}),
            StatusCode::FORBIDDEN,
        ),
        (
            secondary_id,
            "invalid",
            serde_json::json!({"name":"no"}),
            StatusCode::UNAUTHORIZED,
        ),
        (
            RepoId::new(),
            token.as_str(),
            serde_json::json!({"name":"no"}),
            StatusCode::NOT_FOUND,
        ),
        (
            metadata.id,
            token.as_str(),
            serde_json::json!({"name":"no"}),
            StatusCode::NOT_FOUND,
        ),
        (
            secondary_id,
            token.as_str(),
            serde_json::json!({"name":"no","is_private":false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            secondary_id,
            token.as_str(),
            serde_json::json!({"name":"../bad"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            secondary_id,
            token.as_str(),
            serde_json::json!({"name":"x".repeat(4096)}),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        assert_eq!(
            call(
                &app,
                "PATCH",
                format!("/api/v1/repos/{}", id.0),
                credential,
                Some(payload)
            )
            .await
            .0,
            status
        );
    }
    let (_, body) = call(&app, "GET", "/api/v1/repos".into(), &member, None).await;
    assert!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["can_rename"] == false)
    );
    let (_temp, legacy, legacy_token, _, _) = registered_pair();
    assert_eq!(
        call(
            &build_router(legacy.clone()),
            "PATCH",
            format!("/api/v1/repos/{}", legacy.repo_id.0),
            &legacy_token,
            Some(serde_json::json!({"name":"no"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn slow_catalog_returns_the_latest_name_without_losing_acl_or_credential_checks() {
    let (_temp, state, token, secondary_id) = fixture();
    let store = state.auth_store.clone();
    let admin = store.authenticate_session(&token).unwrap();
    let access = state.read_repository(secondary_id).unwrap().access;
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        store
            .rename_registered_repository(&admin, access, "latest-name")
            .unwrap();
    }));
    let (status, body) = call(
        &build_router(state),
        "GET",
        "/api/v1/repos".into(),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == secondary_id.0.to_string())
            .unwrap()["name"],
        "latest-name"
    );
}
#[tokio::test]
async fn rename_reauthenticates_original_sessions_and_pats_after_the_request_body() {
    for use_pat in [false, true] {
        let (_temp, state, session, id) = fixture();
        let admin = state.auth_store.authenticate_session(&session).unwrap();
        let pat = state
            .auth_store
            .create_personal_access_token(&admin, "rename", None)
            .unwrap();
        let token = if use_pat {
            pat.raw_token
        } else {
            session.clone()
        };
        let store = state.auth_store.clone();
        let credential = token.clone();
        let before = store.authorize_repository_access(&admin, id).unwrap();
        let body = RevokeBody {
            data: Some(br#"{"name":"not-authorized"}"#.to_vec()),
            revoke: Some(Box::new(move || {
                if use_pat {
                    store
                        .revoke_personal_access_token(&admin, pat.metadata.id)
                        .unwrap();
                } else {
                    store.revoke_session(&admin, &credential).unwrap();
                }
            })),
        };
        let response = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/v1/repos/{}", id.0))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::new(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let lookup = state.auth_store.authenticate_session(&session);
        let current = if let Ok(admin) = lookup {
            admin
        } else {
            state
                .auth_store
                .authenticate_session(
                    &state
                        .auth_store
                        .login("admin", "Password!2026")
                        .unwrap()
                        .raw_session_token,
                )
                .unwrap()
        };
        assert_eq!(
            state
                .auth_store
                .authorize_repository_access(&current, id)
                .unwrap(),
            before
        );
    }
}
#[tokio::test]
async fn registered_summary_reauthenticates_original_session_after_git_reads() {
    summary_reauth(false).await;
}
#[tokio::test]
async fn registered_summary_reauthenticates_original_pat_after_git_reads() {
    summary_reauth(true).await;
}
async fn summary_reauth(use_pat: bool) {
    let (_temp, state, session, _id) = fixture();
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "summary", None)
        .unwrap();
    let credential = if use_pat {
        pat.raw_token.clone()
    } else {
        session
    };
    let revoke = credential.clone();
    let store = state.auth_store.clone();
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        if use_pat {
            store
                .revoke_personal_access_token(&admin, pat.metadata.id)
                .unwrap();
        } else {
            store.revoke_session(&admin, &revoke).unwrap();
        }
    }));
    let (status, body) = call(
        &build_router(state),
        "GET",
        "/api/v1/repo".into(),
        &credential,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

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
            revoke();
        }
        std::task::Poll::Ready(Some(Ok(http_body::Frame::data(data.into()))))
    }
}
