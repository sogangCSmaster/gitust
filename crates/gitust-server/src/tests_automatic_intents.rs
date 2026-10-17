use super::{
    tests_automatic_checks::{git, policy},
    tests_verification::fixture,
    *,
};
use crate::automatic_run_model::Phase;
use gitust_core::CommitSha;

#[tokio::test]
async fn later_http_preflight_inspection_failure_rejects_every_earlier_intent() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        true,
        "printf pending-before-preflight",
    );
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let _service = crate::automatic_checks::start(&state).await.unwrap();
    let guard = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .clone()
        .lock_owned()
        .await;
    let before = git(&dir, &["rev-parse", "HEAD"]);
    crate::automatic_checks::after_commit(&state, &caller, wt.id, CommitSha(before.clone()), false)
        .unwrap();
    let store = state.automatic_checks.store().unwrap();
    let original = store
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap()
        .queued
        .unwrap()
        .run_id;
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &token,
        Some(serde_json::json!({"branch":"agent/test/broken-ref", "prompt_summary":"second ref"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let second: AgentWorktree = serde_json::from_str(&body).unwrap();
    let common = state.git_engine.repository_common_directory().unwrap();
    std::fs::write(
        common.join("refs/heads").join(&second.branch),
        b"invalid ref\n",
    )
    .unwrap();
    let updates = vec![
        gitust_git::ReceivePackRefUpdate {
            old_oid: before.clone(),
            new_oid: "c".repeat(40),
            refname: format!("refs/heads/{}", wt.branch),
            is_agent_branch: true,
            is_deletion: false,
        },
        gitust_git::ReceivePackRefUpdate {
            old_oid: before.clone(),
            new_oid: "d".repeat(40),
            refname: format!("refs/heads/{}", second.branch),
            is_agent_branch: true,
            is_deletion: false,
        },
    ];
    let matched = vec![
        (wt.id, wt.branch.clone(), wt.status),
        (second.id, second.branch, second.status),
    ];
    assert!(crate::git_http_automatic::prepare(&state, &caller, &matched, &updates).is_err());
    let doc = store
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap();
    assert!(doc.prepared.is_none());
    assert_eq!(doc.queued.unwrap().run_id, original);
    assert_eq!(doc.recent[0].phase, Phase::Rejected);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), before);
    let intents =
        crate::git_http_automatic::prepare(&state, &caller, &matched[..1], &updates[..1]).unwrap();
    assert_eq!(intents.len(), 1);
    for intent in intents {
        crate::automatic_checks::reject(intent).unwrap();
    }
    drop(guard);
}

#[cfg(unix)]
#[tokio::test]
async fn failing_intent_persistence_rejects_mcp_before_ref_publication() {
    use std::os::unix::fs::symlink;
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf must-not-run");
    let _service = crate::automatic_checks::start(&state).await.unwrap();
    let before = git(&dir, &["rev-parse", "HEAD"]);
    let path = state
        .automatic_checks
        .store()
        .unwrap()
        .path()
        .join(format!("{}.json.pending", wt.id.0));
    let outside = tmp.path().join("keep-intent-evidence");
    std::fs::write(&outside, b"unchanged").unwrap();
    symlink(&outside, path).unwrap();
    std::fs::write(dir.join("README.md"), "not published\n").unwrap();
    let (_, body) = call(&app, "POST", "/mcp".into(), &token, Some(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"gitust_worktree_commit","arguments":{"worktree_id":wt.id.0,"expected_head":before,"paths":["README.md"],"message":"deny unsafe journal"}}}))).await;
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["result"]["isError"], true, "{body}");
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(std::fs::read(outside).unwrap(), b"unchanged");
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}
