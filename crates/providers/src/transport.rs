//! SSRF-safe bounded HTTP transport for provider adapters.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use futures_util::StreamExt;
use mcp_vault_auth::SecretString;
use reqwest::{
    Client, Method,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue},
    redirect::Policy,
};
use serde_json::Value;
use tokio::{net::lookup_host, sync::Semaphore, time::sleep};
use url::Url;

use crate::{ProviderError, ProviderMode, ProviderSettings, policy::endpoint_ip_allowed};

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

/// Bounded transport shared by provider adapters.
#[derive(Clone)]
pub struct ProviderTransport {
    settings: ProviderSettings,
    concurrency: Arc<Semaphore>,
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
        })
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
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(self.settings.connect_timeout())
            .timeout(self.settings.timeout())
            .resolve(&host, socket)
            .build()
            .map_err(|_| ProviderError::Transport {
                code: "provider_client_build_failed",
                retryable: false,
            })?;
        let mut request = client
            .request(method, endpoint.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_owned());
        if let Some(timeout) = options.timeout {
            request = request.timeout(timeout);
        }
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
