//! TDD tests for Standard OIDC (JWT RS256/RS512 with live HTTP Discovery + JWKS & HS256)
//! and Real TCP LDAP v3 (RFC 4511 ASN.1 BER BindRequest/BindResponse + SearchRequest/SearchResultEntry).

use std::io::{Read, Write};
use std::net::TcpListener;

use super::tests_pull_requests::api_json;
use super::tests_repository_visibility::fixture;
use super::*;
use gitust_core::EnterpriseRole;
use russh::keys::signature::Signer;
use russh::keys::ssh_key::{private::KeypairData, PrivateKey};
use serde_json::json;

fn b64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        out.push(TABLE[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standard_oidc_id_token_jwks_discovery_and_fail_closed_validation() {
    let (_temp, state, admin_token, _) = fixture();
    let app = build_router(state.clone());

    // Generate a 2048-bit RSA keypair for the external OIDC Identity Provider
    let key_dir = tempfile::tempdir().expect("key dir");
    let key_path = key_dir.path().join("idp_rsa");
    let status = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "rsa", "-b", "2048", "-N", "", "-f"])
        .arg(&key_path)
        .status()
        .expect("ssh-keygen");
    assert!(status.success());
    let priv_key = PrivateKey::from_openssh(std::fs::read(&key_path).expect("read key"))
        .expect("parse openssh rsa key");
    let KeypairData::Rsa(rsa_kp) = priv_key.key_data() else {
        panic!("expected RSA keypair");
    };
    let n_b64 = b64url(rsa_kp.public().n().as_positive_bytes().expect("positive n"));
    let e_b64 = b64url(rsa_kp.public().e().as_positive_bytes().expect("positive e"));

    // Spawn a real HTTP OIDC Identity Provider serving /.well-known/openid-configuration and /.well-known/jwks.json
    let idp_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind idp");
    let idp_addr = idp_listener.local_addr().expect("idp addr");
    let issuer_url = format!("http://127.0.0.1:{}", idp_addr.port());
    let issuer_cl = issuer_url.clone();
    let jwks_payload = json!({
        "keys": [{
            "kty": "RSA",
            "kid": "idp-rsa-2026",
            "use": "sig",
            "alg": "RS512",
            "n": n_b64,
            "e": e_b64
        }]
    });
    let idp_router = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let iss = issuer_cl.clone();
                async move {
                    axum::Json(json!({
                        "issuer": iss,
                        "jwks_uri": format!("{iss}/.well-known/jwks.json"),
                        "id_token_signing_alg_values_supported": ["RS256", "RS512", "HS256"]
                    }))
                }
            }),
        )
        .route(
            "/.well-known/jwks.json",
            axum::routing::get(move || {
                let jwks = jwks_payload.clone();
                async move { axum::Json(jwks) }
            }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(idp_listener, idp_router).await;
    });

    // Register the OIDC provider in Gitust
    let (reg_status, prov_json) = api_json(
        &app,
        "POST",
        "/api/v1/admin/sso/providers".to_owned(),
        &admin_token,
        Some(json!({
            "name": "Okta-Enterprise-OIDC",
            "protocol": "oidc",
            "issuer_or_url": issuer_url,
            "client_id": "gitust-web-client",
            "signing_secret": "oidc-client-secret-fallback-2026",
            "allowed_domains": ["company.internal"],
            "enabled": true,
            "require_invite_binding": true
        })),
    )
    .await;
    assert_eq!(
        reg_status,
        StatusCode::CREATED,
        "register OIDC: {prov_json}"
    );
    let provider_id = prov_json["id"].as_str().expect("provider id").to_owned();

    let now_secs = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0);
    let claims = json!({
        "iss": issuer_url,
        "sub": "okta-subject-001",
        "aud": "gitust-web-client",
        "exp": now_secs + 300,
        "iat": now_secs,
        "nonce": "nonce-xyz-123",
        "email": "oidcuser@company.internal",
        "email_verified": true,
        "preferred_username": "oidcuser"
    });

    // 1. Reject alg: "none" unsigned JWT fail-closed
    let none_hdr = b64url(br#"{"alg":"none","typ":"JWT"}"#);
    let claims_b64 = b64url(&serde_json::to_vec(&claims).expect("claims"));
    let unsigned_jwt = format!("{none_hdr}.{claims_b64}.");
    let (none_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/oidc/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "id_token": unsigned_jwt,
            "expected_nonce": "nonce-xyz-123"
        })),
    )
    .await;
    assert_eq!(none_status, StatusCode::UNAUTHORIZED);

    // 2. Sign a valid RSA JWT ID Token using the IdP's RSA keypair
    let rsa_hdr = b64url(br#"{"alg":"RS512","typ":"JWT","kid":"idp-rsa-2026"}"#);
    let signing_input = format!("{rsa_hdr}.{claims_b64}");
    let ssh_sig = rsa_kp
        .try_sign(signing_input.as_bytes())
        .expect("sign JWT with RSA");
    let valid_jwt = format!("{signing_input}.{}", b64url(ssh_sig.as_bytes()));

    // 3. Without an admin invite, new OIDC user is rejected (PublicSignupDisabled -> 403)
    let (no_inv_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/oidc/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "id_token": valid_jwt,
            "expected_nonce": "nonce-xyz-123"
        })),
    )
    .await;
    assert_eq!(no_inv_status, StatusCode::FORBIDDEN);

    // 4. Issue an admin invite for oidcuser@company.internal and complete OIDC login
    let admin_principal = state
        .auth_store
        .authenticate_session(&admin_token)
        .expect("admin principal");
    let issued_invite = state
        .auth_store
        .create_invite_link(
            &admin_principal,
            "oidcuser@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("issue invite");

    let (login_status, login_json) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/oidc/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "id_token": valid_jwt,
            "expected_nonce": "nonce-xyz-123",
            "invite_token": issued_invite.raw_token
        })),
    )
    .await;
    assert_eq!(
        login_status,
        StatusCode::OK,
        "OIDC login failed: {login_json}"
    );
    assert_eq!(login_json["user"]["username"], "oidcuser");
    assert_eq!(login_json["user"]["email"], "oidcuser@company.internal");
    let ses_token = login_json["raw_session_token"]
        .as_str()
        .expect("session token");

    let (me_status, me_json) =
        api_json(&app, "GET", "/api/v1/auth/me".to_owned(), ses_token, None).await;
    assert_eq!(me_status, StatusCode::OK);
    assert_eq!(me_json["username"], "oidcuser");

    // 5. Rare/adversarial OIDC edge cases:
    // (a) Caller attempting to pass untrusted inline `jwks` in request body -> 400 Bad Request (deny_unknown_fields)
    let (inline_jwks_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/oidc/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "id_token": valid_jwt,
            "jwks": { "keys": [] }
        })),
    )
    .await;
    assert_eq!(
        inline_jwks_status,
        StatusCode::BAD_REQUEST,
        "caller-supplied inline jwks must be rejected at deserialization boundary"
    );

    // (b) Multi-audience array `aud: ["other-app", "gitust-web-client"]` signed with HS256 client secret succeeds for existing user
    let hs_hdr = b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let multi_aud_claims = b64url(
        &serde_json::to_vec(&json!({
            "iss": format!("{issuer_url}/"),
            "sub": "okta-subject-001",
            "aud": ["other-app", "gitust-web-client"],
            "exp": now_secs + 300,
            "iat": now_secs,
            "nonce": "hs-nonce-1",
            "email": "oidcuser@company.internal",
            "email_verified": true
        }))
        .expect("multi aud claims"),
    );
    let hs_input = format!("{hs_hdr}.{multi_aud_claims}");
    let hs_mac =
        gitust_db::totp::hmac_sha256(b"oidc-client-secret-fallback-2026", hs_input.as_bytes());
    let hs_jwt = format!("{hs_input}.{}", b64url(&hs_mac));
    let (hs_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/oidc/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "id_token": hs_jwt,
            "expected_nonce": "hs-nonce-1"
        })),
    )
    .await;
    assert_eq!(hs_status, StatusCode::OK);

    // (c) Expired token (`exp` in past), future `iat` (`now + 300`), `email_verified: false`, wrong `aud` (401), and mismatched `nonce` triggering 429 rate-limit lockout on the 6th failed attempt!
    for (label, bad_claims, nonce_arg, expected_status) in [
        (
            "expired",
            json!({
                "iss": issuer_url, "sub": "okta-subject-001", "aud": "gitust-web-client",
                "exp": now_secs.saturating_sub(10), "iat": now_secs.saturating_sub(60),
                "nonce": "n1", "email": "oidcuser@company.internal", "email_verified": true
            }),
            "n1",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "future-iat",
            json!({
                "iss": issuer_url, "sub": "okta-subject-001", "aud": "gitust-web-client",
                "exp": now_secs + 300, "iat": now_secs + 300,
                "nonce": "n1", "email": "oidcuser@company.internal", "email_verified": true
            }),
            "n1",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "unverified-email",
            json!({
                "iss": issuer_url, "sub": "okta-subject-001", "aud": "gitust-web-client",
                "exp": now_secs + 300, "iat": now_secs,
                "nonce": "n1", "email": "oidcuser@company.internal", "email_verified": false
            }),
            "n1",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "wrong-audience",
            json!({
                "iss": issuer_url, "sub": "okta-subject-001", "aud": ["other-service"],
                "exp": now_secs + 300, "iat": now_secs,
                "nonce": "n1", "email": "oidcuser@company.internal", "email_verified": true
            }),
            "n1",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "nonce-mismatch-and-rate-limit-lockout",
            json!({
                "iss": issuer_url, "sub": "okta-subject-001", "aud": "gitust-web-client",
                "exp": now_secs + 300, "iat": now_secs,
                "nonce": "n1", "email": "oidcuser@company.internal", "email_verified": true
            }),
            "wrong-expected-nonce",
            StatusCode::TOO_MANY_REQUESTS,
        ),
    ] {
        let c_b64 = b64url(&serde_json::to_vec(&bad_claims).expect("bad claims"));
        let s_in = format!("{rsa_hdr}.{c_b64}");
        let sig = rsa_kp.try_sign(s_in.as_bytes()).expect("sign");
        let bad_jwt = format!("{s_in}.{}", b64url(sig.as_bytes()));
        let (st, _) = api_json(
            &app,
            "POST",
            "/api/v1/auth/sso/oidc/login".to_owned(),
            "",
            Some(json!({
                "provider_id": provider_id,
                "id_token": bad_jwt,
                "expected_nonce": nonce_arg
            })),
        )
        .await;
        assert_eq!(
            st, expected_status,
            "OIDC edge case '{label}' status mismatch"
        );
    }
}

fn ber_encode_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len <= 0xff {
        out.push(0x81);
        out.push(len as u8);
    } else {
        out.push(0x82);
        out.push(((len >> 8) & 0xff) as u8);
        out.push((len & 0xff) as u8);
    }
    out.extend_from_slice(content);
    out
}

fn spawn_mock_ldap_v3_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock LDAP server");
    let addr = listener.local_addr().expect("ldap local addr");
    std::thread::spawn(move || {
        for stream_res in listener.incoming() {
            let Ok(mut stream) = stream_res else { break };
            let mut buf = [0u8; 2048];
            // 1. Read LDAP v3 BindRequest
            let Ok(n) = stream.read(&mut buf) else {
                continue;
            };
            if n == 0 {
                continue;
            }
            let packet = &buf[..n];
            let expected_dn = b"uid=ldapdev,ou=users,dc=company,dc=internal";
            let expected_pw = b"CorrectLdapPass!2026";
            let bind_ok = packet.windows(expected_dn.len()).any(|w| w == expected_dn)
                && packet.windows(expected_pw.len()).any(|w| w == expected_pw);
            let result_code: u8 = if bind_ok { 0 } else { 49 };
            // LDAPMessage(1) { BindResponse(0x61) { ENUMERATED(result_code), OCTET STRING(""), OCTET STRING("") } }
            let mut bind_res_body = Vec::new();
            bind_res_body.extend_from_slice(&[0x0a, 0x01, result_code, 0x04, 0x00, 0x04, 0x00]);
            let mut msg1 = vec![0x02, 0x01, 0x01];
            msg1.extend_from_slice(&ber_encode_tlv(0x61, &bind_res_body));
            let _ = stream.write_all(&ber_encode_tlv(0x30, &msg1));
            if !bind_ok {
                continue;
            }
            // 2. Read LDAP v3 SearchRequest
            let Ok(n2) = stream.read(&mut buf) else {
                continue;
            };
            if n2 == 0 {
                continue;
            }
            // Respond with SearchResultEntry(0x64) + SearchResultDone(0x65)
            let attr_mail = ber_encode_tlv(
                0x30,
                &[
                    ber_encode_tlv(0x04, b"mail"),
                    ber_encode_tlv(0x31, &ber_encode_tlv(0x04, b"ldapdev@company.internal")),
                ]
                .concat(),
            );
            let attr_uid = ber_encode_tlv(
                0x30,
                &[
                    ber_encode_tlv(0x04, b"uid"),
                    ber_encode_tlv(0x31, &ber_encode_tlv(0x04, b"ldapdev")),
                ]
                .concat(),
            );
            let attrs_seq = ber_encode_tlv(0x30, &[attr_mail, attr_uid].concat());
            let entry_body = [ber_encode_tlv(0x04, expected_dn), attrs_seq].concat();
            let msg2_entry = [vec![0x02, 0x01, 0x02], ber_encode_tlv(0x64, &entry_body)].concat();
            let msg2_done = [
                vec![0x02, 0x01, 0x02],
                ber_encode_tlv(0x65, &[0x0a, 0x01, 0x00, 0x04, 0x00, 0x04, 0x00]),
            ]
            .concat();
            let mut combined = ber_encode_tlv(0x30, &msg2_entry);
            combined.extend_from_slice(&ber_encode_tlv(0x30, &msg2_done));
            let _ = stream.write_all(&combined);
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_tcp_ldap_v3_bind_and_search_authentication_roundtrip() {
    let (_temp, state, admin_token, _) = fixture();
    let app = build_router(state.clone());
    let ldap_addr = spawn_mock_ldap_v3_server();

    let (reg_status, prov_json) = api_json(
        &app,
        "POST",
        "/api/v1/admin/sso/providers".to_owned(),
        &admin_token,
        Some(json!({
            "name": "Corporate-OpenLDAP",
            "protocol": "ldap",
            "issuer_or_url": format!("ldap://127.0.0.1:{}", ldap_addr.port()),
            "client_id": "ou=users,dc=company,dc=internal",
            "signing_secret": "ldap-service-secret-2026",
            "allowed_domains": ["company.internal"],
            "enabled": true,
            "require_invite_binding": true
        })),
    )
    .await;
    assert_eq!(
        reg_status,
        StatusCode::CREATED,
        "register LDAP: {prov_json}"
    );
    let provider_id = prov_json["id"].as_str().expect("provider id").to_owned();

    let admin_principal = state
        .auth_store
        .authenticate_session(&admin_token)
        .expect("admin principal");
    let issued_invite = state
        .auth_store
        .create_invite_link(
            &admin_principal,
            "ldapdev@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("issue invite");

    // 1. Empty password rejected fail-closed (RFC 4513 anonymous bind prevention)
    let (empty_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/ldap/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "bind_dn": "uid=ldapdev,ou=users,dc=company,dc=internal",
            "ldap_password": "",
            "invite_token": issued_invite.raw_token
        })),
    )
    .await;
    assert_eq!(empty_status, StatusCode::BAD_REQUEST);

    // 2. Wrong LDAP password rejected fail-closed (401 Unauthorized)
    let (bad_pw_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/ldap/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "bind_dn": "uid=ldapdev,ou=users,dc=company,dc=internal",
            "ldap_password": "WrongPassword!000",
            "invite_token": issued_invite.raw_token
        })),
    )
    .await;
    assert_eq!(bad_pw_status, StatusCode::UNAUTHORIZED);

    // 3. Valid LDAP Bind + Search provisions invited user and returns active session
    let (ok_status, ok_json) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/ldap/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "bind_dn": "uid=ldapdev,ou=users,dc=company,dc=internal",
            "ldap_password": "CorrectLdapPass!2026",
            "invite_token": issued_invite.raw_token
        })),
    )
    .await;
    assert_eq!(ok_status, StatusCode::OK, "LDAP login failed: {ok_json}");
    assert_eq!(ok_json["user"]["username"], "ldapdev");
    assert_eq!(ok_json["user"]["email"], "ldapdev@company.internal");

    // 4. Rare/adversarial LDAP edge cases:
    // (a) NUL byte in bind_dn or ldap_password -> rejected 400 Bad Request
    let (nul_status, _) = api_json(
        &app,
        "POST",
        "/api/v1/auth/sso/ldap/login".to_owned(),
        "",
        Some(json!({
            "provider_id": provider_id,
            "bind_dn": "uid=ldapdev\u{0000},ou=users,dc=company,dc=internal",
            "ldap_password": "CorrectLdapPass!2026"
        })),
    )
    .await;
    assert_eq!(nul_status, StatusCode::BAD_REQUEST);

    // (b) bind_dn outside configured base DN suffix (`dc=evil,dc=com` or missing comma delimiter `ou=evil-ou=users,dc=company,dc=internal`) -> rejected 401 Unauthorized
    for bad_dn in [
        "uid=ldapdev,ou=users,dc=evil,dc=com",
        "uid=ldapdev,ou=evil-ou=users,dc=company,dc=internal",
    ] {
        let (suffix_status, _) = api_json(
            &app,
            "POST",
            "/api/v1/auth/sso/ldap/login".to_owned(),
            "",
            Some(json!({
                "provider_id": provider_id,
                "bind_dn": bad_dn,
                "ldap_password": "CorrectLdapPass!2026"
            })),
        )
        .await;
        assert_eq!(suffix_status, StatusCode::UNAUTHORIZED, "bad_dn={bad_dn}");
    }
}
