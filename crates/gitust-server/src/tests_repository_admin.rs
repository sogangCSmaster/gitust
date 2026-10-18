use super::tests_pull_requests::{api_json, git, invite_and_login_role};
use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn online_repository_creation_and_settings_and_archive_enforcement() {
    let (_temp, state, admin_token, _) = fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let primary_id = state.repo_id;

    let admin_principal = state.auth_store.authenticate_session(&admin_token).unwrap();
    let team_id = state
        .auth_store
        .authorize_repository_access(&admin_principal, primary_id)
        .unwrap()
        .owner_team_id;
    let dev_token = invite_and_login_role(
        &state,
        &admin_token,
        "dev-admin-repo@company.internal",
        "dev_admin_repo",
        EnterpriseRole::Developer,
        vec![team_id],
    );

    let app = build_router(state.clone());

    // 1. Developer cannot create a hosted repository via POST /api/v1/admin/repos
    let (status, denied) = api_json(
        &app,
        "POST",
        "/api/v1/admin/repos".to_owned(),
        &dev_token,
        Some(json!({
            "name": "unauth-repo",
            "owner_team_id": team_id.0,
            "is_private": true
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-admin create repo: {denied}"
    );

    // 2. SiteAdmin creates a new hosted bare repository online via POST /api/v1/admin/repos
    let (status, created) = api_json(
        &app,
        "POST",
        "/api/v1/admin/repos".to_owned(),
        &admin_token,
        Some(json!({
            "name": "payments-service",
            "owner_team_id": team_id.0,
            "is_private": true,
            "default_branch": "main",
            "description": "Core payments repo",
            "workbench_enabled": true
        })),
    )
    .await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::OK,
        "admin create repo failed ({status}): {created}"
    );
    let sec_repo_id = created["id"].as_str().expect("repo id string").to_owned();
    assert_eq!(created["name"], "payments-service");
    assert_eq!(created["default_branch"], "main");
    assert_eq!(created["is_private"], true);
    assert_eq!(created["archived"], false);
    assert_eq!(created["workbench_enabled"], true);
    assert_eq!(created["description"], "Core payments repo");

    // 3. Newly created repository immediately appears in GET /api/v1/repos without restart
    let (status, catalog) =
        api_json(&app, "GET", "/api/v1/repos".to_owned(), &admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "catalog: {catalog}");
    let repos = catalog["repositories"].as_array().expect("repos array");
    let sec_entry = repos
        .iter()
        .find(|r| r["id"].as_str() == Some(sec_repo_id.as_str()))
        .expect("secondary repo in catalog");
    assert_eq!(sec_entry["name"], "payments-service");
    assert_eq!(sec_entry["capability"], "browse");
    assert_eq!(sec_entry["can_spawn_worktree"], true);
    assert_eq!(sec_entry["archived"], false);
    assert_eq!(sec_entry["can_administer"], true);
    assert_eq!(sec_entry["description"], "Core payments repo");

    // 4. Browse endpoints (commits & branches) work immediately on the newly created repo
    let (status, branches) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{sec_repo_id}/branches"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "branches: {branches}");
    assert_eq!(branches["branches"][0]["name"], "main");

    let (status, commits) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{sec_repo_id}/commits"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "commits: {commits}");
    assert_eq!(commits["commits"].as_array().map(Vec::len), Some(1));

    // 5. GET & PATCH /api/v1/repos/{id}/settings
    let (status, settings) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{sec_repo_id}/settings"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "get settings: {settings}");
    assert_eq!(settings["archived"], false);
    assert_eq!(settings["workbench_enabled"], true);

    // Developer cannot update settings
    let (status, _) = api_json(
        &app,
        "PATCH",
        format!("/api/v1/repos/{sec_repo_id}/settings"),
        &dev_token,
        Some(json!({ "archived": true })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // SiteAdmin archives the repository
    let (status, updated) = api_json(
        &app,
        "PATCH",
        format!("/api/v1/repos/{sec_repo_id}/settings"),
        &admin_token,
        Some(json!({
            "archived": true,
            "description": "Archived payments repo"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "archive settings: {updated}");
    assert_eq!(updated["archived"], true);
    assert_eq!(updated["description"], "Archived payments repo");

    // 6. While archived, worktree spawn fails with 403 FORBIDDEN, while reads still succeed
    let (status, spawn_archived) = api_json(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".to_owned(),
        &admin_token,
        Some(json!({
            "repo_id": sec_repo_id,
            "branch": "agent/archived-attempt",
            "prompt_summary": "Should be blocked by archive"
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "archived repo must reject worktree spawn: {spawn_archived}"
    );

    let (status, pr_archived) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{sec_repo_id}/pulls"),
        &admin_token,
        Some(json!({
            "title": "Blocked PR",
            "head_branch": "main",
            "base_branch": "main"
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "archived repo must reject PR creation: {pr_archived}"
    );

    let (status, receive_archived) = api_json(
        &app,
        "GET",
        format!("/git/{sec_repo_id}.git/info/refs?service=git-receive-pack"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "archived repo must reject receive-pack advertisement: {receive_archived}"
    );

    let (status, rename_archived) = api_json(
        &app,
        "PATCH",
        format!("/api/v1/repos/{sec_repo_id}"),
        &admin_token,
        Some(json!({ "name": "renamed-while-archived" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "archived repo must reject rename: {rename_archived}"
    );

    let (status, default_branch_archived) = api_json(
        &app,
        "PATCH",
        format!("/api/v1/repos/{sec_repo_id}/default-branch"),
        &admin_token,
        Some(json!({ "default_branch": "main" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "archived repo must reject default branch mutation: {default_branch_archived}"
    );

    let (status, commits_while_archived) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{sec_repo_id}/commits"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "archived repo must still allow reads: {commits_while_archived}"
    );

    // 7. Unarchive restores worktree spawn capability and `"description": null` clears description
    let (status, unarchived) = api_json(
        &app,
        "PATCH",
        format!("/api/v1/repos/{sec_repo_id}/settings"),
        &admin_token,
        Some(json!({
            "archived": false,
            "description": null
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unarchive: {unarchived}");
    assert_eq!(unarchived["archived"], false);
    assert!(
        unarchived["description"].is_null(),
        "null description should clear description: {unarchived}"
    );

    // Persist primary settings too and verify restart recovery of both primary settings and online repos
    let (status, primary_updated) = api_json(
        &app,
        "PATCH",
        format!("/api/v1/repos/{}/settings", primary_id.0),
        &admin_token,
        Some(json!({
            "description": "Persisted primary description",
            "archived": true
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "primary settings: {primary_updated}"
    );

    let primary_path = state.git_engine.repo_path().to_path_buf();
    let auth_store = state.auth_store.clone();
    drop(app);
    drop(state);

    let restarted = AppState::with_engine_and_auth_store(
        GitEngine::new(&primary_path),
        String::new(),
        auth_store,
    )
    .expect("restart AppState");
    let restarted_primary = restarted
        .read_repository(primary_id)
        .expect("restarted primary repo");
    assert!(
        restarted_primary.archived,
        "restarted primary repo should recover archived=true"
    );
    assert_eq!(
        restarted_primary.description.as_deref(),
        Some("Persisted primary description")
    );
    let sec_uuid = RepoId(uuid::Uuid::parse_str(&sec_repo_id).expect("sec repo uuid"));
    let restarted_sec = restarted
        .read_repository(sec_uuid)
        .expect("restarted online-provisioned repo");
    assert_eq!(restarted_sec.access.repo_id, sec_uuid);
    assert!(restarted_sec.workbench_enabled);
    assert!(!restarted_sec.archived);
    assert!(restarted_sec.description.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secondary_repository_isolated_worktree_spawn_sandbox_verify_approve_and_discard() {
    let (_temp, state, admin_token, _) = fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let primary_id = state.repo_id;
    let primary_repo = state.git_engine.repo_path().to_path_buf();
    let primary_before = git(&primary_repo, &["rev-parse", "refs/heads/main"]);

    let admin_principal = state.auth_store.authenticate_session(&admin_token).unwrap();
    let team_id = state
        .auth_store
        .authorize_repository_access(&admin_principal, primary_id)
        .unwrap()
        .owner_team_id;
    let sec_team_id = TeamId::new();
    let outsider_token = invite_and_login_role(
        &state,
        &admin_token,
        "outsider-sec@company.internal",
        "outsider_sec",
        EnterpriseRole::Developer,
        vec![team_id],
    );

    let app = build_router(state.clone());

    // Create an online secondary repository with workbench_enabled: true
    let (status, created) = api_json(
        &app,
        "POST",
        "/api/v1/admin/repos".to_owned(),
        &admin_token,
        Some(json!({
            "name": "analytics-engine",
            "owner_team_id": sec_team_id.0,
            "is_private": true,
            "default_branch": "main",
            "description": "Isolated secondary workbench repo",
            "workbench_enabled": true
        })),
    )
    .await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::OK,
        "create secondary repo: {created}"
    );
    let sec_repo_id_str = created["id"].as_str().unwrap().to_owned();
    let sec_repo_id = RepoId(uuid::Uuid::parse_str(&sec_repo_id_str).unwrap());

    // 1. Spawn worktree targeting the secondary repository
    let (status, wt) = api_json(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".to_owned(),
        &admin_token,
        Some(json!({
            "repo_id": sec_repo_id_str,
            "branch": "agent/sec-feature",
            "prompt_summary": "Add secondary feature"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "spawn secondary worktree: {wt}");
    assert_eq!(wt["repo_id"], sec_repo_id_str);
    let wt_id_str = wt["id"].as_str().unwrap().to_owned();
    let wt_id = WorktreeId(uuid::Uuid::parse_str(&wt_id_str).unwrap());

    // While an agent worktree is active on a workbench-enabled secondary repo,
    // git write validation and catalog `can_push` must remain true!
    let sec_read = state
        .read_repository(sec_repo_id)
        .expect("secondary read repo");
    assert!(
        sec_read
            .engine
            .validate_git_write_service(sec_read.workbench_enabled)
            .is_ok(),
        "workbench-enabled secondary repo with active agent worktrees must remain push-writable"
    );
    let (status, catalog_with_wt) =
        api_json(&app, "GET", "/api/v1/repos".to_owned(), &admin_token, None).await;
    assert_eq!(status, StatusCode::OK);
    let sec_cat = catalog_with_wt["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == sec_repo_id_str)
        .expect("sec repo in catalog");
    assert_eq!(
        sec_cat["can_push"], true,
        "can_push must stay true when agent worktree is active on workbench-enabled secondary repo"
    );

    // Verify primary repository does NOT have `refs/heads/agent/sec-feature`
    let primary_branches = git(
        &primary_repo,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    );
    assert!(
        !primary_branches.contains("agent/sec-feature"),
        "primary repo must not contain secondary worktree branch: {primary_branches}"
    );

    // 2. Commit a change inside the secondary worktree checkout
    let runtime = state
        .get_worktree_runtime(wt_id)
        .expect("secondary worktree runtime");
    std::fs::write(runtime.worktree_dir.join("sec_feature.txt"), "hello-sec\n").unwrap();
    git(&runtime.worktree_dir, &["add", "sec_feature.txt"]);
    git(
        &runtime.worktree_dir,
        &["commit", "-m", "feat: add sec_feature.txt"],
    );

    // 3. Verify in sandbox via POST /api/v1/swarm/worktrees/{id}/sandbox
    let (status, exec_res) = api_json(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{wt_id_str}/sandbox"),
        &admin_token,
        Some(json!({
            "command": "test -f sec_feature.txt",
            "verify": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "sandbox verify: {exec_res}");
    assert_eq!(exec_res["exit_code"], 0);
    let checked_commit = exec_res["checked_commit"]
        .as_str()
        .expect("checked_commit")
        .to_owned();

    // 4. Approve worktree via POST /api/v1/swarm/worktrees/{id}/approve
    let (status, approved) = api_json(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{wt_id_str}/approve"),
        &admin_token,
        Some(json!({
            "expected_commit": checked_commit
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "approve secondary worktree: {approved}"
    );
    assert_eq!(approved["status"], "merged");

    // 5. Verify secondary repo's `main` advanced to `checked_commit` while primary repo's `main` is unchanged
    let primary_after = git(&primary_repo, &["rev-parse", "refs/heads/main"]);
    assert_eq!(
        primary_before, primary_after,
        "primary repo main must be untouched"
    );

    let (status, sec_commits) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{sec_repo_id_str}/commits"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "sec commits: {sec_commits}");
    assert_eq!(sec_commits["commits"][0]["sha"], checked_commit);

    // 6. BOLA isolation on GET /api/v1/swarm/worktrees: outsider cannot see private secondary repo's worktrees
    let (status, outsider_wts) = api_json(
        &app,
        "GET",
        "/api/v1/swarm/worktrees".to_owned(),
        &outsider_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        outsider_wts
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["repo_id"] != sec_repo_id_str),
        "outsider must not see private secondary repo worktree: {outsider_wts}"
    );

    // 7. Discard secondary worktree cleans up secondary repo worktree
    let (status, _) = api_json(
        &app,
        "DELETE",
        format!("/api/v1/swarm/worktrees/{wt_id_str}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        state.get_worktree_runtime(wt_id).is_none(),
        "secondary worktree runtime must be removed for {sec_repo_id:?}"
    );
}
