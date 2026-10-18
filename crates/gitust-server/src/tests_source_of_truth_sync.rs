use super::tests_pull_requests::{api_json, git, invite_and_login_role};
use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_of_truth_topology_backup_mirror_push_and_verified_rollback() {
    let (temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let primary_id = state.repo_id;
    let primary_id_str = primary_id.0.to_string();
    let primary_repo = state.git_engine.repo_path().to_path_buf();

    // Create a local bare repository to act as the upstream backup mirror
    let backup_bare = temp.path().join("backup-mirror.git");
    std::fs::create_dir_all(&backup_bare).unwrap();
    git(&backup_bare, &["init", "--bare", "--initial-branch=main"]);
    let backup_url = format!("file://{}", backup_bare.display());
    state.sync_config.github_remote_url = backup_url.clone();

    let admin_principal = state.auth_store.authenticate_session(&admin_token).unwrap();
    let team_id = state
        .auth_store
        .authorize_repository_access(&admin_principal, primary_id)
        .unwrap()
        .owner_team_id;
    let dev_token = invite_and_login_role(
        &state,
        &admin_token,
        "dev-sot@company.internal",
        "dev_sot",
        EnterpriseRole::Developer,
        vec![team_id],
    );
    let maintainer_token = invite_and_login_role(
        &state,
        &admin_token,
        "maint-sot@company.internal",
        "maint_sot",
        EnterpriseRole::Maintainer,
        vec![team_id],
    );

    let baseline_main_sha = git(&primary_repo, &["rev-parse", "refs/heads/main"]);
    // Create a tag v1.0.0 at baseline_main_sha
    git(&primary_repo, &["tag", "v1.0.0", &baseline_main_sha]);

    let app = build_router(state.clone());

    // Spawn an active agent worktree (`agent/sot-private`) so we can verify it is NEVER pushed to the backup mirror
    // and NEVER deleted or overwritten during rollback!
    let (status, spawned_wt) = api_json(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".to_owned(),
        &admin_token,
        Some(json!({
            "repo_id": primary_id_str,
            "branch": "agent/sot-private",
            "prompt_summary": "Private agent experiment"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "spawn agent worktree: {spawned_wt}");
    let agent_sha = git(
        &primary_repo,
        &["rev-parse", "refs/heads/agent/sot-private"],
    );

    // 1. GET /api/v1/repos/{id}/sync/topology defaults to mode: "inbound_mirror"
    let (status, initial_topology) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/sync/topology"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "get topology: {initial_topology}");
    assert_eq!(initial_topology["mode"], "inbound_mirror");
    assert_eq!(initial_topology["backup_remote_url"], backup_url);

    // Calling backup-push while still in `inbound_mirror` mode must fail closed with 409 CONFLICT
    let (status, push_in_inbound) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{primary_id_str}/sync/backup-push"),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "backup-push must fail with 409 CONFLICT in inbound_mirror mode: {push_in_inbound}"
    );

    // 2. Non-SiteAdmin (Developer & Maintainer) cannot mutate sync topology (403 FORBIDDEN)
    for token in [&dev_token, &maintainer_token] {
        let (status, denied) = api_json(
            &app,
            "PUT",
            format!("/api/v1/repos/{primary_id_str}/sync/topology"),
            token,
            Some(json!({
                "mode": "source_of_truth"
            })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "non-SiteAdmin topology update must fail closed: {denied}"
        );
    }

    // Anti-SSRF: supplying an unconfigured backup_remote_url is rejected with 400 BAD_REQUEST
    let (status, ssrf_denied) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{primary_id_str}/sync/topology"),
        &admin_token,
        Some(json!({
            "mode": "source_of_truth",
            "backup_remote_url": "https://evil.example.com/exfil.git"
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "arbitrary backup_remote_url must be rejected (anti-SSRF): {ssrf_denied}"
    );

    // SiteAdmin switches topology to `source_of_truth`, capturing baseline snapshot
    let (status, sot_topology) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{primary_id_str}/sync/topology"),
        &admin_token,
        Some(json!({
            "mode": "source_of_truth",
            "backup_remote_url": backup_url
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "switch to source_of_truth: {sot_topology}"
    );
    assert_eq!(sot_topology["mode"], "source_of_truth");
    assert_eq!(sot_topology["backup_remote_url"], backup_url);
    let snapshots = sot_topology["snapshots"]
        .as_array()
        .expect("snapshots array");
    assert!(!snapshots.is_empty(), "baseline snapshot must be captured");
    let baseline_snapshot_id = snapshots[0]["snapshot_id"]
        .as_str()
        .expect("snapshot_id string")
        .to_owned();
    let baseline_refs = snapshots[0]["pinned_refs"]
        .as_array()
        .expect("pinned_refs array");
    assert!(
        baseline_refs
            .iter()
            .any(|p| p["ref_name"] == "refs/heads/main" && p["target_sha"] == baseline_main_sha),
        "baseline snapshot must record refs/heads/main: {baseline_refs:?}"
    );
    assert!(
        baseline_refs.iter().all(|p| !p["ref_name"]
            .as_str()
            .unwrap_or_default()
            .starts_with("refs/heads/agent/")),
        "baseline snapshot must filter out refs/heads/agent/*: {baseline_refs:?}"
    );

    // 3. While in `source_of_truth` mode, calling POST /api/v1/sync/github (inbound mirror overwrite) fails closed with 409 CONFLICT
    let (status, inbound_conflict) = api_json(
        &app,
        "POST",
        "/api/v1/sync/github".to_owned(),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "inbound mirror sync must fail with 409 CONFLICT while in source_of_truth mode: {inbound_conflict}"
    );

    // 4. POST /api/v1/repos/{id}/sync/backup-push mirrors ordinary branches & tags to backup_bare and verifies via ls-remote
    let (status, push_report) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{primary_id_str}/sync/backup-push"),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "backup push: {push_report}");
    assert_eq!(push_report["verified_remote_match"], true);
    assert!(
        push_report["pushed_branches"].as_u64().unwrap_or(0) >= 1,
        "at least refs/heads/main must be pushed: {push_report}"
    );
    assert!(
        push_report["pushed_tags"].as_u64().unwrap_or(0) >= 1,
        "at least refs/tags/v1.0.0 must be pushed: {push_report}"
    );
    assert!(
        push_report["filtered_agent_refs"].as_u64().unwrap_or(0) >= 1,
        "active agent branch must be counted as filtered: {push_report}"
    );

    // Inspect backup_bare directly to confirm refs/heads/main is present and refs/heads/agent/* is absent
    let backup_refs = git(
        &backup_bare,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/",
        ],
    );
    assert!(
        backup_refs.contains(&format!("refs/heads/main {baseline_main_sha}")),
        "backup mirror must contain refs/heads/main at baseline sha: {backup_refs}"
    );
    assert!(
        !backup_refs.contains("refs/heads/agent/"),
        "backup mirror must never contain refs/heads/agent/*: {backup_refs}"
    );

    // 5. Advance refs/heads/main, create a post-snapshot ordinary branch, and push to backup mirror
    std::fs::write(primary_repo.join("sot_new.txt"), "post-snapshot commit\n").unwrap();
    git(&primary_repo, &["add", "sot_new.txt"]);
    git(
        &primary_repo,
        &["commit", "-m", "feat: post-snapshot commit on main"],
    );
    let advanced_main_sha = git(&primary_repo, &["rev-parse", "refs/heads/main"]);
    assert_ne!(advanced_main_sha, baseline_main_sha);
    git(
        &primary_repo,
        &["branch", "ephemeral-post-snapshot", &advanced_main_sha],
    );

    let (status, push_ephemeral) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{primary_id_str}/sync/backup-push"),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "backup push ephemeral: {push_ephemeral}"
    );
    let backup_with_ephemeral = git(
        &backup_bare,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    );
    assert!(
        backup_with_ephemeral.contains("refs/heads/ephemeral-post-snapshot"),
        "backup mirror must contain ephemeral-post-snapshot before rollback: {backup_with_ephemeral}"
    );

    let (status, rollback_report) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{primary_id_str}/sync/rollback"),
        &admin_token,
        Some(json!({
            "snapshot_id": baseline_snapshot_id
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rollback: {rollback_report}");
    assert_eq!(rollback_report["snapshot_id"], baseline_snapshot_id);
    assert_eq!(rollback_report["fsck_passed"], true);
    assert!(
        rollback_report["restored_refs"].as_u64().unwrap_or(0) >= 1,
        "restored_refs must be >= 1: {rollback_report}"
    );

    // Confirm refs/heads/main rolled back to baseline_main_sha, post-snapshot branch was removed,
    // and refs/heads/agent/sot-private was preserved untouched!
    let main_after_rollback = git(&primary_repo, &["rev-parse", "refs/heads/main"]);
    assert_eq!(
        main_after_rollback, baseline_main_sha,
        "refs/heads/main must roll back to baseline snapshot SHA"
    );
    let all_heads_after = git(
        &primary_repo,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    );
    assert!(
        !all_heads_after.contains("refs/heads/ephemeral-post-snapshot"),
        "post-snapshot ordinary branch must be removed on rollback: {all_heads_after}"
    );
    let agent_after_rollback = git(
        &primary_repo,
        &["rev-parse", "refs/heads/agent/sot-private"],
    );
    assert_eq!(
        agent_after_rollback, agent_sha,
        "active agent branch must be preserved across rollback"
    );

    // 6. Re-pushing after rollback prunes deleted ordinary branches (`ephemeral-post-snapshot`) from the backup mirror!
    let (status, prune_report) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{primary_id_str}/sync/backup-push"),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "prune push: {prune_report}");
    assert_eq!(prune_report["verified_remote_match"], true);
    let backup_after_prune = git(
        &backup_bare,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    );
    assert!(
        !backup_after_prune.contains("refs/heads/ephemeral-post-snapshot"),
        "deleted ordinary branch must be pruned from backup mirror: {backup_after_prune}"
    );

    // 7. Tampering with topology.json by injecting an agent/* ref pin fails closed (409 CONFLICT)
    let common_dir = state.git_engine.repository_common_directory().unwrap();
    let topology_path = common_dir.join("gitust-sync").join("topology.json");
    let mut raw_topo: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&topology_path).unwrap()).unwrap();
    raw_topo["snapshots"][0]["pinned_refs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "ref_name": "refs/heads/agent/sot-private",
            "target_sha": baseline_main_sha
        }));
    std::fs::write(&topology_path, serde_json::to_vec(&raw_topo).unwrap()).unwrap();

    let (status, tampered_rollback) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{primary_id_str}/sync/rollback"),
        &admin_token,
        Some(json!({
            "snapshot_id": baseline_snapshot_id
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "tampered topology.json with agent/* pin must fail closed: {tampered_rollback}"
    );
}
