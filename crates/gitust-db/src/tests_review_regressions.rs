use crate::crypto::canonicalize_openssh_pubkey;
use crate::{hash_token_sha256, AuthStore};
use gitust_core::{AuthError, AuthenticatedPrincipal};

const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey000";

fn admin(store: &AuthStore) -> (AuthenticatedPrincipal, String) {
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "StrongPassword123!")
        .unwrap();
    let login = store.login("admin", "StrongPassword123!").unwrap();
    let principal = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    (principal, login.raw_session_token)
}

#[test]
fn revoked_ssh_key_can_be_registered_again_and_survives_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    let (principal, _) = admin(&store);
    let key = store
        .register_ssh_public_key(&principal, "old", KEY)
        .unwrap();
    store.revoke_ssh_public_key(&principal, key.id).unwrap();
    assert_eq!(
        store.authenticate_ssh_public_key(KEY),
        Err(AuthError::Unauthenticated)
    );

    let reopened = AuthStore::open_persistent(&path).unwrap();
    let replacement = reopened
        .register_ssh_public_key(&principal, "replacement", KEY)
        .unwrap();
    assert_ne!(replacement.id, key.id);
    // Revoking an already revoked historical record must not remove the new key's index.
    reopened.revoke_ssh_public_key(&principal, key.id).unwrap();
    assert_eq!(
        reopened.authenticate_ssh_public_key(KEY).unwrap(),
        principal
    );
    let reopened = AuthStore::open_persistent(&path).unwrap();
    assert_eq!(
        reopened.authenticate_ssh_public_key(KEY).unwrap(),
        principal
    );
}

#[test]
fn oauth_exchange_rechecks_originating_session_and_pat() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    let (principal, session) = admin(&store);
    let hash = hash_token_sha256(&session);
    let issued = store
        .issue_oauth_session_from_authorization(principal.user_id, hash)
        .unwrap();
    assert_eq!(
        store
            .authenticate_mcp_session(&issued.raw_session_token)
            .unwrap(),
        principal
    );
    assert_eq!(
        store.authenticate_session(&issued.raw_session_token),
        Err(AuthError::Unauthenticated)
    );
    store.revoke_session(&principal, &session).unwrap();
    assert!(matches!(
        store.issue_oauth_session_from_authorization(principal.user_id, hash),
        Err(AuthError::Unauthenticated)
    ));
    // An MCP token cannot authorize another OAuth session.
    assert!(matches!(
        store.issue_oauth_session_from_authorization(
            principal.user_id,
            hash_token_sha256(&issued.raw_session_token)
        ),
        Err(AuthError::Unauthenticated)
    ));
    let pat = store
        .create_personal_access_token(&principal, "oauth", None)
        .unwrap();
    let digest = hash_token_sha256(&pat.raw_token);
    assert!(store
        .issue_oauth_session_from_authorization(principal.user_id, digest)
        .is_ok());
    // A different process revokes the authorizing credential between code creation/exchange.
    let other = AuthStore::open_persistent(&path).unwrap();
    other
        .revoke_personal_access_token(&principal, pat.metadata.id)
        .unwrap();
    assert!(matches!(
        store.issue_oauth_session_from_authorization(principal.user_id, digest),
        Err(AuthError::Unauthenticated)
    ));
}

#[test]
fn ssh_parser_rejects_oversized_input() {
    assert!(canonicalize_openssh_pubkey(KEY).is_ok());
    let oversized = format!("{KEY} {}", "a".repeat(16 * 1024));
    assert!(canonicalize_openssh_pubkey(&oversized).is_err());
}

#[test]
fn ssh_parser_rejects_noncanonical_padding_bits() {
    let canonical = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAAQE=";
    assert!(canonicalize_openssh_pubkey(canonical).is_ok());
    let noncanonical = canonical.replace("AQE=", "AQF=");
    assert!(canonicalize_openssh_pubkey(&noncanonical).is_err());
}

#[cfg(unix)]
#[test]
fn auth_db_symlinks_and_fifos_are_rejected_without_reading() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.json");
    std::fs::write(
        &target,
        serde_json::to_vec(&crate::models::PersistedSnapshot::default()).unwrap(),
    )
    .unwrap();
    let link = temp.path().join("link.json");
    symlink(&target, &link).unwrap();
    assert!(matches!(
        AuthStore::open_persistent(&link),
        Err(AuthError::StorageError(_))
    ));
    let fifo = temp.path().join("fifo.json");
    let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: CString is NUL-terminated and lives through the call.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert!(matches!(
        AuthStore::open_persistent(&fifo),
        Err(AuthError::StorageError(_))
    ));
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[cfg(unix)]
#[test]
fn existing_auth_snapshot_permissions_are_tightened_without_chmodding_its_parent() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    admin(&store);
    for mode in [0o644, 0o700] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let parent_mode = std::fs::metadata(temp.path()).unwrap().permissions().mode();
        let reopened = AuthStore::open_persistent(&path).unwrap();
        assert_eq!(reopened.persistence_path(), Some(path.as_path()));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(temp.path()).unwrap().permissions().mode(),
            parent_mode
        );
    }
    assert_eq!(AuthStore::new_in_memory().persistence_path(), None);
}

#[cfg(unix)]
#[test]
fn failed_pending_write_unblocks_readers_only_after_memory_rollback() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    let (principal, _) = admin(&store);
    let mut guard = store.write_state().unwrap();
    guard.users.get_mut(&principal.user_id).unwrap().username = "uncommitted".into();
    let pending = store.maybe_snapshot(guard);
    let reader = store.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || tx.send(reader.list_users(&principal)).unwrap());
    let premature = rx.recv_timeout(Duration::from_millis(100));
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let failure = store.persist_snapshot(pending);
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    handle.join().unwrap();
    assert!(
        premature.is_err(),
        "reader saw a mutation before failed persistence"
    );
    assert!(matches!(failure, Err(AuthError::StorageError(_))));
    let users = rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    assert_eq!(users[0].username, "admin");
}

#[test]
fn deactivation_releases_revoked_ssh_fingerprint_without_removing_replacements() {
    let store = AuthStore::new_in_memory();
    let (principal, _) = admin(&store);
    let invite = store
        .create_invite_link(
            &principal,
            "dev@company.internal",
            gitust_core::EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    let developer = store
        .redeem_invite_link(&invite.raw_token, "developer", "StrongPassword123!")
        .unwrap();
    let login = store.login("developer", "StrongPassword123!").unwrap();
    let owner = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    store
        .register_ssh_public_key(&owner, "old-owner", KEY)
        .unwrap();
    store.deactivate_user(&principal, developer.id).unwrap();
    store
        .register_ssh_public_key(&principal, "replacement-owner", KEY)
        .unwrap();
    // Repeated deactivation must not remove a replacement owned by somebody else.
    store.deactivate_user(&principal, developer.id).unwrap();
    assert_eq!(store.authenticate_ssh_public_key(KEY).unwrap(), principal);
}
