use super::tests_repository_visibility::fixture;
use super::*;

async fn lookup(app: &Router, token: &str, repo: RepoId) -> serde_json::Value {
    let (status, body) = call(
        app,
        "GET",
        format!(
            "/api/v1/repos/{}/group-members?username=%20DeVeLoPeR%20",
            repo.0
        ),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_str(&body).unwrap()
}
async fn change(
    app: &Router,
    token: &str,
    repo: RepoId,
    user: UserId,
    version: &str,
    member: bool,
) -> (StatusCode, String) {
    call(
        app,
        "PATCH",
        format!("/api/v1/repos/{}/group-members/{}", repo.0, user.0),
        token,
        Some(serde_json::json!({"expected_membership_version":version,"is_member":member})),
    )
    .await
}
#[tokio::test]
async fn repository_group_membership_versions_prevent_delayed_grants_and_survive_restart() {
    let (_temp, mut state, token, secondary) = fixture();
    let store = &state.auth_store;
    let admin = store.authenticate_session(&token).unwrap();
    let primary = store
        .authorize_repository_access(&admin, state.repo_id)
        .unwrap();
    let secondary_record = store
        .authorize_repository_access(&admin, secondary)
        .unwrap();
    let invite = store
        .create_invite_link(
            &admin,
            "developer@company.internal",
            EnterpriseRole::Developer,
            vec![primary.owner_team_id],
            None,
        )
        .unwrap();
    let developer = store
        .redeem_invite_link(&invite.raw_token, "developer", "Password!2026")
        .unwrap();
    let member_token = store
        .login("developer", "Password!2026")
        .unwrap()
        .raw_session_token;
    let app = build_router(state.clone());
    let before = lookup(&app, &token, secondary).await;
    assert_eq!(
        before,
        serde_json::json!({"repository_id":secondary,"owner_team_id":secondary_record.owner_team_id,"user_id":developer.id,
        "username":"developer","role":"Developer","status":"active","is_member":false,"membership_version":"0"})
    );
    let (status, body) = change(&app, &token, secondary, developer.id, "0", true).await;
    assert_eq!(status, StatusCode::OK);
    let added: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(added["membership_version"], "1");
    assert_eq!(added["is_member"], true);
    let (status, body) = change(&app, &token, secondary, developer.id, "0", true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"code":"membership_conflict"})
    );
    let (status, body) = change(&app, &token, secondary, developer.id, "1", true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["membership_version"],
        "1"
    );
    let refreshed = store.authenticate_session(&member_token).unwrap();
    assert!(store
        .authorize_repository_access(&refreshed, state.repo_id)
        .is_ok());
    assert!(store
        .authorize_repository_access(&refreshed, secondary)
        .is_ok());
    let (status, body) = change(&app, &token, secondary, developer.id, "1", false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["membership_version"],
        "2"
    );
    assert_eq!(
        change(&app, &token, secondary, developer.id, "0", true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let refreshed = store.authenticate_session(&member_token).unwrap();
    assert_eq!(refreshed.team_ids, vec![primary.owner_team_id]);
    assert_eq!(refreshed.role, EnterpriseRole::Developer);
    assert!(store
        .authorize_repository_access(&refreshed, secondary)
        .is_err());
    let path = store.persistence_path().unwrap().to_owned();
    state.auth_store = AuthStore::open_persistent(path).unwrap();
    let app = build_router(state.clone());
    assert_eq!(
        lookup(&app, &token, secondary).await["membership_version"],
        "2"
    );
    for version in [
        serde_json::json!(2),
        serde_json::json!("02"),
        serde_json::json!("18446744073709551616"),
    ] {
        let (status, _) = call(
            &app,
            "PATCH",
            format!(
                "/api/v1/repos/{}/group-members/{}",
                secondary.0, developer.id.0
            ),
            &token,
            Some(serde_json::json!({"expected_membership_version":version,"is_member":true})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, body) = call(
        &app,
        "GET",
        format!(
            "/api/v1/repos/{}/group-members?username=missing",
            secondary.0
        ),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"code":"group_member_not_found"})
    );
}
