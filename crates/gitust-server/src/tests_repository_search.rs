use super::*;
use gitust_git::{RepositorySearchReport, SearchMatchKind, SymbolKind};

#[tokio::test]
async fn search_endpoint_returns_committed_code_and_symbol_hits_with_commit_sha_and_cache_hit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);
    std::fs::create_dir_all(repo_dir.join("src")).expect("mkdir src");
    std::fs::write(
        repo_dir.join("src/engine.rs"),
        "pub struct TrigramIndex {\n    pub count: usize,\n}\n\npub fn search_committed_blobs(query: &str) -> usize {\n    query.len()\n}\n",
    )
    .expect("write engine.rs");
    let out = fixture_git()
        .arg("-C")
        .arg(&repo_dir)
        .args(["-c", "commit.gpgsign=false", "add", "."])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git add");
    assert!(out.status.success());
    let out = fixture_git()
        .arg("-C")
        .arg(&repo_dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "add search engine",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@company.internal")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@company.internal")
        .output()
        .expect("git commit");
    assert!(out.status.success());

    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "siteadmin@company.internal",
            "AdminPassword!2026",
        )
        .expect("bootstrap");
    let login = store
        .login("siteadmin", "AdminPassword!2026")
        .expect("login");
    let state = AppState::with_auth_store(
        repo_dir.clone(),
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        store,
    )
    .expect("state");
    let repo_id = state.repo_id;
    let head = state.git_engine.inspect_head_identity().expect("head");
    let app = build_router(state);

    let (status, body) = call(
        &app,
        "GET",
        format!(
            "/api/v1/repos/{}/search?q=search_committed_blobs",
            repo_id.0
        ),
        &login.raw_session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let report: RepositorySearchReport = serde_json::from_str(&body).expect("json");
    assert_eq!(report.commit_sha, head.head_sha);
    assert!(!report.index_cache_hit);
    assert_eq!(report.hits.len(), 1);
    assert_eq!(report.hits[0].path, "src/engine.rs");
    assert_eq!(report.hits[0].line_number, 5);
    assert_eq!(report.hits[0].match_kind, SearchMatchKind::Symbol);
    assert_eq!(
        report.hits[0].symbol_name.as_deref(),
        Some("search_committed_blobs")
    );
    assert_eq!(report.hits[0].symbol_kind, Some(SymbolKind::Function));

    // Second query on same revision reuses commit-pinned index cache.
    let (status2, body2) = call(
        &app,
        "GET",
        format!(
            "/api/v1/repos/{}/search?q=TrigramIndex&rev=main&kind=symbol",
            repo_id.0
        ),
        &login.raw_session_token,
        None,
    )
    .await;
    assert_eq!(status2, StatusCode::OK, "body: {body2}");
    let report2: RepositorySearchReport = serde_json::from_str(&body2).expect("json");
    assert_eq!(report2.commit_sha, head.head_sha);
    assert!(report2.index_cache_hit);
    assert_eq!(report2.hits.len(), 1);
    assert_eq!(report2.hits[0].symbol_kind, Some(SymbolKind::Struct));
}

#[tokio::test]
async fn search_endpoint_rejects_agent_branch_revisions_and_ignores_uncommitted_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);

    let out = fixture_git()
        .arg("-C")
        .arg(&repo_dir)
        .args(["branch", "agent/claude/private-task"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git branch");
    assert!(out.status.success());

    // Write an uncommitted file in the working tree.
    std::fs::write(
        repo_dir.join("dirty_secret.rs"),
        "pub fn dirty_uncommitted_secret_fn() {}\n",
    )
    .expect("write dirty");

    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "siteadmin@company.internal",
            "AdminPassword!2026",
        )
        .expect("bootstrap");
    let login = store
        .login("siteadmin", "AdminPassword!2026")
        .expect("login");
    let state = AppState::with_auth_store(
        repo_dir,
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        store,
    )
    .expect("state");
    let repo_id = state.repo_id;
    let app = build_router(state);

    for bad_rev in [
        "agent/claude/private-task",
        "refs/heads/agent/claude/private-task",
    ] {
        let (status, _) = call(
            &app,
            "GET",
            format!("/api/v1/repos/{}/search?q=hello&rev={bad_rev}", repo_id.0),
            &login.raw_session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let (status_dirty, body_dirty) = call(
        &app,
        "GET",
        format!(
            "/api/v1/repos/{}/search?q=dirty_uncommitted_secret_fn",
            repo_id.0
        ),
        &login.raw_session_token,
        None,
    )
    .await;
    assert_eq!(status_dirty, StatusCode::OK);
    let report: RepositorySearchReport = serde_json::from_str(&body_dirty).expect("json");
    assert!(report.hits.is_empty());
}

#[tokio::test]
async fn search_endpoint_enforces_bola_and_rechecks_revoked_credential_post_read() {
    let (_temp, state, session, _path, second_repo_id) =
        super::tests_repository_catalog::registered_pair();
    let admin = state
        .auth_store
        .authenticate_session(&session)
        .expect("admin");

    // Create a developer who is NOT in the private secondary repository's owning team.
    let invite = state
        .auth_store
        .create_invite_link(
            &admin,
            "outsider@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("invite");
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, "outsider", "DevPassword!2026")
        .expect("redeem");
    let outsider_login = state
        .auth_store
        .login("outsider", "DevPassword!2026")
        .expect("outsider login");

    let app = build_router(state.clone());

    // Outsider cannot search the private secondary repository (BOLA 403).
    let (bola_status, _) = call(
        &app,
        "GET",
        format!("/api/v1/repos/{}/search?q=secondary", second_repo_id.0),
        &outsider_login.raw_session_token,
        None,
    )
    .await;
    assert_eq!(bola_status, StatusCode::FORBIDDEN);

    // Revoking the session during `after_read` hook must fail closed with 401 UNAUTHORIZED.
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "search-pat", None)
        .expect("pat");
    let store = state.auth_store.clone();
    let admin_clone = admin.clone();
    let pat_id = pat.metadata.id;
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        store
            .revoke_personal_access_token(&admin_clone, pat_id)
            .expect("revoke pat");
    }));

    let (revoked_status, _) = call(
        &app,
        "GET",
        format!("/api/v1/repos/{}/search?q=secondary", second_repo_id.0),
        &pat.raw_token,
        None,
    )
    .await;
    assert_eq!(revoked_status, StatusCode::UNAUTHORIZED);
}
