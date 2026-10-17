use super::*;

fn snapshots(state: &AppState, ids: &[RepoId]) -> Vec<(String, String, Vec<Vec<u8>>)> {
    ids.iter()
        .map(|id| {
            let repository = state.read_repository(*id).unwrap();
            let path = repository.engine.repo_path();
            let common = repository.engine.repository_common_directory().unwrap();
            (
                git(path, &["show-ref"]),
                git(path, &["symbolic-ref", "HEAD"]),
                ["identity.json", "registration.json"]
                    .iter()
                    .map(|name| std::fs::read(common.join("gitust-repository").join(name)).unwrap())
                    .collect(),
            )
        })
        .collect()
}

fn change(target: &str, current: &str, sha: &str) -> serde_json::Value {
    serde_json::json!({"target_ref":format!("refs/heads/{target}"),
        "expected_current_ref":format!("refs/heads/{current}"),"expected_target_commit":sha})
}

#[tokio::test]
async fn writable_secondary_default_changes_only_its_head_and_uses_selected_mutation_guard() {
    let (_temp, state, session, bare, id, readonly) = super::super::tests_git_http_write::fixture();
    let main = git(&bare, &["rev-parse", "HEAD"]);
    git(&bare, &["branch", "secondary-default", "HEAD^"]);
    let next = git(&bare, &["rev-parse", "refs/heads/secondary-default"]);
    assert_ne!(main, next);
    let ids = [state.repo_id, readonly, id];
    let before = snapshots(&state, &ids);
    let app = build_router(state.clone());
    let route = format!("/api/v1/repos/{}/default-branch", id.0);
    let request = change("secondary-default", "main", &next);
    let (status, raw) = call(
        &app,
        "PATCH",
        route.clone(),
        &session,
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
        serde_json::json!({"id":id,"branch":"secondary-default","head_commit":next})
    );
    let after = snapshots(&state, &ids);
    assert_eq!(after[..2], before[..2]);
    assert_eq!(after[2].0, before[2].0);
    assert_eq!(after[2].2, before[2].2);
    assert_eq!(after[2].1, "refs/heads/secondary-default");
    assert_eq!(git(&bare, &["rev-parse", "HEAD"]), next);
    assert_eq!(
        call(&app, "PATCH", route.clone(), &session, Some(request))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (_, raw) = call(&app, "GET", "/api/v1/repos".into(), &session, None).await;
    let catalog: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for row in catalog["repositories"].as_array().unwrap() {
        assert_eq!(
            row["can_change_default_branch"],
            row["id"] == id.0.to_string()
        );
        if row["id"] == id.0.to_string() {
            assert_eq!(row["capability"], "browse");
            assert_eq!(row["is_default"], false);
            assert_eq!(row["branch"], "secondary-default");
            assert_eq!(row["head_commit"], next);
        }
    }
    let restore = change("main", "secondary-default", &main);
    let selected_guard = state
        .read_repository(id)
        .unwrap()
        .mutation
        .lock_owned()
        .await;
    assert_eq!(
        call(
            &app,
            "PATCH",
            route.clone(),
            &session,
            Some(restore.clone())
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(snapshots(&state, &ids), after);
    drop(selected_guard);
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "secondary-default", None)
        .unwrap();
    // An unrelated primary operation must not claim this secondary's mutation guard.
    let primary_guard = state.repo_mutation_lock.clone().lock_owned().await;
    assert_eq!(
        call(&app, "PATCH", route, &pat.raw_token, Some(restore))
            .await
            .0,
        StatusCode::OK
    );
    drop(primary_guard);
    assert_eq!(snapshots(&state, &ids), before);
    for id in ids {
        git(
            state.read_repository(id).unwrap().engine.repo_path(),
            &["fsck", "--full", "--strict"],
        );
    }
}

#[tokio::test]
async fn secondary_default_stays_admin_only_and_read_only_or_unknown_targets_are_denied() {
    let (_temp, state, session, bare, id, readonly) = super::super::tests_git_http_write::fixture();
    git(&bare, &["branch", "secondary-default", "HEAD^"]);
    let request = change(
        "secondary-default",
        "main",
        &git(&bare, &["rev-parse", "HEAD^"]),
    );
    let ids = [state.repo_id, readonly, id];
    let before = snapshots(&state, &ids);
    let app = build_router(state.clone());
    for (target, expected) in [
        (readonly, StatusCode::FORBIDDEN),
        (RepoId::new(), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            call(
                &app,
                "PATCH",
                format!("/api/v1/repos/{}/default-branch", target.0),
                &session,
                Some(request.clone())
            )
            .await
            .0,
            expected
        );
    }
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, id)
        .unwrap()
        .owner_team_id;
    for (index, role) in [EnterpriseRole::Maintainer, EnterpriseRole::Developer]
        .into_iter()
        .enumerate()
    {
        let name = format!("default-member-{index}");
        let invite = state
            .auth_store
            .create_invite_link(
                &admin,
                &format!("{name}@company.internal"),
                role,
                vec![team],
                None,
            )
            .unwrap();
        state
            .auth_store
            .redeem_invite_link(&invite.raw_token, &name, "Password!2026")
            .unwrap();
        let token = state
            .auth_store
            .login(&name, "Password!2026")
            .unwrap()
            .raw_session_token;
        assert_eq!(
            call(
                &app,
                "PATCH",
                format!("/api/v1/repos/{}/default-branch", id.0),
                &token,
                Some(request.clone())
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let (_, raw) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
        let catalog: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(catalog["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["can_change_default_branch"] == false));
    }
    assert_eq!(snapshots(&state, &ids), before);
}
