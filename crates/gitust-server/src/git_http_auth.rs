//! Existing Smart HTTP Basic/Bearer credential policy shared by primary writes and hosted reads.
use crate::state::{internal_err, map_auth_err};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use gitust_core::AuthenticatedPrincipal;
use gitust_db::AuthStore;

fn decode_standard_base64(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity((trimmed.len() / 4) * 3);
    let bytes = trimmed.as_bytes();
    for chunk in bytes.chunks_exact(4) {
        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        for (i, &b) in chunk.iter().enumerate() {
            vals[i] = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if i >= 2 => {
                    pad += 1;
                    0
                }
                _ => return None,
            };
        }
        if pad == 1 && chunk[3] != b'=' {
            return None;
        }
        out.push((vals[0] << 2) | (vals[1] >> 4));
        if pad < 2 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if pad == 0 {
            out.push((vals[2] << 6) | vals[3]);
        }
    }
    Some(out)
}

fn extract_git_credentials(
    headers: &HeaderMap,
) -> Result<(Option<String>, String), (StatusCode, String)> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Missing Authorization header (Basic or Bearer PAT required)".to_owned(),
            )
        })?
        .to_str()
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Authorization header is not valid ASCII".to_owned(),
            )
        })?
        .trim();

    if let Some(bearer) = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
    {
        let tok = bearer.trim();
        if tok.is_empty() {
            return Err((StatusCode::UNAUTHORIZED, "Empty Bearer token".to_owned()));
        }
        return Ok((None, tok.to_owned()));
    }

    if let Some(b64) = raw
        .strip_prefix("Basic ")
        .or_else(|| raw.strip_prefix("basic "))
    {
        let decoded = decode_standard_base64(b64).ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Malformed Basic authorization base64".to_owned(),
            )
        })?;
        let cred_str = String::from_utf8(decoded).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Basic authorization is not valid UTF-8".to_owned(),
            )
        })?;
        let (user_part, pass_part) = match cred_str.split_once(':') {
            Some((u, p)) => (u.trim(), p.trim()),
            None => ("", cred_str.trim()),
        };
        let token = if !pass_part.is_empty() {
            pass_part
        } else if !user_part.is_empty() {
            user_part
        } else {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Empty Basic authorization credentials".to_owned(),
            ));
        };
        let explicit_user = if !pass_part.is_empty()
            && !user_part.is_empty()
            && !matches!(
                user_part.to_ascii_lowercase().as_str(),
                "x-access-token" | "git" | "token" | "oauth2" | "pat"
            ) {
            Some(user_part.to_owned())
        } else {
            None
        };
        return Ok((explicit_user, token.to_owned()));
    }

    Err((
        StatusCode::UNAUTHORIZED,
        "Unsupported Authorization scheme (expected Basic or Bearer)".to_owned(),
    ))
}

pub(crate) fn extract_git_http_principal(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, (StatusCode, String)> {
    let (explicit_user, token) = extract_git_credentials(headers)?;
    let principal = if token.starts_with("gitust_pat_") {
        auth_store.authenticate_pat(&token)
    } else {
        auth_store.authenticate_session(&token)
    }
    .map_err(map_auth_err)?;
    if explicit_user
        .as_ref()
        .is_some_and(|expected| !principal.username.eq_ignore_ascii_case(expected))
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Basic username does not match authenticated token owner".into(),
        ));
    }
    Ok(principal)
}
pub(crate) async fn extract_git_http_principal_async(
    headers: &HeaderMap,
    auth_store: &AuthStore,
) -> Result<AuthenticatedPrincipal, Response> {
    let headers = headers.clone();
    let store = auth_store.clone();
    tokio::task::spawn_blocking(move || extract_git_http_principal(&headers, &store))
        .await
        .map_err(|e| internal_err(e).into_response())?
        .map_err(|(status, message)| git_auth_err_response(status, message))
}

pub(crate) fn git_auth_err_response(status: StatusCode, msg: String) -> Response {
    if status == StatusCode::UNAUTHORIZED {
        (
            status,
            [(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"Gitust On-Premise Git\"",
            )],
            msg,
        )
            .into_response()
    } else {
        (status, msg).into_response()
    }
}
