use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
async fn every_built_module_is_served_from_the_embedded_binary() {
    let (app, _) = crate::tests::test_app();
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/design-system/dist/assets");
    let mut chunks = 0;
    for entry in std::fs::read_dir(assets).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("js") {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        chunks += usize::from(name != "index.js");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/assets/{name}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "module {name}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            body.as_ref(),
            std::fs::read(&path).unwrap(),
            "module {name}"
        );
    }
    assert!(chunks >= 3, "production build must defer optional features");
}

#[tokio::test]
async fn unknown_assets_never_fall_back_to_html_or_host_files() {
    let (app, _) = crate::tests::test_app();
    for path in [
        "missing.js",
        "../Cargo.toml",
        "%2e%2e%2fCargo.toml",
        ".env",
        "index.js/extra",
        "%2findex.js",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/assets/{path}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert!(!body.starts_with(b"<!DOCTYPE"));
    }
}
