use super::tests_repository_catalog::registered_pair;
use super::*;
use crate::repository_registry::RepositoryRegistry;

pub(super) fn fixture() -> (tempfile::TempDir, AppState, String, RepoId) {
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
async fn configured_primary_and_secondary_visibility_changes_preserve_metadata_and_git() {
    let (_temp, state, token, secondary_id) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let app = build_router(state.clone());
    for id in [state.repo_id, secondary_id] {
        let before = state
            .auth_store
            .authorize_repository_access(&admin, id)
            .unwrap();
        let context = state.read_repository(id).unwrap();
        let common = context.engine.repository_common_directory().unwrap();
        let identity = std::fs::read(common.join("gitust-repository/identity.json")).unwrap();
        let receipt = std::fs::read(common.join("gitust-repository/registration.json")).unwrap();
        let refs = fixture_git()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
            ])
            .arg("-C")
            .arg(&common)
            .args(["show-ref", "--head"])
            .output()
            .unwrap();
        assert!(refs.status.success());
        let uri = format!("/api/v1/repos/{}/visibility", id.0);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(&uri)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(
                        r#"{"expected_is_private":true,"is_private":false}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"id":id,"is_private":false})
        );
        let mut expected = before;
        expected.is_private = false;
        assert_eq!(
            state
                .auth_store
                .authorize_repository_access(&admin, id)
                .unwrap(),
            expected
        );
        assert_eq!(
            state
                .auth_store
                .configured_repository_access(id, context.access.registration)
                .unwrap(),
            context.access
        );
        assert_eq!(
            std::fs::read(common.join("gitust-repository/identity.json")).unwrap(),
            identity
        );
        assert_eq!(
            std::fs::read(common.join("gitust-repository/registration.json")).unwrap(),
            receipt
        );
        let after = fixture_git()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
            ])
            .arg("-C")
            .arg(&common)
            .args(["show-ref", "--head"])
            .output()
            .unwrap();
        assert!(after.status.success());
        assert_eq!(after.stdout, refs.stdout);
        assert_eq!(
            call(
                &app,
                "PATCH",
                uri.clone(),
                &token,
                Some(serde_json::json!({"expected_is_private":true,"is_private":false}))
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            call(
                &app,
                "PATCH",
                uri,
                &token,
                Some(serde_json::json!({"expected_is_private":false,"is_private":false}))
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    let catalog: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert!(catalog["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["is_private"] == false && row["can_change_visibility"] == true));
}

pub(super) fn user(
    state: &AppState,
    admin: &gitust_core::AuthenticatedPrincipal,
    name: &str,
    team: TeamId,
) -> String {
    let invite = state
        .auth_store
        .create_invite_link(
            admin,
            &format!("{name}@company.internal"),
            EnterpriseRole::Developer,
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
async fn visibility_grants_and_revokes_active_outsider_reads_without_changing_roles_or_worktree_targets(
) {
    let (_temp, state, token, second) = fixture();
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let primary_team = state
        .auth_store
        .authorize_repository_access(&admin, state.repo_id)
        .unwrap()
        .owner_team_id;
    let second_team = state
        .auth_store
        .authorize_repository_access(&admin, second)
        .unwrap()
        .owner_team_id;
    let first_member = user(&state, &admin, "first-member", primary_team);
    let second_member = user(&state, &admin, "second-member", second_team);
    let outside = user(&state, &admin, "outside", TeamId::new());
    let app = build_router(state.clone());
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &first_member,
        Some(serde_json::json!({"branch":"agent/visibility","prompt_summary":"preserve target"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let task: AgentWorktree = serde_json::from_str(&body).unwrap();
    let worktree_before = serde_json::to_value(&*state.worktrees.read()).unwrap();
    let origin = std::fs::read(
        state
            .git_engine
            .repository_common_directory()
            .unwrap()
            .join("worktrees")
            .join(task.id.0.to_string())
            .join("gitust-origin.json"),
    );
    for (id, owner, other) in [
        (state.repo_id, &first_member, &second_member),
        (second, &second_member, &first_member),
    ] {
        let read = format!("/api/v1/repos/{}/tree", id.0);
        assert_eq!(
            call(&app, "GET", read.clone(), owner, None).await.0,
            StatusCode::OK
        );
        for denied in [other, &outside] {
            assert_eq!(
                call(&app, "GET", read.clone(), denied, None).await.0,
                StatusCode::FORBIDDEN
            );
        }
        let uri = format!("/api/v1/repos/{}/visibility", id.0);
        assert_eq!(
            call(
                &app,
                "PATCH",
                uri.clone(),
                &token,
                Some(serde_json::json!({"expected_is_private":true,"is_private":false}))
            )
            .await
            .0,
            StatusCode::OK
        );
        for allowed in [owner, other, &outside] {
            assert_eq!(
                call(&app, "GET", read.clone(), allowed, None).await.0,
                StatusCode::OK
            );
            assert_eq!(
                call(
                    &app,
                    "PATCH",
                    uri.clone(),
                    allowed,
                    Some(serde_json::json!({"expected_is_private":false,"is_private":true}))
                )
                .await
                .0,
                StatusCode::FORBIDDEN
            );
        }
        let anonymous = app
            .clone()
            .oneshot(Request::builder().uri(&read).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &outside, None).await;
        let catalog: serde_json::Value = serde_json::from_str(&catalog).unwrap();
        let row = catalog["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id.0.to_string())
            .unwrap();
        assert_eq!(row["is_private"], false);
        assert_eq!(row["can_change_visibility"], false);
        assert_eq!(
            call(
                &app,
                "PATCH",
                uri,
                &token,
                Some(serde_json::json!({"expected_is_private":false,"is_private":true}))
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            call(&app, "GET", read.clone(), &outside, None).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(call(&app, "GET", read, owner, None).await.0, StatusCode::OK);
    }
    assert_eq!(
        serde_json::to_value(&*state.worktrees.read()).unwrap(),
        worktree_before
    );
    let current_origin = std::fs::read(
        state
            .git_engine
            .repository_common_directory()
            .unwrap()
            .join("worktrees")
            .join(task.id.0.to_string())
            .join("gitust-origin.json"),
    );
    assert!(origin.is_ok());
    assert_eq!(origin.unwrap(), current_origin.unwrap());
    state
        .auth_store
        .deactivate_user(
            &admin,
            state
                .auth_store
                .authenticate_session(&outside)
                .unwrap()
                .user_id,
        )
        .unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            format!("/api/v1/repos/{}/tree", state.repo_id.0),
            &outside,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
