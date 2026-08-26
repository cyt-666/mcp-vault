//! Provider adapter contracts and wire-format translators.

use std::time::Duration;

use async_trait::async_trait;
use mcp_vault_auth::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::{
    AuthStyle, ModelSettings, OpenAiCompatibilityPreset, OpenAiStructuredOutputMode,
    OpenAiThinkingMode, OpenAiTokenLimitField, ProviderError, ProviderKind, ProviderMode,
    ProviderSettings, ProviderTransport, RequestOptions, endpoint_url,
};

/// One caller-authorized deterministic repair for a missing required root string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingRequiredStringFallback {
    /// Missing root property to fill.
    pub target: String,
    /// Existing root string property whose value is copied verbatim.
    pub source: String,
}

impl MissingRequiredStringFallback {
    /// Build a trusted root-property fallback policy.
    pub fn new(target: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            source: source.into(),
        }
    }
}

/// Structured generation input.
#[derive(Clone, Debug, PartialEq)]
pub struct StructuredGenerationRequest {
    /// Provider model identifier.
    pub model: String,
    /// Trusted system instructions owned by the application.
    pub system: String,
    /// Untrusted source/task content, delimited by the caller.
    pub user: String,
    /// Stable schema name.
    pub schema_name: String,
    /// JSON Schema subset required for the response.
    pub schema: Value,
    /// Caller-authorized missing-string repairs applied before full validation.
    pub missing_required_string_fallbacks: Vec<MissingRequiredStringFallback>,
    /// Maximum generated token estimate.
    pub max_output_tokens: u32,
    /// Optional deterministic temperature.
    pub temperature: Option<f32>,
    /// Optional operation-specific total timeout overriding the Provider default.
    pub timeout: Option<Duration>,
}

/// Structured generation result after JSON/schema validation.
#[derive(Clone, Debug, PartialEq)]
pub struct StructuredGenerationResult {
    /// Validated structured JSON.
    pub value: Value,
    /// Provider-returned model identifier when available.
    pub model: Option<String>,
    /// Provider usage metadata without prompt/content bodies.
    pub usage: Option<Value>,
}

/// Effective model and credential options for one structured generation call.
#[derive(Clone, Copy)]
pub struct GenerationOptions<'a> {
    model_settings: &'a ModelSettings,
    generation_token_limit: u32,
    secret: Option<&'a SecretString>,
}

impl<'a> GenerationOptions<'a> {
    /// Construct options after the Provider service has resolved and bounded
    /// model configuration.
    pub const fn new(
        model_settings: &'a ModelSettings,
        generation_token_limit: u32,
        secret: Option<&'a SecretString>,
    ) -> Self {
        Self {
            model_settings,
            generation_token_limit,
            secret,
        }
    }
}

/// Embedding request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddingRequest {
    /// Provider model identifier.
    pub model: String,
    /// Bounded source strings.
    pub inputs: Vec<String>,
}

/// Embedding response after dimension and finite-value validation.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingResult {
    /// One vector per input in the same order.
    pub vectors: Vec<Vec<f32>>,
    /// Provider-returned model identifier when available.
    pub model: Option<String>,
    /// Provider usage metadata without source content.
    pub usage: Option<Value>,
}

/// One model discovered from a provider endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// External provider model ID.
    pub id: String,
    /// Capability metadata returned or inferred by the adapter.
    pub capabilities: Value,
}

/// Adapter boundary independent from Admin/MCP/memory callers.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// Adapter family.
    fn kind(&self) -> ProviderKind;

    /// Generate schema-validated structured output.
    async fn generate_structured(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        options: GenerationOptions<'_>,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError>;

    /// Generate embeddings when the provider supports them.
    async fn embed(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        settings: &ProviderSettings,
        secret: Option<&SecretString>,
        request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError>;

    /// Discover models when supported.
    async fn list_models(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        secret: Option<&SecretString>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError>;
}

/// OpenAI Responses API adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiResponsesAdapter;

/// OpenAI-compatible chat/embedding adapter.
#[derive(Clone, Copy, Debug)]
pub struct OpenAiCompatibleAdapter {
    provider_kind: ProviderKind,
}

impl Default for OpenAiCompatibleAdapter {
    fn default() -> Self {
        Self::new(ProviderKind::OpenAiCompatible)
    }
}

impl OpenAiCompatibleAdapter {
    /// Construct one shared adapter with a first-class provider preset.
    pub const fn new(provider_kind: ProviderKind) -> Self {
        Self { provider_kind }
    }
}

/// Anthropic Messages API adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnthropicMessagesAdapter;

/// OpenAI-compatible embedding-only adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpEmbeddingAdapter;

#[async_trait]
impl ProviderAdapter for OpenAiResponsesAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiResponses
    }

    async fn generate_structured(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        options: GenerationOptions<'_>,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        let endpoint = endpoint_url(base_url, "responses")?;
        let body = json!({
            "model": request.model,
            "instructions": request.system,
            "input": request.user,
            "max_output_tokens": options.generation_token_limit,
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": request.schema_name,
                    "strict": true,
                    "schema": request.schema
                }
            }
        });
        let response = transport
            .request_json(
                reqwest::Method::POST,
                &endpoint,
                mode,
                &body,
                RequestOptions::new(AuthStyle::Bearer, options.secret)
                    .with_timeout(request.timeout),
            )
            .await?;
        structured_result_for_request(&response.body, request, false)
    }

    async fn embed(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _settings: &ProviderSettings,
        _secret: Option<&SecretString>,
        _request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }

    async fn list_models(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        secret: Option<&SecretString>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        list_openai_models(transport, base_url, mode, secret).await
    }
}

#[async_trait]
impl ProviderAdapter for OpenAiCompatibleAdapter {
    fn kind(&self) -> ProviderKind {
        self.provider_kind
    }

    async fn generate_structured(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        options: GenerationOptions<'_>,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        let endpoint = endpoint_url(base_url, "chat/completions")?;
        let body = openai_chat_body(
            request,
            options.model_settings,
            self.provider_kind,
            base_url.host_str(),
            options.generation_token_limit,
        )?;
        let allow_envelope_repair = body
            .get("response_format")
            .and_then(|format| format.get("type"))
            .and_then(Value::as_str)
            != Some("json_schema");
        let response = transport
            .request_json(
                reqwest::Method::POST,
                &endpoint,
                mode,
                &body,
                RequestOptions::new(AuthStyle::Bearer, options.secret)
                    .with_timeout(request.timeout),
            )
            .await?;
        structured_result_for_request(&response.body, request, allow_envelope_repair)
    }

    async fn embed(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        _settings: &ProviderSettings,
        secret: Option<&SecretString>,
        request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError> {
        openai_embeddings(transport, base_url, mode, secret, request).await
    }

    async fn list_models(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        secret: Option<&SecretString>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        list_openai_models(transport, base_url, mode, secret).await
    }
}

#[async_trait]
impl ProviderAdapter for AnthropicMessagesAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::AnthropicMessages
    }

    async fn generate_structured(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        options: GenerationOptions<'_>,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        let endpoint = endpoint_url(base_url, "messages")?;
        let system = format!(
            "{}\nReturn only one JSON value matching this schema:\n{}",
            request.system,
            serde_json::to_string(&request.schema)
                .map_err(|_| ProviderError::InvalidConfiguration("schema is invalid"))?
        );
        let body = json!({
            "model": request.model,
            "system": system,
            "max_tokens": options.generation_token_limit,
            "messages": [{"role": "user", "content": request.user}]
        });
        let response = transport
            .request_json(
                reqwest::Method::POST,
                &endpoint,
                mode,
                &body,
                RequestOptions::new(AuthStyle::Anthropic, options.secret)
                    .with_timeout(request.timeout),
            )
            .await?;
        structured_result_for_request(&response.body, request, true)
    }

    async fn embed(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _settings: &ProviderSettings,
        _secret: Option<&SecretString>,
        _request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }

    async fn list_models(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _secret: Option<&SecretString>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }
}

#[async_trait]
impl ProviderAdapter for HttpEmbeddingAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::EmbeddingHttp
    }

    async fn generate_structured(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _options: GenerationOptions<'_>,
        _request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }

    async fn embed(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        _settings: &ProviderSettings,
        secret: Option<&SecretString>,
        request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError> {
        openai_embeddings(transport, base_url, mode, secret, request).await
    }

    async fn list_models(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        secret: Option<&SecretString>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        list_openai_models(transport, base_url, mode, secret).await
    }
}

#[cfg(test)]
fn structured_result(
    body: &Value,
    schema: &Value,
) -> Result<StructuredGenerationResult, ProviderError> {
    structured_result_with_envelope_repair(body, schema, false)
}

fn structured_result_for_request(
    body: &Value,
    request: &StructuredGenerationRequest,
    allow_envelope_repair: bool,
) -> Result<StructuredGenerationResult, ProviderError> {
    structured_result_with_repairs(
        body,
        &request.schema,
        allow_envelope_repair,
        &request.missing_required_string_fallbacks,
    )
}

#[cfg(test)]
fn structured_result_with_envelope_repair(
    body: &Value,
    schema: &Value,
    allow_envelope_repair: bool,
) -> Result<StructuredGenerationResult, ProviderError> {
    structured_result_with_repairs(body, schema, allow_envelope_repair, &[])
}

fn structured_result_with_repairs(
    body: &Value,
    schema: &Value,
    allow_envelope_repair: bool,
    missing_required_string_fallbacks: &[MissingRequiredStringFallback],
) -> Result<StructuredGenerationResult, ProviderError> {
    let text = extract_text(body)?;
    let text = structured_json_text(&text);
    let value: Value = serde_json::from_str(text)
        .map_err(|_| response_contract_error(body, "structured output is not JSON"))?;
    let value = if allow_envelope_repair {
        normalize_single_array_envelope(value, schema)
    } else {
        value
    };
    let value =
        normalize_missing_required_strings(value, schema, missing_required_string_fallbacks);
    validate_json_schema(&value, schema)?;
    Ok(StructuredGenerationResult {
        value,
        model: body.get("model").and_then(Value::as_str).map(str::to_owned),
        usage: body.get("usage").cloned(),
    })
}

fn normalize_missing_required_strings(
    mut value: Value,
    schema: &Value,
    fallbacks: &[MissingRequiredStringFallback],
) -> Value {
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return value;
    };
    let Some(required) = schema.get("required").and_then(Value::as_array) else {
        return value;
    };
    for fallback in fallbacks {
        if object.contains_key(&fallback.target)
            || !required
                .iter()
                .any(|property| property.as_str() == Some(fallback.target.as_str()))
            || !required
                .iter()
                .any(|property| property.as_str() == Some(fallback.source.as_str()))
            || properties
                .get(&fallback.target)
                .and_then(|property| property.get("type"))
                .and_then(Value::as_str)
                != Some("string")
            || properties
                .get(&fallback.source)
                .and_then(|property| property.get("type"))
                .and_then(Value::as_str)
                != Some("string")
        {
            continue;
        }
        let Some(source) = object
            .get(&fallback.source)
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        object.insert(fallback.target.clone(), Value::String(source));
    }
    value
}

fn extract_text(body: &Value) -> Result<String, ProviderError> {
    if let Some(text) = body.get("output_text").and_then(Value::as_str) {
        return Ok(text.to_owned());
    }
    if let Some(text) = body
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
    {
        return Ok(text.to_owned());
    }
    if let Some(text) = body
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|part| part.get("text").and_then(Value::as_str))
        })
    {
        return Ok(text.to_owned());
    }
    if let Some(content) = body.get("content").and_then(Value::as_array)
        && let Some(text) = content
            .iter()
            .find_map(|item| item.get("text").and_then(Value::as_str))
    {
        return Ok(text.to_owned());
    }
    if let Some(output) = body.get("output").and_then(Value::as_array) {
        for item in output {
            if let Some(content) = item.get("content").and_then(Value::as_array)
                && let Some(text) = content
                    .iter()
                    .find_map(|part| part.get("text").and_then(Value::as_str))
            {
                return Ok(text.to_owned());
            }
        }
    }
    Err(response_contract_error(
        body,
        "provider final content is missing",
    ))
}

fn openai_chat_body(
    request: &StructuredGenerationRequest,
    model_settings: &ModelSettings,
    provider_kind: ProviderKind,
    provider_host: Option<&str>,
    generation_token_limit: u32,
) -> Result<Value, ProviderError> {
    let preset = model_settings
        .openai_compatibility_preset
        .resolve(provider_kind, provider_host);
    let output_mode = model_settings.openai_structured_output_mode.resolve(preset);
    let token_field = model_settings.openai_token_limit_field.resolve(preset);
    let prompt_constrained = matches!(
        output_mode,
        OpenAiStructuredOutputMode::JsonObject | OpenAiStructuredOutputMode::PromptOnly
    );
    let system = if prompt_constrained {
        json_mode_system_prompt(&request.system, &request.schema)?
    } else {
        request.system.clone()
    };
    let mut body = json!({
        "model": request.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": request.user}
        ]
    });
    match output_mode {
        OpenAiStructuredOutputMode::Auto => {
            unreachable!("automatic structured-output mode must resolve")
        }
        OpenAiStructuredOutputMode::StrictJsonSchema => {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": request.schema_name,
                    "strict": true,
                    "schema": request.schema
                }
            });
        }
        OpenAiStructuredOutputMode::JsonObject => {
            body["response_format"] = json!({"type": "json_object"});
        }
        OpenAiStructuredOutputMode::PromptOnly => {}
    }

    match token_field {
        OpenAiTokenLimitField::Auto => unreachable!("automatic token-limit field must resolve"),
        OpenAiTokenLimitField::MaxTokens => {
            body["max_tokens"] = json!(generation_token_limit);
        }
        OpenAiTokenLimitField::MaxCompletionTokens => {
            body["max_completion_tokens"] = json!(generation_token_limit);
        }
    }

    let thinking_mode = match (preset, model_settings.openai_thinking_mode) {
        (
            OpenAiCompatibilityPreset::DeepSeek | OpenAiCompatibilityPreset::XiaomiMimo,
            OpenAiThinkingMode::Auto,
        ) => OpenAiThinkingMode::Enabled,
        (_, configured) => configured,
    };
    let model = request.model.to_ascii_lowercase();
    match preset {
        OpenAiCompatibilityPreset::DeepSeek
        | OpenAiCompatibilityPreset::XiaomiMimo
        | OpenAiCompatibilityPreset::ZhipuGlm => {
            if let Some(value) = thinking_mode.as_str() {
                body["thinking"] = json!({"type": value});
            }
        }
        OpenAiCompatibilityPreset::MoonshotKimi => {
            let always_thinking = model.starts_with("kimi-k3")
                || model.starts_with("kimi-k2.7-code")
                || model.contains("/kimi-k3")
                || model.contains("/kimi-k2.7-code");
            if !always_thinking && let Some(value) = thinking_mode.as_str() {
                body["thinking"] = json!({"type": value});
            }
        }
        OpenAiCompatibilityPreset::AlibabaQwen => {
            if let Some(value) = thinking_mode.as_bool() {
                body["enable_thinking"] = json!(value);
            }
        }
        OpenAiCompatibilityPreset::GoogleGemini => match thinking_mode {
            OpenAiThinkingMode::Auto => {}
            OpenAiThinkingMode::Enabled => body["reasoning_effort"] = json!("medium"),
            OpenAiThinkingMode::Disabled => body["reasoning_effort"] = json!("none"),
        },
        OpenAiCompatibilityPreset::Auto => {
            unreachable!("automatic provider preset must resolve")
        }
        OpenAiCompatibilityPreset::Generic => {}
    }

    let forwards_temperature = match preset {
        OpenAiCompatibilityPreset::Generic => true,
        OpenAiCompatibilityPreset::DeepSeek | OpenAiCompatibilityPreset::XiaomiMimo => {
            thinking_mode == OpenAiThinkingMode::Disabled
        }
        OpenAiCompatibilityPreset::ZhipuGlm => {
            request
                .temperature
                .is_some_and(|temperature| temperature > 0.0)
                && thinking_mode != OpenAiThinkingMode::Enabled
        }
        OpenAiCompatibilityPreset::Auto
        | OpenAiCompatibilityPreset::MoonshotKimi
        | OpenAiCompatibilityPreset::GoogleGemini
        | OpenAiCompatibilityPreset::AlibabaQwen => false,
    };
    if forwards_temperature && let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    Ok(body)
}

fn json_mode_system_prompt(system: &str, schema: &Value) -> Result<String, ProviderError> {
    let schema = serde_json::to_string(schema)
        .map_err(|_| ProviderError::InvalidConfiguration("schema is invalid"))?;
    Ok(format!(
        "{system}\nReturn only one compact JSON object without explanations, comments, or Markdown fences. The top-level object must include every required property even when its value is an empty array. Never rename an envelope property or return an array/item directly. The result must match this JSON Schema exactly:\n{schema}"
    ))
}

fn normalize_single_array_envelope(value: Value, schema: &Value) -> Value {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return value;
    };
    if properties.len() != 1
        || schema.get("additionalProperties").and_then(Value::as_bool) != Some(false)
    {
        return value;
    }
    let Some((envelope_key, array_schema)) = properties.iter().next() else {
        return value;
    };
    if array_schema.get("type").and_then(Value::as_str) != Some("array") {
        return value;
    }
    let required = schema.get("required").and_then(Value::as_array);
    if required.is_none_or(|required| {
        required.len() != 1 || required[0].as_str() != Some(envelope_key.as_str())
    }) {
        return value;
    }
    let Some(item_schema) = array_schema.get("items") else {
        return value;
    };
    if item_schema
        .get("required")
        .and_then(Value::as_array)
        .is_none_or(|required| required.is_empty())
    {
        return value;
    }
    let can_wrap = if let Some(items) = value.as_array() {
        items
            .iter()
            .all(|item| validate_json_schema_at(item, item_schema, "$item").is_ok())
    } else if let Some(object) = value.as_object() {
        !object.contains_key(envelope_key)
            && validate_json_schema_at(&value, item_schema, "$item").is_ok()
    } else {
        false
    };
    if !can_wrap {
        return value;
    }
    let items = if value.is_array() {
        value
    } else {
        Value::Array(vec![value])
    };
    let mut object = serde_json::Map::new();
    object.insert(envelope_key.clone(), items);
    Value::Object(object)
}

fn structured_json_text(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(fenced) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(line_end) = fenced.find('\n') else {
        return trimmed;
    };
    let language = fenced[..line_end].trim();
    if !language.is_empty() && !language.eq_ignore_ascii_case("json") {
        return trimmed;
    }
    fenced[line_end + 1..]
        .strip_suffix("```")
        .map(str::trim)
        .unwrap_or(trimmed)
}

fn response_contract_error(body: &Value, fallback: &'static str) -> ProviderError {
    let finish_reason = body
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str);
    let incomplete_reason = body
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str);
    let reason = match (finish_reason, incomplete_reason) {
        (_, Some("max_output_tokens")) => "provider output reached token limit",
        (_, Some("content_filter")) => "provider output was filtered",
        (Some("length"), _) => "provider output reached token limit",
        (Some("content_filter"), _) => "provider output was filtered",
        (Some("repetition_truncation"), _) => "provider output repetition was truncated",
        _ if body.get("status").and_then(Value::as_str) == Some("incomplete") => {
            "provider response was incomplete"
        }
        _ => fallback,
    };
    ProviderError::InvalidResponse(reason)
}

async fn openai_embeddings(
    transport: &ProviderTransport,
    base_url: &Url,
    mode: ProviderMode,
    secret: Option<&SecretString>,
    request: &EmbeddingRequest,
) -> Result<EmbeddingResult, ProviderError> {
    if request.inputs.is_empty() || request.inputs.len() > 128 {
        return Err(ProviderError::InvalidConfiguration(
            "embedding batch size is invalid",
        ));
    }
    let endpoint = endpoint_url(base_url, "embeddings")?;
    let body = json!({"model": request.model, "input": request.inputs});
    let response = transport
        .request_json(
            reqwest::Method::POST,
            &endpoint,
            mode,
            &body,
            RequestOptions::new(AuthStyle::Bearer, secret),
        )
        .await?;
    let data = response
        .body
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse("embedding data is missing"))?;
    let mut vectors = Vec::with_capacity(data.len());
    for item in data {
        let values = item.get("embedding").and_then(Value::as_array).ok_or(
            ProviderError::InvalidResponse("embedding vector is missing"),
        )?;
        let vector = values
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .map(|value| value as f32)
                    .filter(|value| value.is_finite())
                    .ok_or(ProviderError::InvalidResponse("embedding value is invalid"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if vector.is_empty() {
            return Err(ProviderError::InvalidResponse("embedding vector is empty"));
        }
        vectors.push(vector);
    }
    if vectors.len() != request.inputs.len()
        || vectors
            .windows(2)
            .any(|vectors| vectors[0].len() != vectors[1].len())
    {
        return Err(ProviderError::InvalidResponse(
            "embedding response count or dimensions are inconsistent",
        ));
    }
    Ok(EmbeddingResult {
        vectors,
        model: response
            .body
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned),
        usage: response.body.get("usage").cloned(),
    })
}

async fn list_openai_models(
    transport: &ProviderTransport,
    base_url: &Url,
    mode: ProviderMode,
    secret: Option<&SecretString>,
) -> Result<Vec<DiscoveredModel>, ProviderError> {
    let endpoint = endpoint_url(base_url, "models")?;
    let response = transport
        .request_json(
            reqwest::Method::GET,
            &endpoint,
            mode,
            &json!({}),
            RequestOptions::new(AuthStyle::Bearer, secret),
        )
        .await?;
    let data = response
        .body
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse("model data is missing"))?;
    data.iter()
        .map(|model| {
            let id = model
                .get("id")
                .and_then(Value::as_str)
                .ok_or(ProviderError::InvalidResponse("model ID is missing"))?;
            Ok(DiscoveredModel {
                id: id.to_owned(),
                capabilities: json!({}),
            })
        })
        .collect()
}

fn validate_json_schema(value: &Value, schema: &Value) -> Result<(), ProviderError> {
    validate_json_schema_at(value, schema, "$")
}

fn validate_json_schema_at(value: &Value, schema: &Value, path: &str) -> Result<(), ProviderError> {
    if let Some(schema_type) = schema.get("type")
        && !schema_type_matches(value, schema_type)
    {
        return Err(schema_validation_error("type_mismatch", path));
    }
    if let Some(enums) = schema.get("enum").and_then(Value::as_array)
        && !enums.iter().any(|candidate| candidate == value)
    {
        return Err(schema_validation_error("enum_mismatch", path));
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required {
                let Some(key) = key.as_str() else {
                    return Err(schema_validation_error("schema_invalid", path));
                };
                if !object.contains_key(key) {
                    return Err(schema_validation_error(
                        "required_property_missing",
                        &schema_property_path(path, key),
                    ));
                }
            }
        }
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && object.keys().any(|key| !properties.contains_key(key))
        {
            return Err(schema_validation_error("unexpected_property", path));
        }
        for (key, child_schema) in properties {
            if let Some(child) = object.get(key) {
                validate_json_schema_at(child, child_schema, &schema_property_path(path, key))?;
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        let item_count = u64::try_from(array.len()).unwrap_or(u64::MAX);
        if schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| item_count > maximum)
        {
            return Err(schema_validation_error("array_too_long", path));
        }
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| item_count < minimum)
        {
            return Err(schema_validation_error("array_too_short", path));
        }
        for (index, item) in array.iter().enumerate() {
            validate_json_schema_at(item, items, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

fn schema_type_matches(value: &Value, schema_type: &Value) -> bool {
    if let Some(schema_type) = schema_type.as_str() {
        return value_matches_schema_type(value, schema_type);
    }
    schema_type.as_array().is_some_and(|types| {
        !types.is_empty()
            && types.iter().all(Value::is_string)
            && types.iter().any(|schema_type| {
                schema_type
                    .as_str()
                    .is_some_and(|schema_type| value_matches_schema_type(value, schema_type))
            })
    })
}

fn value_matches_schema_type(value: &Value, schema_type: &str) -> bool {
    match schema_type {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn schema_property_path(path: &str, property: &str) -> String {
    format!("{path}.{property}")
}

fn schema_validation_error(issue: &'static str, path: &str) -> ProviderError {
    ProviderError::SchemaValidation {
        issue,
        path: path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MissingRequiredStringFallback, StructuredGenerationRequest, extract_text, openai_chat_body,
        structured_result, structured_result_with_envelope_repair, structured_result_with_repairs,
        validate_json_schema,
    };
    use crate::{
        ModelSettings, OpenAiCompatibilityPreset, OpenAiStructuredOutputMode, OpenAiThinkingMode,
        OpenAiTokenLimitField, ProviderKind,
    };
    use serde_json::json;

    fn generation_request(model: &str) -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            model: model.to_owned(),
            system: "Extract durable memories.".to_owned(),
            user: "<untrusted_markdown>source</untrusted_markdown>".to_owned(),
            schema_name: "memory_extraction".to_owned(),
            schema: json!({
                "type": "object",
                "properties": {"memories": {"type": "array", "items": {"type": "object"}}},
                "required": ["memories"],
                "additionalProperties": false
            }),
            missing_required_string_fallbacks: Vec::new(),
            max_output_tokens: 8_192,
            temperature: Some(0.0),
            timeout: None,
        }
    }

    fn chat_body(
        provider_kind: ProviderKind,
        model: &str,
        settings: &ModelSettings,
        generation_token_limit: u32,
    ) -> serde_json::Value {
        openai_chat_body(
            &generation_request(model),
            settings,
            provider_kind,
            Some("proxy.example.test"),
            generation_token_limit,
        )
        .unwrap()
    }

    #[test]
    fn extracts_responses_and_anthropic_text_shapes() {
        assert_eq!(
            extract_text(&json!({"output_text": "{\"ok\":true}"})).unwrap(),
            "{\"ok\":true}"
        );
        assert_eq!(
            extract_text(&json!({"content": [{"type": "text", "text": "{\"ok\":true}"}]})).unwrap(),
            "{\"ok\":true}"
        );
        assert_eq!(
            extract_text(&json!({"choices": [{"message": {"content": "{\"ok\":true}"}}]})).unwrap(),
            "{\"ok\":true}"
        );
        assert_eq!(
            extract_text(&json!({"choices": [{"message": {"content": [{"type": "text", "text": "{\"ok\":true}"}]}}]})).unwrap(),
            "{\"ok\":true}"
        );
    }

    #[test]
    fn first_class_provider_presets_emit_their_documented_contracts() {
        let generic = chat_body(
            ProviderKind::OpenAiCompatible,
            "generic-model",
            &ModelSettings::default(),
            8_192,
        );
        assert_eq!(generic["response_format"]["type"], "json_schema");
        assert_eq!(generic["max_tokens"], 8_192);
        assert_eq!(generic["temperature"], 0.0);

        let deepseek = chat_body(
            ProviderKind::DeepSeek,
            "deepseek-v4-pro",
            &ModelSettings::default(),
            32_768,
        );
        assert_eq!(deepseek["response_format"]["type"], "json_object");
        assert_eq!(deepseek["max_tokens"], 32_768);
        assert_eq!(deepseek["thinking"]["type"], "enabled");
        assert!(deepseek.get("temperature").is_none());

        let mimo = chat_body(
            ProviderKind::XiaomiMimo,
            "mimo-v2.5",
            &ModelSettings::default(),
            32_768,
        );
        assert_eq!(mimo["response_format"]["type"], "json_object");
        assert_eq!(mimo["max_completion_tokens"], 32_768);
        assert_eq!(mimo["thinking"]["type"], "enabled");
        assert!(mimo.get("temperature").is_none());

        let zhipu = chat_body(
            ProviderKind::ZhipuGlm,
            "glm-5.2",
            &ModelSettings::default(),
            8_192,
        );
        assert_eq!(zhipu["response_format"]["type"], "json_object");
        assert_eq!(zhipu["max_tokens"], 8_192);
        assert!(zhipu.get("thinking").is_none());
        assert!(zhipu.get("temperature").is_none());

        let kimi = chat_body(
            ProviderKind::MoonshotKimi,
            "kimi-k2.6",
            &ModelSettings::default(),
            32_768,
        );
        assert_eq!(kimi["response_format"]["type"], "json_schema");
        assert_eq!(kimi["max_completion_tokens"], 32_768);
        assert!(kimi.get("thinking").is_none());
        assert!(kimi.get("temperature").is_none());

        let gemini = chat_body(
            ProviderKind::GoogleGemini,
            "gemini-3.7-flash",
            &ModelSettings::default(),
            32_768,
        );
        assert_eq!(gemini["response_format"]["type"], "json_schema");
        assert_eq!(gemini["max_tokens"], 32_768);
        assert!(gemini.get("reasoning_effort").is_none());
        assert!(gemini.get("temperature").is_none());

        let qwen = chat_body(
            ProviderKind::AlibabaQwen,
            "qwen3.8-max",
            &ModelSettings::default(),
            32_768,
        );
        assert_eq!(qwen["response_format"]["type"], "json_object");
        assert_eq!(qwen["max_completion_tokens"], 32_768);
        assert!(qwen.get("enable_thinking").is_none());
        assert!(qwen.get("temperature").is_none());

        for body in [&deepseek, &mimo, &zhipu, &qwen] {
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(system.contains("Return only one compact JSON object"));
            assert!(system.contains("Never rename an envelope property"));
            assert!(system.contains("\"memories\""));
        }
    }

    #[test]
    fn generic_provider_migrates_official_hosts_without_guessing_from_model_names() {
        let body = openai_chat_body(
            &generation_request("mimo-v2.5"),
            &ModelSettings::default(),
            ProviderKind::OpenAiCompatible,
            Some("api.xiaomimimo.com"),
            32_768,
        )
        .unwrap();

        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["max_completion_tokens"], 32_768);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body.get("temperature").is_none());

        let local_proxy = openai_chat_body(
            &generation_request("mimo-v2.5"),
            &ModelSettings::default(),
            ProviderKind::OpenAiCompatible,
            Some("127.0.0.1"),
            8_192,
        )
        .unwrap();
        assert_eq!(local_proxy["response_format"]["type"], "json_schema");
        assert_eq!(local_proxy["max_tokens"], 8_192);
        assert!(local_proxy.get("thinking").is_none());
    }

    #[test]
    fn model_settings_override_preset_output_token_and_thinking_axes() {
        let body = chat_body(
            ProviderKind::OpenAiCompatible,
            "vendor-alias",
            &ModelSettings {
                openai_compatibility_preset: OpenAiCompatibilityPreset::XiaomiMimo,
                openai_structured_output_mode: OpenAiStructuredOutputMode::PromptOnly,
                openai_token_limit_field: OpenAiTokenLimitField::MaxTokens,
                openai_thinking_mode: OpenAiThinkingMode::Disabled,
                generation_token_limit: Some(12_000),
            },
            12_000,
        );

        assert!(body.get("response_format").is_none());
        assert_eq!(body["max_tokens"], 12_000);
        assert_eq!(body["thinking"]["type"], "disabled");
        assert_eq!(body["temperature"], 0.0);
    }

    #[test]
    fn qwen_and_gemini_translate_explicit_thinking_controls() {
        let qwen = chat_body(
            ProviderKind::AlibabaQwen,
            "qwen3.8-max",
            &ModelSettings {
                openai_thinking_mode: OpenAiThinkingMode::Enabled,
                ..ModelSettings::default()
            },
            32_768,
        );
        assert_eq!(qwen["enable_thinking"], true);

        let gemini = chat_body(
            ProviderKind::GoogleGemini,
            "gemini-2.5-flash",
            &ModelSettings {
                openai_thinking_mode: OpenAiThinkingMode::Disabled,
                ..ModelSettings::default()
            },
            32_768,
        );
        assert_eq!(gemini["reasoning_effort"], "none");
    }

    #[test]
    fn structured_output_rejects_missing_or_extra_properties() {
        let schema = json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        });
        assert!(structured_result(&json!({"output_text": "{\"answer\":\"ok\"}"}), &schema).is_ok());
        let missing = structured_result(&json!({"output_text": "{}"}), &schema).unwrap_err();
        assert_eq!(
            missing.schema_diagnostic(),
            Some(("required_property_missing", "$.answer"))
        );
        let extra = structured_result(
            &json!({"output_text": "{\"answer\":\"ok\",\"extra\":true}"}),
            &schema,
        )
        .unwrap_err();
        assert_eq!(
            extra.schema_diagnostic(),
            Some(("unexpected_property", "$"))
        );
    }

    #[test]
    fn structured_output_repairs_only_an_unambiguous_single_array_envelope() {
        let schema = json!({
            "type": "object",
            "properties": {
                "memories": {
                    "type": "array",
                    "maxItems": 3,
                    "items": {
                        "type": "object",
                        "properties": {"evidence_quote": {"type": "string"}},
                        "required": ["evidence_quote"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["memories"],
            "additionalProperties": false
        });
        let strict_direct_item = structured_result(
            &json!({"output_text": "{\"evidence_quote\":\"exact\"}"}),
            &schema,
        )
        .unwrap_err();
        assert_eq!(
            strict_direct_item.schema_diagnostic(),
            Some(("required_property_missing", "$.memories"))
        );

        let direct_item = structured_result_with_envelope_repair(
            &json!({"output_text": "{\"evidence_quote\":\"exact\"}"}),
            &schema,
            true,
        )
        .unwrap();
        assert_eq!(
            direct_item.value,
            json!({"memories": [{"evidence_quote": "exact"}]})
        );
        let direct_array = structured_result_with_envelope_repair(
            &json!({"output_text": "[{\"evidence_quote\":\"exact\"}]"}),
            &schema,
            true,
        )
        .unwrap();
        assert_eq!(
            direct_array.value,
            json!({"memories": [{"evidence_quote": "exact"}]})
        );

        let unknown = structured_result_with_envelope_repair(
            &json!({"output_text": "{\"result\":\"not enough information\"}"}),
            &schema,
            true,
        )
        .unwrap_err();
        assert_eq!(
            unknown.schema_diagnostic(),
            Some(("required_property_missing", "$.memories"))
        );
        let empty =
            structured_result_with_envelope_repair(&json!({"output_text": "{}"}), &schema, true)
                .unwrap_err();
        assert_eq!(
            empty.schema_diagnostic(),
            Some(("required_property_missing", "$.memories"))
        );
    }

    #[test]
    fn structured_output_repairs_only_an_authorized_missing_string_from_existing_output() {
        let schema = json!({
            "type": "object",
            "properties": {
                "source_summary": {"type": "string"},
                "raw_memory": {"type": "string"}
            },
            "required": ["source_summary", "raw_memory"],
            "additionalProperties": false
        });
        let policy = [MissingRequiredStringFallback::new(
            "source_summary",
            "raw_memory",
        )];
        let repaired = structured_result_with_repairs(
            &json!({"output_text": r#"{"raw_memory":"durable decision"}"#}),
            &schema,
            false,
            &policy,
        )
        .unwrap();
        assert_eq!(repaired.value["source_summary"], "durable decision");
        assert_eq!(repaired.value["raw_memory"], "durable decision");

        let empty =
            structured_result_with_repairs(&json!({"output_text": "{}"}), &schema, false, &policy)
                .unwrap_err();
        assert_eq!(empty.code(), "provider_schema_invalid");
    }

    #[test]
    fn schema_subset_validates_nested_arrays() {
        let schema = json!({
            "type": "array",
            "items": {"type": "integer"}
        });
        assert!(validate_json_schema(&json!([1, 2, 3]), &schema).is_ok());
        let invalid = validate_json_schema(&json!([1, "2"]), &schema).unwrap_err();
        assert_eq!(invalid.schema_diagnostic(), Some(("type_mismatch", "$[1]")));

        let bounded = json!({
            "type": "array",
            "maxItems": 1,
            "items": {"type": ["string", "null"]}
        });
        assert!(validate_json_schema(&json!([null]), &bounded).is_ok());
        assert!(validate_json_schema(&json!(["ok"]), &bounded).is_ok());
        let too_long = validate_json_schema(&json!(["one", "two"]), &bounded).unwrap_err();
        assert_eq!(too_long.schema_diagnostic(), Some(("array_too_long", "$")));
    }

    #[test]
    fn structured_output_accepts_a_single_json_code_fence() {
        let schema = json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        });
        let result = structured_result(
            &json!({"choices": [{"message": {"content": "```JSON\n{\"answer\":\"ok\"}\n```"}, "finish_reason": "stop"}]}),
            &schema,
        )
        .unwrap();

        assert_eq!(result.value["answer"], "ok");
    }

    #[test]
    fn structured_output_reports_redacted_finish_reason_categories() {
        let schema = json!({"type": "object"});
        let truncated = structured_result(
            &json!({"choices": [{"message": {"content": "{\"answer\":"}, "finish_reason": "length"}]}),
            &schema,
        )
        .unwrap_err();
        assert_eq!(truncated.code(), "provider_output_truncated");

        let malformed = structured_result(
            &json!({"choices": [{"message": {"content": "not json"}, "finish_reason": "stop"}]}),
            &schema,
        )
        .unwrap_err();
        assert_eq!(malformed.code(), "provider_structured_json_invalid");

        let missing = structured_result(
            &json!({"choices": [{"message": {"content": null}, "finish_reason": "stop"}]}),
            &schema,
        )
        .unwrap_err();
        assert_eq!(missing.code(), "provider_final_content_missing");

        let incomplete = structured_result(
            &json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output_text": ""}),
            &schema,
        )
        .unwrap_err();
        assert_eq!(incomplete.code(), "provider_output_truncated");
    }
}
