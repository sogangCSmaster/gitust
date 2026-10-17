use crate::persistence::MAX_AUTH_DB_BYTES;
use crate::AuthStore;
use gitust_core::AuthError;

#[test]
fn oversized_snapshot_keeps_previous_disk_state_and_rolls_back_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).expect("open store");
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "SyntheticPassword!2026")
        .expect("bootstrap");
    let login = store
        .login("admin", "SyntheticPassword!2026")
        .expect("login");
    let caller = store
        .authenticate_session(&login.raw_session_token)
        .expect("principal");
    let original = std::fs::read(&path).expect("read previous snapshot");

    // Exercise the total snapshot bound independently of individual field validation.
    let mut guard = store.write_state().expect("write transaction");
    guard.users.get_mut(&caller.user_id).expect("user").email =
        "a".repeat(usize::try_from(MAX_AUTH_DB_BYTES).expect("addressable limit"));
    let pending = store.maybe_snapshot(guard);
    assert!(matches!(
        store.persist_snapshot(pending),
        Err(AuthError::StorageError(_))
    ));

    assert_eq!(
        std::fs::read(&path).expect("read preserved snapshot"),
        original
    );
    assert_eq!(
        store.list_users(&caller).expect("users")[0].email,
        "admin@company.internal"
    );
    assert_eq!(
        store
            .authenticate_session(&login.raw_session_token)
            .expect("session remains valid"),
        caller
    );
    let reopened = AuthStore::open_persistent(&path).expect("reopen preserved store");
    assert_eq!(
        reopened
            .authenticate_session(&login.raw_session_token)
            .expect("restored session"),
        caller
    );
    assert_eq!(std::fs::read_dir(temp.path()).expect("entries").count(), 2);
}

#[test]
fn oversized_email_cannot_bootstrap_or_create_an_invite() {
    let store = AuthStore::new_in_memory();
    let oversized = format!("{}@company.internal", "a".repeat(238));
    assert!(matches!(
        store.bootstrap_initial_admin_cli("admin", &oversized, "SyntheticPassword!2026"),
        Err(AuthError::InvalidInput(_))
    ));
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "SyntheticPassword!2026")
        .expect("bootstrap");
    let login = store
        .login("admin", "SyntheticPassword!2026")
        .expect("login");
    let caller = store
        .authenticate_session(&login.raw_session_token)
        .expect("principal");
    assert!(matches!(
        store.create_invite_link(
            &caller,
            &oversized,
            gitust_core::EnterpriseRole::Developer,
            vec![],
            None
        ),
        Err(AuthError::InvalidInput(_))
    ));
    assert!(store.list_invites(&caller).expect("invites").is_empty());
}
