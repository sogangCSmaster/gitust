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
