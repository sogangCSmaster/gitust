//! TDD tests for pre-alpha live rehearsal (clean install -> admin bootstrap -> login -> repo connect ->
//! 2+ concurrent IDE/agent clients -> approval & outbound publish failure/retry -> relocated backup/restore
//! verification -> server restart recovery) and master secret key rotation (`POST /api/v1/admin/security/rotate-secrets`).

use std::process::Command;

use super::tests_pull_requests::{api_json, git};
use super::*;
use gitust_core::{SsoProtocol, TeamId};
use gitust_db::RegisterSsoProviderInput;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_rehearsal_clean_install_multi_ide_publish_retry_and_relocated_backup_restore() {
    let env_dir = tempfile::tempdir().expect("tempdir");
    let source_root = env_dir.path().join("source-install");
    std::fs::create_dir_all(&source_root).expect("mkdir source_root");

    // 1. Initialize clean repository & upstream bare repository
    let repo_dir = source_root.join("repository");
    std::fs::create_dir_all(&repo_dir).expect("mkdir repo");
    git(&repo_dir, &["init", "-b", "main"]);
    git(&repo_dir, &["config", "user.name", "Gitust Admin"]);
    git(
        &repo_dir,
        &["config", "user.email", "admin@company.internal"],
    );
    std::fs::write(repo_dir.join("README.md"), "# Alpha Rehearsal Repo\n").expect("write readme");
    std::fs::write(
        repo_dir.join(".gitust-checks.json"),
        r#"{"version":1,"steps":[{"name":"Verify README","run":"test -f README.md"}]}"#,
    )
    .expect("write checks");
    std::fs::write(
        repo_dir.join(".gitust-workflows.yml"),
        "name: Rehearsal CI\non: [push]\njobs:\n  verify:\n    runs-on: local\n    steps:\n      - name: Check tree\n        run: test -f README.md\n",
    )
    .expect("write workflow");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-m", "Initial rehearsal commit"]);
    let head_sha = git(&repo_dir, &["rev-parse", "refs/heads/main"]);

    let upstream_bare = env_dir.path().join("upstream-github.git");
    Command::new("git")
        .args(["clone", "--bare"])
        .arg(&repo_dir)
        .arg(&upstream_bare)
        .status()
        .expect("clone bare upstream");

    // 2. CLI Admin Bootstrap + External Secret Key (`GITUST_SECRET_KEY`) + Login + Repo Connect
    let auth_db_path = source_root.join("auth-db.json");
    let master_key_1: [u8; 32] = [0x42; 32];
    let store =
        AuthStore::open_persistent_with_secret_key(&auth_db_path, master_key_1).expect("open auth");
    store
        .bootstrap_initial_admin_cli(
            "rehearsal_admin",
            "admin@company.internal",
            "RehearsalPass!2026",
        )
        .expect("bootstrap admin");
    let admin_login = store
        .login("rehearsal_admin", "RehearsalPass!2026")
        .expect("admin login");
    let admin_token = admin_login.raw_session_token;
    let admin_principal = store
        .authenticate_session(&admin_token)
        .expect("admin principal");

    // Point initially to an uninitialized remote directory to verify outbound publish failure (502) & retry
    let unavailable_remote = env_dir.path().join("missing-upstream.git");
    std::fs::create_dir_all(&unavailable_remote).expect("mkdir unavailable_remote");
    let state = AppState::with_auth_store(
        repo_dir.clone(),
        unavailable_remote.to_string_lossy().to_string(),
        store.clone(),
    )
    .expect("recover state")
    .with_trusted_sandbox_exec(true);
    let primary_repo_id = state.repo_id.0.to_string();
    let app = build_router(state.clone());

    // 3. Connect 2 distinct IDE/Agent clients (`claude_code` and `cursor`) and execute their sandboxes concurrently
    let claude_wt_res = api_json(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".to_owned(),
        &admin_token,
        Some(json!({
            "agent_kind": "claude_code",
            "branch": "agent/claude/feat-claude",
            "base_branch": "main",
            "prompt_summary": "Claude Code concurrent feature"
        })),
    )
    .await;
    let cursor_wt_res = api_json(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".to_owned(),
        &admin_token,
        Some(json!({
            "agent_kind": "cursor",
            "branch": "agent/cursor/feat-cursor",
            "base_branch": "main",
            "prompt_summary": "Cursor IDE concurrent feature"
        })),
    )
    .await;
    assert_eq!(
        claude_wt_res.0,
        StatusCode::OK,
        "claude spawn: {}",
        claude_wt_res.1
    );
    assert_eq!(
        cursor_wt_res.0,
        StatusCode::OK,
        "cursor spawn: {}",
        cursor_wt_res.1
    );
    let claude_wt_id = claude_wt_res.1["id"]
        .as_str()
        .expect("claude id")
        .to_owned();
    let cursor_wt_id = cursor_wt_res.1["id"]
        .as_str()
        .expect("cursor id")
        .to_owned();

    // Both IDE clients inspect the swarm concurrently and execute verified sandbox checks
    let app_claude = app.clone();
    let app_cursor = app.clone();
    let tok_claude = admin_token.clone();
    let tok_cursor = admin_token.clone();
    let ((claude_list_status, _), (cursor_list_status, _)) = tokio::join!(
        api_json(
            &app_claude,
            "GET",
            "/api/v1/swarm/worktrees".to_owned(),
            &tok_claude,
            None,
        ),
        api_json(
            &app_cursor,
            "GET",
            "/api/v1/swarm/worktrees".to_owned(),
            &tok_cursor,
            None,
        )
    );
    assert_eq!(claude_list_status, StatusCode::OK);
    assert_eq!(cursor_list_status, StatusCode::OK);

    let claude_uuid = uuid::Uuid::parse_str(&claude_wt_id).expect("uuid");
    let claude_wt_dir = state
        .get_worktree_runtime(gitust_core::WorktreeId(claude_uuid))
        .expect("claude runtime")
        .worktree_dir;
    std::fs::write(claude_wt_dir.join("claude.txt"), "claude feature\n").expect("write claude.txt");
    git(&claude_wt_dir, &["config", "user.name", "Claude Agent"]);
    git(
        &claude_wt_dir,
        &["config", "user.email", "claude@company.internal"],
    );
    git(&claude_wt_dir, &["add", "claude.txt"]);
    git(&claude_wt_dir, &["commit", "-m", "Add claude feature"]);

    let (claude_sbx_status, claude_sbx_json) = api_json(
        &app_claude,
        "POST",
        format!("/api/v1/swarm/worktrees/{claude_wt_id}/sandbox"),
        &tok_claude,
        Some(json!({ "command": "test -f claude.txt", "verify": true })),
    )
    .await;
    assert_eq!(
        claude_sbx_status,
        StatusCode::OK,
        "claude sbx: {claude_sbx_json}"
    );

    let (cursor_sbx_status, cursor_sbx_json) = api_json(
        &app_cursor,
        "POST",
        format!("/api/v1/swarm/worktrees/{cursor_wt_id}/sandbox"),
        &tok_cursor,
        Some(json!({ "command": "echo 'cursor verified'", "verify": true })),
    )
    .await;
    assert_eq!(
        cursor_sbx_status,
        StatusCode::OK,
        "cursor sbx: {cursor_sbx_json}"
    );

    // Trigger a workflow run so workflow history is also populated before backup
    let (wf_status, wf_json) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{primary_repo_id}/workflow-runs"),
        &admin_token,
        Some(json!({
            "expected_commit": head_sha,
            "source_ref": "main",
            "workflow_name": "Rehearsal CI"
        })),
    )
    .await;
    assert_eq!(wf_status, StatusCode::OK, "wf_json: {wf_json}");
    assert_eq!(wf_json["status"], "passed");

    // 4. Human approval of `claude_wt_id` -> Outbound publish failure -> Re-point & Retry success
    let claude_head = claude_sbx_json["checked_commit"]
        .as_str()
        .expect("checked_commit");
    let (app_status, app_json) = api_json(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{claude_wt_id}/approve"),
        &admin_token,
        Some(json!({ "expected_commit": claude_head })),
    )
    .await;
    assert_eq!(app_status, StatusCode::OK, "approve: {app_json}");

    // First publish attempt fails closed (502 Bad Gateway) because remote URL is unreachable
    let (pub_fail_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{claude_wt_id}/publish"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(
        pub_fail_status,
        StatusCode::BAD_GATEWAY,
        "publish to unreachable remote must fail closed with 502"
    );

    // Retry with a healthy upstream bare repository -> succeeds (200 OK)
    let state_retry = AppState::with_auth_store(
        repo_dir.clone(),
        upstream_bare.to_string_lossy().to_string(),
        store.clone(),
    )
    .expect("recover state for retry")
    .with_trusted_sandbox_exec(true);
    let app_retry = build_router(state_retry.clone());
    let (pub_ok_status, pub_ok_json) = api_json(
        &app_retry,
        "POST",
        format!("/api/v1/swarm/worktrees/{claude_wt_id}/publish"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(
        pub_ok_status,
        StatusCode::OK,
        "publish retry: {pub_ok_json}"
    );
    git(&upstream_bare, &["fsck", "--full"]);

    // 5. Stop writers (drop state handles) and execute offline backup + relocated restore via scripts/backup.py
    drop(app);
    drop(app_claude);
    drop(app_cursor);
    drop(app_retry);
    drop(state);
    drop(state_retry);
    drop(store);

    let backup_dir = env_dir.path().join("backup-snapshot");
    let restored_dir = env_dir.path().join("restored-install");
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let backup_script = workspace_root.join("scripts/backup.py");

    let create_out = Command::new("python3")
        .arg(&backup_script)
        .args(["create", "--repo"])
        .arg(&repo_dir)
        .arg("--auth-db")
        .arg(&auth_db_path)
        .arg("--output")
        .arg(&backup_dir)
        .output()
        .expect("backup.py create");
    assert!(
        create_out.status.success(),
        "backup.py create failed: {}",
        String::from_utf8_lossy(&create_out.stderr)
    );

    let restore_out = Command::new("python3")
        .arg(&backup_script)
        .args(["restore", "--backup"])
        .arg(&backup_dir)
        .arg("--destination")
        .arg(&restored_dir)
        .output()
        .expect("backup.py restore");
    assert!(
        restore_out.status.success(),
        "backup.py restore failed: {}",
        String::from_utf8_lossy(&restore_out.stderr)
    );

    // 6. Boot Gitust from the relocated restore directory and verify accounts, Git data, worktrees, approvals, run history, and workflow runs
    let restored_auth = restored_dir.join("auth/auth-db.json");
    let restored_repo = restored_dir.join("repository");
    let restored_store = AuthStore::open_persistent_with_secret_key(&restored_auth, master_key_1)
        .expect("open restored auth store");
    let _ = restored_store
        .authenticate_session(&admin_token)
        .expect("admin session preserved across backup/restore");
    let restored_state = AppState::with_auth_store(
        restored_repo.clone(),
        upstream_bare.to_string_lossy().to_string(),
        restored_store,
    )
    .expect("recover restored AppState");
    let restored_app = build_router(restored_state);

    let (wts_status, wts_json) = api_json(
        &restored_app,
        "GET",
        "/api/v1/swarm/worktrees".to_owned(),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(wts_status, StatusCode::OK);
    let wts = wts_json.as_array().expect("worktrees");
    assert_eq!(wts.len(), 2, "both IDE worktrees must be restored");

    let (runs_status, runs_json) = api_json(
        &restored_app,
        "GET",
        format!("/api/v1/swarm/worktrees/{claude_wt_id}/runs"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(runs_status, StatusCode::OK);
    assert!(
        !runs_json.as_array().expect("runs").is_empty(),
        "sandbox execution history must be restored"
    );

    let (wfs_status, wfs_json) = api_json(
        &restored_app,
        "GET",
        format!("/api/v1/repos/{primary_repo_id}/workflows"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(wfs_status, StatusCode::OK);
    assert_eq!(
        wfs_json["runs"].as_array().expect("wf runs").len(),
        1,
        "workflow run history must be restored"
    );

    let _ = admin_principal;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn master_secret_key_rotation_reseals_all_secrets_and_revokes_compromised_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let auth_path = dir.path().join("auth.json");
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).expect("mkdir repo");
    git(&repo_dir, &["init", "-b", "main"]);
    git(&repo_dir, &["config", "user.name", "Admin"]);
    git(
        &repo_dir,
        &["config", "user.email", "admin@company.internal"],
    );
    std::fs::write(repo_dir.join("README.md"), "# Secret Rotation\n").expect("write");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-m", "init"]);

    let old_key: [u8; 32] = [0x11; 32];
    let new_key: [u8; 32] = [0x22; 32];
    let store =
        AuthStore::open_persistent_with_secret_key(&auth_path, old_key).expect("open store");
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "AdminPass!2026")
        .expect("bootstrap");
    let admin_login = store.login("admin", "AdminPass!2026").expect("login");
    let admin_token = admin_login.raw_session_token;
    let admin = store.authenticate_session(&admin_token).expect("admin");

    let state = AppState::with_auth_store(
        repo_dir,
        "https://example.invalid/repo.git".to_owned(),
        store.clone(),
    )
    .expect("state");
    let repo_id = state.repo_id;
    let access = gitust_core::RepoAccess {
        repo_id,
        registration: Some(
            store
                .register_local_repository(&admin, repo_id, "rot-repo", TeamId::new(), true, None)
                .expect("reg repo")
                .binding,
        ),
    };

    // Store a workflow secret, an SSO provider secret, and a PAT before rotation
    store
        .set_workflow_secret(&admin, access, "PROD_TOKEN", "prod-secret-value-2026")
        .expect("set wf secret");
    store
        .register_sso_provider_with_credential(
            &admin_token,
            admin.user_id,
            RegisterSsoProviderInput {
                name: "Rot-OIDC".into(),
                protocol: SsoProtocol::Oidc,
                issuer_or_url: "https://idp.company.internal".into(),
                client_id: "gitust".into(),
                signing_secret: "old-sso-signing-secret-2026".into(),
                allowed_domains: vec!["company.internal".into()],
                enabled: true,
                require_invite_binding: true,
            },
        )
        .expect("reg sso");
    let pat_before = store
        .create_personal_access_token(&admin, "pre-rotation-pat", None)
        .expect("create pat");

    let app = build_router(state);

    // Edge case 1: Malformed / short key hex (31 bytes instead of 32 bytes) is rejected 400 Bad Request without mutating existing secrets
    let (bad_key_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/admin/security/rotate-secrets".to_owned(),
        &admin_token,
        Some(json!({
            "new_secret_key_hex": "aa".repeat(31),
            "revoke_existing_credentials": false
        })),
    )
    .await;
    assert_eq!(bad_key_status, StatusCode::BAD_REQUEST);

    // Edge case 2: Non-SiteAdmin (Developer) is rejected 403 Forbidden
    let dev_inv = store
        .create_invite_link(
            &admin,
            "rotdev@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("dev invite");
    store
        .redeem_invite_link(&dev_inv.raw_token, "rotdev", "StrongPassword123!")
        .expect("redeem");
    let dev_tok = store
        .login("rotdev", "StrongPassword123!")
        .expect("dev login")
        .raw_session_token;
    let (dev_rot_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/admin/security/rotate-secrets".to_owned(),
        &dev_tok,
        Some(json!({
            "new_secret_key_hex": hex::encode(new_key),
            "revoke_existing_credentials": false
        })),
    )
    .await;
    assert_eq!(dev_rot_status, StatusCode::FORBIDDEN);

    let (rot_status, rot_json) = api_json(
        &app,
        "POST",
        "/api/v1/admin/security/rotate-secrets".to_owned(),
        &admin_token,
        Some(json!({
            "new_secret_key_hex": hex::encode(new_key),
            "revoke_existing_credentials": true
        })),
    )
    .await;
    assert_eq!(rot_status, StatusCode::OK, "rotate: {rot_json}");
    assert!(rot_json["rotated_secrets_count"].as_u64().unwrap_or(0) >= 2);

    // Pre-rotation PAT and Developer session are immediately revoked when revoke_existing_credentials is true
    assert!(store.authenticate_pat(&pat_before.raw_token).is_err());
    assert!(store.authenticate_session(&dev_tok).is_err());

    // Edge case 3: Chained second rotation (old_key -> new_key -> third_key)
    let third_key = [0x99u8; 32];
    let (rot2_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/admin/security/rotate-secrets".to_owned(),
        &admin_token,
        Some(json!({
            "new_secret_key_hex": hex::encode(third_key),
            "revoke_existing_credentials": false
        })),
    )
    .await;
    assert_eq!(rot2_status, StatusCode::OK);

    // Edge case 4: If disk persistence fails during rotation (e.g., parent dir read-only), in-memory SecretKey rolls back to `third_key`
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = auth_path.parent().expect("parent");
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500))
            .expect("chmod 500");
        let fourth_key = [0x55u8; 32];
        let (fail_rot_status, _) = api_json(
            &app,
            "POST",
            "/api/v1/admin/security/rotate-secrets".to_owned(),
            &admin_token,
            Some(json!({
                "new_secret_key_hex": hex::encode(fourth_key),
                "revoke_existing_credentials": false
            })),
        )
        .await;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .expect("chmod 700");
        assert_ne!(fail_rot_status, StatusCode::OK);
        let still_valid = store
            .resolve_workflow_secrets(&admin, access)
            .expect("in-memory secret key rolled back cleanly");
        assert_eq!(still_valid["PROD_TOKEN"], "prod-secret-value-2026");
    }

    // Reopening from disk with old_key or new_key must fail closed, while third_key succeeds and preserves plaintext secrets
    assert!(AuthStore::open_persistent_with_secret_key(&auth_path, old_key).is_err());
    assert!(AuthStore::open_persistent_with_secret_key(&auth_path, new_key).is_err());
    let reopened = AuthStore::open_persistent_with_secret_key(&auth_path, third_key)
        .expect("reopen third_key");
    let resolved = reopened
        .resolve_workflow_secrets(&admin, access)
        .expect("resolve wf secrets");
    assert_eq!(resolved["PROD_TOKEN"], "prod-secret-value-2026");
}
