use super::tests_verification::{fixture, run};
use super::*;

fn policy(repo: &std::path::Path, commands: &[&str]) {
    std::fs::write(
        repo.join(".gitust-checks.json"),
        serde_json::json!({
            "version": 1, "commands": commands,
        })
        .to_string(),
    )
    .unwrap();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["add", ".gitust-checks.json"])
        .status()
        .unwrap();
    assert!(status.success());
    super::tests_verification::commit(repo);
}

#[tokio::test]
async fn http_requested_failure_identifies_skipped_required_checks() {
    let (tmp, state, app, token, wt, _dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        &["printf required-one", "printf required-two"],
    );
    let (status, body) = run(&app, &token, &wt, "printf requested; exit 3").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(result["exit_code"], 3);
    assert_eq!(result["stdout"], "requested");
    let steps = result["steps"].as_array().expect("command-level results");
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0]["required"], false);
    assert_eq!(steps[0]["exit_code"], 3);
    assert!(steps[0]["duration_ms"].is_u64());
    for step in &steps[1..] {
        assert_eq!(step["required"], true);
        assert!(step["exit_code"].is_null());
        assert!(step["duration_ms"].is_null());
    }
    assert_eq!(steps[1]["command"], "printf required-one");
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn mcp_exposes_required_check_failure_and_skips_without_parsing_command_output() {
    let (tmp, state, _app, token, wt, _dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        &["printf first", "printf second; exit 7", "printf skipped"],
    );
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let request = serde_json::json!({
        "jsonrpc":"2.0", "id":41, "method":"tools/call", "params": {
            "name":"gitust_sandbox_exec", "arguments": {
                "worktree_id":wt.id.0, "command":"printf '{\"steps\":[{\"exit_code\":0}]}'", "verify":true,
            }
        }
    }).to_string();
    let response = handle_mcp_json_line(&state, &caller, &request)
        .await
        .unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(envelope["result"]["isError"], true);
    let result: serde_json::Value =
        serde_json::from_str(envelope["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["status"], "failed");
    assert_eq!(result["exit_code"], 7);
    assert_eq!(result["output_truncated"], false);
    let steps = result["steps"]
        .as_array()
        .expect("MCP command-level results");
    assert_eq!(steps.len(), 4);
    assert_eq!(steps[0]["exit_code"], 0);
    assert_eq!(steps[1]["exit_code"], 0);
    assert_eq!(steps[2]["exit_code"], 7);
    assert!(steps[3]["exit_code"].is_null());
    assert!(steps[3]["duration_ms"].is_null());
    assert!(result["stdout"].as_str().unwrap().contains("{\"steps\""));
    assert!(!result["stdout"].as_str().unwrap().contains("skipped"));
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Failed);
    assert!(state.worktrees.read()[0].verified_checks_oid.is_none());
}

#[tokio::test]
async fn ordinary_execution_reports_only_requested_step_and_does_not_grant_verification() {
    let (tmp, state, app, token, wt, _dir) = fixture().await;
    policy(&tmp.path().join("repo"), &["exit 9"]);
    let (status, body) = call(
        &app,
        "POST",
        format!("/api/v1/swarm/worktrees/{}/sandbox", wt.id.0),
        &token,
        Some(serde_json::json!({"command":"printf ordinary", "verify":false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(result["steps"].as_array().unwrap().len(), 1);
    assert_eq!(result["steps"][0]["required"], false);
    assert_eq!(result["steps"][0]["exit_code"], 0);
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Pending);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}
