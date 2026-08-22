//! Typed provider configuration and privacy/transport policy.

use std::{collections::BTreeMap, net::IpAddr, time::Duration};

use serde::{Deserialize, Serialize};

use crate::ProviderError;

/// Supported provider adapter families.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI Responses API structured generation.
    OpenAiResponses,
    /// OpenAI-compatible chat/Responses/embeddings endpoint.
    OpenAiCompatible,
    /// Anthropic Messages API structured generation.
    AnthropicMessages,
    /// OpenAI-compatible embedding-only endpoint.
    EmbeddingHttp,
    /// Optional local FastEmbed runtime.
    FastEmbedLocal,
}

impl ProviderKind {
    /// Stable storage label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai_responses",
            Self::OpenAiCompatible => "openai_compatible",
            Self::AnthropicMessages => "anthropic_messages",
            Self::EmbeddingHttp => "embedding_http",
            Self::FastEmbedLocal => "fastembed_local",
        }
    }
}

impl TryFrom<&str> for ProviderKind {
    type Error = ProviderError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "openai_responses" => Ok(Self::OpenAiResponses),
            "openai_compatible" => Ok(Self::OpenAiCompatible),
            "anthropic_messages" => Ok(Self::AnthropicMessages),
            "embedding_http" => Ok(Self::EmbeddingHttp),
            "fastembed_local" => Ok(Self::FastEmbedLocal),
            _ => Err(ProviderError::InvalidConfiguration(
                "provider type is unsupported",
            )),
        }
    }
}

/// Per-Vault provider privacy mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    /// Do not send content to any provider.
    #[default]
    Disabled,
    /// Permit only loopback/private local endpoints.
    LocalOnly,
    /// Permit public HTTPS providers and explicit safe policy exceptions.
    RemoteAllowed,
}

/// Typed provider transport settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ProviderSettings {
    /// Overall request timeout.
    pub timeout_ms: u64,
    /// TCP/TLS connect timeout.
    pub connect_timeout_ms: u64,
    /// Maximum transient retry count.
    pub max_retries: u32,
    /// Maximum concurrent requests for this provider.
    pub max_concurrency: u32,
    /// Maximum serialized request body.
    pub max_request_bytes: usize,
    /// Maximum response body.
    pub max_response_bytes: usize,
    /// Permit explicitly configured private HTTPS endpoints in remote mode.
    pub allow_private_networks: bool,
    /// Additional non-secret provider headers.
    pub headers: BTreeMap<String, String>,
    /// Optional organization/project identifiers.
    pub organization: Option<String>,
    /// Optional local model cache directory.
    pub model_cache_dir: Option<String>,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            connect_timeout_ms: 5_000,
            max_retries: 2,
            max_concurrency: 4,
            max_request_bytes: 2 * 1024 * 1024,
            max_response_bytes: 4 * 1024 * 1024,
            allow_private_networks: false,
            headers: BTreeMap::new(),
            organization: None,
            model_cache_dir: None,
        }
    }
}

impl ProviderSettings {
    /// Decode and validate settings stored as JSON.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, ProviderError> {
        let settings: Self = serde_json::from_value(value.clone())
            .map_err(|_| ProviderError::InvalidConfiguration("provider settings are invalid"))?;
        settings.validate()?;
        Ok(settings)
    }

    /// Validate resource and header bounds.
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.timeout_ms == 0
            || self.timeout_ms > 10 * 60 * 1000
            || self.connect_timeout_ms == 0
            || self.connect_timeout_ms > self.timeout_ms
            || self.max_retries > 8
            || self.max_concurrency == 0
            || self.max_concurrency > 64
            || self.max_request_bytes == 0
            || self.max_request_bytes > 16 * 1024 * 1024
            || self.max_response_bytes == 0
            || self.max_response_bytes > 32 * 1024 * 1024
        {
            return Err(ProviderError::InvalidConfiguration(
                "provider resource limits are invalid",
            ));
        }
        if self.headers.len() > 32
            || self
                .headers
                .iter()
                .any(|(key, value)| key.is_empty() || key.len() > 128 || value.len() > 1024)
        {
            return Err(ProviderError::InvalidConfiguration(
                "provider headers are invalid",
            ));
        }
        Ok(())
    }

    /// Overall request timeout.
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    /// TCP/TLS connection timeout.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }
}

/// Model capabilities discovered or manually assigned by the owner.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ModelCapabilities {
    /// Structured JSON output is supported.
    pub structured_output: bool,
    /// Text embedding is supported.
    pub embeddings: bool,
    /// Reranking is supported.
    pub reranking: bool,
    /// Fixed embedding dimension, when known.
    pub dimension: Option<u32>,
    /// Context window estimate.
    pub context_window: Option<u32>,
    /// Output token limit estimate.
    pub max_output_tokens: Option<u32>,
}

impl ModelCapabilities {
    /// Decode capabilities stored as JSON.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, ProviderError> {
        serde_json::from_value(value.clone())
            .map_err(|_| ProviderError::InvalidConfiguration("model capabilities are invalid"))
    }
}

/// Validate an IP address against local/provider endpoint policy.
pub fn endpoint_ip_allowed(ip: IpAddr, mode: ProviderMode, allow_private_networks: bool) -> bool {
    let link_local = match ip {
        IpAddr::V4(value) => value.is_link_local(),
        IpAddr::V6(value) => value.is_unicast_link_local(),
    };
    if ip.is_unspecified() || ip.is_multicast() || link_local {
        return false;
    }
    if is_metadata_address(ip) {
        return false;
    }
    let private = is_private_address(ip) || ip.is_loopback();
    match mode {
        ProviderMode::Disabled => false,
        ProviderMode::LocalOnly => private,
        ProviderMode::RemoteAllowed => !private || allow_private_networks,
    }
}

fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private() || ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1])
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            (first & 0xfe00) == 0xfc00
        }
    }
}

fn is_metadata_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.octets() == [169, 254, 169, 254] || ip.octets() == [100, 100, 100, 200]
        }
        IpAddr::V6(ip) => ip == "fd00:ec2::254".parse().unwrap_or(ip),
    }
}
