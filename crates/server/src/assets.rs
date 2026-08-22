//! Static Admin asset serving kept outside Vault content.

use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::Response,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../frontend/admin/dist/"]
struct AdminAssets;

const FALLBACK_INDEX: &str = r#"<!doctype html>
<html lang="en">
  <head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>MCP Vault Admin</title></head>
  <body><div id="root">Admin frontend assets have not been built yet.</div></body>
</html>
"#;

/// Serve a compiled Admin asset or the safe shell fallback.
pub async fn serve(uri: Uri) -> Response {
    if uri.path().starts_with("/dav/")
        || uri.path().starts_with("/mcp/")
        || uri.path().starts_with("/health/")
    {
        return not_found();
    }

    let requested = uri.path().trim_start_matches('/');
    let path = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(asset) = safe_asset(path) {
        return asset_response(path, asset.data.into_owned());
    }

    if let Some(index) = safe_asset("index.html") {
        return asset_response("index.html", index.data.into_owned());
    }

    asset_response("index.html", FALLBACK_INDEX.as_bytes().to_vec())
}

fn not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("control-plane route not found\n"))
        .expect("static response headers are valid")
}

fn safe_asset(path: &str) -> Option<rust_embed::EmbeddedFile> {
    if path.is_empty()
        || path.contains('\0')
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }

    AdminAssets::get(path)
}

fn asset_response(path: &str, bytes: Vec<u8>) -> Response {
    let content_type = mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream");

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .expect("static response headers are valid")
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::serve;

    #[tokio::test]
    async fn root_serves_a_non_vault_admin_asset() {
        let response = Router::new()
            .fallback(serve)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let expected_title = "MCP Vault 管理端".as_bytes();
        assert!(
            body.windows(expected_title.len())
                .any(|window| window == expected_title)
        );
    }

    #[tokio::test]
    async fn traversal_does_not_escape_embedded_assets() {
        let response = Router::new()
            .fallback(serve)
            .oneshot(
                Request::builder()
                    .uri("/../AGENTS.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            !body
                .windows("# AGENTS.md".len())
                .any(|window| window == b"# AGENTS.md")
        );
    }
}
