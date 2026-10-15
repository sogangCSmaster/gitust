use chrono::{Duration, Utc};
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole, TeamId};
use std::fs;

use crate::crypto::canonicalize_openssh_pubkey;
use crate::{AuthStore, MAX_LOGIN_ATTEMPTS, MAX_RATE_LIMIT_TRACKED_KEYS};

fn setup_store_with_admin() -> (AuthStore, AuthenticatedPrincipal) {
    let store = AuthStore::new_in_memory();
    let admin_profile = store
        .bootstrap_initial_admin_cli(
            "rootadmin",
            "rootadmin@company.internal",
            "SuperSecretAdminPass!2026",
        )
        .expect("initial CLI admin bootstrap must succeed");

    let login_res = store
        .login("rootadmin", "SuperSecretAdminPass!2026")
        .expect("admin login must succeed");
    let principal = store
        .authenticate_session(&login_res.raw_session_token)
        .expect("session auth must succeed");

    assert_eq!(admin_profile.id, principal.user_id);
    (store, principal)
}

#[test]
fn gate_cli_only_initial_admin_and_no_second_bootstrap() {
    let (store, _) = setup_store_with_admin();

    let second = store.bootstrap_initial_admin_cli(
        "attacker",
        "attacker@company.internal",
        "AnotherSecretPass!2026",
    );
    assert_eq!(second, Err(AuthError::AdminAlreadyBootstrapped));
}

#[test]
fn gate_invite_link_single_use_expiration_and_revocation() {
    let (store, admin) = setup_store_with_admin();
    let team_a = TeamId::new();

    // 1. Issue valid invite link and redeem once -> succeeds
    let issued = store
        .create_invite_link(
            &admin,
            "alice@company.internal",
            EnterpriseRole::Developer,
            vec![team_a],
            None,
        )
        .expect("admin should issue invite");

    let alice = store
        .redeem_invite_link(&issued.raw_token, "alice", "AliceStrongPass!2026")
        .expect("first redemption must succeed");
    assert_eq!(alice.username, "alice");

    // 2. Reusing the exact same invite link a second time must be rejected (One-Time Use)
    let reuse_err = store.redeem_invite_link(&issued.raw_token, "alice2", "AliceStrongPass!2026");
    assert_eq!(reuse_err, Err(AuthError::TokenAlreadyUsed));

    // 3. Revoked invite link must be rejected
    let issued_bob = store
        .create_invite_link(
            &admin,
            "bob@company.internal",
            EnterpriseRole::Developer,
            vec![team_a],
            None,
        )
        .expect("admin should issue bob invite");
    store
        .revoke_invite_link(&admin, issued_bob.invite.id)
        .expect("admin should revoke bob invite");

    let revoked_err = store.redeem_invite_link(&issued_bob.raw_token, "bob", "BobStrongPass!2026");
    assert_eq!(revoked_err, Err(AuthError::TokenRevoked));

    // 4. Expired invite link must be rejected
    let issued_charlie = store
        .create_invite_link(
            &admin,
            "charlie@company.internal",
            EnterpriseRole::Developer,
            vec![team_a],
            Some(1),
        )
        .expect("admin should issue charlie invite");
    {
        let mut guard = store.state.write();
        if let Some(inv) = guard.invites_by_id.get_mut(&issued_charlie.invite.id) {
            inv.expires_at = Utc::now() - Duration::minutes(1);
        }
    }
    let expired_err = store.redeem_invite_link(
        &issued_charlie.raw_token,
        "charlie",
        "CharlieStrongPass!2026",
    );
    assert_eq!(expired_err, Err(AuthError::TokenExpired));
}

#[test]
fn gate_deactivated_account_immediately_invalidates_session_pat_ssh_and_reset_links() {
    let (store, admin) = setup_store_with_admin();
    let team_a = TeamId::new();

    let invite = store
        .create_invite_link(
            &admin,
            "dev1@company.internal",
            EnterpriseRole::Developer,
            vec![team_a],
            None,
        )
        .expect("invite must succeed");
    let dev_profile = store
        .redeem_invite_link(&invite.raw_token, "dev1", "Dev1StrongPass!2026")
        .expect("redeem must succeed");

    let session = store
        .login("dev1", "Dev1StrongPass!2026")
        .expect("login must succeed");
    let dev_principal = store
        .authenticate_session(&session.raw_session_token)
        .expect("session valid");

    let pat = store
        .create_personal_access_token(&dev_principal, "laptop-git-https", Some(30))
        .expect("pat creation must succeed");
    let ssh_key_str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGitustTestKeyDev100 dev1@company.internal";
    store
        .register_ssh_public_key(&dev_principal, "MacBook Pro", ssh_key_str)
        .expect("ssh key registration must succeed");

    let reset_link = store
        .create_password_reset_link(&admin, dev_profile.id, None)
        .expect("issue password reset");

    // Verify all 3 credentials work before deactivation
    assert!(store
        .authenticate_session(&session.raw_session_token)
        .is_ok());
    assert!(store.authenticate_pat(&pat.raw_token).is_ok());
    assert!(store.authenticate_ssh_public_key(ssh_key_str).is_ok());

    // SiteAdmin deactivates dev1
    store
        .deactivate_user(&admin, dev_profile.id)
        .expect("deactivation must succeed");

    // All credentials + pending reset link + re-login must immediately fail with AccountDeactivated / TokenRevoked!
    assert_eq!(
        store.authenticate_session(&session.raw_session_token),
        Err(AuthError::AccountDeactivated)
    );
    assert_eq!(
        store.authenticate_pat(&pat.raw_token),
        Err(AuthError::AccountDeactivated)
    );
    assert_eq!(
        store.authenticate_ssh_public_key(ssh_key_str),
        Err(AuthError::AccountDeactivated)
    );
    assert_eq!(
        store.redeem_password_reset_link(&reset_link.raw_token, "NewDevPass!2026"),
        Err(AuthError::TokenRevoked)
    );
    assert_eq!(
        store.login("dev1", "Dev1StrongPass!2026").unwrap_err(),
        AuthError::AccountDeactivated
    );
}

#[test]
fn gate_idor_bola_cross_user_token_and_cross_team_repo_blocked() {
    let (store, admin) = setup_store_with_admin();
    let team_alpha = TeamId::new();
    let team_beta = TeamId::new();

    let inv_a = store
        .create_invite_link(
            &admin,
            "alpha@company.internal",
            EnterpriseRole::Developer,
            vec![team_alpha],
            None,
        )
        .expect("invite alpha");
    store
        .redeem_invite_link(&inv_a.raw_token, "alpha", "AlphaStrongPass!2026")
        .expect("redeem alpha");
    let ses_a = store
        .login("alpha", "AlphaStrongPass!2026")
        .expect("login alpha");
    let principal_a = store
        .authenticate_session(&ses_a.raw_session_token)
        .expect("auth alpha");

    let inv_b = store
        .create_invite_link(
            &admin,
            "beta@company.internal",
            EnterpriseRole::Developer,
            vec![team_beta],
            None,
        )
        .expect("invite beta");
    store
        .redeem_invite_link(&inv_b.raw_token, "beta", "BetaStrongPass!2026")
        .expect("redeem beta");
    let ses_b = store
        .login("beta", "BetaStrongPass!2026")
        .expect("login beta");
    let principal_b = store
        .authenticate_session(&ses_b.raw_session_token)
        .expect("auth beta");

    // 1. BFLA: Non-admin User Beta tries to issue an invite link -> must be blocked
    let bfla_invite = store.create_invite_link(
        &principal_b,
        "friend@company.internal",
        EnterpriseRole::Developer,
        vec![],
        None,
    );
    assert_eq!(bfla_invite.unwrap_err(), AuthError::AdminPrivilegeRequired);

    // 2. IDOR / BOLA: User Beta tries to revoke User Alpha's PAT & SSH key
    let alpha_pat = store
        .create_personal_access_token(&principal_a, "alpha-token", None)
        .expect("alpha pat");
    let alpha_ssh = store
        .register_ssh_public_key(
            &principal_a,
            "alpha-key",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey000 comment@host",
        )
        .expect("alpha ssh");

    assert_eq!(
        store.revoke_personal_access_token(&principal_b, alpha_pat.metadata.id),
        Err(AuthError::ForbiddenResourceOwnership)
    );
    assert_eq!(
        store.revoke_ssh_public_key(&principal_b, alpha_ssh.id),
        Err(AuthError::ForbiddenResourceOwnership)
    );

    // 3. Canonical SSH key check: re-registering same key with a different comment must be rejected
    assert!(store
        .register_ssh_public_key(
            &principal_b,
            "duplicate-key-different-comment",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey000 attacker@otherhost",
        )
        .is_err());

    // 4. Cross-Team Repository BOLA: User Beta tries to access Team Alpha's private repository
    let alpha_repo = store
        .create_team_repository(&principal_a, "alpha-core-engine", team_alpha, true)
        .expect("create team alpha repo");

    assert!(store
        .authorize_repository_access(&principal_a, alpha_repo.id)
        .is_ok());
    assert_eq!(
        store.authorize_repository_access(&principal_b, alpha_repo.id),
        Err(AuthError::ForbiddenRepositoryAccess)
    );
}

#[test]
fn gate_login_rate_limiting_blocks_dual_identifier_alternation_and_password_reset_clears() {
    let (store, admin) = setup_store_with_admin();
    let root_id = admin.user_id;

    // Alternate between username ("rootadmin") and email ("rootadmin@company.internal")
    for i in 0..MAX_LOGIN_ATTEMPTS {
        let id_str = if i % 2 == 0 {
            "rootadmin"
        } else {
            "rootadmin@company.internal"
        };
        let res = store.login(id_str, "WrongPassword123!");
        assert_eq!(res.unwrap_err(), AuthError::InvalidCredentials);
    }

    // Next attempt using EITHER username or email must be blocked by canonical uid rate limiter!
    assert_eq!(
        store
            .login("rootadmin", "SuperSecretAdminPass!2026")
            .unwrap_err(),
        AuthError::RateLimitExceeded
    );
    assert_eq!(
        store
            .login("rootadmin@company.internal", "SuperSecretAdminPass!2026")
            .unwrap_err(),
        AuthError::RateLimitExceeded
    );

    // An attacker locking out their own client scope must NOT lock out the legitimate admin
    // from a different client scope (Account Lockout DoS defense).
    assert!(store
        .login_with_client_scope("rootadmin", "SuperSecretAdminPass!2026", Some("10.0.0.10"))
        .is_ok());

    // Redeeming a password reset link clears the lockout and rotates the password
    let reset = store
        .create_password_reset_link(&admin, root_id, None)
        .expect("issue reset");
    store
        .redeem_password_reset_link(&reset.raw_token, "RotatedAdminPass!2026")
        .expect("redeem reset");

    assert!(store.login("rootadmin", "RotatedAdminPass!2026").is_ok());

    // Flooding MAX_RATE_LIMIT_TRACKED_KEYS fake identifier keys must NOT fail open or allow
    // brute-forcing a real account.
    {
        let now = Utc::now();
        let mut st = store.state.write();
        for i in 0..MAX_RATE_LIMIT_TRACKED_KEYS {
            AuthStore::record_failed_login_locked(&mut st, format!("scope:flood:id:fake_{i}"), now);
        }
    }
    for _ in 0..MAX_LOGIN_ATTEMPTS {
        let _ = store.login_with_client_scope("rootadmin", "WrongPass!999", Some("flood"));
    }
    assert_eq!(
        store
            .login_with_client_scope("rootadmin", "RotatedAdminPass!2026", Some("flood"))
            .unwrap_err(),
        AuthError::RateLimitExceeded
    );

    // Oversized (1.9 MiB) login identifiers are rejected before allocating or inserting into
    // `failed_logins`, and all stored `:id:` keys use fixed-length SHA-256 hex digests.
    let huge_id = "x".repeat(1_900_000);
    assert_eq!(
        store
            .login_with_client_scope(&huge_id, "WrongPass!999", Some("huge"))
            .unwrap_err(),
        AuthError::InvalidCredentials
    );
    assert!(store
        .state
        .read()
        .failed_logins
        .keys()
        .all(|k| k.len() <= 200));
}

#[test]
fn ssh_pubkey_rejects_mismatched_wire_header_and_malformed_base64() {
    // Declared `ssh-rsa`, but wire blob header encodes `ssh-ed25519`.
    assert!(
        canonicalize_openssh_pubkey("ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey000 user@host")
            .is_err()
    );
    // Non-4-byte-aligned base64 blob.
    assert!(
        canonicalize_openssh_pubkey("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey user@host")
            .is_err()
    );
}

#[test]
fn gate_persistent_db_never_writes_raw_tokens_to_disk() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let db_path = temp_dir.path().join("gitust-auth.json");

    let store = AuthStore::open_persistent(&db_path).expect("open persistent store");
    store
        .bootstrap_initial_admin_cli("admin1", "admin1@company.internal", "Admin1StrongPass!2026")
        .expect("bootstrap admin");
    let login = store
        .login("admin1", "Admin1StrongPass!2026")
        .expect("login admin");
    let admin_principal = store
        .authenticate_session(&login.raw_session_token)
        .expect("auth session");

    let issued_invite = store
        .create_invite_link(
            &admin_principal,
            "newdev@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("create invite");

    let on_disk_json = fs::read_to_string(&db_path).expect("read db file");
    assert!(!on_disk_json.contains(&issued_invite.raw_token));
    assert!(!on_disk_json.contains(&login.raw_session_token));
    assert!(!on_disk_json.contains("Admin1StrongPass!2026"));
}
