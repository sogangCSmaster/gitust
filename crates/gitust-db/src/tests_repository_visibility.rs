use crate::{AuthStore, RepositoryVisibilityError};
use gitust_core::{AuthError, EnterpriseRole, RepoAccess, RepoId, TeamId};
use std::sync::{Arc, Barrier};

fn fixture() -> (tempfile::TempDir, AuthStore, String, RepoAccess) {
    let temp = tempfile::tempdir().unwrap();
    let store = AuthStore::open_persistent(temp.path().join("auth/auth.json")).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&token).unwrap();
    let row = store
        .register_local_repository(
            &admin,
            RepoId::new(),
            "repository",
            TeamId::new(),
            true,
            None,
        )
        .unwrap();
    (
        temp,
        store,
        token,
        RepoAccess {
            repo_id: row.metadata.id,
            registration: Some(row.binding),
        },
    )
}
fn outsider(store: &AuthStore, token: &str) -> gitust_core::AuthenticatedPrincipal {
    let admin = store.authenticate_session(token).unwrap();
    let invite = store
        .create_invite_link(
            &admin,
            "outside@company.internal",
            EnterpriseRole::Developer,
            vec![TeamId::new()],
            None,
        )
        .unwrap();
    store
        .redeem_invite_link(&invite.raw_token, "outside", "Password!2026")
        .unwrap();
    let session = store.login("outside", "Password!2026").unwrap();
    store
        .authenticate_session(&session.raw_session_token)
        .unwrap()
}
#[test]
fn visibility_uses_current_original_credentials_and_exact_registered_binding() {
    let (_temp, store, token, access) = fixture();
    let admin = store.authenticate_session(&token).unwrap();
    let outside = outsider(&store, &token);
    let before = store
        .authorize_repository_access(&admin, access.repo_id)
        .unwrap();
    assert!(store
        .authorize_service_repository(&outside, access, access.repo_id)
        .is_err());
    let changed = store
        .change_registered_repository_visibility(&token, admin.user_id, access, true, false)
        .unwrap();
    let mut expected = before.clone();
    expected.is_private = false;
    assert_eq!(changed, expected);
    assert!(store
        .authorize_service_repository(&outside, access, access.repo_id)
        .is_ok());
    assert!(matches!(
        store.change_registered_repository_visibility(&token, admin.user_id, access, true, false),
        Err(RepositoryVisibilityError::Conflict)
    ));
    let wrong = RepoAccess {
        registration: Some(gitust_core::RepositoryRegistration::new(access.repo_id)),
        ..access
    };
    assert!(matches!(
        store.change_registered_repository_visibility(&token, admin.user_id, wrong, false, true),
        Err(RepositoryVisibilityError::Auth(AuthError::StorageError(_)))
    ));
    assert!(store
        .change_registered_repository_visibility(&token, outside.user_id, access, false, true)
        .is_err());
    let pat = store
        .create_personal_access_token(&admin, "visibility", None)
        .unwrap();
    store
        .revoke_personal_access_token(&admin, pat.metadata.id)
        .unwrap();
    assert!(matches!(
        store.change_registered_repository_visibility(
            &pat.raw_token,
            admin.user_id,
            access,
            false,
            true
        ),
        Err(RepositoryVisibilityError::Auth(AuthError::Unauthenticated))
    ));
    let closed = store
        .change_registered_repository_visibility(&token, admin.user_id, access, false, true)
        .unwrap();
    assert_eq!(closed, before);
    assert!(store
        .authorize_service_repository(&outside, access, access.repo_id)
        .is_err());
    store.revoke_session(&admin, &token).unwrap();
    assert!(matches!(
        store.change_registered_repository_visibility(&token, admin.user_id, access, true, false),
        Err(RepositoryVisibilityError::Auth(AuthError::Unauthenticated))
    ));
}
#[test]
fn competing_visibility_requests_compare_under_one_authoritative_write_lock() {
    let (_temp, store, token, access) = fixture();
    let user = store.authenticate_session(&token).unwrap().user_id;
    let barrier = Arc::new(Barrier::new(3));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (store, token, barrier) = (store.clone(), token.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                store.change_registered_repository_visibility(&token, user, access, true, false)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(RepositoryVisibilityError::Conflict)))
            .count(),
        1
    );
    let current = store.authenticate_session(&token).unwrap();
    assert!(
        !store
            .authorize_repository_access(&current, access.repo_id)
            .unwrap()
            .is_private
    );
}
#[test]
fn visibility_persistence_keeps_prepublication_failure_distinct_from_uncertain_outcome() {
    let (temp, store, token, access) = fixture();
    let admin = store.authenticate_session(&token).unwrap();
    let outside = outsider(&store, &token);
    let db = store.persistence_path().unwrap().to_owned();
    let old = std::fs::read(&db).unwrap();
    let before = store
        .authorize_repository_access(&admin, access.repo_id)
        .unwrap();
    // Fault the real persistence path after admission, without adding a production failure hook.
    let mut guard = store.write_state().unwrap();
    guard
        .repositories
        .get_mut(&access.repo_id)
        .unwrap()
        .metadata
        .is_private = false;
    let pending = store.maybe_snapshot(guard);
    let parent = db.parent().unwrap();
    let saved = temp.path().join("saved-auth");
    std::fs::rename(parent, &saved).unwrap();
    std::fs::write(parent, "not a directory").unwrap();
    let error = store.persist_snapshot_classified(pending).err().unwrap();
    assert!(!error.publication_attempted);
    std::fs::remove_file(parent).unwrap();
    std::fs::rename(&saved, parent).unwrap();
    assert_eq!(std::fs::read(&db).unwrap(), old);
    assert_eq!(
        store
            .authorize_repository_access(&admin, access.repo_id)
            .unwrap(),
        before
    );
    assert!(store
        .authorize_service_repository(&outside, access, access.repo_id)
        .is_err());

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = None);
        }
    }
    let reset = Reset;
    crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = Some(db.clone()));
    assert!(matches!(
        store.change_registered_repository_visibility(&token, admin.user_id, access, true, false),
        Err(RepositoryVisibilityError::Unconfirmed(_))
    ));
    // Rename may already have happened. A stale retry remains a conflict, not a CAS bypass.
    assert!(matches!(
        store.change_registered_repository_visibility(&token, admin.user_id, access, true, false),
        Err(RepositoryVisibilityError::Conflict)
    ));
    assert!(matches!(
        store.change_registered_repository_visibility(&token, admin.user_id, access, false, false),
        Err(RepositoryVisibilityError::Unconfirmed(_))
    ));
    drop(reset);
    assert!(
        !store
            .change_registered_repository_visibility(&token, admin.user_id, access, false, false)
            .unwrap()
            .is_private
    );
    let reopened = AuthStore::open_persistent(db).unwrap();
    let current = reopened.authenticate_session(&token).unwrap();
    assert!(
        !reopened
            .authorize_repository_access(&current, access.repo_id)
            .unwrap()
            .is_private
    );
    assert_eq!(
        reopened
            .configured_repository_access(access.repo_id, access.registration)
            .unwrap(),
        access
    );
}
