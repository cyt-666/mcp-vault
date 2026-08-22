//! Listener-specific router composition.

use axum::{Extension, Router, middleware, routing::get};

use crate::{assets, health::Readiness};

/// Compose the public data plane.
pub fn data_router(readiness: Readiness) -> Router {
    Router::new()
        .merge(crate::health::router(readiness))
        .nest("/dav/v1/vaults", mcp_vault_webdav::unconfigured_router())
        .nest("/mcp/v1/vaults", mcp_vault_mcp::router())
}

/// Compose the stateful public data plane used by the running server.
pub fn data_router_with_webdav(
    readiness: Readiness,
    webdav: mcp_vault_webdav::WebDavService,
) -> Router {
    Router::new()
        .merge(crate::health::router(readiness))
        .nest("/dav/v1/vaults", mcp_vault_webdav::router(webdav))
        .nest("/mcp/v1/vaults", mcp_vault_mcp::router())
}

/// Compose the stateful data plane with both protocol adapters.
pub fn data_router_with_webdav_and_mcp(
    readiness: Readiness,
    webdav: mcp_vault_webdav::WebDavService,
    mcp: mcp_vault_mcp::McpService,
) -> Router {
    data_router_with_webdav_and_mcp_and_metrics(
        readiness,
        webdav,
        mcp,
        crate::metrics::Metrics::new(false),
    )
}

/// Compose the stateful data plane with an injected bounded metrics registry.
pub fn data_router_with_webdav_and_mcp_and_metrics(
    readiness: Readiness,
    webdav: mcp_vault_webdav::WebDavService,
    mcp: mcp_vault_mcp::McpService,
    metrics: crate::metrics::Metrics,
) -> Router {
    Router::new()
        .merge(crate::health::router(readiness))
        .route("/metrics", get(crate::metrics::endpoint))
        .nest("/dav/v1/vaults", mcp_vault_webdav::router(webdav))
        .nest("/mcp/v1/vaults", mcp_vault_mcp::stateful_router(mcp))
        .layer(middleware::from_fn(crate::metrics::observe_data))
        .layer(Extension(metrics))
}

/// Compose the private control plane.
pub fn control_router() -> Router {
    Router::new()
        .nest("/api/v1", mcp_vault_admin_api::router())
        .fallback(assets::serve)
}

/// Compose the stateful private control plane.
pub fn control_router_with_admin(admin: mcp_vault_admin_api::AdminApiState) -> Router {
    Router::new()
        .nest("/api/v1", mcp_vault_admin_api::stateful_router(admin))
        .fallback(assets::serve)
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::{control_router, data_router};
    use crate::health::Readiness;

    #[tokio::test]
    async fn data_plane_does_not_expose_control_routes() {
        let response = data_router(Readiness::new())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn control_plane_does_not_expose_data_routes() {
        let response = control_router()
            .oneshot(
                Request::builder()
                    .uri("/mcp/v1/vaults/default")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn data_protocol_mounts_are_explicit_boundaries() {
        let router = data_router(Readiness::new());

        for path in [
            "/dav/v1/vaults/default/notes/today.md",
            "/mcp/v1/vaults/default",
        ] {
            let response = router
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), axum::http::StatusCode::NOT_IMPLEMENTED);
        }
    }
}
