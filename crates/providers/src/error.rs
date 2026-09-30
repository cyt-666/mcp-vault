//! Redacted provider and vector errors.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Safe classification of a strict function-call stream failure. Values never
/// include tool names, argument fragments, message content, or request data.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrictFunctionCallIssue {
    NoToolCall,
    WrongFinishReason,
    MissingToolCallId,
    InvalidChoice,
    MixedMessageContent,
    WrongToolName,
    MissingArguments,
    MultipleToolCalls,
    InconsistentToolCallId,
    InvalidToolDelta,
    InvalidStreamEvent,
    InvalidFinishSequence,
}

/// Safe classification of the JSON parser failure. It never contains parser
/// text or a fragment of the generated response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredJsonParserCategory {
    Syntax,
    Eof,
    Data,
    Io,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredJsonParseIssue {
    TrailingComma,
    ExpectedValue,
    ExpectedKey,
    ExpectedColon,
    ExpectedCommaOrEnd,
    TrailingCharacters,
    InvalidEscape,
    InvalidUnicode,
    ControlCharacter,
    InvalidNumber,
    NumberOutOfRange,
    UnexpectedEof,
    GenericSyntax,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredJsonFinishReason {
    Stop,
    Length,
    ContentFilter,
    RepetitionTruncation,
    ToolCalls,
    Unknown,
}

/// Redacted metadata for a structured JSON parse failure.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StructuredJsonDiagnostic {
    pub parser_category: StructuredJsonParserCategory,
    pub issue: StructuredJsonParseIssue,
    pub line: usize,
    pub column: usize,
    pub content_bytes: usize,
    pub parsed_bytes: usize,
    pub fence_detected: bool,
    pub finish_reason: Option<StructuredJsonFinishReason>,
}

/// Errors at the provider/application boundary.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// Provider configuration is not valid.
    #[error("invalid provider configuration: {0}")]
    InvalidConfiguration(&'static str),
    /// A requested provider/model/binding was not found.
    #[error("provider resource was not found")]
    NotFound,
    /// The selected model is not registered.
    #[error("the selected model was not found")]
    ModelNotFound,
    /// The selected model is disabled.
    #[error("the selected model is disabled")]
    ModelDisabled,
    /// The selected model does not declare the role's required capability.
    #[error("the selected model lacks the required {capability} capability")]
    ModelCapabilityMismatch { capability: &'static str },
    /// Provider was disabled by configuration.
    #[error("provider is disabled")]
    Disabled,
    /// Vault privacy policy rejected the request.
    #[error("provider request is disabled by Vault privacy policy")]
    PrivacyDenied,
    /// The operation-scoped transport request budget has been exhausted.
    #[error("provider transport request budget is exhausted")]
    RequestBudgetExhausted,
    /// The live evaluation's frozen provider identity no longer matches state.
    #[error("provider runtime configuration drifted")]
    RuntimeConfigurationDrift,
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
    /// A strict single-function response failed a bounded protocol check.
    #[error("strict function-call response shape is invalid")]
    StrictFunctionCallInvalid { issue: StrictFunctionCallIssue },
    /// Structured JSON could not be parsed; only safe parser metadata is kept.
    #[error("provider structured JSON could not be parsed")]
    StructuredJsonInvalid {
        diagnostic: StructuredJsonDiagnostic,
    },
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
            Self::ModelNotFound => "model_not_found",
            Self::ModelDisabled => "model_disabled",
            Self::ModelCapabilityMismatch { .. } => "model_capability_mismatch",
            Self::Disabled => "provider_disabled",
            Self::PrivacyDenied => "provider_privacy_denied",
            Self::RequestBudgetExhausted => "provider_request_budget_exhausted",
            Self::RuntimeConfigurationDrift => "provider_runtime_configuration_drift",
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
            Self::StructuredJsonInvalid { .. } => "provider_structured_json_invalid",
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
            Self::StrictFunctionCallInvalid { .. } => "provider_response_invalid",
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

    /// Return safe structured JSON parser metadata without exposing content.
    pub fn structured_json_diagnostic(&self) -> Option<&StructuredJsonDiagnostic> {
        match self {
            Self::StructuredJsonInvalid { diagnostic } => Some(diagnostic),
            _ => None,
        }
    }

    /// Safe protocol classification for a strict function-call failure.
    pub const fn strict_function_call_issue(&self) -> Option<StrictFunctionCallIssue> {
        match self {
            Self::StrictFunctionCallInvalid { issue } => Some(*issue),
            _ => None,
        }
    }

    /// Whether a structured-generation failure is isolated to one generated
    /// output and may be skipped by a bounded batch without retrying the same
    /// potentially billable request.
    pub fn is_generation_output_failure(&self) -> bool {
        match self {
            Self::SchemaValidation { .. }
            | Self::StructuredJsonInvalid { .. }
            | Self::ResponseTooLarge
            | Self::StrictFunctionCallInvalid { .. } => true,
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
