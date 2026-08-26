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
    /// Generic OpenAI-compatible Chat Completions/embeddings endpoint.
    OpenAiCompatible,
    /// DeepSeek official OpenAI-compatible API.
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// Xiaomi MiMo official OpenAI-compatible API.
    XiaomiMimo,
    /// Zhipu GLM official OpenAI-compatible API.
    ZhipuGlm,
    /// Moonshot/Kimi official OpenAI-compatible API.
    MoonshotKimi,
    /// Google Gemini OpenAI-compatible API.
    GoogleGemini,
    /// Alibaba Qwen/DashScope OpenAI-compatible API.
    AlibabaQwen,
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
            Self::DeepSeek => "deepseek",
            Self::XiaomiMimo => "xiaomi_mimo",
            Self::ZhipuGlm => "zhipu_glm",
            Self::MoonshotKimi => "moonshot_kimi",
            Self::GoogleGemini => "google_gemini",
            Self::AlibabaQwen => "alibaba_qwen",
            Self::AnthropicMessages => "anthropic_messages",
            Self::EmbeddingHttp => "embedding_http",
            Self::FastEmbedLocal => "fastembed_local",
        }
    }

    /// Whether this Provider uses the shared OpenAI-compatible Chat and model
    /// endpoint adapter.
    pub const fn uses_openai_chat(self) -> bool {
        matches!(
            self,
            Self::OpenAiCompatible
                | Self::DeepSeek
                | Self::XiaomiMimo
                | Self::ZhipuGlm
                | Self::MoonshotKimi
                | Self::GoogleGemini
                | Self::AlibabaQwen
        )
    }
}

impl TryFrom<&str> for ProviderKind {
    type Error = ProviderError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "openai_responses" => Ok(Self::OpenAiResponses),
            "openai_compatible" => Ok(Self::OpenAiCompatible),
            "deepseek" => Ok(Self::DeepSeek),
            "xiaomi_mimo" => Ok(Self::XiaomiMimo),
            "zhipu_glm" => Ok(Self::ZhipuGlm),
            "moonshot_kimi" => Ok(Self::MoonshotKimi),
            "google_gemini" => Ok(Self::GoogleGemini),
            "alibaba_qwen" => Ok(Self::AlibabaQwen),
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

/// Provider preset for one OpenAI-compatible model.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiCompatibilityPreset {
    /// Resolve from the first-class Provider kind, then from an exact official
    /// API host for a legacy generic OpenAI-compatible Provider.
    #[default]
    Auto,
    /// Generic OpenAI Chat Completions behavior.
    Generic,
    /// DeepSeek official API behavior.
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// Xiaomi MiMo official API behavior.
    XiaomiMimo,
    /// Zhipu GLM official API behavior.
    ZhipuGlm,
    /// Moonshot/Kimi official API behavior.
    MoonshotKimi,
    /// Google Gemini OpenAI-compatibility behavior.
    GoogleGemini,
    /// Alibaba Qwen/DashScope OpenAI-compatibility behavior.
    AlibabaQwen,
}

impl OpenAiCompatibilityPreset {
    /// Resolve a concrete preset without changing persisted configuration.
    pub fn resolve(self, provider_kind: ProviderKind, provider_host: Option<&str>) -> Self {
        if self != Self::Auto {
            return self;
        }
        match provider_kind {
            ProviderKind::DeepSeek => Self::DeepSeek,
            ProviderKind::XiaomiMimo => Self::XiaomiMimo,
            ProviderKind::ZhipuGlm => Self::ZhipuGlm,
            ProviderKind::MoonshotKimi => Self::MoonshotKimi,
            ProviderKind::GoogleGemini => Self::GoogleGemini,
            ProviderKind::AlibabaQwen => Self::AlibabaQwen,
            ProviderKind::OpenAiCompatible => Self::from_provider_host(provider_host),
            _ => Self::Generic,
        }
    }

    fn from_provider_host(provider_host: Option<&str>) -> Self {
        let Some(host) = provider_host else {
            return Self::Generic;
        };
        let host = host.to_ascii_lowercase();
        if host == "api.deepseek.com" {
            Self::DeepSeek
        } else if host == "api.xiaomimimo.com" {
            Self::XiaomiMimo
        } else if host == "open.bigmodel.cn" {
            Self::ZhipuGlm
        } else if matches!(host.as_str(), "api.moonshot.ai" | "api.moonshot.cn") {
            Self::MoonshotKimi
        } else if host == "generativelanguage.googleapis.com" {
            Self::GoogleGemini
        } else if host == "dashscope.aliyuncs.com"
            || host == "dashscope-intl.aliyuncs.com"
            || host.ends_with(".dashscope.aliyuncs.com")
            || host.ends_with(".maas.aliyuncs.com")
        {
            Self::AlibabaQwen
        } else {
            Self::Generic
        }
    }

    /// Whether the preset uses a `thinking` object rather than another
    /// vendor-specific control.
    pub const fn uses_thinking_object(self) -> bool {
        matches!(
            self,
            Self::DeepSeek | Self::XiaomiMimo | Self::ZhipuGlm | Self::MoonshotKimi
        )
    }
}

/// Structured-output dialect, independent from the provider preset.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiStructuredOutputMode {
    /// Use the selected provider preset's documented default.
    #[default]
    Auto,
    /// Strict OpenAI `json_schema` Structured Outputs.
    StrictJsonSchema,
    /// `response_format=json_object` plus a full schema prompt.
    JsonObject,
    /// Prompt-constrained JSON without `response_format`.
    PromptOnly,
}

impl OpenAiStructuredOutputMode {
    /// Resolve a concrete structured-output dialect.
    pub const fn resolve(self, preset: OpenAiCompatibilityPreset) -> Self {
        match self {
            Self::Auto
                if matches!(
                    preset,
                    OpenAiCompatibilityPreset::DeepSeek
                        | OpenAiCompatibilityPreset::XiaomiMimo
                        | OpenAiCompatibilityPreset::ZhipuGlm
                        | OpenAiCompatibilityPreset::AlibabaQwen
                ) =>
            {
                Self::JsonObject
            }
            Self::Auto => Self::StrictJsonSchema,
            concrete => concrete,
        }
    }
}

/// Thinking behavior for provider presets with a documented control.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiThinkingMode {
    /// Preserve the provider/model default.
    #[default]
    Auto,
    /// Preserve model reasoning before the final answer.
    Enabled,
    /// Disable reasoning for lower latency/cost when explicitly requested.
    Disabled,
}

impl OpenAiThinkingMode {
    /// Wire value for providers using `thinking.type`.
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Enabled => Some("enabled"),
            Self::Disabled => Some("disabled"),
        }
    }

    /// Wire boolean for providers using `enable_thinking`.
    pub const fn as_bool(self) -> Option<bool> {
        match self {
            Self::Auto => None,
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
        }
    }
}

/// Default bounded per-call generation budget for reasoning-first presets.
pub const DEFAULT_REASONING_GENERATION_TOKENS: u32 = 32_768;

/// Token-limit field used by an OpenAI-compatible Chat Completions endpoint.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiTokenLimitField {
    /// Resolve from the selected wire profile.
    #[default]
    Auto,
    /// Legacy/widely compatible `max_tokens` field.
    MaxTokens,
    /// Modern/reasoning-model `max_completion_tokens` field.
    MaxCompletionTokens,
}

impl OpenAiTokenLimitField {
    /// Resolve the concrete field for one provider preset.
    pub const fn resolve(self, preset: OpenAiCompatibilityPreset) -> Self {
        match self {
            Self::Auto
                if matches!(
                    preset,
                    OpenAiCompatibilityPreset::XiaomiMimo
                        | OpenAiCompatibilityPreset::MoonshotKimi
                        | OpenAiCompatibilityPreset::AlibabaQwen
                ) =>
            {
                Self::MaxCompletionTokens
            }
            Self::Auto => Self::MaxTokens,
            concrete => concrete,
        }
    }
}

/// Typed model-specific settings stored with a provider model record.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ModelSettings {
    /// Provider preset used by OpenAI-compatible structured generation.
    pub openai_compatibility_preset: OpenAiCompatibilityPreset,
    /// Structured-output mode, independently overridable per model.
    pub openai_structured_output_mode: OpenAiStructuredOutputMode,
    /// Token-limit field, independently selectable for compatible vendors.
    pub openai_token_limit_field: OpenAiTokenLimitField,
    /// Thinking behavior, translated by the selected provider preset.
    pub openai_thinking_mode: OpenAiThinkingMode,
    /// Optional bounded generation-token ceiling for one model call.
    pub generation_token_limit: Option<u32>,
}

impl ModelSettings {
    /// Decode settings persisted as JSON, accepting empty legacy objects.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, ProviderError> {
        serde_json::from_value(value.clone())
            .map_err(|_| ProviderError::InvalidConfiguration("model settings are invalid"))
    }

    /// Reject compatibility profiles on adapters that do not use the
    /// OpenAI-compatible Chat Completions wire format.
    pub fn validate_for_model(
        &self,
        provider_kind: ProviderKind,
        provider_host: Option<&str>,
        external_model_id: &str,
    ) -> Result<(), ProviderError> {
        if !provider_kind.uses_openai_chat()
            && (self.openai_compatibility_preset != OpenAiCompatibilityPreset::Auto
                || self.openai_structured_output_mode != OpenAiStructuredOutputMode::Auto
                || self.openai_token_limit_field != OpenAiTokenLimitField::Auto
                || self.openai_thinking_mode != OpenAiThinkingMode::Auto)
        {
            return Err(ProviderError::InvalidConfiguration(
                "OpenAI compatibility settings do not match provider type",
            ));
        }
        if self
            .generation_token_limit
            .is_some_and(|limit| limit == 0 || limit > 1_048_576)
        {
            return Err(ProviderError::InvalidConfiguration(
                "model generation-token limit is invalid",
            ));
        }
        let preset = self
            .openai_compatibility_preset
            .resolve(provider_kind, provider_host);
        if self.openai_thinking_mode != OpenAiThinkingMode::Auto
            && matches!(preset, OpenAiCompatibilityPreset::Generic)
        {
            return Err(ProviderError::InvalidConfiguration(
                "thinking control requires a provider compatibility preset",
            ));
        }
        let model = external_model_id.to_ascii_lowercase();
        if self.openai_thinking_mode == OpenAiThinkingMode::Disabled
            && matches!(preset, OpenAiCompatibilityPreset::MoonshotKimi)
            && (model.starts_with("kimi-k3")
                || model.starts_with("kimi-k2.7-code")
                || model.contains("/kimi-k3")
                || model.contains("/kimi-k2.7-code"))
        {
            return Err(ProviderError::InvalidConfiguration(
                "selected Kimi model cannot disable thinking",
            ));
        }
        if self.openai_thinking_mode == OpenAiThinkingMode::Disabled
            && matches!(preset, OpenAiCompatibilityPreset::GoogleGemini)
            && (model.starts_with("gemini-3") || model.starts_with("gemini-2.5-pro"))
        {
            return Err(ProviderError::InvalidConfiguration(
                "selected Gemini model cannot disable thinking",
            ));
        }
        Ok(())
    }

    /// Resolve one bounded generation budget, respecting a lower model
    /// capability limit when the operator recorded one.
    pub fn effective_generation_token_limit(
        &self,
        provider_kind: ProviderKind,
        provider_host: Option<&str>,
        capabilities: &ModelCapabilities,
        requested: u32,
    ) -> u32 {
        let preset = self
            .openai_compatibility_preset
            .resolve(provider_kind, provider_host);
        let default = if matches!(
            preset,
            OpenAiCompatibilityPreset::DeepSeek
                | OpenAiCompatibilityPreset::XiaomiMimo
                | OpenAiCompatibilityPreset::MoonshotKimi
                | OpenAiCompatibilityPreset::GoogleGemini
                | OpenAiCompatibilityPreset::AlibabaQwen
        ) {
            requested.max(DEFAULT_REASONING_GENERATION_TOKENS)
        } else {
            requested
        };
        let configured = self.generation_token_limit.unwrap_or(default);
        capabilities
            .max_output_tokens
            .map_or(configured, |limit| limit.min(configured))
    }
}

impl ModelCapabilities {
    /// Decode capabilities stored as JSON.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, ProviderError> {
        let capabilities: Self = serde_json::from_value(value.clone())
            .map_err(|_| ProviderError::InvalidConfiguration("model capabilities are invalid"))?;
        capabilities.validate()?;
        Ok(capabilities)
    }

    /// Validate optional model limits without assuming every provider reports
    /// a complete capability document.
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self
            .dimension
            .is_some_and(|value| value == 0 || value > 1_000_000)
            || self
                .context_window
                .is_some_and(|value| value == 0 || value > 100_000_000)
            || self
                .max_output_tokens
                .is_some_and(|value| value == 0 || value > 10_000_000)
        {
            return Err(ProviderError::InvalidConfiguration(
                "model capability limits are invalid",
            ));
        }
        if self.dimension.is_some() && !self.embeddings {
            return Err(ProviderError::InvalidConfiguration(
                "embedding dimension requires embedding capability",
            ));
        }
        Ok(())
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
