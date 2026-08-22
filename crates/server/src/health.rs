//! Public, non-sensitive process health state.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::Serialize;

/// In-memory readiness state shared by the listener composition root.
#[derive(Clone, Debug, Default)]
pub struct Readiness {
    ready: Arc<AtomicBool>,
}

impl Readiness {
    /// Create a state that is not ready until startup completes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark all critical WP-00 startup steps complete.
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    /// Mark readiness false before graceful shutdown or maintenance.
    pub fn mark_not_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }

    /// Return the current readiness bit.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Share the read-only readiness bit with the control-plane diagnostics
    /// adapter without coupling that crate to server health types.
    pub fn shared_flag(&self) -> Arc<AtomicBool> {
        self.ready.clone()
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// Build the public data-plane health routes.
pub fn router(readiness: Readiness) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(readiness)
}

async fn live() -> impl IntoResponse {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(readiness): State<Readiness>) -> impl IntoResponse {
    if readiness.is_ready() {
        (StatusCode::OK, Json(HealthResponse { status: "ready" }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse { status: "starting" }),
        )
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{Readiness, router};

    #[tokio::test]
    async fn liveness_is_available_before_readiness() {
        let response = router(Readiness::new())
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], br#"{"status":"ok"}"#);
    }

    #[tokio::test]
    async fn readiness_changes_only_after_startup_marks_it_ready() {
        let readiness = Readiness::new();
        let router = router(readiness.clone());

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );

        readiness.mark_ready();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
