//! Real local fetch plus a post-fetch revocation barrier, with no global PATH or live accounts.
use super::*;
use std::sync::Arc;
fn git(path: &std::path::Path, args: &[&str]) -> String {
    let mut command = std::process::Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@company.internal")
        .env("GIT_COMMITTER_EMAIL", "fixture@company.internal")
        .output()
        .unwrap();
    assert!(output.status.success(), "synthetic Git operation failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
async fn post_fetch_revocation(use_pat: bool) {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    std::fs::create_dir(&primary).unwrap();
    git(&primary, &["init", "-q", "-b", "main"]);
    std::fs::write(primary.join("README.md"), "original\n").unwrap();
    git(&primary, &["add", "."]);
    git(&primary, &["commit", "-qm", "initial"]);
    let upstream = temp.path().join("upstream");
    git(
        temp.path(),
        &[
            "clone",
            "--no-local",
            primary.to_str().unwrap(),
            upstream.to_str().unwrap(),
        ],
    );
    git(&upstream, &["checkout", "-qb", "published-after-fetch"]);
    std::fs::write(upstream.join("README.md"), "new upstream\n").unwrap();
    git(&upstream, &["commit", "-qam", "new upstream"]);
    let expected = git(
        &upstream,
        &["rev-parse", "refs/heads/published-after-fetch"],
    );
    let before = git(
        &primary,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ],
    );
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let session = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&session).unwrap();
    let pat = store
        .create_personal_access_token(&admin, "sync", None)
        .unwrap();
    let credential = if use_pat {
        pat.raw_token.clone()
    } else {
        session
    };
    let state = AppState::with_auth_store(
        primary.clone(),
        upstream.to_str().unwrap().into(),
        store.clone(),
    )
    .unwrap();
    let revoke = credential.clone();
    let check = credential.clone();
    let callback_store = store.clone();
    *state.sync_after_fetch_hook.lock() = Some(Arc::new(move || {
        if use_pat {
            callback_store
                .revoke_personal_access_token(&admin, pat.metadata.id)
                .unwrap();
            assert!(callback_store.authenticate_pat(&check).is_err());
        } else {
            callback_store.revoke_session(&admin, &revoke).unwrap();
            assert!(callback_store.authenticate_session(&check).is_err());
        }
    }));
    let (status, _body) = call(
        &build_router(state),
        "POST",
        "/api/v1/sync/github".into(),
        &credential,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Staging fetch completed, but no public head/tag mutation was authorized.
    assert_eq!(
        git(
            &primary,
            &["rev-parse", "refs/remotes/upstream/published-after-fetch"]
        ),
        expected
    );
    assert_eq!(
        git(
            &primary,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/tags"
            ]
        ),
        before
    );
    git(&primary, &["fsck", "--full"]);
    git(&upstream, &["fsck", "--full"]);
}
#[tokio::test]
async fn sync_rechecks_original_session_after_fetch() {
    post_fetch_revocation(false).await;
}
#[tokio::test]
async fn sync_rechecks_original_pat_after_fetch() {
    post_fetch_revocation(true).await;
}
