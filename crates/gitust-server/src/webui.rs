//! Zero-dependency, single-binary embedded Web UI & Invite/Reset SPA handlers for `gitust serve`.
//!
//! Embeds the compiled `@gitust/design-system` production bundle directly into the `gitust`
//! binary so `gitust serve` works out-of-the-box on any air-gapped on-premise server without
//! requiring Node.js or external static file directories at runtime.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;

const INDEX_HTML: &str = include_str!("../../../packages/design-system/dist/index.html");
const INDEX_JS: &str = include_str!("../../../packages/design-system/dist/assets/index.js");
const INDEX_CSS: &str = include_str!("../../../packages/design-system/dist/assets/index.css");

const STRICT_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; font-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

fn base_security_headers(content_type: &'static str, cache_control: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(STRICT_CSP),
    );
    headers
}

/// Serves the embedded Superhuman-grade Web UI (`GET /`, `GET /auth/accept-invite`, `GET /auth/reset-password`).
pub(crate) async fn web_index_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        base_security_headers("text/html; charset=utf-8", "no-store"),
        INDEX_HTML,
    )
}

/// Serves the embedded SolidJS + Keyboard Engine bundle (`GET /assets/index.js`).
pub(crate) async fn web_js_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        base_security_headers("application/javascript; charset=utf-8", "no-cache"),
        INDEX_JS,
    )
}

/// Serves the embedded `--ds-*` design tokens stylesheet (`GET /assets/index.css`).
pub(crate) async fn web_css_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        base_security_headers("text/css; charset=utf-8", "no-cache"),
        INDEX_CSS,
    )
}
