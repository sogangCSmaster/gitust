//! Zero-dependency, single-binary embedded Web UI & Invite/Reset SPA handlers for `gitust serve`.
//!
//! Embeds the compiled `@gitust/design-system` production bundle directly into the `gitust`
//! binary so `gitust serve` works out-of-the-box on any air-gapped on-premise server without
//! requiring Node.js or external static file directories at runtime.

use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

const INDEX_HTML: &str = include_str!("../../../packages/design-system/dist/index.html");
struct EmbeddedAsset {
    name: &'static str,
    content_type: &'static str,
    bytes: &'static [u8],
}
include!(concat!(env!("OUT_DIR"), "/webui_assets.rs"));

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

/// Only build-time embedded assets are addressable; request paths never reach the filesystem.
pub(crate) async fn web_asset_handler(Path(path): Path<String>) -> Response {
    match EMBEDDED_ASSETS.binary_search_by(|asset| asset.name.cmp(path.as_str())) {
        Ok(index) => {
            let asset = &EMBEDDED_ASSETS[index];
            (
                StatusCode::OK,
                base_security_headers(asset.content_type, "no-cache"),
                asset.bytes,
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            base_security_headers("text/plain; charset=utf-8", "no-store"),
            "Asset not found",
        )
            .into_response(),
    }
}
