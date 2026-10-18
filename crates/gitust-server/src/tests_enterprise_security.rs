//! Integration tests for Slice 9 (Phase 4.4):
//! - RFC 6238 Two-Factor Authentication (TOTP enrollment, verification, login enforcement, replay protection)
//! - OIDC/LDAP Enterprise SSO Provider registration, signature verification, domain lock, and invite-bound provisioning
//! - Tamper-Evident SHA-256 Hash-Chained Audit Trail (`GENESIS_AUDIT_HASH`, chain verification, and tamper detection)

use super::tests_pull_requests::{api_json, invite_and_login_role};
use super::tests_repository_visibility::fixture;
use super::*;
use chrono::Utc;
use gitust_core::EnterpriseRole;
use gitust_db::{
    compute_sso_assertion_signature, compute_totp_code_at, AccountInviteRequest, GENESIS_AUDIT_HASH,
};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn totp_enrollment_verification_login_enforcement_replay_rejection_and_disable() {
    let (_temp, state, admin_token, _) = fixture();
    let app = build_router(state.clone());
    let dev_token = invite_and_login_role(
        &state,
        &admin_token,
        "dev2fa@company.internal",
        "dev2fa",
        EnterpriseRole::Developer,
        vec![],
    );

    // 1. Initial TOTP status is not enrolled
    let (status, st_res) = api_json(
        &app,
        "GET",
        "/api/v1/auth/totp/status".to_owned(),
        &dev_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "totp status: {st_res}");
    assert_eq!(st_res["enrolled"], false);
    assert_eq!(st_res["verified"], false);

    // 2. Enroll in TOTP -> returns Base32 secret & otpauth:// URI
    let (status, enroll_res) = api_json(
        &app,
        "POST",
        "/api/v1/auth/totp/enroll".to_owned(),
        &dev_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "totp enroll: {enroll_res}");
    let secret_b32 = enroll_res["secret_base32"]
        .as_str()
        .expect("secret_base32")
        .to_owned();
    assert!(!secret_b32.is_empty());
    assert!(enroll_res["otpauth_uri"]
        .as_str()
        .unwrap_or_default()
        .starts_with("otpauth://totp/"));
    assert_eq!(enroll_res["digits"], 6);
    assert_eq!(enroll_res["period_secs"], 30);

    // 3. Before verification, password-only login still works
    let (status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/login".to_owned(),
        "",
        Some(json!({
            "identifier": "dev2fa",
            "password": "Password1234!"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 4. Verify enrollment with wrong code -> 401 Unauthorized
    let (status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/totp/verify".to_owned(),
        &dev_token,
        Some(json!({ "code": "000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 5. Verify enrollment with valid RFC 6238 code at step (now - 30s) so step `now` remains unconsumed for login
    let now_secs = u64::try_from(Utc::now().timestamp()).unwrap_or(0);
    let verify_code = compute_totp_code_at(&secret_b32, now_secs.saturating_sub(30))
        .expect("compute verify totp code");
    let (status, ver_res) = api_json(
        &app,
        "POST",
        "/api/v1/auth/totp/verify".to_owned(),
        &dev_token,
        Some(json!({ "code": verify_code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "totp verify: {ver_res}");
    assert_eq!(ver_res["enrolled"], true);
    assert_eq!(ver_res["verified"], true);

    // 6. Password login without totp_code now fails closed with 401 Unauthorized (TotpRequired)
    let (status, missing_totp_res) = api_json(
        &app,
        "POST",
        "/api/v1/auth/login".to_owned(),
        "",
        Some(json!({
            "identifier": "dev2fa",
            "password": "Password1234!"
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "missing totp_code must be rejected: {missing_totp_res}"
    );

    // 7. Replaying the already-consumed verification code fails closed with 401 Unauthorized
    let (status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/login".to_owned(),
        "",
        Some(json!({
            "identifier": "dev2fa",
            "password": "Password1234!",
            "totp_code": verify_code
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "replayed code must fail");

    // 8. Login with current step's valid TOTP code succeeds
    let login_code = compute_totp_code_at(&secret_b32, now_secs).expect("compute login totp code");
    let (status, login_res) = api_json(
        &app,
        "POST",
        "/api/v1/auth/login".to_owned(),
        "",
        Some(json!({
            "identifier": "dev2fa",
            "password": "Password1234!",
            "totp_code": login_code
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login with totp: {login_res}");
    assert!(login_res["raw_session_token"]
        .as_str()
        .unwrap_or_default()
        .starts_with("gitust_ses_"));

    // 9. Replaying the same login_code in the same 30s step fails closed
    let (status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/login".to_owned(),
        "",
        Some(json!({
            "identifier": "dev2fa",
            "password": "Password1234!",
            "totp_code": login_code
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 10. Disable 2FA using next step's valid TOTP code (+30s within clock-skew window)
    let disable_code = compute_totp_code_at(&secret_b32, now_secs.saturating_add(30))
        .expect("compute disable totp code");
    let (status, dis_res) = api_json(
        &app,
        "DELETE",
        "/api/v1/auth/totp".to_owned(),
        &dev_token,
        Some(json!({ "code": disable_code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "disable totp: {dis_res}");
    assert_eq!(dis_res["enrolled"], false);
    assert_eq!(dis_res["verified"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sso_oidc_ldap_invite_bound_provisioning_and_tamper_evident_hash_chained_audit_log() {
    let (_temp, state, admin_token, _) = fixture();
    let app = build_router(state.clone());
    let dev_token = invite_and_login_role(
        &state,
        &admin_token,
        "existing@company.internal",
        "existingdev",
        EnterpriseRole::Developer,
        vec![],
    );

    // 1. Non-admin cannot register or list SSO providers or read the enterprise audit log (403)
    for (method, path) in [
        ("GET", "/api/v1/admin/sso/providers"),
        ("GET", "/api/v1/admin/audit-log"),
    ] {
        let (status, _) = api_json(&app, method, path.to_owned(), &dev_token, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // 2. SiteAdmin registers an OIDC SSO provider
    let signing_secret = "corp-oidc-hmac-signing-secret-key-2026";
    let (status, prov_res) = api_json(
        &app,
        "POST",
        "/api/v1/admin/sso/providers".to_owned(),
        &admin_token,
        Some(json!({
            "name": "Corp Okta OIDC",
            "protocol": "oidc",
            "issuer_or_url": "https://id.company.internal/oauth2/default",
            "client_id": "gitust-enterprise-web",
            "signing_secret": signing_secret,
            "allowed_domains": ["company.internal"],
            "enabled": true,
            "require_invite_binding": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register sso: {prov_res}");
    let provider_id = Uuid::parse_str(prov_res["id"].as_str().expect("provider id")).unwrap();
    assert_eq!(prov_res["protocol"], "oidc");
    assert!(prov_res.get("signing_secret").is_none());

    // 3. Existing user can sign in via signed OIDC assertion
    let exp = u64::try_from(Utc::now().timestamp()).unwrap_or(0) + 300;
    let valid_sig = compute_sso_assertion_signature(
        signing_secret,
        provider_id,
        "existing@company.internal",
        "existingdev",
        exp,
    );
    let (status, sso_login_res) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "email": "existing@company.internal",
            "username": "existingdev",
            "exp_epoch_secs": exp,
            "signature_hex": valid_sig
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "existing sso login: {sso_login_res}"
    );
    assert_eq!(sso_login_res["user"]["username"], "existingdev");

    // 4. Forged signature is rejected with 401 Unauthorized
    let (status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "email": "existing@company.internal",
            "username": "existingdev",
            "exp_epoch_secs": exp,
            "signature_hex": "00".repeat(32)
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 5. Uninvited new user attempting SSO login without an admin invite token is blocked (403 PublicSignupDisabled)
    let new_sig = compute_sso_assertion_signature(
        signing_secret,
        provider_id,
        "newbie@company.internal",
        "newbiedev",
        exp,
    );
    let (status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "email": "newbie@company.internal",
            "username": "newbiedev",
            "exp_epoch_secs": exp,
            "signature_hex": new_sig
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 6. Admin issues an invite for `newbie@company.internal`; SSO login with `invite_token` provisions account & session
    let admin_principal = state.auth_store.authenticate_session(&admin_token).unwrap();
    let issued_invite = state
        .auth_store
        .create_invite_link_with_credential(
            &admin_token,
            admin_principal.user_id,
            AccountInviteRequest {
                target_email: "newbie@company.internal",
                assigned_role: EnterpriseRole::Maintainer,
                team_ids: vec![],
                ttl_hours: Some(24),
            },
        )
        .expect("issue invite for sso newbie");

    let (status, provisioned_res) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "email": "newbie@company.internal",
            "username": "newbiedev",
            "exp_epoch_secs": exp,
            "signature_hex": new_sig,
            "invite_token": issued_invite.raw_token
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "invite-bound sso provision: {provisioned_res}"
    );
    assert_eq!(provisioned_res["user"]["username"], "newbiedev");
    assert_eq!(provisioned_res["user"]["role"], "Maintainer");

    // 7. Verify Tamper-Evident SHA-256 Hash-Chained Audit Log (`GET /api/v1/admin/audit-log`)
    let (status, audit_res) = api_json(
        &app,
        "GET",
        "/api/v1/admin/audit-log".to_owned(),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "audit log: {audit_res}");
    assert_eq!(audit_res["verification"]["verified"], true);
    let entries = audit_res["entries"].as_array().expect("entries array");
    assert!(entries.len() >= 4);
    assert_eq!(entries[0]["seq"], 1);
    assert_eq!(entries[0]["prev_hash"], GENESIS_AUDIT_HASH);
    for pair in entries.windows(2) {
        assert_eq!(pair[1]["prev_hash"], pair[0]["entry_hash"]);
    }

    // 7b. Reject pipe delimiter in SSO provider name and rate-limit invalid-invite SSO probes
    let (bad_name_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/admin/sso/providers".to_owned(),
        &admin_token,
        Some(json!({
            "name": "Bad|Provider",
            "protocol": "oidc",
            "issuer_or_url": "https://idp.company.internal",
            "client_id": "gitust-web",
            "signing_secret": signing_secret,
            "allowed_domains": ["company.internal"]
        })),
    )
    .await;
    assert_eq!(bad_name_status, StatusCode::BAD_REQUEST);

    let probe_sig = compute_sso_assertion_signature(
        signing_secret,
        provider_id,
        "probe@company.internal",
        "probedev",
        exp,
    );
    for _ in 0..5 {
        let (st, _) = api_json(
            &app,
            "POST",
            "/api/v1/auth/sso/login".to_owned(),
            "",
            Some(json!({
                "provider_id": provider_id,
                "email": "probe@company.internal",
                "username": "probedev",
                "exp_epoch_secs": exp,
                "signature_hex": probe_sig,
                "invite_token": "gitust_inv_invalid_probe"
            })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    let (rl_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "email": "probe@company.internal",
            "username": "probedev",
            "exp_epoch_secs": exp,
            "signature_hex": probe_sig,
            "invite_token": "gitust_inv_invalid_probe"
        })),
    )
    .await;
    assert_eq!(rl_status, StatusCode::TOO_MANY_REQUESTS);

    // 8. Simulate on-disk tampering of an audit entry in auth-db.json and verify detection
    let db_path = state
        .auth_store
        .persistence_path()
        .expect("persistence path")
        .to_path_buf();
    let raw_db = std::fs::read_to_string(&db_path).expect("read auth-db.json");
    let tampered_db = raw_db.replacen("\"sso.login\"", "\"sso.tampered\"", 1);
    assert_ne!(raw_db, tampered_db);
    std::fs::write(&db_path, tampered_db).expect("write tampered auth-db.json");

    let (status, tampered_audit) = api_json(
        &app,
        "GET",
        "/api/v1/admin/audit-log".to_owned(),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tampered_audit["verification"]["verified"], false,
        "hash chain verification must detect tampered entry: {tampered_audit}"
    );
    assert!(tampered_audit["verification"]["first_invalid_seq"].is_number());
}
