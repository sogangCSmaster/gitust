use crate::AuthStore;
use gitust_core::{AuthError, EnterpriseRole, RepoAccess, RepoId, RepositoryRegistration, TeamId};

fn fixture() -> (
    tempfile::TempDir,
    AuthStore,
    gitust_core::AuthenticatedPrincipal,
    RepoAccess,
) {
    let temp = tempfile::tempdir().unwrap();
    let store = AuthStore::open_persistent(temp.path().join("auth.json")).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let session = store.login("admin", "Password!2026").unwrap();
    let admin = store
        .authenticate_session(&session.raw_session_token)
        .unwrap();
    let row = store
        .register_local_repository(&admin, RepoId::new(), "original", TeamId::new(), true, None)
        .unwrap();
    let access = RepoAccess {
        repo_id: row.metadata.id,
        registration: Some(row.binding),
    };
    (temp, store, admin, access)
}
#[test]
fn registered_rename_changes_only_the_canonical_name_and_never_adopts_other_bindings() {
    let (_temp, store, admin, access) = fixture();
    let before = store
        .authorize_repository_access(&admin, access.repo_id)
        .unwrap();
    let after = store
        .rename_registered_repository(&admin, access, " renamed-repo ")
        .unwrap();
    let mut expected = before.clone();
    expected.name = "renamed-repo".into();
    assert_eq!(after, expected);
    assert_eq!(
        store
            .configured_repository_access(access.repo_id, access.registration)
            .unwrap(),
        access
    );
    let wrong = RepoAccess {
        registration: Some(RepositoryRegistration::new(access.repo_id)),
        ..access
    };
    assert!(store
        .rename_registered_repository(&admin, wrong, "wrong")
        .is_err());
    assert!(store
        .rename_registered_repository(
            &admin,
            RepoAccess {
                registration: None,
                ..access
            },
            "wrong"
        )
        .is_err());
    assert!(store
        .rename_registered_repository(&admin, access, "../wrong")
        .is_err());
    assert_eq!(
        store
            .authorize_repository_access(&admin, access.repo_id)
            .unwrap(),
        expected
    );
}
#[test]
fn rename_uses_the_stored_active_role_and_catalog_capability_uses_the_same_snapshot() {
    let (_temp, store, admin, access) = fixture();
    let invite = store
        .create_invite_link(
            &admin,
            "member@company.internal",
            EnterpriseRole::Developer,
            vec![
                store
                    .authorize_repository_access(&admin, access.repo_id)
                    .unwrap()
                    .owner_team_id,
            ],
            None,
        )
        .unwrap();
    let user = store
        .redeem_invite_link(&invite.raw_token, "member", "Password!2026")
        .unwrap();
    let mut forged = admin.clone();
    forged.user_id = user.id;
    assert!(matches!(
        store.rename_registered_repository(&forged, access, "wrong"),
        Err(AuthError::AdminPrivilegeRequired)
    ));
    assert!(
        !store
            .authorized_repository_catalog(&forged, &[access])
            .unwrap()[0]
            .can_rename
    );
    assert!(
        store
            .authorized_repository_catalog(&admin, &[access])
            .unwrap()[0]
            .can_rename
    );
    store.deactivate_user(&admin, user.id).unwrap();
    assert!(store
        .rename_registered_repository(&forged, access, "wrong")
        .is_err());
    let metadata = store
        .create_team_repository(&admin, "metadata", TeamId::new(), true)
        .unwrap();
    assert!(store
        .rename_registered_repository(
            &admin,
            RepoAccess {
                repo_id: metadata.id,
                registration: None
            },
            "wrong"
        )
        .is_err());
}
#[test]
fn failed_directory_sync_cannot_turn_a_same_name_retry_into_unconfirmed_success() {
    let (_temp, store, admin, access) = fixture();
    let path = store.persistence_path().unwrap().to_owned();
    let before = store
        .authorize_repository_access(&admin, access.repo_id)
        .unwrap();
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = None);
        }
    }
    let reset = Reset;
    crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = Some(path.clone()));
    assert!(store
        .rename_registered_repository(&admin, access, "renamed")
        .is_err());
    assert!(store
        .rename_registered_repository(&admin, access, "renamed")
        .is_err());
    drop(reset);
    let mut expected = before;
    expected.name = "renamed".into();
    assert_eq!(
        store
            .rename_registered_repository(&admin, access, "renamed")
            .unwrap(),
        expected
    );
    let reopened = AuthStore::open_persistent(path).unwrap();
    assert_eq!(
        reopened
            .configured_repository_access(access.repo_id, access.registration)
            .unwrap(),
        access
    );
    assert_eq!(
        reopened
            .authorize_repository_access(&admin, access.repo_id)
            .unwrap(),
        expected
    );
}
