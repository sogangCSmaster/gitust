use super::tests_repository_visibility::fixture;
use super::*;

#[tokio::test]
async fn group_invite_is_bound_to_hosted_owner_and_replay_never_returns_another_link() {
    let (_temp, state, token, secondary) = fixture();
    let app = build_router(state.clone());
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    for (n, id) in [state.repo_id, secondary].into_iter().enumerate() {
        let row = state
            .auth_store
            .authorize_repository_access(&admin, id)
            .unwrap();
        let invite_id = gitust_core::InviteId::new();
        let body = serde_json::json!({
            "invite_id":invite_id,"target_email":format!(" INVITED{n}@COMPANY.INTERNAL "),
            "assigned_role":"Developer"
        });
        let uri = format!("/api/v1/repos/{}/group-invites", id.0);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&uri)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(created["repository_id"], serde_json::json!(id));
        assert_eq!(
            created["owner_team_id"],
            serde_json::json!(row.owner_team_id)
        );
        assert_eq!(created["invite"]["id"], serde_json::json!(invite_id));
        assert_eq!(
            created["invite"]["team_ids"],
            serde_json::json!([row.owner_team_id])
        );
        assert_eq!(created["invite"]["repository_id"], serde_json::json!(id));
        assert!(created.get("raw_token").is_none());
        let link = created["invite_url_path"].as_str().unwrap();
        let raw = link.strip_prefix("/auth/accept-invite?token=").unwrap();
        let (status, replay) = call(&app, "POST", uri.clone(), &token, Some(body.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let replay: serde_json::Value = serde_json::from_str(&replay).unwrap();
        assert_eq!(replay["code"], "invite_already_issued");
        assert_eq!(replay["invite"], created["invite"]);
        assert!(replay.get("invite_url_path").is_none());
        state
            .auth_store
            .redeem_invite_link(raw, &format!("invited{n}"), "Password!2026")
            .unwrap();
        let (status, replay) = call(&app, "POST", uri, &token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let replay: serde_json::Value = serde_json::from_str(&replay).unwrap();
        assert_eq!(replay["code"], "invite_already_issued");
        assert!(!replay["invite"]["used_at"].is_null());
        let session = state
            .auth_store
            .login(&format!("invited{n}"), "Password!2026")
            .unwrap();
        let member = state
            .auth_store
            .authenticate_session(&session.raw_session_token)
            .unwrap();
        assert_eq!(member.team_ids, vec![row.owner_team_id]);
    }
}
