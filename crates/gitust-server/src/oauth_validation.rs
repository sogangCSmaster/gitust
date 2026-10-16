use super::{OAuthTokenRequest, MAX_OAUTH_FORM_BYTES};
use axum::http::{header, HeaderMap};

pub(super) fn is_valid_redirect_uri(uri: &str) -> bool {
    let trimmed = uri.trim();
    if trimmed.is_empty()
        || trimmed.len() > 512
        || trimmed.contains('#')
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return false;
    }
    // fail-open-ok: invalid redirect URI is rejected, never accepted through a fallback.
    let Ok(parsed) = trimmed.parse::<axum::http::Uri>() else {
        return false;
    };
    let (Some(scheme), Some(authority)) = (parsed.scheme_str(), parsed.authority()) else {
        return false;
    };
    if !scheme.as_bytes()[0].is_ascii_alphabetic() || authority.as_str().contains('@') {
        return false;
    }
    let host = authority.host();
    if host.is_empty() || authority.as_str().ends_with(':') {
        return false;
    }
    if authority.as_str().rsplit_once(':').is_some()
        && !authority.as_str().ends_with(']')
        && !matches!(authority.port_u16(), Some(1..=u16::MAX))
    {
        return false;
    }
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "http" => matches!(
            host.to_ascii_lowercase().as_str(),
            "127.0.0.1" | "localhost" | "[::1]"
        ),
        "https" => {
            host.parse::<std::net::IpAddr>().is_ok()
                || (host.starts_with('[')
                    && host.ends_with(']')
                    && host[1..host.len() - 1]
                        .parse::<std::net::Ipv6Addr>()
                        .is_ok())
                || host.split('.').all(|label| {
                    !label.is_empty()
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
        }
        "javascript" | "data" | "file" | "vbscript" | "blob" | "ws" | "wss" | "ftp" => false,
        _ => true,
    }
}

pub(super) fn is_valid_pkce_token(s: &str) -> bool {
    (43..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                if i + 2 >= bytes.len() {
                    return None;
                }
                let hi = (bytes[i + 1] as char).to_digit(16)?;
                let lo = (bytes[i + 2] as char).to_digit(16)?;
                out.push(((hi << 4) | lo) as u8);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok() // fail-open-ok: invalid UTF-8 returns `None` and fails closed via `.ok_or(...)` in `parse_token_request`.
}

pub(super) fn parse_token_request(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<OAuthTokenRequest, &'static str> {
    if body.is_empty() || body.len() > MAX_OAUTH_FORM_BYTES {
        return Err("request body must be between 1 and 16384 bytes");
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok()) // fail-open-ok: non-ASCII Content-Type falls through to strict JSON parser below, which rejects invalid payloads.
        .unwrap_or("");
    if content_type.starts_with("application/x-www-form-urlencoded") {
        let text = std::str::from_utf8(body).map_err(|_| "invalid UTF-8 form body")?;
        let mut req = OAuthTokenRequest::default();
        for pair in text.split('&') {
            if pair.is_empty() {
                continue;
            }
            // fail-open-ok: missing form value becomes empty and is rejected for required fields.
            let (k_raw, v_raw) = pair.split_once('=').unwrap_or((pair, ""));
            let k = percent_decode(k_raw).ok_or("invalid percent-encoding in form key")?;
            let v = percent_decode(v_raw).ok_or("invalid percent-encoding in form value")?;
            match k.as_str() {
                "grant_type" => req.grant_type = v,
                "code" => req.code = v,
                "client_id" => req.client_id = v,
                "redirect_uri" => req.redirect_uri = v,
                "code_verifier" => req.code_verifier = v,
                _ => {}
            }
        }
        Ok(req)
    } else {
        serde_json::from_slice(body).map_err(|_| "invalid token request payload")
    }
}
