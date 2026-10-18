use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::{json, Value};

pub(super) fn git(repo: &std::path::Path, args: &[&str]) -> String {
    super::tests_automatic_checks::git(repo, args)
}

pub(super) async fn api_json(
    app: &Router,
    method: &str,
    uri: String,
    token: &str,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(payload.map_or_else(Body::empty, |v| Body::from(v.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

pub(super) fn invite_and_login_role(
    state: &AppState,
    admin_token: &str,
    email: &str,
    username: &str,
    role: EnterpriseRole,
    team_ids: Vec<TeamId>,
) -> String {
    let admin = state.auth_store.authenticate_session(admin_token).unwrap();
    let inv = state
        .auth_store
        .create_invite_link(&admin, email, role, team_ids, Some(24))
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&inv.raw_token, username, "Password1234!")
        .unwrap();
    state
        .auth_store
        .login(username, "Password1234!")
        .unwrap()
        .raw_session_token
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_request_reviews_inline_comments_branch_protection_stale_approval_checks_and_merge() {
    let (_temp, state, admin_token, _) = fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let repo_id = state.repo_id;
    let repo = state.git_engine.repo_path().to_path_buf();

    let admin_principal = state.auth_store.authenticate_session(&admin_token).unwrap();
    let team_id = state
        .auth_store
        .authorize_repository_access(&admin_principal, repo_id)
        .unwrap()
        .owner_team_id;
    let dev_token = invite_and_login_role(
        &state,
        &admin_token,
        "dev-pr@company.internal",
        "dev_pr",
        EnterpriseRole::Developer,
        vec![team_id],
    );
    let maint_token = invite_and_login_role(
        &state,
        &admin_token,
        "maint-pr@company.internal",
        "maint_pr",
        EnterpriseRole::Maintainer,
        vec![team_id],
    );

    // Create feature branch with a committed `.gitust-checks.json` and a new file.
    let base_sha = git(&repo, &["rev-parse", "refs/heads/main"]);
    git(&repo, &["checkout", "-b", "feature/pr-flow"]);
    std::fs::write(
        repo.join(".gitust-checks.json"),
        json!({
            "version": 1,
            "commands": ["test -f src_feature.rs && printf pr-check-ok"]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join("src_feature.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    )
    .unwrap();
    git(&repo, &["add", ".gitust-checks.json", "src_feature.rs"]);
    git(&repo, &["commit", "-qm", "feat: add answer"]);
    let head_sha_1 = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "main"]);

    let app = build_router(state.clone());

    // 1. Configure branch protection on `main`: require PR, 1 approval, passing checks, restrict direct pushes.
    let (status, rules_body) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{}/branch-protection", repo_id.0),
        &maint_token,
        Some(json!({
            "rules": [{
                "branch_pattern": "main",
                "require_pull_request": true,
                "required_approvals": 1,
                "require_passing_checks": true,
                "restrict_direct_pushes": true
            }]
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PUT branch-protection: {rules_body}"
    );
    assert_eq!(rules_body["rules"].as_array().unwrap().len(), 1);

    // 2. Developer creates a Pull Request from `feature/pr-flow` into `main`.
    let (status, created) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls", repo_id.0),
        &dev_token,
        Some(json!({
            "title": "Add answer helper",
            "description": "Implements answer() in src_feature.rs",
            "base_branch": "main",
            "head_branch": "feature/pr-flow"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "POST pulls: {created}");
    let pr_id = created["pull"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["pull"]["number"], 1);
    assert_eq!(created["pull"]["status"], "open");
    assert_eq!(created["pull"]["base_sha"], base_sha);
    assert_eq!(created["pull"]["head_sha"], head_sha_1);

    // 3. Developer adds an inline diff comment; path traversal in file_path must be rejected.
    let (bad_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/comments", repo_id.0),
        &dev_token,
        Some(json!({
            "body": "unsafe path",
            "file_path": "../etc/passwd",
            "line_number": 1
        })),
    )
    .await;
    assert_eq!(bad_status, StatusCode::BAD_REQUEST);

    let (comment_status, commented) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/comments", repo_id.0),
        &dev_token,
        Some(json!({
            "body": "Please double-check return type",
            "file_path": "src_feature.rs",
            "line_number": 1,
            "commit_sha": head_sha_1
        })),
    )
    .await;
    assert_eq!(comment_status, StatusCode::OK);
    assert_eq!(commented["pull"]["comments"].as_array().unwrap().len(), 1);

    // 4. Developer cannot submit `approve` verdict (403), but can submit `request_changes`.
    let (dev_approve_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/reviews", repo_id.0),
        &dev_token,
        Some(json!({
            "verdict": "approve",
            "body": "LGTM from dev"
        })),
    )
    .await;
    assert_eq!(dev_approve_status, StatusCode::FORBIDDEN);

    let (req_changes_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/reviews", repo_id.0),
        &dev_token,
        Some(json!({
            "verdict": "request_changes",
            "body": "Needs one more test"
        })),
    )
    .await;
    assert_eq!(req_changes_status, StatusCode::OK);

    // 5. Maintainer approves at `head_sha_1`, but `changes_requested` from dev still blocks merge!
    let (maint_approve_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/reviews", repo_id.0),
        &maint_token,
        Some(json!({
            "verdict": "approve",
            "body": "Approved v1"
        })),
    )
    .await;
    assert_eq!(maint_approve_status, StatusCode::OK);

    let (merge_blocked_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/merge", repo_id.0),
        &maint_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(merge_blocked_status, StatusCode::CONFLICT);

    // 6. Developer pushes a second commit on `feature/pr-flow` and clears their `request_changes` with `comment`.
    git(&repo, &["checkout", "feature/pr-flow"]);
    std::fs::write(
        repo.join("src_feature.rs"),
        "pub fn answer() -> u32 { 42 }\npub fn extra() -> bool { true }\n",
    )
    .unwrap();
    git(&repo, &["add", "src_feature.rs"]);
    git(&repo, &["commit", "-qm", "feat: add extra"]);
    let head_sha_2 = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "main"]);

    let (clear_review_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/reviews", repo_id.0),
        &dev_token,
        Some(json!({
            "verdict": "comment",
            "body": "Addressed in second commit"
        })),
    )
    .await;
    assert_eq!(clear_review_status, StatusCode::OK);

    // 7. Inspect PR detail: `head_sha` synced to `head_sha_2`, and prior approval at `head_sha_1` is now stale (`approved_count == 0`)!
    let (detail_status, detail) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{}/pulls/{pr_id}", repo_id.0),
        &maint_token,
        None,
    )
    .await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(detail["pull"]["head_sha"], head_sha_2);
    assert_eq!(detail["protection"]["approved_count"], 0);
    assert_eq!(detail["protection"]["changes_requested"], false);
    assert_eq!(detail["protection"]["checks_passed"], false);

    // 8. Maintainer approves `head_sha_2`, but merge is STILL blocked until required checks pass for `head_sha_2`!
    let (reapprove_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/reviews", repo_id.0),
        &maint_token,
        Some(json!({
            "verdict": "approve",
            "body": "Approved v2",
            "reviewed_head_sha": head_sha_2
        })),
    )
    .await;
    assert_eq!(reapprove_status, StatusCode::OK);

    let (merge_no_checks_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/merge", repo_id.0),
        &maint_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(merge_no_checks_status, StatusCode::CONFLICT);

    // 9. Run explicit Local Checks on `refs/heads/feature/pr-flow` at `head_sha_2`.
    let (checks_status, checks_body) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/local-check-runs", repo_id.0),
        &admin_token,
        Some(json!({
            "expected_source_ref": "refs/heads/feature/pr-flow",
            "expected_commit": head_sha_2,
            "timeout_secs": 30
        })),
    )
    .await;
    assert_eq!(checks_status, StatusCode::OK, "checks: {checks_body}");
    assert_eq!(checks_body["status"], "passed");

    // 10. Merge now succeeds and advances `refs/heads/main`!
    let (merge_ok_status, merged_body) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/{pr_id}/merge", repo_id.0),
        &maint_token,
        Some(json!({
            "expected_base_sha": base_sha,
            "expected_head_sha": head_sha_2
        })),
    )
    .await;
    assert_eq!(merge_ok_status, StatusCode::OK, "merge: {merged_body}");
    assert_eq!(merged_body["pull"]["status"], "merged");
    let merged_sha = merged_body["merged_sha"].as_str().unwrap();
    let new_main_sha = git(&repo, &["rev-parse", "refs/heads/main"]);
    assert_eq!(new_main_sha, merged_sha);
    assert_ne!(new_main_sha, base_sha);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_branch_rejects_direct_git_http_push_while_allowing_unprotected_branch() {
    let (_temp, state, admin_token, _) = fixture();
    let repo_id = state.repo_id;
    let repo = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());

    let (status, _) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{}/branch-protection", repo_id.0),
        &admin_token,
        Some(json!({
            "rules": [{
                "branch_pattern": "main",
                "require_pull_request": true,
                "required_approvals": 1,
                "require_passing_checks": false,
                "restrict_direct_pushes": true
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let main_sha = git(&repo, &["rev-parse", "refs/heads/main"]);
    let pkt = format!("{main_sha} {main_sha} refs/heads/main\0 report-status\n");
    let pkt_line = format!("{:04x}{pkt}0000", pkt.len() + 4);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/git/{}/git-receive-pack", repo_id.0))
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .header(
                    header::CONTENT_TYPE,
                    "application/x-git-receive-pack-request",
                )
                .body(Body::from(pkt_line))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draft_status_transitions_and_secondary_git_write_repo_pull_request_merge() {
    let (temp, mut state, admin_token, _) = fixture();
    let admin = state.auth_store.authenticate_session(&admin_token).unwrap();
    let team_id = state
        .auth_store
        .authorize_repository_access(&admin, state.repo_id)
        .unwrap()
        .owner_team_id;

    let write_bare = temp.path().join("secondary-pr-write.git");
    let readonly_bare = temp.path().join("secondary-pr-readonly.git");
    for (bare, name) in [
        (&write_bare, "secondary-pr-write"),
        (&readonly_bare, "secondary-pr-readonly"),
    ] {
        git(
            temp.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                bare.to_str().unwrap(),
            ],
        );
        let work = temp.path().join(format!("{name}-work"));
        git(
            temp.path(),
            &["clone", bare.to_str().unwrap(), work.to_str().unwrap()],
        );
        git(&work, &["checkout", "-b", "main"]);
        std::fs::write(work.join("README.md"), "base\n").unwrap();
        git(&work, &["add", "README.md"]);
        git(&work, &["commit", "-qm", "initial main"]);
        git(&work, &["push", "-u", "origin", "main"]);
        git(&work, &["checkout", "-b", "topic/sec"]);
        std::fs::write(work.join("sec.txt"), "secondary change\n").unwrap();
        git(&work, &["add", "sec.txt"]);
        git(&work, &["commit", "-qm", "topic change"]);
        git(&work, &["push", "-u", "origin", "topic/sec"]);

        let eng = GitEngine::new(bare);
        let id = eng.load_or_create_repository_id().unwrap();
        let reg = state
            .auth_store
            .register_local_repository(&admin, id, name, team_id, true, None)
            .unwrap();
        eng.complete_repository_registration(reg.binding).unwrap();
    }

    let write_id = GitEngine::new(&write_bare)
        .load_existing_repository_id()
        .unwrap();
    let readonly_id = GitEngine::new(&readonly_bare)
        .load_existing_repository_id()
        .unwrap();
    state.repository_registry = Some(
        crate::repository_registry::RepositoryRegistry::load_with_git_write(
            &state,
            vec![readonly_bare],
            vec![write_bare.clone()],
        )
        .unwrap(),
    );
    let app = build_router(state.clone());

    // Read-only secondary repository rejects PR creation with 403 FORBIDDEN.
    let (ro_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls", readonly_id.0),
        &admin_token,
        Some(json!({
            "title": "Should fail on readonly",
            "base_branch": "main",
            "head_branch": "topic/sec"
        })),
    )
    .await;
    assert_eq!(ro_status, StatusCode::FORBIDDEN);

    // Writable secondary repository supports draft PR creation, status transitions, and merge.
    let (create_status, created) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls", write_id.0),
        &admin_token,
        Some(json!({
            "title": "Secondary PR",
            "description": "Draft first",
            "base_branch": "main",
            "head_branch": "topic/sec",
            "draft": true
        })),
    )
    .await;
    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(created["pull"]["status"], "draft");

    // Merging a draft PR is rejected with 409 CONFLICT.
    let (draft_merge_status, _) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/1/merge", write_id.0),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(draft_merge_status, StatusCode::CONFLICT);

    // Transition draft -> open, then merge by PR number.
    let (open_status, opened) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/1/status", write_id.0),
        &admin_token,
        Some(json!({ "status": "open" })),
    )
    .await;
    assert_eq!(open_status, StatusCode::OK);
    assert_eq!(opened["pull"]["status"], "open");

    let (merge_status, merged) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/pulls/1/merge", write_id.0),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert_eq!(merge_status, StatusCode::OK, "secondary merge: {merged}");
    assert_eq!(merged["pull"]["status"], "merged");
}
