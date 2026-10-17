use super::tests_verification::fixture;
use super::*;

#[tokio::test]
async fn explicit_registration_requires_both_anchors_and_never_upgrades_a_running_legacy_scope() {
    let (_tmp, state, _app, token, worktree, _) = fixture().await;
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let registered = state
        .auth_store
        .register_local_repository(&admin, state.repo_id, "actual", TeamId::new(), true, None)
        .unwrap();
    let restart = || {
        AppState::with_engine_and_auth_store(
            state.git_engine.clone(),
            "https://github.com/example/fixture.git".into(),
            state.auth_store.clone(),
        )
    };
    assert!(
        restart().is_err(),
        "DB-only registration cannot start a legacy service"
    );
    assert!(crate::state::check_repo_bola(
        &state.auth_store,
        state.repo_id,
        state.repo_access,
        &admin,
        state.repo_id
    )
    .is_err());
    state
        .git_engine
        .complete_repository_registration(registered.binding)
        .unwrap();
    let resumed = restart().unwrap();
    assert_eq!(resumed.repo_id, state.repo_id);
    let response = build_router(resumed.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/repo")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let receipt = state
        .git_engine
        .repository_common_directory()
        .unwrap()
        .join("gitust-repository/registration.json");
    let before = std::fs::read(&receipt).unwrap();
    let mut output = Vec::new();
    let result = crate::serve_mcp_stream(
        resumed.clone(),
        tokio::io::BufReader::new(tokio::io::empty()),
        &mut output,
    )
    .await;
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(output.is_empty());
    assert!(resumed.automatic_checks.store().is_err());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::serve_mcp_stdio(
            state.git_engine.repo_path().to_owned(),
            "https://github.com/example/fixture.git".into(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(std::fs::read(&receipt).unwrap() == before);
    let pending = receipt.with_file_name("registration.pending");
    std::fs::rename(&receipt, &pending).unwrap();
    assert!(restart().is_err(), "pending evidence cannot become absent");
    let pending_stdio = crate::serve_mcp_stdio(
        state.git_engine.repo_path().to_owned(),
        "https://github.com/example/fixture.git".into(),
    )
    .await
    .unwrap_err();
    assert!(pending_stdio
        .to_string()
        .contains("registration evidence is incomplete"));
    state
        .git_engine
        .complete_repository_registration(registered.binding)
        .unwrap();
    assert!(
        AppState::with_engine_and_auth_store(
            state.git_engine.clone(),
            "https://github.com/example/fixture.git".into(),
            AuthStore::new_in_memory()
        )
        .is_err(),
        "receipt-only registration cannot use a different empty auth store"
    );
    let fence = resumed.git_engine.begin_execution_fence(&worktree).unwrap();
    drop(fence);
    assert!(
        restart().is_err(),
        "unfinished execution must prevent registration/startup"
    );
    assert!(state
        .git_engine
        .complete_repository_registration(registered.binding)
        .is_err());
}
