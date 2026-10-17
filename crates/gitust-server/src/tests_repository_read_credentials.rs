//! Every legacy object-read adapter must preserve the original credential through Git I/O.
use super::*;
use std::sync::Arc;
async fn revoked_after_git(route: &str, use_pat: bool) {
    let (_temp, state, session, _path, _id) = super::tests_repository_catalog::registered_pair();
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "read", None)
        .unwrap();
    let token = if use_pat {
        pat.raw_token.clone()
    } else {
        session
    };
    let revoke = token.clone();
    let store = state.auth_store.clone();
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        if use_pat {
            store
                .revoke_personal_access_token(&admin, pat.metadata.id)
                .unwrap();
        } else {
            store.revoke_session(&admin, &revoke).unwrap();
        }
    }));
    let request = Request::builder()
        .uri(route)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = build_router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}
macro_rules! read_auth_test {
    ($name:ident,$route:literal,$pat:literal) => {
        #[tokio::test]
        async fn $name() {
            revoked_after_git($route, $pat).await;
        }
    };
}
read_auth_test!(
    legacy_read_credential_commits_session,
    "/api/v1/repo/commits",
    false
);
read_auth_test!(
    legacy_read_credential_commits_pat,
    "/api/v1/repo/commits",
    true
);
read_auth_test!(
    legacy_read_credential_tree_session,
    "/api/v1/repo/tree",
    false
);
read_auth_test!(legacy_read_credential_tree_pat, "/api/v1/repo/tree", true);
read_auth_test!(
    legacy_read_credential_blob_session,
    "/api/v1/repo/blob?path=README.md",
    false
);
read_auth_test!(
    legacy_read_credential_blob_pat,
    "/api/v1/repo/blob?path=README.md",
    true
);
read_auth_test!(
    legacy_read_credential_diff_session,
    "/api/v1/repo/diff",
    false
);
read_auth_test!(legacy_read_credential_diff_pat, "/api/v1/repo/diff", true);
