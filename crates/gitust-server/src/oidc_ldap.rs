//! Standard OpenID Connect (OIDC Core 1.0 ID Token JWT RS256/RS512 + JWKS Discovery + HS256)
//! and Real TCP LDAP v3 (RFC 4511 ASN.1 BER Simple Bind + Base-Object Attribute Search) Authentication.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    time::Duration,
};

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::Response,
};
use chrono::Utc;
use gitust_core::{AuthError, SsoProtocol};
use gitust_db::{
    compute_sso_assertion_signature, totp::hmac_sha256, LoginSessionResponse,
    SsoAssertionLoginInput,
};
use russh::keys::{
    signature::Verifier,
    ssh_key::{public::RsaPublicKey, Algorithm, HashAlg, Mpint, Signature},
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    repository_reads::response,
    state::{internal_err, map_auth_err, AppState},
    workflows::{bad_req, ApiError},
};

const MAX_OIDC_LDAP_BODY_BYTES: usize = 32 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcLoginInput {
    pub provider_id: Uuid,
    pub id_token: String,
    #[serde(default)]
    pub expected_nonce: Option<String>,
    #[serde(default)]
    pub invite_token: Option<String>,
    #[serde(default)]
    pub totp_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcJwkKey {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default, rename = "use")]
    pub use_: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcJwksDocument {
    pub keys: Vec<OidcJwkKey>,
}

#[derive(Debug, Deserialize)]
struct OidcDiscoveryDocument {
    issuer: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct JwtHeader {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JwtAudience {
    Single(String),
    Multiple(Vec<String>),
}

impl JwtAudience {
    fn contains(&self, expected: &str) -> bool {
        match self {
            Self::Single(s) => s == expected,
            Self::Multiple(v) => v.iter().any(|s| s == expected),
        }
    }
}

#[derive(Debug, Deserialize)]
struct JwtClaims {
    iss: String,
    sub: String,
    aud: JwtAudience,
    exp: u64,
    #[serde(default)]
    iat: Option<u64>,
    #[serde(default)]
    nonce: Option<String>,
    email: String,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    preferred_username: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LdapLoginInput {
    pub provider_id: Uuid,
    pub bind_dn: String,
    pub ldap_password: String,
    #[serde(default)]
    pub invite_token: Option<String>,
    #[serde(default)]
    pub totp_code: Option<String>,
}

fn decode_b64url(input: &str) -> Result<Vec<u8>, AuthError> {
    let trimmed = input.trim().trim_end_matches('=');
    let mut out = Vec::with_capacity((trimmed.len() * 3) / 4 + 1);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for b in trimmed.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return Err(AuthError::InvalidCredentials),
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

fn parse_http_host_port_and_path(raw_url: &str) -> Result<(String, u16, String), AuthError> {
    let rest = raw_url
        .trim()
        .strip_prefix("http://")
        .ok_or(AuthError::InvalidCredentials)?;
    let (host_port, path) = match rest.split_once('/') {
        Some((hp, p)) => (hp, format!("/{p}")),
        None => (rest, "/".to_owned()),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (
            h.to_owned(),
            p.parse::<u16>()
                .map_err(|_| AuthError::InvalidCredentials)?,
        ),
        None => (host_port.to_owned(), 80),
    };
    if host.is_empty()
        || host.bytes().any(|b| b <= 0x20 || b == 0x7f)
        || path.bytes().any(|b| b <= 0x20 || b == 0x7f)
    {
        return Err(AuthError::InvalidCredentials);
    }
    Ok((host, port, path))
}

fn http_get_json_sync<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, AuthError> {
    let (host, port, path) = parse_http_host_port_and_path(url)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| AuthError::InvalidCredentials)?
        .next()
        .ok_or(AuthError::InvalidCredentials)?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|_| AuthError::InvalidCredentials)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| AuthError::InvalidCredentials)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| AuthError::InvalidCredentials)?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|_| AuthError::InvalidCredentials)?;
    let mut resp = Vec::new();
    stream
        .take(64 * 1024)
        .read_to_end(&mut resp)
        .map_err(|_| AuthError::InvalidCredentials)?;
    let resp_str = std::str::from_utf8(&resp).map_err(|_| AuthError::InvalidCredentials)?;
    let (headers, body) = resp_str
        .split_once("\r\n\r\n")
        .ok_or(AuthError::InvalidCredentials)?;
    if !headers.starts_with("HTTP/1.1 200") && !headers.starts_with("HTTP/1.0 200") {
        return Err(AuthError::InvalidCredentials);
    }
    serde_json::from_str(body.trim()).map_err(|_| AuthError::InvalidCredentials)
}

fn verify_oidc_jwt_and_extract_identity(
    issuer_url: &str,
    client_id: &str,
    signing_secret: &str,
    id_token: &str,
    expected_nonce: Option<&str>,
) -> Result<(String, String), AuthError> {
    let parts: Vec<&str> = id_token.trim().split('.').collect();
    if parts.len() != 3 || parts[2].is_empty() {
        return Err(AuthError::InvalidCredentials);
    }
    let header_bytes = decode_b64url(parts[0])?;
    let payload_bytes = decode_b64url(parts[1])?;
    let sig_bytes = decode_b64url(parts[2])?;
    let header: JwtHeader =
        serde_json::from_slice(&header_bytes).map_err(|_| AuthError::InvalidCredentials)?;
    let claims: JwtClaims =
        serde_json::from_slice(&payload_bytes).map_err(|_| AuthError::InvalidCredentials)?;

    let signing_input = format!("{}.{}", parts[0], parts[1]);
    match header.alg.as_str() {
        "HS256" => {
            let expected_mac = hmac_sha256(signing_secret.as_bytes(), signing_input.as_bytes());
            if !bool::from(expected_mac.ct_eq(sig_bytes.as_slice())) {
                return Err(AuthError::InvalidCredentials);
            }
        }
        "RS256" | "RS512" => {
            let (iss_host, iss_port, _) = parse_http_host_port_and_path(issuer_url)?;
            let disc_url = format!(
                "{}/.well-known/openid-configuration",
                issuer_url.trim_end_matches('/')
            );
            let disc: OidcDiscoveryDocument = http_get_json_sync(&disc_url)?;
            if disc.issuer.trim_end_matches('/') != issuer_url.trim_end_matches('/') {
                return Err(AuthError::InvalidCredentials);
            }
            let (jwks_host, jwks_port, _) = parse_http_host_port_and_path(&disc.jwks_uri)?;
            if !iss_host.eq_ignore_ascii_case(&jwks_host) || iss_port != jwks_port {
                return Err(AuthError::InvalidCredentials);
            }
            let jwks: OidcJwksDocument = http_get_json_sync(&disc.jwks_uri)?;
            let jwk = jwks
                .keys
                .iter()
                .find(|k| {
                    k.kty == "RSA"
                        && k.alg.as_deref().is_none_or(|a| a == header.alg)
                        && k.use_.as_deref().is_none_or(|u| u == "sig")
                        && match (&header.kid, &k.kid) {
                            (Some(hk), Some(kk)) => hk == kk,
                            (None, _) => true,
                            _ => false,
                        }
                })
                .ok_or(AuthError::InvalidCredentials)?;
            let n_raw = decode_b64url(jwk.n.as_deref().ok_or(AuthError::InvalidCredentials)?)?;
            let e_raw = decode_b64url(jwk.e.as_deref().ok_or(AuthError::InvalidCredentials)?)?;
            if n_raw.is_empty() || e_raw.is_empty() {
                return Err(AuthError::InvalidCredentials);
            }
            let n_mp = Mpint::from_positive_bytes(&n_raw);
            let e_mp = Mpint::from_positive_bytes(&e_raw);
            let rsa_pub =
                RsaPublicKey::new(e_mp, n_mp).map_err(|_| AuthError::InvalidCredentials)?;
            if rsa_pub.key_size() < 2048 {
                return Err(AuthError::InvalidCredentials);
            }
            let hash_alg = if header.alg == "RS512" {
                HashAlg::Sha512
            } else {
                HashAlg::Sha256
            };
            let ssh_sig = Signature::new(
                Algorithm::Rsa {
                    hash: Some(hash_alg),
                },
                sig_bytes,
            )
            .map_err(|_| AuthError::InvalidCredentials)?;
            Verifier::verify(&rsa_pub, signing_input.as_bytes(), &ssh_sig)
                .map_err(|_| AuthError::InvalidCredentials)?;
        }
        _ => return Err(AuthError::InvalidCredentials),
    }

    // Validate standard OIDC Core 1.0 claims
    if claims.iss.trim_end_matches('/') != issuer_url.trim_end_matches('/')
        || !claims.aud.contains(client_id)
        || claims.sub.trim().is_empty()
        || claims.email_verified == Some(false)
    {
        return Err(AuthError::InvalidCredentials);
    }
    // fail-open-ok: UTC epoch timestamp is non-negative.
    let now_secs = u64::try_from(Utc::now().timestamp()).unwrap_or(0);
    if claims.exp <= now_secs || claims.exp > now_secs.saturating_add(3600) {
        return Err(AuthError::InvalidCredentials);
    }
    if let Some(iat) = claims.iat {
        if iat > now_secs.saturating_add(60) {
            return Err(AuthError::InvalidCredentials);
        }
    }
    if let Some(exp_nonce) = expected_nonce {
        let actual_nonce = claims
            .nonce
            .as_deref()
            .ok_or(AuthError::InvalidCredentials)?;
        if !bool::from(exp_nonce.as_bytes().ct_eq(actual_nonce.as_bytes())) {
            return Err(AuthError::InvalidCredentials);
        }
    }

    let email = claims.email.trim().to_ascii_lowercase();
    let username = match claims
        .preferred_username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(u) => u.to_owned(),
        None => match email.split_once('@') {
            Some((local, _)) if !local.is_empty() => local.to_owned(),
            _ => claims.sub.trim().to_owned(),
        },
    };
    Ok((email, username))
}

#[path = "ldap_v3.rs"]
mod ldap_v3;
use ldap_v3::authenticate_ldap_v3_over_tcp;

pub(crate) async fn oidc_login_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let res: Result<LoginSessionResponse, ApiError> = async move {
        let (parts, body) = request.into_parts();
        let client_scope = parts
            .extensions
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip().to_string());
        let bytes = axum::body::to_bytes(body, MAX_OIDC_LDAP_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: OidcLoginInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;

        tokio::task::spawn_blocking(move || {
            state
                .auth_store
                .check_sso_rate_limit(&input.provider_id.to_string(), client_scope.as_deref())
                .map_err(map_auth_err)?;
            let (prov, secret) = state
                .auth_store
                .get_enabled_sso_provider(input.provider_id, SsoProtocol::Oidc)
                .map_err(map_auth_err)?;
            let verified = verify_oidc_jwt_and_extract_identity(
                &prov.issuer_or_url,
                &prov.client_id,
                &secret,
                &input.id_token,
                input.expected_nonce.as_deref(),
            );
            let (email, username) = match verified {
                Ok(pair) => pair,
                Err(err) => {
                    if let Err(rate_err) = state.auth_store.record_sso_failed_attempt(
                        &input.provider_id.to_string(),
                        client_scope.as_deref(),
                    ) {
                        return Err(map_auth_err(rate_err));
                    }
                    return Err(map_auth_err(err));
                }
            };
            // fail-open-ok: UTC epoch timestamp is non-negative.
            let exp_epoch_secs = u64::try_from(Utc::now().timestamp())
                .unwrap_or(0)
                .saturating_add(120);
            let signature_hex = compute_sso_assertion_signature(
                &secret,
                input.provider_id,
                &email,
                &username,
                exp_epoch_secs,
            );
            state
                .auth_store
                .login_with_sso_assertion(
                    SsoAssertionLoginInput {
                        provider_id: input.provider_id,
                        email,
                        username,
                        exp_epoch_secs,
                        signature_hex,
                        invite_token: input.invite_token,
                        totp_code: input.totp_code,
                    },
                    client_scope.as_deref(),
                )
                .map_err(map_auth_err)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

pub(crate) async fn ldap_login_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let res: Result<LoginSessionResponse, ApiError> = async move {
        let (parts, body) = request.into_parts();
        let client_scope = parts
            .extensions
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip().to_string());
        let bytes = axum::body::to_bytes(body, MAX_OIDC_LDAP_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: LdapLoginInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        let bind_dn = input.bind_dn.trim().to_owned();
        if bind_dn.is_empty()
            || bind_dn.len() > 256
            || bind_dn.contains('\0')
            || input.ldap_password.is_empty()
            || input.ldap_password.len() > 256
            || input.ldap_password.contains('\0')
        {
            return Err(bad_req(
                "LDAP bind_dn and non-empty ldap_password are required",
            ));
        }

        tokio::task::spawn_blocking(move || {
            state
                .auth_store
                .check_sso_rate_limit(&bind_dn, client_scope.as_deref())
                .map_err(map_auth_err)?;
            let (prov, secret) = state
                .auth_store
                .get_enabled_sso_provider(input.provider_id, SsoProtocol::Ldap)
                .map_err(map_auth_err)?;
            let dn_low = bind_dn.to_ascii_lowercase();
            let base_low = prov.client_id.trim().to_ascii_lowercase();
            if dn_low != base_low && !dn_low.ends_with(&format!(",{base_low}")) {
                if let Err(rate_err) = state
                    .auth_store
                    .record_sso_failed_attempt(&bind_dn, client_scope.as_deref())
                {
                    return Err(map_auth_err(rate_err));
                }
                return Err(map_auth_err(AuthError::InvalidCredentials));
            }
            let verified =
                authenticate_ldap_v3_over_tcp(&prov.issuer_or_url, &bind_dn, &input.ldap_password);
            let (email, username) = match verified {
                Ok(pair) => pair,
                Err(err) => {
                    if let Err(rate_err) = state
                        .auth_store
                        .record_sso_failed_attempt(&bind_dn, client_scope.as_deref())
                    {
                        return Err(map_auth_err(rate_err));
                    }
                    return Err(map_auth_err(err));
                }
            };
            // fail-open-ok: UTC epoch timestamp is non-negative.
            let exp_epoch_secs = u64::try_from(Utc::now().timestamp())
                .unwrap_or(0)
                .saturating_add(120);
            let signature_hex = compute_sso_assertion_signature(
                &secret,
                input.provider_id,
                &email,
                &username,
                exp_epoch_secs,
            );
            state
                .auth_store
                .login_with_sso_assertion(
                    SsoAssertionLoginInput {
                        provider_id: input.provider_id,
                        email,
                        username,
                        exp_epoch_secs,
                        signature_hex,
                        invite_token: input.invite_token,
                        totp_code: input.totp_code,
                    },
                    client_scope.as_deref(),
                )
                .map_err(map_auth_err)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}
