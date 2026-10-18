use super::tests_pull_requests::{api_json, git, invite_and_login_role};
use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_expected_head_sha_alias_and_deleted_branch_detail_and_reopen_duplicate_and_commit_message_bounds(
) {
    let (_temp, state, admin_token, _) = fixture();
    let repo_id = state.repo_id.0;
    let repo = state.git_engine.repo_path().to_path_buf();

    git(&repo, &["checkout", "-b", "feature/regressions"]);
    std::fs::write(repo.join("reg.txt"), "v1\n").unwrap();
    git(&repo, &["add", "reg.txt"]);
    git(&repo, &["commit", "-m", "feat: regression v1"]);
    let head_sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "main"]);

    let app = build_router(state.clone());

    // Create PR #1 for feature/regressions -> main
    let (status, created) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls"),
        &admin_token,
        Some(json!({
            "title": "Regression PR 1",
            "description": "Tests Gate A findings",
            "base_branch": "main",
            "head_branch": "feature/regressions"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create PR #1: {created}");

    // 1. Finding 1: AddReviewPayload accepts `expected_head_sha` alias and rejects stale `expected_head_sha`
    let (status, stale_rev) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls/1/reviews"),
        &admin_token,
        Some(json!({
            "verdict": "approve",
            "body": "stale sha check",
            "expected_head_sha": "1111111111111111111111111111111111111111"
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "stale expected_head_sha must 409: {stale_rev}"
    );

    let (status, ok_rev) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls/1/reviews"),
        &admin_token,
        Some(json!({
            "verdict": "approve",
            "body": "matching expected_head_sha",
            "expected_head_sha": head_sha
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "matching expected_head_sha must 200: {ok_rev}"
    );

    // 4a. Finding 4: Merge commit_message rejects NUL or >4096 bytes with 400 BAD_REQUEST
    let (status, nul_msg) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls/1/merge"),
        &admin_token,
        Some(json!({
            "commit_message": "bad\u{0000}message"
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "NUL in commit_message must 400: {nul_msg}"
    );

    let (status, long_msg) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls/1/merge"),
        &admin_token,
        Some(json!({
            "commit_message": "x".repeat(4097)
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        ">4096 byte commit_message must 400: {long_msg}"
    );

    // 4b. Finding 4: Closing PR #1, opening PR #2 for the same branch pair, and trying to reopen PR #1 must 409 CONFLICT
    let (status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls/1/status"),
        &admin_token,
        Some(json!({ "status": "closed" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, created2) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls"),
        &admin_token,
        Some(json!({
            "title": "Regression PR 2",
            "description": "Second open PR for same branch pair",
            "base_branch": "main",
            "head_branch": "feature/regressions"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create PR #2: {created2}");

    let (status, reopen_dup) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls/1/status"),
        &admin_token,
        Some(json!({ "status": "open" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "reopening PR #1 while PR #2 is open for same branches must 409: {reopen_dup}"
    );

    // 2. Finding 2: Deleting head branch out-of-band still allows GET /pulls/2 to return 200 OK with can_merge: false
    git(&repo, &["branch", "-D", "feature/regressions"]);
    let (status, detail_deleted) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{repo_id}/pulls/2"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET detail when branch deleted out-of-band must 200: {detail_deleted}"
    );
    assert_eq!(detail_deleted["merge_preview"]["can_merge"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wildcard_specificity_and_private_repo_team_removal_invalidate_maintainer_review() {
    let (_temp, state, admin_token, _) = fixture();
    let repo_id = state.repo_id.0;
    let repo = state.git_engine.repo_path().to_path_buf();

    let admin_principal = state.auth_store.authenticate_session(&admin_token).unwrap();
    let team_id = state
        .auth_store
        .authorize_repository_access(&admin_principal, state.repo_id)
        .unwrap()
        .owner_team_id;
    let maint_token = invite_and_login_role(
        &state,
        &admin_token,
        "maint-team@company.internal",
        "maint_team",
        EnterpriseRole::Maintainer,
        vec![team_id],
    );
    let maint_principal = state.auth_store.authenticate_session(&maint_token).unwrap();

    // Create release/v1.0 branch and feature/rel-fix branch
    git(&repo, &["checkout", "-b", "release/v1.0"]);
    git(&repo, &["checkout", "-b", "feature/rel-fix"]);
    std::fs::write(repo.join("rel.txt"), "rel-fix\n").unwrap();
    git(&repo, &["add", "rel.txt"]);
    git(&repo, &["commit", "-m", "fix: release v1.0"]);
    git(&repo, &["checkout", "main"]);

    let app = build_router(state.clone());

    // 3a. Finding 3: Configure "*" first and "release/*" second; "release/*" must win for "release/v1.0"
    let (status, rules) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{repo_id}/branch-protection"),
        &admin_token,
        Some(json!({
            "rules": [
                {
                    "branch_pattern": "*",
                    "require_pull_request": false,
                    "required_approvals": 0,
                    "require_passing_checks": false
                },
                {
                    "branch_pattern": "release/*",
                    "require_pull_request": true,
                    "required_approvals": 1,
                    "require_passing_checks": false
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "put rules: {rules}");

    let (status, created) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls"),
        &admin_token,
        Some(json!({
            "title": "Release fix PR",
            "description": "Targets release/v1.0",
            "base_branch": "release/v1.0",
            "head_branch": "feature/rel-fix"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create PR: {created}");

    let (status, detail_before) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{repo_id}/pulls/1"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        detail_before["protection"]["required_approvals"], 1,
        "most specific wildcard 'release/*' must win over '*': {detail_before}"
    );
    assert_eq!(detail_before["protection"]["require_pull_request"], true);

    // 3b. Finding 3: Maintainer approves PR on private repo -> approved_count == 1;
    // then Maintainer is removed from the private repo's team -> approved_count drops to 0!
    let (status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id}/pulls/1/reviews"),
        &maint_token,
        Some(json!({
            "verdict": "approve",
            "body": "Approved while on team"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, detail_approved) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{repo_id}/pulls/1"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail_approved["protection"]["approved_count"], 1);

    // Remove Maintainer from private repository group
    let (status, rem) = api_json(
        &app,
        "PATCH",
        format!(
            "/api/v1/repos/{repo_id}/group-members/{}",
            maint_principal.user_id.0
        ),
        &admin_token,
        Some(json!({
            "expected_membership_version": "0",
            "is_member": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "remove member: {rem}");

    let (status, detail_after_removal) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{repo_id}/pulls/1"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        detail_after_removal["protection"]["approved_count"], 0,
        "removed team member's approval on private repo must not count: {detail_after_removal}"
    );
}
