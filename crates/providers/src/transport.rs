//! SSRF-safe bounded HTTP transport for provider adapters.

use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use futures_util::StreamExt;
use mcp_vault_auth::SecretString;
use reqwest::{
    Client, Method, RequestBuilder,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue},
    redirect::Policy,
};
use serde_json::Value;
use tokio::{
    net::lookup_host,
    sync::Semaphore,
    time::{Instant, sleep, timeout_at},
};
use url::Url;

use crate::{
    ProviderError, ProviderMode, ProviderSettings,
    policy::endpoint_ip_allowed,
    sse::{SseDecoder, SseEvent},
};

/// Header authentication style used by a provider adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthStyle {
    /// Authorization: Bearer `redacted-secret`.
    Bearer,
    /// x-api-key plus anthropic-version.
    Anthropic,
    /// No authentication header.
    None,
}

/// Per-request authentication and deadline overrides.
#[derive(Clone, Copy)]
pub struct RequestOptions<'a> {
    secret: Option<&'a SecretString>,
    auth_style: AuthStyle,
    timeout: Option<Duration>,
}

impl<'a> RequestOptions<'a> {
    /// Construct options using the Provider's configured total timeout.
    pub const fn new(auth_style: AuthStyle, secret: Option<&'a SecretString>) -> Self {
        Self {
            secret,
            auth_style,
            timeout: None,
        }
    }

    /// Override the Provider's configured total timeout for this operation.
    pub const fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }
}

/// One validated JSON response.
#[derive(Clone, Debug)]
pub struct JsonResponse {
    /// HTTP status code.
    pub status: u16,
    /// Parsed response JSON.
    pub body: Value,
}

/// Action returned by an SSE adapter after it has validated one data event.
///
/// The transport deliberately does not interpret provider-specific JSON. An
/// adapter may use event names and data to distinguish progress from a
/// terminal event, and must return an error for malformed or unusable data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SseEventAction {
    /// The frame was intentionally ignored (for example a provider heartbeat).
    /// Ignored frames do not refresh the first-event or idle deadline.
    Ignore,
    /// The event is valid progress and the stream should continue.
    Progress,
    /// The adapter has received and validated its terminal event.
    Terminal,
}

/// Status and bounded-consumption facts for one SSE response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SseResponse {
    /// HTTP status code (always successful for a returned response).
    pub status: u16,
    /// Number of data events delivered to the adapter.
    pub event_count: usize,
    /// Whether the adapter ended consumption with [`SseEventAction::Terminal`].
    pub terminal: bool,
}

/// Durable per-request budget checked at the existing transport boundary.
#[async_trait::async_trait]
pub trait RequestBudget: Send + Sync {
    /// Reserve one network attempt and its actual serialized body size.
    async fn reserve(&self, body_bytes: usize) -> Result<(), ProviderError>;
}

/// Bounded transport shared by provider adapters.
#[derive(Clone)]
pub struct ProviderTransport {
    settings: ProviderSettings,
    concurrency: Arc<Semaphore>,
    budget: Option<Arc<dyn RequestBudget>>,
}

impl ProviderTransport {
    /// Construct a transport with one bounded concurrency gate.
    pub fn new(settings: ProviderSettings) -> Result<Self, ProviderError> {
        settings.validate()?;
        let concurrency = Arc::new(Semaphore::new(
            usize::try_from(settings.max_concurrency)
                .map_err(|_| ProviderError::InvalidConfiguration("concurrency is invalid"))?,
        ));
        Ok(Self {
            settings,
            concurrency,
            budget: None,
        })
    }

    /// Attach operation accounting while preserving the shared concurrency gate,
    /// endpoint validation, authentication and retry policy.
    pub fn with_budget(mut self, budget: Arc<dyn RequestBudget>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Send a bounded JSON request with transient retry policy.
    pub async fn request_json(
        &self,
        method: Method,
        endpoint: &Url,
        mode: ProviderMode,
        body: &Value,
        options: RequestOptions<'_>,
    ) -> Result<JsonResponse, ProviderError> {
        let serialized = serde_json::to_vec(body)
            .map_err(|_| ProviderError::InvalidConfiguration("request JSON is invalid"))?;
        if serialized.len() > self.settings.max_request_bytes {
            return Err(ProviderError::InvalidConfiguration(
                "provider request is too large",
            ));
        }
        let mut attempt = 0_u32;
        loop {
            match self
                .request_once(method.clone(), endpoint, mode, &serialized, options)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) if error.retryable() && attempt < self.settings.max_retries => {
                    let delay = 100_u64.saturating_mul(2_u64.saturating_pow(attempt.min(6)));
                    attempt = attempt.saturating_add(1);
                    sleep(Duration::from_millis(delay)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Send exactly one bounded JSON request without applying the configured
    /// transient retry loop. This is used by evaluation protocols that must
    /// make one observable provider attempt per semantic stage.
    pub async fn request_json_once(
        &self,
        method: Method,
        endpoint: &Url,
        mode: ProviderMode,
        body: &Value,
        options: RequestOptions<'_>,
    ) -> Result<JsonResponse, ProviderError> {
        let serialized = serde_json::to_vec(body)
            .map_err(|_| ProviderError::InvalidConfiguration("request JSON is invalid"))?;
        if serialized.len() > self.settings.max_request_bytes {
            return Err(ProviderError::InvalidConfiguration(
                "provider request is too large",
            ));
        }
        self.request_once(method, endpoint, mode, &serialized, options)
            .await
    }

    /// Send one bounded incremental SSE request.
    ///
    /// A stream is never automatically replayed: one invocation reserves one
    /// budget unit and performs at most one HTTP request. The callback owns
    /// provider-specific JSON/schema validation. Returning `Progress` or
    /// `Terminal` marks an event as valid; only `Progress` refreshes the idle
    /// deadline. `Ignore` is available for heartbeats or unknown events and
    /// does not refresh any deadline. Returning an error fails closed
    /// immediately. Comments and incomplete framing bytes do not refresh any
    /// deadline.
    pub async fn request_sse<F, Fut>(
        &self,
        method: Method,
        endpoint: &Url,
        mode: ProviderMode,
        body: &Value,
        options: RequestOptions<'_>,
        mut on_event: F,
    ) -> Result<SseResponse, ProviderError>
    where
        F: FnMut(SseEvent) -> Fut,
        Fut: Future<Output = Result<SseEventAction, ProviderError>>,
    {
        let serialized = serde_json::to_vec(body)
            .map_err(|_| ProviderError::InvalidConfiguration("request JSON is invalid"))?;
        if serialized.len() > self.settings.max_request_bytes {
            return Err(ProviderError::InvalidConfiguration(
                "provider request is too large",
            ));
        }
        if mode == ProviderMode::Disabled {
            return Err(ProviderError::PrivacyDenied);
        }

        let (host, socket) = validated_socket(endpoint, mode, &self.settings).await?;
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| ProviderError::Transport {
                code: "provider_concurrency_closed",
                retryable: true,
            })?;

        // A stream uses its own total deadline. The legacy non-stream timeout
        // is intentionally not inherited. An explicit operation timeout wins.
        let total_timeout = options
            .timeout
            .unwrap_or_else(|| Duration::from_millis(self.settings.stream_total_timeout_ms));
        let client = self.build_client(&host, socket, total_timeout)?;
        let request = self.authenticated_request(
            client
                .request(method, endpoint.clone())
                .header("accept", "text/event-stream"),
            serialized.as_slice(),
            options,
        )?;
        if let Some(budget) = &self.budget {
            budget.reserve(serialized.len()).await?;
        }

        // The stream clock begins immediately before the network request. The
        // semaphore queue and local request preparation are bounded separately
        // and must not consume the provider's first-event budget.
        let request_started = Instant::now();
        let total_deadline = request_started + total_timeout;
        let first_deadline = std::cmp::min(
            total_deadline,
            request_started + Duration::from_millis(self.settings.stream_first_event_timeout_ms),
        );

        let response = match timeout_at(first_deadline, request.send()).await {
            Err(_) => {
                return Err(stream_timeout_error(if first_deadline == total_deadline {
                    "provider_stream_total_timeout"
                } else {
                    "provider_stream_first_event_timeout"
                }));
            }
            Ok(Err(error)) => {
                return Err(if error.is_timeout() {
                    stream_timeout_error(if Instant::now() >= total_deadline {
                        "provider_stream_total_timeout"
                    } else {
                        "provider_stream_first_event_timeout"
                    })
                } else {
                    ProviderError::Transport {
                        code: if error.is_connect() {
                            "provider_connect_failed"
                        } else {
                            "provider_request_failed"
                        },
                        retryable: false,
                    }
                });
            }
            Ok(Ok(response)) => response,
        };

        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(ProviderError::EndpointDenied);
        }
        if !response.status().is_success() {
            return Err(ProviderError::HttpStatus {
                status,
                retryable: false,
            });
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let media_type = content_type.split(';').next().unwrap_or_default().trim();
        if !media_type.eq_ignore_ascii_case("text/event-stream") {
            return Err(ProviderError::InvalidResponse(
                "provider content type is not SSE",
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.settings.max_response_bytes as u64)
        {
            return Err(ProviderError::ResponseTooLarge);
        }

        let max_event_bytes = self.settings.max_response_bytes;
        let mut decoder = SseDecoder::new(max_event_bytes)?;
        let mut stream = response.bytes_stream();
        let mut response_bytes = 0_usize;
        let mut event_count = 0_usize;
        let mut last_event = None;

        loop {
            let (deadline, timeout_code) = next_sse_deadline(
                last_event,
                first_deadline,
                total_deadline,
                Duration::from_millis(self.settings.stream_idle_timeout_ms),
            );
            let next = match timeout_at(deadline, stream.next()).await {
                Err(_) => return Err(stream_timeout_error(timeout_code)),
                Ok(next) => next,
            };
            let Some(chunk) = next else {
                let events = decoder.finish()?;
                for event in events {
                    event_count = event_count.saturating_add(1);
                    match invoke_sse_callback(
                        &mut on_event,
                        event,
                        total_deadline,
                        "provider_stream_total_timeout",
                    )
                    .await?
                    {
                        SseEventAction::Ignore => {}
                        SseEventAction::Progress => {}
                        SseEventAction::Terminal => {
                            return Ok(SseResponse {
                                status,
                                event_count,
                                terminal: true,
                            });
                        }
                    }
                }
                return Ok(SseResponse {
                    status,
                    event_count,
                    terminal: false,
                });
            };
            let chunk = chunk.map_err(stream_response_read_error)?;
            response_bytes = response_bytes.saturating_add(chunk.len());
            if response_bytes > self.settings.max_response_bytes {
                return Err(ProviderError::ResponseTooLarge);
            }
            for event in decoder.push(&chunk)? {
                event_count = event_count.saturating_add(1);
                let (event_deadline, event_timeout_code) = next_sse_deadline(
                    last_event,
                    first_deadline,
                    total_deadline,
                    Duration::from_millis(self.settings.stream_idle_timeout_ms),
                );
                match invoke_sse_callback(&mut on_event, event, event_deadline, event_timeout_code)
                    .await?
                {
                    SseEventAction::Ignore => {}
                    SseEventAction::Progress => last_event = Some(Instant::now()),
                    SseEventAction::Terminal => {
                        return Ok(SseResponse {
                            status,
                            event_count,
                            terminal: true,
                        });
                    }
                }
            }
        }
    }

    async fn request_once(
        &self,
        method: Method,
        endpoint: &Url,
        mode: ProviderMode,
        body: &[u8],
        options: RequestOptions<'_>,
    ) -> Result<JsonResponse, ProviderError> {
        if mode == ProviderMode::Disabled {
            return Err(ProviderError::PrivacyDenied);
        }
        let (host, socket) = validated_socket(endpoint, mode, &self.settings).await?;
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| ProviderError::Transport {
                code: "provider_concurrency_closed",
                retryable: true,
            })?;
        let client = self.build_client(&host, socket, self.settings.timeout())?;
        let mut request =
            self.authenticated_request(client.request(method, endpoint.clone()), body, options)?;
        if let Some(timeout) = options.timeout {
            request = request.timeout(timeout);
        }
        if let Some(budget) = &self.budget {
            budget.reserve(body.len()).await?;
        }
        let response = request
            .send()
            .await
            .map_err(|error| ProviderError::Transport {
                code: if error.is_timeout() {
                    "provider_timeout"
                } else if error.is_connect() {
                    "provider_connect_failed"
                } else {
                    "provider_request_failed"
                },
                retryable: error.is_timeout() || error.is_connect(),
            })?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(ProviderError::EndpointDenied);
        }
        if !response.status().is_success() {
            return Err(ProviderError::HttpStatus {
                status,
                retryable: status == 408 || status == 429 || status >= 500,
            });
        }
        if let Some(content_type) = response.headers().get("content-type") {
            let content_type = content_type.to_str().unwrap_or_default();
            if !content_type.starts_with("application/json") && !content_type.contains("+json") {
                return Err(ProviderError::InvalidResponse(
                    "provider content type is not JSON",
                ));
            }
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.settings.max_response_bytes as u64)
        {
            return Err(ProviderError::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(response_read_error)?;
            if bytes.len().saturating_add(chunk.len()) > self.settings.max_response_bytes {
                return Err(ProviderError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::InvalidResponse("response is not JSON"))?;
        Ok(JsonResponse { status, body })
    }

    fn build_client(
        &self,
        host: &str,
        socket: SocketAddr,
        timeout: Duration,
    ) -> Result<Client, ProviderError> {
        Client::builder()
            .redirect(Policy::none())
            .connect_timeout(self.settings.connect_timeout())
            .timeout(timeout)
            .resolve(host, socket)
            .build()
            .map_err(|_| ProviderError::Transport {
                code: "provider_client_build_failed",
                retryable: false,
            })
    }

    fn authenticated_request(
        &self,
        mut request: RequestBuilder,
        body: &[u8],
        options: RequestOptions<'_>,
    ) -> Result<RequestBuilder, ProviderError> {
        request = request
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_owned());
        for (name, value) in &self.settings.headers {
            if name.eq_ignore_ascii_case("authorization")
                || name.eq_ignore_ascii_case("x-api-key")
                || name.eq_ignore_ascii_case("host")
                || name.eq_ignore_ascii_case("content-length")
            {
                return Err(ProviderError::InvalidConfiguration(
                    "provider settings contain a protected header",
                ));
            }
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                ProviderError::InvalidConfiguration("provider header name is invalid")
            })?;
            let value = HeaderValue::from_str(value).map_err(|_| {
                ProviderError::InvalidConfiguration("provider header value is invalid")
            })?;
            request = request.header(name, value);
        }
        if options.auth_style == AuthStyle::Anthropic {
            request = request.header("anthropic-version", "2023-06-01");
        }
        if let Some(secret) = options.secret {
            match options.auth_style {
                AuthStyle::Bearer => {
                    let value = format!("Bearer {}", secret.expose_secret());
                    request = request.header(AUTHORIZATION, value);
                }
                AuthStyle::Anthropic => {
                    request = request.header("x-api-key", secret.expose_secret());
                }
                AuthStyle::None => {}
            }
        }
        Ok(request)
    }
}

fn stream_timeout_error(code: &'static str) -> ProviderError {
    ProviderError::Transport {
        code,
        retryable: false,
    }
}

fn next_sse_deadline(
    last_event: Option<Instant>,
    first_deadline: Instant,
    total_deadline: Instant,
    idle_timeout: Duration,
) -> (Instant, &'static str) {
    match last_event {
        None => (
            first_deadline,
            if first_deadline == total_deadline {
                "provider_stream_total_timeout"
            } else {
                "provider_stream_first_event_timeout"
            },
        ),
        Some(last) => {
            let idle_deadline = last + idle_timeout;
            if idle_deadline <= total_deadline {
                (idle_deadline, "provider_stream_idle_timeout")
            } else {
                (total_deadline, "provider_stream_total_timeout")
            }
        }
    }
}

async fn invoke_sse_callback<F, Fut>(
    on_event: &mut F,
    event: SseEvent,
    deadline: Instant,
    timeout_code: &'static str,
) -> Result<SseEventAction, ProviderError>
where
    F: FnMut(SseEvent) -> Fut,
    Fut: Future<Output = Result<SseEventAction, ProviderError>>,
{
    timeout_at(deadline, on_event(event))
        .await
        .map_err(|_| stream_timeout_error(timeout_code))?
}

fn response_read_error(error: reqwest::Error) -> ProviderError {
    let code = if error.is_timeout() {
        "provider_response_timeout"
    } else {
        // This mapper is called only after an HTTP success while consuming
        // the response byte stream. Reqwest does not consistently expose
        // nested Hyper/body source errors through `is_body()`, so every
        // non-timeout stream error is an interrupted/incomplete response.
        "provider_response_incomplete"
    };
    // A successful status was already received. The remote model may have
    // completed billable work, so automatically replaying the request is not
    // safe even when the body failure itself looks transient.
    ProviderError::Transport {
        code,
        retryable: false,
    }
}

fn stream_response_read_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        stream_timeout_error("provider_stream_total_timeout")
    } else {
        response_read_error(error)
    }
}

/// Append a provider-relative API path without allowing a caller to replace
/// the configured host.
pub fn endpoint_url(base: &Url, suffix: &str) -> Result<Url, ProviderError> {
    if suffix.is_empty()
        || suffix.starts_with('/')
        || suffix.contains("..")
        || suffix.chars().any(char::is_control)
    {
        return Err(ProviderError::InvalidConfiguration(
            "provider endpoint suffix is invalid",
        ));
    }
    let mut url = base.clone();
    let path = base.path().trim_end_matches('/');
    let path = if path.is_empty() {
        format!("/v1/{suffix}")
    } else {
        format!("{path}/{suffix}")
    };
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

async fn validated_socket(
    endpoint: &Url,
    mode: ProviderMode,
    settings: &ProviderSettings,
) -> Result<(String, SocketAddr), ProviderError> {
    if endpoint.username() != ""
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ProviderError::EndpointDenied);
    }
    let scheme = endpoint.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(ProviderError::EndpointDenied);
    }
    let host = endpoint
        .host_str()
        .ok_or(ProviderError::EndpointDenied)?
        .to_owned();
    let port = endpoint
        .port_or_known_default()
        .ok_or(ProviderError::EndpointDenied)?;
    if scheme == "http" && mode == ProviderMode::RemoteAllowed {
        return Err(ProviderError::EndpointDenied);
    }
    let addresses = lookup_host((host.as_str(), port))
        .await
        .map_err(|_| ProviderError::Transport {
            code: "provider_dns_failed",
            retryable: true,
        })?
        .collect::<Vec<_>>();
    if addresses.is_empty()
        || addresses.iter().any(|address| {
            !endpoint_ip_allowed(address.ip(), mode, settings.allow_private_networks)
        })
    {
        return Err(ProviderError::EndpointDenied);
    }
    let socket = addresses[0];
    if socket.ip().is_unspecified() {
        return Err(ProviderError::EndpointDenied);
    }
    Ok((host, socket))
}

/// Return whether an HTTP status should be retried.
pub const fn retryable_status(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

/// Expose the endpoint validation seam to tests and Admin diagnostics without
/// exposing DNS or local absolute paths in errors.
pub async fn validate_endpoint(
    endpoint: &Url,
    mode: ProviderMode,
    settings: &ProviderSettings,
) -> Result<IpAddr, ProviderError> {
    Ok(validated_socket(endpoint, mode, settings).await?.1.ip())
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TwoAttempts(AtomicUsize);
    #[async_trait::async_trait]
    impl RequestBudget for TwoAttempts {
        async fn reserve(&self, bytes: usize) -> Result<(), ProviderError> {
            assert_eq!(bytes, 7);
            if self.0.fetch_add(1, Ordering::SeqCst) >= 2 {
                return Err(ProviderError::Transport {
                    code: "test_budget_exhausted",
                    retryable: false,
                });
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn actual_transport_retries_each_reserve_budget_before_dispatch() {
        let sent = Arc::new(AtomicUsize::new(0));
        let count = sent.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/embeddings",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/embeddings",
                    axum::routing::post(move || {
                        let count = count.clone();
                        async move {
                            count.fetch_add(1, Ordering::SeqCst);
                            axum::http::StatusCode::TOO_MANY_REQUESTS
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let budget = Arc::new(TwoAttempts(AtomicUsize::new(0)));
        let transport = ProviderTransport::new(ProviderSettings {
            max_retries: 5,
            ..Default::default()
        })
        .unwrap()
        .with_budget(budget.clone());
        let error = transport
            .request_json(
                Method::POST,
                &endpoint,
                ProviderMode::LocalOnly,
                &serde_json::json!({"x":1}),
                RequestOptions::new(AuthStyle::None, None),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "test_budget_exhausted");
        assert_eq!(sent.load(Ordering::SeqCst), 2);
        assert_eq!(budget.0.load(Ordering::SeqCst), 3);
        server.abort();
    }

    #[tokio::test]
    async fn request_json_once_never_retries_even_when_transport_allows_retries() {
        let sent = Arc::new(AtomicUsize::new(0));
        let count = sent.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/chat/completions",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/chat/completions",
                    axum::routing::post(move || {
                        let count = count.clone();
                        async move {
                            count.fetch_add(1, Ordering::SeqCst);
                            axum::http::StatusCode::TOO_MANY_REQUESTS
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let budget = Arc::new(TwoAttempts(AtomicUsize::new(0)));
        let transport = ProviderTransport::new(ProviderSettings {
            max_retries: 5,
            ..Default::default()
        })
        .unwrap()
        .with_budget(budget.clone());
        let error = transport
            .request_json_once(
                Method::POST,
                &endpoint,
                ProviderMode::LocalOnly,
                &serde_json::json!({"x":1}),
                RequestOptions::new(AuthStyle::None, None),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "provider_rate_limited");
        assert_eq!(sent.load(Ordering::SeqCst), 1);
        assert_eq!(budget.0.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[cfg(test)]
mod sse_tests {
    use super::*;
    use axum::{
        Router,
        body::{Body, Bytes},
        http::header,
        response::Response,
        routing::post,
    };
    use futures_util::stream;
    use std::{convert::Infallible, net::SocketAddr, sync::Mutex};
    use tokio::time::sleep;

    async fn server(chunks: Vec<(Duration, Bytes)>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/stream",
            post(move || {
                let chunks = chunks.clone();
                async move {
                    let body = Body::from_stream(stream::unfold(
                        (chunks, 0_usize),
                        |(chunks, index)| async move {
                            let (delay, bytes) = chunks.get(index)?.clone();
                            sleep(delay).await;
                            Some((Result::<Bytes, Infallible>::Ok(bytes), (chunks, index + 1)))
                        },
                    ));
                    Response::builder()
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(body)
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (address, task)
    }

    fn settings() -> ProviderSettings {
        ProviderSettings {
            timeout_ms: 20,
            connect_timeout_ms: 5,
            stream_first_event_timeout_ms: 100,
            stream_idle_timeout_ms: 15,
            stream_total_timeout_ms: 200,
            max_retries: 8,
            ..ProviderSettings::default()
        }
    }

    #[tokio::test]
    async fn stream_uses_stream_deadline_and_flushes_eof_event() {
        let (address, task) = server(vec![
            (
                Duration::from_millis(40),
                Bytes::from_static(b"data: {\"n\":1}\n\n"),
            ),
            (
                Duration::from_millis(1),
                Bytes::from_static(b"data: {\"n\":2}"),
            ),
        ])
        .await;
        let transport = ProviderTransport::new(settings()).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_callback = seen.clone();
        let response = transport
            .request_sse(
                Method::POST,
                &Url::parse(&format!("http://{address}/stream")).unwrap(),
                ProviderMode::LocalOnly,
                &serde_json::json!({"stream": true}),
                RequestOptions::new(AuthStyle::None, None),
                move |event| {
                    seen_by_callback.lock().unwrap().push(event.data);
                    async { Ok(SseEventAction::Progress) }
                },
            )
            .await
            .unwrap();
        assert_eq!(response.event_count, 2);
        assert!(!response.terminal);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [r#"{"n":1}"#, r#"{"n":2}"#]
        );
        task.abort();
    }

    #[tokio::test]
    async fn stream_idle_and_total_deadlines_are_distinct() {
        let (address, task) = server(vec![
            (
                Duration::from_millis(10),
                Bytes::from_static(b"data: {}\n\n"),
            ),
            (
                Duration::from_millis(50),
                Bytes::from_static(b"data: {}\n\n"),
            ),
        ])
        .await;
        let transport = ProviderTransport::new(ProviderSettings {
            stream_idle_timeout_ms: 10,
            stream_total_timeout_ms: 100,
            ..settings()
        })
        .unwrap();
        let error = transport
            .request_sse(
                Method::POST,
                &Url::parse(&format!("http://{address}/stream")).unwrap(),
                ProviderMode::LocalOnly,
                &serde_json::json!({}),
                RequestOptions::new(AuthStyle::None, None),
                |_| async { Ok(SseEventAction::Progress) },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "provider_stream_idle_timeout");
        task.abort();

        let (address, task) = server(vec![
            (
                Duration::from_millis(1),
                Bytes::from_static(b"data: {}\n\n"),
            ),
            (
                Duration::from_millis(50),
                Bytes::from_static(b"data: {}\n\n"),
            ),
        ])
        .await;
        let transport = ProviderTransport::new(ProviderSettings {
            stream_first_event_timeout_ms: 20,
            stream_idle_timeout_ms: 19,
            stream_total_timeout_ms: 20,
            ..settings()
        })
        .unwrap();
        let error = transport
            .request_sse(
                Method::POST,
                &Url::parse(&format!("http://{address}/stream")).unwrap(),
                ProviderMode::LocalOnly,
                &serde_json::json!({}),
                RequestOptions::new(AuthStyle::None, None),
                |_| async {
                    sleep(Duration::from_millis(5)).await;
                    Ok(SseEventAction::Progress)
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "provider_stream_total_timeout");
        task.abort();
    }

    #[tokio::test]
    async fn callback_processing_is_bounded_by_first_event_deadline() {
        let (address, task) = server(vec![(
            Duration::from_millis(1),
            Bytes::from_static(b"data: {}\n\n"),
        )])
        .await;
        let transport = ProviderTransport::new(ProviderSettings {
            stream_first_event_timeout_ms: 10,
            stream_idle_timeout_ms: 100,
            stream_total_timeout_ms: 100,
            ..settings()
        })
        .unwrap();
        let error = transport
            .request_sse(
                Method::POST,
                &Url::parse(&format!("http://{address}/stream")).unwrap(),
                ProviderMode::LocalOnly,
                &serde_json::json!({}),
                RequestOptions::new(AuthStyle::None, None),
                |_| async {
                    sleep(Duration::from_millis(50)).await;
                    Ok(SseEventAction::Progress)
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "provider_stream_first_event_timeout");
        task.abort();
    }

    #[tokio::test]
    async fn callback_error_and_terminal_never_replay_the_request() {
        let (address, task) = server(vec![
            (
                Duration::from_millis(1),
                Bytes::from_static(b"data: {}\n\n"),
            ),
            (
                Duration::from_millis(20),
                Bytes::from_static(b"data: {}\n\n"),
            ),
        ])
        .await;
        let transport = ProviderTransport::new(settings()).unwrap();
        let response = transport
            .request_sse(
                Method::POST,
                &Url::parse(&format!("http://{address}/stream")).unwrap(),
                ProviderMode::LocalOnly,
                &serde_json::json!({}),
                RequestOptions::new(AuthStyle::None, None),
                |_| async { Ok(SseEventAction::Terminal) },
            )
            .await
            .unwrap();
        assert!(response.terminal);
        let error = transport
            .request_sse(
                Method::POST,
                &Url::parse(&format!("http://{address}/stream")).unwrap(),
                ProviderMode::LocalOnly,
                &serde_json::json!({}),
                RequestOptions::new(AuthStyle::None, None),
                |_| async {
                    Err(ProviderError::InvalidResponse(
                        "test callback rejected event",
                    ))
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "provider_response_invalid");
        task.abort();
    }
}
