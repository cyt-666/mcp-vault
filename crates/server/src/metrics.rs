//! Small bounded Prometheus-style process metrics surface.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use axum::{
    extract::Extension,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Plane label for request counters without exposing raw paths or IDs.
#[derive(Clone, Copy, Debug)]
pub enum MetricsPlane {
    /// Public MCP/WebDAV/health listener.
    Data,
    /// Private Admin listener.
    Control,
}

impl MetricsPlane {
    const fn label(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Control => "control",
        }
    }
}

#[derive(Default)]
struct MetricsInner {
    data_requests: AtomicU64,
    data_errors: AtomicU64,
    control_requests: AtomicU64,
    control_errors: AtomicU64,
    request_duration_ms: AtomicU64,
    backup_operations_completed: AtomicU64,
    backup_operations_failed: AtomicU64,
}

/// Cloneable bounded metrics registry. Labels are fixed and no request path,
/// credential, Vault content, or user input is stored.
#[derive(Clone, Default)]
pub struct Metrics {
    enabled: bool,
    inner: Arc<MetricsInner>,
}

impl Metrics {
    /// Construct a registry with opt-in exposition.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            inner: Arc::new(MetricsInner::default()),
        }
    }

    /// Whether `/metrics` should be exposed.
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Record a bounded request outcome.
    pub fn observe_request(&self, plane: MetricsPlane, status: StatusCode, elapsed_ms: u64) {
        let (requests, errors) = match plane {
            MetricsPlane::Data => (&self.inner.data_requests, &self.inner.data_errors),
            MetricsPlane::Control => (&self.inner.control_requests, &self.inner.control_errors),
        };
        requests.fetch_add(1, Ordering::Relaxed);
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            errors.fetch_add(1, Ordering::Relaxed);
        }
        self.inner
            .request_duration_ms
            .fetch_add(elapsed_ms, Ordering::Relaxed);
    }

    /// Record backup worker operation outcomes without dynamic labels.
    pub fn observe_backup(&self, success: bool) {
        let counter = if success {
            &self.inner.backup_operations_completed
        } else {
            &self.inner.backup_operations_failed
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Render a stable text exposition with no dynamic labels.
    pub fn render(&self) -> String {
        let data_requests = self.inner.data_requests.load(Ordering::Relaxed);
        let control_requests = self.inner.control_requests.load(Ordering::Relaxed);
        let data_errors = self.inner.data_errors.load(Ordering::Relaxed);
        let control_errors = self.inner.control_errors.load(Ordering::Relaxed);
        let duration = self.inner.request_duration_ms.load(Ordering::Relaxed);
        format!(
            "# TYPE mcp_vault_http_requests_total counter\n\
mcp_vault_http_requests_total{{plane=\"{}\"}} {data_requests}\n\
mcp_vault_http_requests_total{{plane=\"{}\"}} {control_requests}\n\
# TYPE mcp_vault_http_errors_total counter\n\
mcp_vault_http_errors_total{{plane=\"{}\"}} {data_errors}\n\
mcp_vault_http_errors_total{{plane=\"{}\"}} {control_errors}\n\
# TYPE mcp_vault_http_duration_ms_total counter\n\
mcp_vault_http_duration_ms_total {duration}\n\
# TYPE mcp_vault_backup_operations_completed_total counter\n\
mcp_vault_backup_operations_completed_total {}\n\
# TYPE mcp_vault_backup_operations_failed_total counter\n\
mcp_vault_backup_operations_failed_total {}\n",
            MetricsPlane::Data.label(),
            MetricsPlane::Control.label(),
            MetricsPlane::Data.label(),
            MetricsPlane::Control.label(),
            self.inner
                .backup_operations_completed
                .load(Ordering::Relaxed),
            self.inner.backup_operations_failed.load(Ordering::Relaxed),
        )
    }
}

/// Serve the opt-in non-sensitive exposition endpoint.
pub async fn endpoint(Extension(metrics): Extension<Metrics>) -> Response {
    if !metrics.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut response = (StatusCode::OK, metrics.render()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
}

/// Observe data-plane requests without logging paths or bodies.
pub async fn observe_data(
    Extension(metrics): Extension<Metrics>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    observe(MetricsPlane::Data, metrics, request, next).await
}

/// Observe control-plane requests without logging paths or bodies.
pub async fn observe_control(
    Extension(metrics): Extension<Metrics>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    observe(MetricsPlane::Control, metrics, request, next).await
}

async fn observe(
    plane: MetricsPlane,
    metrics: Metrics,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let response = next.run(request).await;
    metrics.observe_request(
        plane,
        response.status(),
        started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    );
    response
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::{Metrics, MetricsPlane};

    #[test]
    fn exposition_has_fixed_non_sensitive_labels() {
        let metrics = Metrics::new(true);
        metrics.observe_request(MetricsPlane::Data, StatusCode::OK, 7);
        metrics.observe_request(MetricsPlane::Control, StatusCode::INTERNAL_SERVER_ERROR, 11);
        let output = metrics.render();
        assert!(output.contains("plane=\"data\""));
        assert!(output.contains("plane=\"control\""));
        assert!(output.contains("mcp_vault_http_duration_ms_total 18"));
        assert!(!output.contains("/mcp/v1"));
    }
}
