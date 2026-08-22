//! Provider adapter contracts and wire-format translators.

use async_trait::async_trait;
use mcp_vault_auth::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::{
    AuthStyle, ProviderError, ProviderKind, ProviderMode, ProviderSettings, ProviderTransport,
    endpoint_url,
};

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
    /// Maximum generated token estimate.
    pub max_output_tokens: u32,
    /// Optional deterministic temperature.
    pub temperature: Option<f32>,
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
        settings: &ProviderSettings,
        secret: Option<&SecretString>,
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
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiCompatibleAdapter;

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
        _settings: &ProviderSettings,
        secret: Option<&SecretString>,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        let endpoint = endpoint_url(base_url, "responses")?;
        let body = json!({
            "model": request.model,
            "instructions": request.system,
            "input": request.user,
            "max_output_tokens": request.max_output_tokens,
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
                secret,
                AuthStyle::Bearer,
                &body,
            )
            .await?;
        structured_result(&response.body, &request.schema)
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
        ProviderKind::OpenAiCompatible
    }

    async fn generate_structured(
        &self,
        transport: &ProviderTransport,
        base_url: &Url,
        mode: ProviderMode,
        _settings: &ProviderSettings,
        secret: Option<&SecretString>,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        let endpoint = endpoint_url(base_url, "chat/completions")?;
        let mut body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": request.system},
                {"role": "user", "content": request.user}
            ],
            "max_tokens": request.max_output_tokens,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": request.schema_name,
                    "strict": true,
                    "schema": request.schema
                }
            }
        });
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        let response = transport
            .request_json(
                reqwest::Method::POST,
                &endpoint,
                mode,
                secret,
                AuthStyle::Bearer,
                &body,
            )
            .await?;
        structured_result(&response.body, &request.schema)
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
        _settings: &ProviderSettings,
        secret: Option<&SecretString>,
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
            "max_tokens": request.max_output_tokens,
            "messages": [{"role": "user", "content": request.user}]
        });
        let response = transport
            .request_json(
                reqwest::Method::POST,
                &endpoint,
                mode,
                secret,
                AuthStyle::Anthropic,
                &body,
            )
            .await?;
        structured_result(&response.body, &request.schema)
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
        _settings: &ProviderSettings,
        _secret: Option<&SecretString>,
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

fn structured_result(
    body: &Value,
    schema: &Value,
) -> Result<StructuredGenerationResult, ProviderError> {
    let text = extract_text(body)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|_| ProviderError::InvalidResponse("structured output is not JSON"))?;
    validate_json_schema(&value, schema)?;
    Ok(StructuredGenerationResult {
        value,
        model: body.get("model").and_then(Value::as_str).map(str::to_owned),
        usage: body.get("usage").cloned(),
    })
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
    Err(ProviderError::InvalidResponse(
        "structured text was not present",
    ))
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
            secret,
            AuthStyle::Bearer,
            &body,
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
            secret,
            AuthStyle::Bearer,
            &json!({}),
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
    let Some(schema_type) = schema.get("type").and_then(Value::as_str) else {
        return Ok(());
    };
    let valid_type = match schema_type {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if !valid_type {
        return Err(ProviderError::SchemaValidation);
    }
    if let Some(enums) = schema.get("enum").and_then(Value::as_array)
        && !enums.iter().any(|candidate| candidate == value)
    {
        return Err(ProviderError::SchemaValidation);
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        if let Some(required) = schema.get("required").and_then(Value::as_array)
            && required
                .iter()
                .any(|key| key.as_str().is_none_or(|key| !object.contains_key(key)))
        {
            return Err(ProviderError::SchemaValidation);
        }
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && object.keys().any(|key| !properties.contains_key(key))
        {
            return Err(ProviderError::SchemaValidation);
        }
        for (key, child_schema) in properties {
            if let Some(child) = object.get(key) {
                validate_json_schema(child, child_schema)?;
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for item in array {
            validate_json_schema(item, items)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extract_text, structured_result, validate_json_schema};
    use serde_json::json;

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
        assert!(structured_result(&json!({"output_text": "{}"}), &schema).is_err());
        assert!(
            structured_result(
                &json!({"output_text": "{\"answer\":\"ok\",\"extra\":true}"}),
                &schema
            )
            .is_err()
        );
    }

    #[test]
    fn schema_subset_validates_nested_arrays() {
        let schema = json!({
            "type": "array",
            "items": {"type": "integer"}
        });
        assert!(validate_json_schema(&json!([1, 2, 3]), &schema).is_ok());
        assert!(validate_json_schema(&json!([1, "2"]), &schema).is_err());
    }
}
