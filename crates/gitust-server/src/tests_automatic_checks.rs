use super::tests_verification::{commit, fixture, run};
use super::*;
use gitust_core::CommitSha;
use serde_json::{json, Value};

pub(super) fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

pub(super) fn policy(repo: &std::path::Path, enabled: bool, command: &str) {
    std::fs::write(
        repo.join(".gitust-checks.json"),
        json!({"version":1,"auto_run":enabled,"commands":[command]}).to_string(),
    )
    .unwrap();
    git(repo, &["add", ".gitust-checks.json"]);
    commit(repo);
}

async fn commit_via_mcp(
    app: &Router,
    token: &str,
    wt: &AgentWorktree,
    dir: &std::path::Path,
) -> Value {
    let before = git(dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("README.md"), "automatic revision\n").unwrap();
    let (status, body) = call(
        app,
        "POST",
        "/mcp".into(),
        token,
        Some(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call", "params":{
            "name":"gitust_worktree_commit", "arguments":{"worktree_id":wt.id.0,
            "expected_head":before,"paths":["README.md"],"message":"Automatic checks fixture"}}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["result"]["isError"], false, "{body}");
    serde_json::from_str(value["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

pub(super) async fn settle(
    state: &AppState,
    wt: &AgentWorktree,
) -> Vec<crate::run_history::RunRecord> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while state.automatic_checks.is_running(wt.id) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automatic Checks completed");
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    crate::run_history::read(&runtime.common_git_dir, wt.id, &wt.branch).unwrap()
}

#[tokio::test]
async fn automatic_checks_after_mcp_commit_record_revision_without_approval() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        true,
        "printf automatic-check; test -f README.md",
    );
    git(&dir, &["merge", "--no-edit", "main"]);
    let _service = crate::automatic_checks::start(&state).unwrap();
    let result = commit_via_mcp(&app, &token, &wt, &dir).await;
    let records = settle(&state, &wt).await;
    assert_eq!(
        records.len(),
        1,
        "a successful MCP commit must trigger required Checks"
    );
    let records = serde_json::to_value(records).unwrap();
    assert_eq!(records[0]["automatic"], true);
    assert_eq!(records[0]["verification_requested"], false);
    assert_eq!(records[0]["checked_commit"], result["commit"]);
    assert_eq!(
        records[0]["checks_blob_oid"],
        git(
            &tmp.path().join("repo"),
            &["rev-parse", "main:.gitust-checks.json"]
        )
    );
    assert_eq!(
        records[0]["result"]["steps"][1]["output"]["stdout"],
        "automatic-check"
    );
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Pending);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    let url = format!("/api/v1/swarm/worktrees/{}/approve", wt.id.0);
    assert_eq!(
        call(
            &app,
            "POST",
            url.clone(),
            &token,
            Some(json!({"expected_commit":result["commit"]}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    assert_eq!(
        call(
            &app,
            "POST",
            url,
            &token,
            Some(json!({"expected_commit":result["commit"]}))
        )
        .await
        .0,
        StatusCode::OK
    );
    git(&tmp.path().join("repo"), &["fsck", "--strict"]);
}

#[tokio::test]
async fn disabled_policy_preserves_manual_checks_and_failed_automatic_checks_stay_failed() {
    for (enabled, exit) in [(false, 0), (true, 7)] {
        let (tmp, state, app, token, wt, dir) = fixture().await;
        policy(&tmp.path().join("repo"), enabled, &format!("exit {exit}"));
        let _service = crate::automatic_checks::start(&state).unwrap();
        commit_via_mcp(&app, &token, &wt, &dir).await;
        let records = settle(&state, &wt).await;
        assert_eq!(records.len(), usize::from(enabled));
        assert_eq!(
            state.worktrees.read()[0].status,
            if enabled {
                WorktreeStatus::Failed
            } else {
                WorktreeStatus::Pending
            }
        );
        assert!(state.worktrees.read()[0].verified_commit.is_none());
        if enabled {
            assert_eq!(records[0].result.exit_code, exit);
        }
    }
}

#[tokio::test]
async fn queued_updates_coalesce_and_never_replace_manual_verification() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf latest-check");
    let _service = crate::automatic_checks::start(&state).unwrap();
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let guard = runtime.exec_lock.lock().await;
    let first = CommitSha(git(&dir, &["rev-parse", "HEAD"]));
    assert!(crate::automatic_checks::after_commit(&state, &caller, wt.id, first, false).unwrap());
    commit(&dir);
    let latest = CommitSha(git(&dir, &["rev-parse", "HEAD"]));
    for _ in 0..3 {
        assert!(crate::automatic_checks::after_commit(
            &state,
            &caller,
            wt.id,
            latest.clone(),
            false
        )
        .unwrap());
    }
    drop(guard);
    let records = serde_json::to_value(settle(&state, &wt).await).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["checked_commit"], latest.0);
    assert_eq!(run(&app, &token, &wt, "true").await.0, StatusCode::OK);
    assert!(
        crate::automatic_checks::after_commit(&state, &caller, wt.id, latest.clone(), false)
            .unwrap()
    );
    assert_eq!(settle(&state, &wt).await.len(), 2);
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Verified);
    assert_eq!(state.worktrees.read()[0].verified_commit, Some(latest));
}

#[tokio::test]
async fn queued_runs_stop_on_shutdown_and_unavailable_scheduling_is_explicit() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf should-not-run");
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let service = crate::automatic_checks::start(&state).unwrap();
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let guard = runtime.exec_lock.lock().await;
    let sha = CommitSha(git(&dir, &["rev-parse", "HEAD"]));
    crate::automatic_checks::after_commit(&state, &caller, wt.id, sha, false).unwrap();
    drop(service);
    drop(guard);
    assert!(settle(&state, &wt).await.is_empty());
    let committed = commit_via_mcp(&app, &token, &wt, &dir).await;
    assert_eq!(committed["automatic_checks"], "unavailable");
    assert_eq!(committed["status"], "failed");
    assert_eq!(committed["commit"], git(&dir, &["rev-parse", "HEAD"]));
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn automatic_checks_require_trusted_mode_ownership_and_live_admin() {
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf should-not-run");
    let service = crate::automatic_checks::start(&state).unwrap();
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let sha = CommitSha(git(&dir, &["rev-parse", "HEAD"]));
    assert!(!crate::automatic_checks::after_commit(
        &state.clone().with_trusted_sandbox_exec(false),
        &caller,
        wt.id,
        sha.clone(),
        false
    )
    .unwrap());
    let mut developer = caller.clone();
    developer.role = EnterpriseRole::Developer;
    assert!(
        !crate::automatic_checks::after_commit(&state, &developer, wt.id, sha.clone(), false)
            .unwrap()
    );
    let invite = state
        .auth_store
        .create_invite_link(
            &caller,
            "other@company.internal",
            EnterpriseRole::SiteAdmin,
            vec![],
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, "other", "Password!2026")
        .unwrap();
    let other_token = state
        .auth_store
        .login("other", "Password!2026")
        .unwrap()
        .raw_session_token;
    let other = state.auth_store.authenticate_session(&other_token).unwrap();
    assert!(
        !crate::automatic_checks::after_commit(&state, &other, wt.id, sha.clone(), false).unwrap()
    );
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let guard = runtime.exec_lock.lock().await;
    crate::automatic_checks::after_commit(&state, &caller, wt.id, sha, false).unwrap();
    state
        .auth_store
        .deactivate_user(&other, caller.user_id)
        .unwrap();
    drop(guard);
    assert!(settle(&state, &wt).await.is_empty());
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    drop(service);
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_service_cancels_an_active_automatic_command_and_releases_its_lock() {
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        true,
        "echo $$ > automatic.pid; touch automatic.started; sleep 10; touch automatic.finished",
    );
    let service = crate::automatic_checks::start(&state).unwrap();
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let sha = CommitSha(git(&dir, &["rev-parse", "HEAD"]));
    crate::automatic_checks::after_commit(&state, &caller, wt.id, sha, false).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !dir.join("automatic.started").is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automatic command started");
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Compiling);
    let pid: i32 = std::fs::read_to_string(dir.join("automatic.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(pid > 0);
    drop(service);
    let runtime = state.get_worktree_runtime(wt.id).unwrap();
    let _guard = tokio::time::timeout(std::time::Duration::from_secs(5), runtime.exec_lock.lock())
        .await
        .unwrap();
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while unsafe { libc::kill(-pid, 0) } == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automatic process group stopped");
    assert!(!dir.join("automatic.finished").exists());
    assert!(
        crate::run_history::read(&runtime.common_git_dir, wt.id, &wt.branch)
            .unwrap()
            .is_empty()
    );
}
