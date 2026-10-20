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

#[test]
fn rsa_keys_bind_and_malformed_wire_fields_are_rejected() {
    // Valid wire structure: string("ssh-rsa") || mpint(e=65537) || mpint(n=0x0080...01)
    const RSA_VALID: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAAQE= rsa-user@host";
    // Missing modulus `n` field after `e`: string("ssh-rsa") || mpint(e=65537)
    const RSA_MISSING_N: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB";
    // Trailing garbage byte after `n`: string("ssh-rsa") || mpint(e=65537) || mpint(n=1) || 0xff
    const RSA_TRAILING_BYTES: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAAQH/";

    let store = AuthStore::new_in_memory();
    let owner = admin(&store);
    let view = store
        .register_ssh_public_key(&owner, "rsa-valid", RSA_VALID)
        .expect("register valid rsa");
    assert!(!view.revoked);
    let binding = store
        .authenticate_ssh_key_binding("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAAQE=")
        .expect("bind rsa without comment");
    assert_eq!(store.authorize_ssh_key_binding(&binding).unwrap(), owner);

    for (title, bad_key) in [
        ("missing-n", RSA_MISSING_N),
        ("trailing", RSA_TRAILING_BYTES),
    ] {
        assert!(
            matches!(
                store.register_ssh_public_key(&owner, title, bad_key),
                Err(AuthError::InvalidInput(_))
            ),
            "malformed RSA key '{title}' must be rejected"
        );
    }

    // Rare/adversarial wire cases:
    // 1. Outer header says `ssh-rsa` but inner wire blob is `ssh-ed25519`
    const SPOOFED_OUTER_RSA: &str =
        "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
    // 2. Outer header says `ssh-ed25519` but inner wire blob is `ssh-rsa`
    const SPOOFED_OUTER_ED25519: &str = "ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAAAAQE=";
    // 3. Redundant leading 0x00 byte in mpint `e` (`[0x00, 0x01, 0x00, 0x01]` when MSB of 0x01 is 0 — RFC 4251 malleability)
    const RSA_NONCANONICAL_ZERO_PAD: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAAEAAABAAEAAAABAQ==";
    // 4. Negative mpint (`e = [0x81]` without leading 0x00 sign-extension byte)
    const RSA_NEGATIVE_MPINT: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAABgQAAAAEB";
    // 5. Even public exponent (`e = [0x02]`)
    const RSA_EVEN_EXPONENT: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAABAgAAAAEB";

    for (title, adv_key) in [
        ("spoofed-outer-rsa", SPOOFED_OUTER_RSA),
        ("spoofed-outer-ed25519", SPOOFED_OUTER_ED25519),
        ("noncanonical-zero-pad", RSA_NONCANONICAL_ZERO_PAD),
        ("negative-mpint", RSA_NEGATIVE_MPINT),
        ("even-exponent", RSA_EVEN_EXPONENT),
    ] {
        assert!(
            matches!(
                store.register_ssh_public_key(&owner, title, adv_key),
                Err(AuthError::InvalidInput(_))
            ),
            "adversarial RSA key '{title}' must be rejected fail-closed"
        );
    }
}
