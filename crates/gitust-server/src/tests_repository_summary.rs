use super::*;

#[tokio::test]
async fn connected_repository_summary_is_authenticated_and_has_no_host_path() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("connected-project");
    init_temp_repo(&repo);
    let (app, token) = test_app_at(repo.clone(), false);
    let route = "/api/v1/repo".to_owned();
    assert_eq!(
        call(&app, "GET", route.clone(), "invalid", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = call(&app, "GET", route, &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let summary: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(summary["name"], "connected-project");
    assert_eq!(summary["branch"], "main");
    assert!(uuid::Uuid::parse_str(summary["id"].as_str().unwrap()).is_ok());
    assert_eq!(summary.as_object().unwrap().len(), 4);
    let expected = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(expected.status.success());
    assert_eq!(
        summary["head_commit"],
        String::from_utf8(expected.stdout).unwrap().trim()
    );
    assert!(!body.contains(tmp.path().to_str().unwrap()));
}

#[tokio::test]
async fn connected_repository_summary_keeps_repository_access_and_account_revocation() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("private-project");
    init_temp_repo(&repo);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let admin_token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&admin_token).unwrap();
    let team = TeamId::new();
    let registered = store
        .create_team_repository(&admin, "private-project", team, true)
        .unwrap();
    let mut tokens = Vec::new();
    for (name, teams) in [("member", vec![team]), ("outsider", vec![])] {
        let invite = store
            .create_invite_link(
                &admin,
                &format!("{name}@company.internal"),
                EnterpriseRole::Developer,
                teams,
                None,
            )
            .unwrap();
        store
            .redeem_invite_link(&invite.raw_token, name, "Password!2026")
            .unwrap();
        tokens.push(
            store
                .login(name, "Password!2026")
                .unwrap()
                .raw_session_token,
        );
    }
    let mut state = AppState::with_auth_store(
        repo,
        "https://example.test/project.git".into(),
        store.clone(),
    )
    .unwrap();
    state.repo_id = registered.id;
    let app = build_router(state);
    let route = "/api/v1/repo".to_owned();
    let (status, body) = call(&app, "GET", route.clone(), &tokens[0], None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&app, "GET", route.clone(), &tokens[1], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(!body.contains("private-project"));
    let member = store.authenticate_session(&tokens[0]).unwrap();
    store.deactivate_user(&admin, member.user_id).unwrap();
    assert_ne!(
        call(&app, "GET", route, &tokens[0], None).await.0,
        StatusCode::OK
    );
}
