//! Native streaming response aggregators for Responses and Anthropic Messages.
//! They return the ordinary adapter response shape after terminal validation.

use crate::ProviderError;
use serde_json::{Value, json};

#[derive(Default)]
pub(crate) struct ResponsesStreamAggregator {
    completed: Option<Value>,
    text: String,
}

impl ResponsesStreamAggregator {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    pub(crate) fn push_event(&mut self, event: &str, data: &Value) -> Result<(), ProviderError> {
        match event {
            "response.failed" | "response.incomplete" | "error" => Err(
                ProviderError::InvalidResponse("native stream response failed"),
            ),
            "response.output_text.delta" => {
                if let Some(text) = data.get("delta").and_then(Value::as_str) {
                    self.text.push_str(text);
                }
                Ok(())
            }
            "response.completed" => {
                self.completed = data.get("response").cloned().or_else(|| Some(data.clone()));
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn finish(&mut self) -> Result<Value, ProviderError> {
        let state = std::mem::take(self);
        state.finish_inner()
    }

    fn finish_inner(self) -> Result<Value, ProviderError> {
        let mut response = self.completed.ok_or(ProviderError::InvalidResponse(
            "native stream ended without response.completed",
        ))?;
        if response
            .get("output_text")
            .and_then(Value::as_str)
            .is_none()
            && !self.text.is_empty()
        {
            response["output_text"] = Value::String(self.text);
        }
        if response
            .get("output_text")
            .and_then(Value::as_str)
            .is_none()
        {
            return Err(ProviderError::InvalidResponse(
                "provider final content is missing",
            ));
        }
        if response
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status != "completed")
        {
            return Err(ProviderError::InvalidResponse(
                "native stream response incomplete",
            ));
        }
        Ok(response)
    }
}

#[derive(Default)]
pub(crate) struct AnthropicStreamAggregator {
    id: Option<String>,
    model: Option<String>,
    content: String,
    usage: Option<Value>,
    stopped: bool,
    stop_reason: Option<String>,
    started: bool,
    open_blocks: std::collections::BTreeMap<u64, String>,
    closed_blocks: std::collections::BTreeSet<u64>,
}

impl AnthropicStreamAggregator {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    pub(crate) fn push_event(&mut self, event: &str, data: &Value) -> Result<(), ProviderError> {
        match event {
            "error" => Err(ProviderError::InvalidResponse("anthropic stream error")),
            "message_start" => {
                if self.started {
                    return Err(ProviderError::InvalidResponse(
                        "duplicate anthropic message_start",
                    ));
                }
                self.started = true;
                let message = data.get("message").unwrap_or(data);
                self.id = message.get("id").and_then(Value::as_str).map(str::to_owned);
                self.model = message
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(usage) = message.get("usage") {
                    self.usage = Some(usage.clone());
                }
                Ok(())
            }
            "content_block_start" => {
                let index = data.get("index").and_then(Value::as_u64).ok_or(
                    ProviderError::InvalidResponse("anthropic block index is missing"),
                )?;
                if self.open_blocks.contains_key(&index) || self.closed_blocks.contains(&index) {
                    return Err(ProviderError::InvalidResponse(
                        "anthropic block index is reused",
                    ));
                }
                let kind = data
                    .get("content_block")
                    .and_then(|v| v.get("type"))
                    .and_then(Value::as_str)
                    .ok_or(ProviderError::InvalidResponse(
                        "anthropic block type is missing",
                    ))?;
                if !matches!(kind, "text" | "thinking") {
                    return Err(ProviderError::InvalidResponse(
                        "anthropic block type is unsupported",
                    ));
                }
                self.open_blocks.insert(index, kind.to_owned());
                Ok(())
            }
            "content_block_delta" => {
                let index = data.get("index").and_then(Value::as_u64).ok_or(
                    ProviderError::InvalidResponse("anthropic block index is missing"),
                )?;
                let kind = self
                    .open_blocks
                    .get(&index)
                    .ok_or(ProviderError::InvalidResponse(
                        "anthropic block index is unknown",
                    ))?;
                let delta = data.get("delta").unwrap_or(data);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if kind != "text" {
                            return Err(ProviderError::InvalidResponse(
                                "anthropic text delta targets non-text block",
                            ));
                        }
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            self.content.push_str(text);
                        }
                        Ok(())
                    }
                    Some("thinking_delta") | Some("signature_delta") => Ok(()),
                    _ => Ok(()),
                }
            }
            "content_block_stop" => {
                let index = data.get("index").and_then(Value::as_u64).ok_or(
                    ProviderError::InvalidResponse("anthropic block index is missing"),
                )?;
                if self.open_blocks.remove(&index).is_none() {
                    return Err(ProviderError::InvalidResponse(
                        "anthropic block stop is unknown",
                    ));
                }
                self.closed_blocks.insert(index);
                Ok(())
            }
            "message_delta" => {
                let delta = data.get("delta").unwrap_or(data);
                self.stop_reason = delta
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(usage) = data.get("usage") {
                    merge_usage(&mut self.usage, usage);
                }
                Ok(())
            }
            "message_stop" => {
                if self.stopped || !self.open_blocks.is_empty() {
                    return Err(ProviderError::InvalidResponse(
                        "anthropic message stopped with open block",
                    ));
                }
                self.stopped = true;
                Ok(())
            }
            "ping" => Ok(()),
            _ => Ok(()),
        }
    }
    pub(crate) fn finish(&mut self) -> Result<Value, ProviderError> {
        let state = std::mem::take(self);
        state.finish_inner()
    }

    fn finish_inner(self) -> Result<Value, ProviderError> {
        if !self.started {
            return Err(ProviderError::InvalidResponse(
                "anthropic stream missing message_start",
            ));
        }
        if !self.stopped {
            return Err(ProviderError::InvalidResponse(
                "anthropic stream ended without message_stop",
            ));
        }
        if self.stop_reason.as_deref() != Some("end_turn") {
            return Err(ProviderError::InvalidResponse(
                "anthropic stream stop reason is unsupported",
            ));
        }
        if self.content.is_empty() {
            return Err(ProviderError::InvalidResponse(
                "provider final content is missing",
            ));
        }
        let mut body =
            json!({"content":[{"type":"text","text":self.content}],"stop_reason":"end_turn"});
        if let Some(id) = self.id {
            body["id"] = Value::String(id);
        }
        if let Some(model) = self.model {
            body["model"] = Value::String(model);
        }
        if let Some(usage) = self.usage {
            body["usage"] = usage;
        }
        Ok(body)
    }
}

fn merge_usage(target: &mut Option<Value>, update: &Value) {
    match (target.as_mut(), update) {
        (Some(Value::Object(existing)), Value::Object(new_values)) => {
            for (key, value) in new_values {
                existing.insert(key.clone(), value.clone());
            }
        }
        _ => *target = Some(update.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses_requires_completed_and_preserves_usage_shape() {
        let mut a = ResponsesStreamAggregator::new();
        a.push_event(
            "response.output_text.delta",
            &json!({"delta":"{\"ok\":true}"}),
        )
        .unwrap();
        a.push_event("response.completed", &json!({"response":{"status":"completed","output_text":"{\"ok\":true}","usage":{"input_tokens":1}}})).unwrap();
        assert_eq!(a.finish().unwrap()["output_text"], "{\"ok\":true}");
    }
    #[test]
    fn anthropic_aggregates_text_and_ignores_thinking() {
        let mut a = AnthropicStreamAggregator::new();
        a.push_event(
            "message_start",
            &json!({"message":{"id":"m","model":"x","usage":{"input_tokens":2}}}),
        )
        .unwrap();
        a.push_event(
            "content_block_start",
            &json!({"index":0,"content_block":{"type":"thinking"}}),
        )
        .unwrap();
        a.push_event(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"thinking_delta","thinking":"private"}}),
        )
        .unwrap();
        a.push_event("content_block_stop", &json!({"index":0}))
            .unwrap();
        a.push_event(
            "content_block_start",
            &json!({"index":1,"content_block":{"type":"text"}}),
        )
        .unwrap();
        a.push_event(
            "content_block_delta",
            &json!({"index":1,"delta":{"type":"text_delta","text":"{}"}}),
        )
        .unwrap();
        a.push_event("content_block_stop", &json!({"index":1}))
            .unwrap();
        a.push_event(
            "message_delta",
            &json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        )
        .unwrap();
        a.push_event("message_stop", &json!({})).unwrap();
        let out = a.finish().unwrap();
        assert_eq!(out["content"][0]["text"], "{}");
        assert!(!out.to_string().contains("private"));
    }

    #[test]
    fn anthropic_rejects_unknown_or_open_blocks_at_stop() {
        let mut unknown = AnthropicStreamAggregator::new();
        unknown
            .push_event("message_start", &json!({"message":{}}))
            .unwrap();
        assert!(
            unknown
                .push_event(
                    "content_block_delta",
                    &json!({"index":9,"delta":{"type":"text_delta","text":"x"}})
                )
                .is_err()
        );
        let mut open = AnthropicStreamAggregator::new();
        open.push_event("message_start", &json!({"message":{}}))
            .unwrap();
        open.push_event(
            "content_block_start",
            &json!({"index":0,"content_block":{"type":"text"}}),
        )
        .unwrap();
        assert!(open.push_event("message_stop", &json!({})).is_err());
    }
}
