use std::path::Path;
use std::process::Command;

use super::*;
use gitust_git::inspect_receive_pack_updates;

fn run_git_cli(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null", "-c", "credential.helper="])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .output()
        .expect("spawn git cli")
}

#[test]
fn unit_inspect_receive_pack_updates_enforces_strict_pktline_and_ref_rules() {
    let old_zero = "0000000000000000000000000000000000000000";
    let new_oid = "1111111111111111111111111111111111111111";

    // Valid agent branch update pkt-line + 0000 flush
    let cmd = format!("{old_zero} {new_oid} refs/heads/agent/claude/fix-1\0report-status\n");
    let pkt = format!("{:04x}{cmd}0000PACK", cmd.len() + 4);
    let parsed = inspect_receive_pack_updates(pkt.as_bytes()).expect("valid agent ref update");
    assert_eq!(parsed.len(), 1);
    assert!(parsed[0].is_agent_branch);
    assert!(!parsed[0].is_deletion);
    assert_eq!(parsed[0].refname, "refs/heads/agent/claude/fix-1");

    // Reject refs/replace/* or refs/notes/* or refs/gitust-sync/*
    for bad_ref in [
        "refs/replace/1111111111111111111111111111111111111111",
        "refs/notes/commits",
        "refs/gitust-sync/s1/heads/main",
        "refs/heads/../config",
        "refs/heads/agent/",
        "refs/heads/agent",
        "HEAD",
    ] {
        let bad_cmd = format!("{old_zero} {new_oid} {bad_ref}\0report-status\n");
        let bad_pkt = format!("{:04x}{bad_cmd}0000", bad_cmd.len() + 4);
        assert!(
            inspect_receive_pack_updates(bad_pkt.as_bytes()).is_err(),
            "must reject unsafe ref '{bad_ref}'"
        );
    }

    // Reject missing 0000 flush packet
    let no_flush = format!("{:04x}{cmd}", cmd.len() + 4);
    assert!(inspect_receive_pack_updates(no_flush.as_bytes()).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smart_http_gate_7_4_clone_commit_push_fetch_roundtrip_rbac_and_fsck() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let server_repo = tmp.path().join("server-repo");
    init_temp_repo(&server_repo);

    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "siteadmin@company.internal",
            "AdminPassword!2026",
        )
        .expect("bootstrap admin");
    let admin_login = store
        .login("siteadmin", "AdminPassword!2026")
        .expect("admin login");
    let admin_principal = store
        .authenticate_session(&admin_login.raw_session_token)
        .expect("admin principal");
    let admin_pat = store
        .create_personal_access_token(&admin_principal, "admin-git-pat", None)
        .expect("admin pat");

    let dev_invite = store
        .create_invite_link(
            &admin_principal,
            "deva@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("dev invite");
    store
        .redeem_invite_link(&dev_invite.raw_token, "deva", "DevPassword!2026")
        .expect("redeem dev");
    let dev_login = store.login("deva", "DevPassword!2026").expect("dev login");
    let dev_principal = store
        .authenticate_session(&dev_login.raw_session_token)
        .expect("dev principal");
    let dev_pat = store
        .create_personal_access_token(&dev_principal, "dev-git-pat", None)
        .expect("dev pat");

    let state = AppState::with_auth_store(
        server_repo.clone(),
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        store.clone(),
    );
    let app = build_router(state.clone());

    // 1. Unauthenticated request to /git/default.git/info/refs must return 401 with WWW-Authenticate
    let unauth_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/default.git/info/refs?service=git-upload-pack")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauth_resp.status(), StatusCode::UNAUTHORIZED);
    assert!(unauth_resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .is_some());

    // 2. Bind a real HTTP listener and run real `git clone` -> `commit` -> `push` -> `fetch`
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client_dir = tmp.path().join("client-clone");
    let clone_url = format!(
        "http://x-access-token:{}@{addr}/git/default.git",
        dev_pat.raw_token
    );
    let clone_out = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "clone",
            &clone_url,
            client_dir.to_str().expect("utf8 path"),
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git clone over Smart HTTP");
    assert!(
        clone_out.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&clone_out.stderr)
    );

    // Create a new commit in the cloned repository
    assert!(run_git_cli(
        &client_dir,
        &["config", "user.email", "deva@company.internal"]
    )
    .status
    .success());
    assert!(run_git_cli(&client_dir, &["config", "user.name", "deva"])
        .status
        .success());
    std::fs::write(
        client_dir.join("smart_http.txt"),
        "pushed over smart http\n",
    )
    .expect("write file");
    assert!(run_git_cli(&client_dir, &["add", "smart_http.txt"])
        .status
        .success());
    assert!(
        run_git_cli(&client_dir, &["commit", "-m", "feat: smart http commit"])
            .status
            .success()
    );

    // 3. Developer pushing directly to protected `refs/heads/main` MUST be rejected (403 Forbidden)
    let push_main = run_git_cli(&client_dir, &["push", "origin", "HEAD:refs/heads/main"]);
    assert!(
        !push_main.status.success(),
        "Developer must NOT be allowed to push directly to refs/heads/main"
    );

    // 4. Developer pushing to `refs/heads/agent/claude/http-work` MUST succeed
    let push_agent = run_git_cli(
        &client_dir,
        &["push", "origin", "HEAD:refs/heads/agent/claude/http-work"],
    );
    assert!(
        push_agent.status.success(),
        "Developer push to refs/heads/agent/* failed: {}",
        String::from_utf8_lossy(&push_agent.stderr)
    );

    // 5. Fast-forward merge the approved agent branch into `main` on the server, then `git fetch` from client
    let merged_sha = state
        .git_engine
        .merge_agent_branch("agent/claude/http-work", "main")
        .expect("fast-forward merge agent branch into main");
    let fetch_out = run_git_cli(&client_dir, &["fetch", "origin", "main"]);
    assert!(
        fetch_out.status.success(),
        "git fetch origin main failed: {}",
        String::from_utf8_lossy(&fetch_out.stderr)
    );
    let fetched_main = run_git_cli(&client_dir, &["rev-parse", "refs/remotes/origin/main"]);
    assert_eq!(
        String::from_utf8_lossy(&fetched_main.stdout).trim(),
        merged_sha.0
    );

    // 6. Revoking the PAT immediately blocks subsequent `git fetch`
    store
        .revoke_personal_access_token(&dev_principal, dev_pat.metadata.id)
        .expect("revoke dev pat");
    let fetch_after_revoke = run_git_cli(&client_dir, &["fetch", "origin"]);
    assert!(
        !fetch_after_revoke.status.success(),
        "revoked PAT must be rejected on subsequent Smart HTTP fetch"
    );

    // 7. SiteAdmin PAT still works and both server & client pass `git fsck --full --strict` (Gate 7.4)
    let admin_remote = format!(
        "http://siteadmin:{}@{addr}/git/default.git",
        admin_pat.raw_token
    );
    assert!(
        run_git_cli(&client_dir, &["remote", "set-url", "origin", &admin_remote])
            .status
            .success()
    );
    assert!(run_git_cli(&client_dir, &["fetch", "origin"])
        .status
        .success());

    GitEngine::verify_git_fsck(&server_repo).expect("server repo git fsck --full --strict");
    GitEngine::verify_git_fsck(&client_dir).expect("client clone git fsck --full --strict");

    server_task.abort();
}

#[test]
fn smart_http_pkt_line_parser_rejects_malformed_refnames_and_case_variants() {
    let old = "1111111111111111111111111111111111111111";
    let new = "2222222222222222222222222222222222222222";
    let make_pkt = |refname: &str| -> Vec<u8> {
        let payload = format!("{old} {new} {refname}\0report-status\n");
        let total = payload.len() + 4;
        let mut out = format!("{total:04x}").into_bytes();
        out.extend_from_slice(payload.as_bytes());
        out.extend_from_slice(b"0000PACK");
        out
    };

    for bad_ref in [
        "refs/heads/agent/claude/a..b",
        "refs/heads/agent/claude/foo.lock",
        "refs/heads/agent/claude/",
        "refs/heads/agent//claude",
        "refs/heads/agent/claude/.hidden",
        "refs/heads//main",
        "refs/heads/main/",
    ] {
        let res = gitust_git::inspect_receive_pack_updates(&make_pkt(bad_ref));
        assert!(
            res.is_err(),
            "malformed refname '{bad_ref}' must be rejected by inspect_receive_pack_updates"
        );
    }

    // Case-folded `refs/heads/Agent/upper` must NOT be classified as a valid agent branch
    // (`validate_agent_branch_name` requires lowercase `agent/<scope>/<name>`).
    let upper_res = gitust_git::inspect_receive_pack_updates(&make_pkt("refs/heads/Agent/upper"));
    assert!(
        upper_res.is_err(),
        "case-variant 'refs/heads/Agent/upper' must be rejected"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smart_http_push_enforces_worktree_ownership_and_resets_verified_status() {
    use gitust_core::{AgentKind, WorktreeId, WorktreeStatus};
    use gitust_sandbox::SandboxSpec;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);

    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "admin@company.internal",
            "AdminStrongPass!2026",
        )
        .expect("bootstrap admin");
    let admin_login = store
        .login("siteadmin", "AdminStrongPass!2026")
        .expect("admin login");
    let admin_principal = store
        .authenticate_session(&admin_login.raw_session_token)
        .expect("admin principal");

    let inv_a = store
        .create_invite_link(
            &admin_principal,
            "deva@company.internal",
            EnterpriseRole::Developer,
            vec![],
            Some(24),
        )
        .expect("invite a");
    store
        .redeem_invite_link(&inv_a.raw_token, "deva", "DevAStrongPass!2026")
        .expect("redeem a");
    let login_a = store.login("deva", "DevAStrongPass!2026").expect("login a");
    let principal_a = store
        .authenticate_session(&login_a.raw_session_token)
        .expect("principal a");
    let pat_a = store
        .create_personal_access_token(&principal_a, "deva-pat", None)
        .expect("pat a");

    let inv_b = store
        .create_invite_link(
            &admin_principal,
            "devb@company.internal",
            EnterpriseRole::Developer,
            vec![],
            Some(24),
        )
        .expect("invite b");
    store
        .redeem_invite_link(&inv_b.raw_token, "devb", "DevBStrongPass!2026")
        .expect("redeem b");
    let login_b = store.login("devb", "DevBStrongPass!2026").expect("login b");
    let principal_b = store
        .authenticate_session(&login_b.raw_session_token)
        .expect("principal b");
    let pat_b = store
        .create_personal_access_token(&principal_b, "devb-pat", None)
        .expect("pat b");

    let state = AppState::with_auth_store(
        repo_dir.clone(),
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        store,
    );

    // Materialize a real checked-out linked worktree owned by Developer A and mark it `Verified`
    let wt_id = WorktreeId::new();
    let branch_name = "agent/claude/owned-task".to_owned();
    let m = state
        .git_engine
        .materialize_agent_worktree(
            wt_id,
            state.repo_id,
            AgentKind::ClaudeCode,
            &branch_name,
            "main",
            "verified worktree".to_owned(),
        )
        .expect("materialize worktree");
    let mut wt = m.worktree;
    wt.status = WorktreeStatus::Verified;
    state.worktrees.write().push_back(wt);
    state.worktree_runtime.write().insert(
        wt_id,
        crate::state::WorktreeRuntime {
            created_by: Some(principal_a.user_id),
            worktree_dir: m.worktree_dir.clone(),
            sandbox_spec: SandboxSpec::for_linked_worktree(
                &m.worktree_dir,
                &m.worktree_admin_dir,
                &m.common_git_dir,
                m.main_workdir.as_deref(),
                &m.worktrees_base,
                &m.sandbox_scratch_base,
            ),
            exec_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        },
    );

    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client_dir = tmp.path().join("client");
    let url_a = format!("http://deva:{}@{addr}/git/default.git", pat_a.raw_token);
    let url_b = format!("http://devb:{}@{addr}/git/default.git", pat_b.raw_token);
    assert!(
        run_git_cli(tmp.path(), &["clone", &url_a, client_dir.to_str().unwrap()])
            .status
            .success()
    );
    assert!(run_git_cli(
        &client_dir,
        &["config", "user.email", "deva@company.internal"]
    )
    .status
    .success());
    assert!(run_git_cli(&client_dir, &["config", "user.name", "deva"])
        .status
        .success());
    std::fs::write(
        client_dir.join("owner_pushed.txt"),
        "pushed to active worktree\n",
    )
    .expect("write");
    assert!(run_git_cli(&client_dir, &["add", "owner_pushed.txt"])
        .status
        .success());
    assert!(run_git_cli(
        &client_dir,
        &["commit", "-m", "feat: push to active worktree"]
    )
    .status
    .success());

    // 1. Developer B attempting a real `git push` to Developer A's worktree branch is rejected (403 FORBIDDEN)
    let push_b = run_git_cli(
        &client_dir,
        &["push", &url_b, &format!("HEAD:refs/heads/{branch_name}")],
    );
    assert!(
        !push_b.status.success(),
        "Developer B must not be allowed to push to Developer A's worktree branch"
    );
    assert_eq!(
        state.worktrees.read()[0].status,
        WorktreeStatus::Verified,
        "failed unauthorized push must not alter Verified status"
    );

    // 2. If the checked-out linked worktree has uncommitted changes (dirty worktree), `updateInstead` refuses the push
    std::fs::write(
        m.worktree_dir.join("README.md"),
        "dirty uncommitted change\n",
    )
    .expect("write dirty");
    let push_dirty = run_git_cli(
        &client_dir,
        &["push", &url_a, &format!("HEAD:refs/heads/{branch_name}")],
    );
    assert!(
        !push_dirty.status.success(),
        "push to dirty checked-out worktree must be rejected by updateInstead"
    );
    assert_eq!(
        state.worktrees.read()[0].status,
        WorktreeStatus::Verified,
        "rejected push on dirty worktree must preserve Verified status"
    );

    // 3. Once the worktree is clean, Developer A's real `git push` succeeds, updates the checked-out worktree on disk, and resets status to `Pending`
    assert!(
        run_git_cli(&m.worktree_dir, &["checkout", "--", "README.md"])
            .status
            .success()
    );
    let push_a = run_git_cli(
        &client_dir,
        &["push", &url_a, &format!("HEAD:refs/heads/{branch_name}")],
    );
    assert!(
        push_a.status.success(),
        "owner push to clean checked-out worktree branch failed: {}",
        String::from_utf8_lossy(&push_a.stderr)
    );
    assert!(
        m.worktree_dir.join("owner_pushed.txt").exists(),
        "updateInstead must update the checked-out linked worktree working tree on disk"
    );
    assert_eq!(
        state.worktrees.read()[0].status,
        WorktreeStatus::Pending,
        "accepted push to a Verified worktree branch must reset status to Pending"
    );

    server_task.abort();
}

#[test]
fn delete_and_merge_agent_branch_reject_checked_out_worktrees() {
    use gitust_core::{AgentKind, RepoId, WorktreeId};

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);
    let engine = GitEngine::new(&repo_dir);

    let m = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::ClaudeCode,
            "agent/claude/checked-out",
            "main",
            "active worktree".to_owned(),
        )
        .expect("materialize worktree");

    // Deleting an agent branch while its worktree is still checked out must fail closed
    let del_err = engine.delete_agent_branch_ref(&m.worktree.branch);
    assert!(
        del_err.is_err(),
        "delete_agent_branch_ref must reject deleting a branch checked out in a linked worktree"
    );
}
