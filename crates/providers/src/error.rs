//! Redacted provider and vector errors.

use thiserror::Error;

/// Errors at the provider/application boundary.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// Provider configuration is not valid.
    #[error("invalid provider configuration: {0}")]
    InvalidConfiguration(&'static str),
    /// A requested provider/model/binding was not found.
    #[error("provider resource was not found")]
    NotFound,
    /// Provider was disabled by configuration.
    #[error("provider is disabled")]
    Disabled,
    /// Vault privacy policy rejected the request.
    #[error("provider request is disabled by Vault privacy policy")]
    PrivacyDenied,
    /// SSRF or endpoint policy rejected the target.
    #[error("provider endpoint is not permitted")]
    EndpointDenied,
    /// A request could not be sent safely.
    #[error("provider transport failed")]
    Transport {
        /// Stable redacted transport category.
        code: &'static str,
        /// Whether the caller may retry.
        retryable: bool,
    },
    /// The provider returned an HTTP status that was not accepted.
    #[error("provider returned HTTP status {status}")]
    HttpStatus {
        /// Status code only; response bodies are never retained.
        status: u16,
        /// Whether the status is transient.
        retryable: bool,
    },
    /// The provider response exceeded the configured bound.
    #[error("provider response exceeded the configured limit")]
    ResponseTooLarge,
    /// A provider response was not the expected JSON contract.
    #[error("provider response shape is invalid: {0}")]
    InvalidResponse(&'static str),
    /// Structured output failed the caller-supplied schema subset.
    ///
    /// `issue` is a stable project-owned category and `path` is assembled only
    /// from trusted schema property names plus array indexes. Neither field
    /// contains response values or arbitrary Provider output.
    #[error("provider structured output failed schema validation: {issue} at {path}")]
    SchemaValidation {
        /// Stable redacted mismatch category.
        issue: &'static str,
        /// JSON path within the caller-supplied schema.
        path: String,
    },
    /// The provider returned an unsupported embedding dimension.
    #[error("embedding dimension does not match the selected model")]
    DimensionMismatch,
    /// The selected model/provider capability is unavailable.
    #[error("provider capability is unavailable")]
    CapabilityUnavailable,
    /// A retryable provider job could not complete.
    #[error("provider job is temporarily unavailable")]
    TemporarilyUnavailable,
    /// Operational state failed.
    #[error("provider state is unavailable")]
    State(#[from] mcp_vault_state::StateError),
    /// Encrypted provider secret failed at the auth boundary.
    #[error("provider secret is unavailable")]
    Auth(#[from] mcp_vault_auth::AuthError),
    /// A URL could not be parsed.
    #[error("provider URL is invalid")]
    Url(#[from] url::ParseError),
}

impl ProviderError {
    /// Return whether this error is safe to retry without configuration
    /// changes.
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Transport { retryable, .. } | Self::HttpStatus { retryable, .. } => *retryable,
            Self::TemporarilyUnavailable => true,
            _ => false,
        }
    }

    /// Stable redacted diagnostic code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration(_) => "provider_config_invalid",
            Self::NotFound => "provider_not_found",
            Self::Disabled => "provider_disabled",
            Self::PrivacyDenied => "provider_privacy_denied",
            Self::EndpointDenied => "provider_endpoint_denied",
            Self::Transport { code, .. } => code,
            Self::HttpStatus { status, .. } => match status {
                401 | 403 => "provider_auth_failed",
                408 => "provider_timeout",
                429 => "provider_rate_limited",
                500..=599 => "provider_server_error",
                _ => "provider_http_error",
            },
            Self::ResponseTooLarge => "provider_response_too_large",
            Self::InvalidResponse("provider content type is not JSON") => {
                "provider_response_content_type_invalid"
            }
            Self::InvalidResponse("response is not JSON") => "provider_response_json_invalid",
            Self::InvalidResponse("provider final content is missing") => {
                "provider_final_content_missing"
            }
            Self::InvalidResponse("structured output is not JSON") => {
                "provider_structured_json_invalid"
            }
            Self::InvalidResponse("provider output reached token limit") => {
                "provider_output_truncated"
            }
            Self::InvalidResponse("provider output was filtered") => "provider_output_filtered",
            Self::InvalidResponse("provider output repetition was truncated") => {
                "provider_output_repetition_truncated"
            }
            Self::InvalidResponse("provider response was incomplete") => {
                "provider_response_incomplete"
            }
            Self::InvalidResponse(_) => "provider_response_invalid",
            Self::SchemaValidation { .. } => "provider_schema_invalid",
            Self::DimensionMismatch => "embedding_dimension_mismatch",
            Self::CapabilityUnavailable => "provider_capability_unavailable",
            Self::TemporarilyUnavailable => "provider_temporarily_unavailable",
            Self::State(_) => "provider_state_error",
            Self::Auth(_) => "provider_secret_unavailable",
            Self::Url(_) => "provider_url_invalid",
        }
    }

    /// Return a redacted schema mismatch category and trusted schema path.
    pub fn schema_diagnostic(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::SchemaValidation { issue, path } => Some((*issue, path)),
            _ => None,
        }
    }

    /// Whether a structured-generation failure is isolated to one generated
    /// output and may be skipped by a bounded batch without retrying the same
    /// potentially billable request.
    pub fn is_generation_output_failure(&self) -> bool {
        match self {
            Self::SchemaValidation { .. } | Self::ResponseTooLarge => true,
            Self::InvalidResponse(reason) => matches!(
                *reason,
                "provider final content is missing"
                    | "structured output is not JSON"
                    | "provider output reached token limit"
                    | "provider output was filtered"
                    | "provider output repetition was truncated"
            ),
            _ => false,
        }
    }
}
