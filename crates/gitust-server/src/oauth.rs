//! OAuth 2.0 Authorization Server & Protected Resource Metadata (RFC 8414 / RFC 9728),
//! Dynamic Client Registration (RFC 7591), and Authorization Code + PKCE (`S256`, RFC 7636)
//! endpoints for remote HTTP MCP clients (Claude Code, Cursor, etc.).

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use gitust_core::AuthenticatedPrincipal;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::state::{
    internal_err, map_auth_err, AppState, PendingOAuthCode, RegisteredOAuthClient,
    OAUTH_CODE_TTL_SECS,
};

#[path = "oauth_validation.rs"]
mod validation;
use validation::{is_valid_pkce_token, is_valid_redirect_uri, parse_token_request};

const MAX_OAUTH_FORM_BYTES: usize = 16 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthProtectedResourceMetadata {
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub bearer_methods_supported: Vec<String>,
    pub scopes_supported: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthAuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub response_types_supported: Vec<String>,
    pub grant_types_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
    pub code_challenge_methods_supported: Vec<String>,
    pub scopes_supported: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthClientRegistrationRequest {
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthClientRegistrationResponse {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthAuthorizeRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthAuthorizeResponse {
    pub code: String,
    pub redirect_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub expires_in: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OAuthTokenRequest {
    pub grant_type: String,
    pub code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_verifier: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
}

fn oauth_error_response(status: StatusCode, error: &'static str, description: &str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": error,
            "error_description": description,
        })),
    )
        .into_response()
}

fn resolve_base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok()) // fail-open-ok: invalid Host header falls back to strict loopback `127.0.0.1:3000`.
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 255
                && s.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']')
                })
        })
        // fail-open-ok: invalid Host is replaced with loopback metadata, never a caller-supplied destination.
        .unwrap_or("127.0.0.1:3000");
    format!("http://{host}")
}

/// Encodes a 32-byte SHA-256 digest as RFC 4648 base64url without padding (43 ASCII chars).
pub fn base64url_encode_sha256(digest: &[u8; 32]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(43);
    let mut i = 0;
    while i + 3 <= 32 {
        let b0 = digest[i] as usize;
        let b1 = digest[i + 1] as usize;
        let b2 = digest[i + 2] as usize;
        out.push(ALPHABET[b0 >> 2] as char);
        out.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        out.push(ALPHABET[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        out.push(ALPHABET[b2 & 0x3f] as char);
        i += 3;
    }
    // Remaining 2 bytes (30, 31) of the 32-byte SHA-256 digest -> 3 base64url chars (no '=' pad)
    let b0 = digest[30] as usize;
    let b1 = digest[31] as usize;
    out.push(ALPHABET[b0 >> 2] as char);
    out.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
    out.push(ALPHABET[(b1 & 0x0f) << 2] as char);
    out
}

/// Computes `BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))` per RFC 7636 §4.6.
#[must_use]
pub fn pkce_s256_challenge(code_verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    base64url_encode_sha256(&digest)
}

fn hash_oauth_code_sha256(raw_code: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(raw_code.as_bytes());
    hasher.finalize().into()
}

fn generate_oauth_code() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("gitust_oac_{}", hex::encode(bytes))
}

/// `GET /.well-known/oauth-protected-resource` (RFC 9728)
pub(crate) async fn oauth_protected_resource_metadata_handler(
    headers: HeaderMap,
) -> Json<OAuthProtectedResourceMetadata> {
    let base = resolve_base_url(&headers);
    Json(OAuthProtectedResourceMetadata {
        resource: format!("{base}/mcp"),
        authorization_servers: vec![base],
        bearer_methods_supported: vec!["header".to_owned()],
        scopes_supported: vec!["mcp".to_owned()],
    })
}

/// `GET /.well-known/oauth-authorization-server` (RFC 8414)
pub(crate) async fn oauth_authorization_server_metadata_handler(
    headers: HeaderMap,
) -> Json<OAuthAuthorizationServerMetadata> {
    let base = resolve_base_url(&headers);
    Json(OAuthAuthorizationServerMetadata {
        issuer: base.clone(),
        authorization_endpoint: format!("{base}/oauth/authorize"),
        token_endpoint: format!("{base}/oauth/token"),
        registration_endpoint: format!("{base}/oauth/register"),
        response_types_supported: vec!["code".to_owned()],
        grant_types_supported: vec!["authorization_code".to_owned()],
        token_endpoint_auth_methods_supported: vec!["none".to_owned()],
        code_challenge_methods_supported: vec!["S256".to_owned()],
        scopes_supported: vec!["mcp".to_owned()],
    })
}

/// `POST /oauth/register` (RFC 7591 Dynamic Client Registration for MCP clients)
pub(crate) async fn oauth_register_client_handler(
    State(state): State<AppState>,
    Json(req): Json<OAuthClientRegistrationRequest>,
) -> Response {
    if req.redirect_uris.is_empty() || req.redirect_uris.len() > 8 {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect_uris must contain between 1 and 8 URIs",
        );
    }
    let mut clean_uris = Vec::with_capacity(req.redirect_uris.len());
    for uri in &req.redirect_uris {
        if !is_valid_redirect_uri(uri) {
            return oauth_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_redirect_uri",
                "redirect_uri must be a loopback HTTP URI, HTTPS URI, or valid native app scheme",
            );
        }
        clean_uris.push(uri.trim().to_owned());
    }
    let client_name = req
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        // fail-open-ok: optional client display name is presentation only; redirect and capacity checks still run.
        .unwrap_or("MCP OAuth Client");
    if client_name.len() > 256 {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "client_name exceeds 256 bytes",
        );
    }

    let client_id = format!("gitust_client_{}", Uuid::new_v4().simple());
    let record = RegisteredOAuthClient {
        client_id: client_id.clone(),
        client_name: client_name.to_owned(),
        redirect_uris: clean_uris.clone(),
        created_at: Utc::now(),
    };
    if !state.oauth_mcp.lock().insert_client(record) {
        return oauth_error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "temporarily_unavailable",
            "client registration capacity reached",
        );
    }

    (
        StatusCode::CREATED,
        Json(OAuthClientRegistrationResponse {
            client_id,
            client_name: client_name.to_owned(),
            redirect_uris: clean_uris,
            token_endpoint_auth_method: "none".to_owned(),
            grant_types: vec!["authorization_code".to_owned()],
            response_types: vec!["code".to_owned()],
        }),
    )
        .into_response()
}

/// `POST /oauth/authorize` — Authenticated endpoint that issues a single-use PKCE `S256`
/// authorization code for an active user.
pub(crate) async fn oauth_authorize_handler(
    State(state): State<AppState>,
    _caller: AuthenticatedPrincipal,
    headers: HeaderMap,
    Json(req): Json<OAuthAuthorizeRequest>,
) -> Response {
    let caller = match crate::auth::extract_principal_async(&headers, &state.auth_store).await {
        Ok(c) => c,
        Err(err) => return err.into_response(),
    };
    // fail-open-ok: an invalid bearer header is rejected immediately below; it never authorizes an exchange.
    let Some(credential) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
    else {
        return oauth_error_response(
            StatusCode::UNAUTHORIZED,
            "invalid_request",
            "missing bearer credential",
        );
    };
    let authorizing_credential = gitust_db::hash_token_sha256(credential);
    if req.scope.as_deref().is_some_and(|scope| scope != "mcp") {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_scope",
            "only scope=mcp is supported",
        );
    }
    if req.response_type.trim() != "code" {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "unsupported_response_type",
            "only response_type=code is supported",
        );
    }
    if req.code_challenge_method.trim() != "S256" {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "PKCE with code_challenge_method=S256 is required",
        );
    }
    let challenge = req.code_challenge.trim();
    if !is_valid_pkce_token(challenge) {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "code_challenge must be a 43..=128 character base64url S256 string",
        );
    }
    if let Some(ref st) = req.state {
        if st.len() > 512 {
            return oauth_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "state parameter exceeds 512 bytes",
            );
        }
    }

    let client_id = req.client_id.trim();
    let redirect_uri = req.redirect_uri.trim();
    if client_id.is_empty() || client_id.len() > 128 || !is_valid_redirect_uri(redirect_uri) {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid client_id or redirect_uri",
        );
    }

    let raw_code = generate_oauth_code();
    let code_sha256 = hash_oauth_code_sha256(&raw_code);
    let expires_at = Utc::now() + Duration::seconds(OAUTH_CODE_TTL_SECS);

    {
        let mut oauth = state.oauth_mcp.lock();
        let Some(client) = oauth.get_client(client_id) else {
            return oauth_error_response(
                StatusCode::BAD_REQUEST,
                "unauthorized_client",
                "unknown client_id; register via POST /oauth/register first",
            );
        };
        if !client.redirect_uris.iter().any(|u| u == redirect_uri) {
            return oauth_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "redirect_uri does not match registered client redirect_uris",
            );
        }
        oauth.insert_code(PendingOAuthCode {
            code_sha256,
            user_id: caller.user_id,
            authorizing_credential,
            client_id: client_id.to_owned(),
            redirect_uri: redirect_uri.to_owned(),
            code_challenge_s256: challenge.to_owned(),
            expires_at,
        });
    }

    (
        StatusCode::OK,
        Json(OAuthAuthorizeResponse {
            code: raw_code,
            redirect_uri: redirect_uri.to_owned(),
            state: req.state,
            expires_in: OAUTH_CODE_TTL_SECS,
        }),
    )
        .into_response()
}

/// `POST /oauth/token` — Exchanges a single-use authorization code + PKCE `code_verifier`
/// for a Bearer session token backed directly by `AuthStore`.
pub(crate) async fn oauth_token_handler(
    State(state): State<AppState>,
    http_req: Request,
) -> Response {
    let headers = http_req.headers().clone();
    // fail-open-ok: oversized token body immediately fails closed with 400 `invalid_request`.
    let Ok(body) = axum::body::to_bytes(http_req.into_body(), MAX_OAUTH_FORM_BYTES).await else {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request body exceeds 16384 bytes",
        );
    };
    let req = match parse_token_request(&headers, &body) {
        Ok(r) => r,
        Err(msg) => return oauth_error_response(StatusCode::BAD_REQUEST, "invalid_request", msg),
    };

    let code = req.code.trim();
    let client_id = req.client_id.trim();
    let redirect_uri = req.redirect_uri.trim();
    let code_verifier = req.code_verifier.trim();

    if code.is_empty() || code.len() > 256 {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid authorization code",
        );
    }
    // Atomically consume the single-use authorization code before any `.await`.
    let code_hash = hash_oauth_code_sha256(code);
    let Some(pending) = state.oauth_mcp.lock().take_code(&code_hash) else {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "authorization code is invalid or already used",
        );
    };

    if req.grant_type.trim() != "authorization_code" {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "only grant_type=authorization_code is supported",
        );
    }
    if code.is_empty()
        || code.len() > 256
        || client_id.is_empty()
        || client_id.len() > 128
        || redirect_uri.is_empty()
        || redirect_uri.len() > 512
        || !is_valid_pkce_token(code_verifier)
    {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid or missing code, client_id, redirect_uri, or PKCE code_verifier",
        );
    }

    if Utc::now() >= pending.expires_at {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "authorization code has expired",
        );
    }
    if pending.client_id != client_id || pending.redirect_uri != redirect_uri {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "client_id or redirect_uri mismatch",
        );
    }

    let computed_challenge = pkce_s256_challenge(code_verifier);
    if computed_challenge
        .as_bytes()
        .ct_eq(pending.code_challenge_s256.as_bytes())
        .unwrap_u8()
        != 1
    {
        return oauth_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "PKCE S256 code_verifier verification failed",
        );
    }

    let store = state.auth_store.clone();
    let user_id = pending.user_id;
    let credential = pending.authorizing_credential;
    let session = match tokio::task::spawn_blocking(move || {
        store.issue_oauth_session_from_authorization(user_id, credential)
    })
    .await
    .map_err(internal_err)
    {
        Ok(Ok(s)) => s,
        Ok(Err(auth_err)) => {
            let (status, msg) = map_auth_err(auth_err);
            return oauth_error_response(status, "invalid_grant", &msg);
        }
        Err((status, msg)) => return oauth_error_response(status, "server_error", &msg),
    };

    let expires_in = (session.expires_at - Utc::now()).num_seconds().max(1);
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(OAuthTokenResponse {
            access_token: session.raw_session_token,
            token_type: "Bearer".to_owned(),
            expires_in,
            scope: "mcp".to_owned(),
        }),
    )
        .into_response()
}
