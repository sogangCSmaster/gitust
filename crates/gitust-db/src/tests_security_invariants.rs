//! Structural upgrade invariants; no shared-password login or external identity service is used.
use crate::*;
use gitust_core::{RepoAccess, RepoId, SsoProtocol};

fn fixture() -> (AuthStore, String, AuthenticatedPrincipal) {
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "FixturePassword!2026")
        .unwrap();
    let token = store
        .login("admin", "FixturePassword!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&token).unwrap();
    (store, token, admin)
}
fn provider(
    store: &AuthStore,
    token: &str,
    admin: &AuthenticatedPrincipal,
) -> gitust_core::SsoProviderRecord {
    store
        .register_sso_provider_with_credential(
            token,
            admin.user_id,
            RegisterSsoProviderInput {
                name: "fixture-assertion-bridge".into(),
                protocol: SsoProtocol::Oidc,
                issuer_or_url: "https://identity.invalid".into(),
                client_id: "fixture".into(),
                signing_secret: "synthetic-signing-secret-for-tests".into(),
                allowed_domains: vec!["company.internal".into()],
                enabled: true,
                require_invite_binding: true,
            },
        )
        .unwrap()
}
#[test]
fn serialized_secret_records_never_include_a_decryption_key() {
    let (store, token, admin) = fixture();
    store
        .set_workflow_secret(
            &admin,
            RepoAccess {
                repo_id: RepoId::new(),
                registration: None,
            },
            "FIXTURE",
            "synthetic-value",
        )
        .unwrap();
    store
        .enroll_totp_with_credential(&token, admin.user_id)
        .unwrap();
    provider(&store, &token, &admin);
    let guard = store.state.read();
    for encoded in [
        serde_json::to_string(&guard.workflow_secrets).unwrap(),
        serde_json::to_string(&guard.totp_enrollments).unwrap(),
        serde_json::to_string(&guard.sso_providers).unwrap(),
    ] {
        assert!(
            !encoded.contains("key_hex"),
            "secret record contains decryption key field"
        );
    }
}
#[test]
fn sso_provisioning_never_installs_a_password_authenticator() {
    let (store, token, admin) = fixture();
    let provider = provider(&store, &token, &admin);
    let invite = store
        .create_invite_link(
            &admin,
            "sso@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    let exp = chrono::Utc::now().timestamp() as u64 + 60;
    let user = store
        .login_with_sso_assertion(
            SsoAssertionLoginInput {
                provider_id: provider.id,
                email: "sso@company.internal".into(),
                username: "ssofixture".into(),
                exp_epoch_secs: exp,
                signature_hex: compute_sso_assertion_signature(
                    "synthetic-signing-secret-for-tests",
                    provider.id,
                    "sso@company.internal",
                    "ssofixture",
                    exp,
                ),
                invite_token: Some(invite.raw_token),
                totp_code: None,
            },
            None,
        )
        .unwrap()
        .user;
    let guard = store.state.read();
    let hash = &guard.users[&user.id].password_hash;
    assert!(
        !hash.starts_with("$argon2"),
        "SSO account must have password authentication disabled"
    );
}
