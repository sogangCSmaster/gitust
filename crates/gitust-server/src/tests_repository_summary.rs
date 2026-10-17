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

fn admin_store(path: Option<&std::path::Path>) -> (AuthStore, String, AuthenticatedPrincipal) {
    let store = match path {
        Some(path) => AuthStore::open_persistent(path).unwrap(),
        None => AuthStore::new_in_memory(),
    };
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let principal = store.authenticate_session(&token).unwrap();
    (store, token, principal)
}

#[tokio::test]
async fn connected_repository_summary_keeps_repository_access_and_account_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("configured-project");
    init_temp_repo(&repo);
    let (store, _, admin) = admin_store(None);
    let invite = store
        .create_invite_link(
            &admin,
            "member@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    store
        .redeem_invite_link(&invite.raw_token, "member", "Password!2026")
        .unwrap();
    let token = store
        .login("member", "Password!2026")
        .unwrap()
        .raw_session_token;
    let member = store.authenticate_session(&token).unwrap();
    let state = AppState::with_auth_store(
        repo,
        "https://example.invalid/repo.git".into(),
        store.clone(),
    )
    .unwrap();
    assert!(!store.repository_exists(state.repo_id).unwrap());
    let app = build_router(state);
    assert_eq!(
        call(&app, "GET", "/api/v1/repo".into(), &token, None)
            .await
            .0,
        StatusCode::OK
    );
    store.deactivate_user(&admin, member.user_id).unwrap();
    assert_ne!(
        call(&app, "GET", "/api/v1/repo".into(), &token, None)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn restart_reuses_the_same_repository_id_for_summary_recovered_worktrees_and_sync() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("stable-project");
    init_temp_repo(&repo);
    let (store, token, _) = admin_store(None);
    let initial = AppState::with_auth_store(
        repo.clone(),
        "https://example.invalid/repo.git".into(),
        store.clone(),
    )
    .unwrap();
    let id = initial.repo_id;
    let worktree = initial
        .git_engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            id,
            AgentKind::CodexSwarm,
            "agent/test/stable-id",
            "main",
            "persist repository ID",
        )
        .unwrap();
    drop(initial);
    for _ in 0..2 {
        let state = AppState::with_auth_store(
            repo.clone(),
            "https://example.invalid/repo.git".into(),
            store.clone(),
        )
        .unwrap();
        assert_eq!(state.repo_id, id);
        assert_eq!(state.sync_config.repo_id, id);
        let recovered = state
            .worktrees
            .read()
            .iter()
            .find(|wt| wt.id == worktree.worktree.id)
            .unwrap()
            .clone();
        assert_eq!(recovered.repo_id, id);
        let app = build_router(state);
        let (status, body) = call(&app, "GET", "/api/v1/repo".into(), &token, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"],
            id.0.to_string()
        );
    }
}

#[test]
fn configured_identity_collision_is_rejected_at_startup_instead_of_adopting_metadata_acl() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("collision-project");
    init_temp_repo(&repo);
    let (store, _, principal) = admin_store(None);
    let registered = store
        .create_team_repository(&principal, "metadata-only", TeamId::new(), true)
        .unwrap();
    let engine = gitust_git::GitEngine::new(&repo);
    engine.load_or_create_repository_id().unwrap();
    let path = repo.join(".git/gitust-repository/identity.json");
    let bytes =
        serde_json::to_vec(&serde_json::json!({"version":1,"repo_id":registered.id})).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        AppState::with_auth_store(repo, "https://example.invalid/repo.git".into(), store),
        Err(gitust_git::GitEngineError::RepositoryIdentity(_))
    ));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn a_later_auth_metadata_collision_denies_admin_requests_including_an_empty_worktree_list() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("later-collision");
    init_temp_repo(&repo);
    let auth_path = temp.path().join("auth/auth.json");
    let (store, token, principal) = admin_store(Some(&auth_path));
    store
        .create_team_repository(&principal, "metadata-only", TeamId::new(), true)
        .unwrap();
    let state = AppState::with_auth_store(
        repo.clone(),
        "https://example.invalid/repo.git".into(),
        store,
    )
    .unwrap();
    let configured = state.repo_id;
    let app = build_router(state);
    assert_eq!(
        call(&app, "GET", "/api/v1/repo".into(), &token, None)
            .await
            .0,
        StatusCode::OK
    );
    // Simulate another authorized metadata writer; atomic replacement forces a fresh store read.
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&auth_path).unwrap()).unwrap();
    snapshot["repositories"][0]["id"] = serde_json::json!(configured);
    let staging = auth_path.with_file_name("external-auth-update.json");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)
        .unwrap();
    file.write_all(&serde_json::to_vec(&snapshot).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    std::fs::rename(staging, &auth_path).unwrap();
    for route in ["/api/v1/repo", "/api/v1/swarm/worktrees"] {
        let (status, body) = call(&app, "GET", route.into(), &token, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(!body.contains("metadata-only"));
    }
    let (status, body) = call(&app, "POST", "/api/v1/swarm/worktrees".into(), &token, Some(serde_json::json!({
        "repo_id":configured,"branch":"agent/test/denied-collision","prompt_summary":"must not spawn"
    }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(gitust_git::GitEngine::new(repo)
        .list_agent_worktrees()
        .unwrap()
        .is_empty());
}
