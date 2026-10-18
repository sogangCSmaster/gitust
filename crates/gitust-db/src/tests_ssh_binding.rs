use crate::AuthStore;
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole};

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn admin(store: &AuthStore) -> AuthenticatedPrincipal {
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "StrongPassword123!")
        .unwrap();
    let session = store.login("admin", "StrongPassword123!").unwrap();
    store
        .authenticate_session(&session.raw_session_token)
        .unwrap()
}

#[test]
fn ssh_binding_keeps_revoked_registration_identity_after_reopen_and_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    let owner = admin(&store);
    assert!(matches!(
        store.authenticate_ssh_key_binding(KEY),
        Err(AuthError::Unauthenticated)
    ));
    let old = store.register_ssh_public_key(&owner, "old", KEY).unwrap();
    let binding = store.authenticate_ssh_key_binding(KEY).unwrap();
    assert_eq!(store.authorize_ssh_key_binding(&binding).unwrap(), owner);
    assert_eq!(store.authenticate_ssh_public_key(KEY).unwrap(), owner);

    store.revoke_ssh_public_key(&owner, old.id).unwrap();
    let reopened = AuthStore::open_persistent(&path).unwrap();
    let replacement = reopened
        .register_ssh_public_key(&owner, "replacement", KEY)
        .unwrap();
    assert_ne!(old.id, replacement.id);
    assert_eq!(
        reopened.authorize_ssh_key_binding(&binding),
        Err(AuthError::Unauthenticated)
    );
    let fresh = reopened.authenticate_ssh_key_binding(KEY).unwrap();
    assert_eq!(reopened.authorize_ssh_key_binding(&fresh).unwrap(), owner);
}

#[test]
fn ssh_binding_does_not_follow_same_key_to_a_different_owner() {
    let store = AuthStore::new_in_memory();
    let operator = admin(&store);
    let invite = store
        .create_invite_link(
            &operator,
            "developer@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    let user = store
        .redeem_invite_link(&invite.raw_token, "developer", "StrongPassword123!")
        .unwrap();
    let session = store.login("developer", "StrongPassword123!").unwrap();
    let developer = store
        .authenticate_session(&session.raw_session_token)
        .unwrap();
    store
        .register_ssh_public_key(&developer, "developer", KEY)
        .unwrap();
    let binding = store.authenticate_ssh_key_binding(KEY).unwrap();
    assert_eq!(
        store.authorize_ssh_key_binding(&binding).unwrap(),
        developer
    );

    store.deactivate_user(&operator, user.id).unwrap();
    assert_eq!(
        store.authorize_ssh_key_binding(&binding),
        Err(AuthError::AccountDeactivated)
    );
    store
        .register_ssh_public_key(&operator, "replacement-owner", KEY)
        .unwrap();
    assert_eq!(
        store.authorize_ssh_key_binding(&binding),
        Err(AuthError::AccountDeactivated)
    );
    let fresh = store.authenticate_ssh_key_binding(KEY).unwrap();
    assert_eq!(store.authorize_ssh_key_binding(&fresh).unwrap(), operator);
}

#[test]
fn ecdsa_nistp256_384_521_keys_bind_and_mismatched_inner_curve_is_rejected() {
    const ECDSA_P256: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0A=";
    const ECDSA_P384: &str = "ecdsa-sha2-nistp384 AAAAE2VjZHNhLXNoYTItbmlzdHAzODQAAAAIbmlzdHAzODQAAABhBAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYA==";
    const ECDSA_P521: &str = "ecdsa-sha2-nistp521 AAAAE2VjZHNhLXNoYTItbmlzdHA1MjEAAAAIbmlzdHA1MjEAAACFBAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhA==";
    const ECDSA_BAD_CURVE: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAzODQAAABBBAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

    let store = AuthStore::new_in_memory();
    let owner = admin(&store);
    for (title, key) in [
        ("p256", ECDSA_P256),
        ("p384", ECDSA_P384),
        ("p521", ECDSA_P521),
    ] {
        let view = store.register_ssh_public_key(&owner, title, key).unwrap();
        assert!(!view.revoked);
        let binding = store.authenticate_ssh_key_binding(key).unwrap();
        assert_eq!(store.authorize_ssh_key_binding(&binding).unwrap(), owner);
        assert_eq!(store.authenticate_ssh_public_key(key).unwrap(), owner);
    }
    assert!(matches!(
        store.register_ssh_public_key(&owner, "bad-curve", ECDSA_BAD_CURVE),
        Err(AuthError::InvalidInput(_))
    ));
}
