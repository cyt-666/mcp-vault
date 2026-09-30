//! OpenAI-compatible streamed structured-response aggregation.
//! Reasoning deltas are counted for activity only and are never retained.

use crate::{ProviderError, StrictFunctionCallIssue};
use serde_json::{Value, json};

const MAX_FUNCTION_ARGUMENT_BYTES: usize = 1024 * 1024;
const MAX_FUNCTION_NAME_BYTES: usize = 64;
const MAX_FUNCTION_ID_BYTES: usize = 128;
const MAX_FUNCTION_CALL_FRAGMENTS: usize = 16_384;

#[derive(Default)]
pub(crate) struct StreamResponseAggregator {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Value>,
    content: String,
    finish_reason: Option<String>,
    saw_done: bool,
    reasoning_bytes: usize,
    expected_tool_name: Option<String>,
    tool_name: String,
    tool_arguments: String,
    tool_call_id: Option<String>,
    saw_tool_call: bool,
    function_call_fragments: usize,
}

impl StreamResponseAggregator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn for_function_call(name: &str) -> Self {
        Self {
            expected_tool_name: Some(name.to_owned()),
            ..Self::default()
        }
    }

    fn strict_issue(
        &self,
        issue: StrictFunctionCallIssue,
        fallback: &'static str,
    ) -> ProviderError {
        if self.expected_tool_name.is_some() {
            ProviderError::StrictFunctionCallInvalid { issue }
        } else {
            ProviderError::InvalidResponse(fallback)
        }
    }

    pub(crate) fn push_data(&mut self, data: &str) -> Result<(), ProviderError> {
        if data == "[DONE]" {
            self.saw_done = true;
            return Ok(());
        }
        if self.saw_done {
            return Err(self.strict_issue(
                StrictFunctionCallIssue::InvalidStreamEvent,
                "stream data after done",
            ));
        }
        let value: Value = serde_json::from_str(data).map_err(|_| {
            self.strict_issue(
                StrictFunctionCallIssue::InvalidStreamEvent,
                "stream event is not JSON",
            )
        })?;
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            self.id = Some(id.to_owned());
        }
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            self.model = Some(model.to_owned());
        }
        if let Some(usage) = value.get("usage")
            && !usage.is_null()
        {
            self.usage = Some(usage.clone());
        }
        let Some(choices) = value.get("choices").and_then(Value::as_array) else {
            return Ok(());
        };
        if self.expected_tool_name.is_some() && choices.len() > 1 {
            return Err(self.strict_issue(
                StrictFunctionCallIssue::InvalidChoice,
                "multiple stream choices are unsupported",
            ));
        }
        for choice in choices {
            if self.finish_reason.is_some() {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::InvalidFinishSequence,
                    "stream data followed finish reason",
                ));
            }
            let index_value = choice.get("index").and_then(Value::as_u64);
            if self.expected_tool_name.is_some() && index_value.is_none() {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::InvalidChoice,
                    "stream choice index is missing",
                ));
            }
            let index = index_value.unwrap_or(0);
            if index != 0 {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::InvalidChoice,
                    "stream choice index is unsupported",
                ));
            }
            if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
                if let Some(content) = delta.get("content")
                    && !content.is_null()
                {
                    self.content.push_str(content.as_str().ok_or_else(|| {
                        self.strict_issue(
                            StrictFunctionCallIssue::InvalidToolDelta,
                            "stream content is not a string",
                        )
                    })?);
                }
                if let Some(reasoning) = delta.get("reasoning_content")
                    && !reasoning.is_null()
                {
                    let reasoning = reasoning.as_str().ok_or_else(|| {
                        self.strict_issue(
                            StrictFunctionCallIssue::InvalidStreamEvent,
                            "stream reasoning is not a string",
                        )
                    })?;
                    self.reasoning_bytes = self.reasoning_bytes.saturating_add(reasoning.len());
                }
                if let Some(tool_calls) = delta.get("tool_calls") {
                    let tool_calls = tool_calls.as_array().ok_or_else(|| {
                        self.strict_issue(
                            StrictFunctionCallIssue::InvalidToolDelta,
                            "stream tool calls are not an array",
                        )
                    })?;
                    if self.expected_tool_name.is_none() && !tool_calls.is_empty() {
                        return Err(ProviderError::InvalidResponse(
                            "unexpected provider tool call",
                        ));
                    }
                    for call in tool_calls {
                        self.function_call_fragments =
                            self.function_call_fragments.saturating_add(1);
                        if self.function_call_fragments > MAX_FUNCTION_CALL_FRAGMENTS {
                            return Err(ProviderError::ResponseTooLarge);
                        }
                        let index = call.get("index").and_then(Value::as_u64).ok_or_else(|| {
                            self.strict_issue(
                                StrictFunctionCallIssue::InvalidToolDelta,
                                "stream tool call index is missing",
                            )
                        })?;
                        if index != 0 {
                            return Err(self.strict_issue(
                                StrictFunctionCallIssue::MultipleToolCalls,
                                "multiple provider tool calls are unsupported",
                            ));
                        }
                        if let Some(call_type) = call.get("type")
                            && call_type.as_str() != Some("function")
                        {
                            return Err(self.strict_issue(
                                StrictFunctionCallIssue::InvalidToolDelta,
                                "provider tool call type is unsupported",
                            ));
                        }
                        if let Some(id) = call.get("id").and_then(Value::as_str) {
                            if id.len() > MAX_FUNCTION_ID_BYTES {
                                return Err(ProviderError::ResponseTooLarge);
                            }
                            if self
                                .tool_call_id
                                .as_deref()
                                .is_some_and(|current| current != id)
                            {
                                return Err(self.strict_issue(
                                    StrictFunctionCallIssue::InconsistentToolCallId,
                                    "multiple provider tool call IDs are unsupported",
                                ));
                            }
                            self.tool_call_id = Some(id.to_owned());
                        }
                        let function =
                            call.get("function")
                                .and_then(Value::as_object)
                                .ok_or_else(|| {
                                    self.strict_issue(
                                        StrictFunctionCallIssue::InvalidToolDelta,
                                        "provider function payload is missing",
                                    )
                                })?;
                        if let Some(name) = function.get("name") {
                            self.tool_name.push_str(name.as_str().ok_or_else(|| {
                                self.strict_issue(
                                    StrictFunctionCallIssue::InvalidToolDelta,
                                    "provider function name is invalid",
                                )
                            })?);
                            if self.tool_name.len() > MAX_FUNCTION_NAME_BYTES {
                                return Err(ProviderError::ResponseTooLarge);
                            }
                        }
                        if let Some(arguments) = function.get("arguments") {
                            self.tool_arguments
                                .push_str(arguments.as_str().ok_or_else(|| {
                                    self.strict_issue(
                                        StrictFunctionCallIssue::InvalidToolDelta,
                                        "provider function arguments are invalid",
                                    )
                                })?);
                            if self.tool_arguments.len() > MAX_FUNCTION_ARGUMENT_BYTES {
                                return Err(ProviderError::ResponseTooLarge);
                            }
                        }
                        self.saw_tool_call = true;
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str)
                && self.finish_reason.replace(reason.to_owned()).is_some()
            {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::InvalidFinishSequence,
                    "stream finish reason was repeated",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<Value, ProviderError> {
        let state = std::mem::take(self);
        state.finish_inner()
    }

    fn finish_inner(self) -> Result<Value, ProviderError> {
        let output_content = if let Some(expected_tool_name) = self.expected_tool_name.as_deref() {
            if !self.saw_tool_call {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::NoToolCall,
                    "provider did not return a completed function tool call",
                ));
            }
            if !self.content.trim().is_empty() {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::MixedMessageContent,
                    "provider mixed tool call and message content",
                ));
            }
            if self.finish_reason.as_deref() != Some("tool_calls") {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::WrongFinishReason,
                    "provider did not return a completed function tool call",
                ));
            }
            if self.tool_name != expected_tool_name {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::WrongToolName,
                    "provider function tool name does not match",
                ));
            }
            if self.tool_arguments.is_empty() {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::MissingArguments,
                    "provider function arguments are missing",
                ));
            }
            if self.tool_call_id.is_none() {
                return Err(self.strict_issue(
                    StrictFunctionCallIssue::MissingToolCallId,
                    "provider function call ID is missing",
                ));
            }
            self.tool_arguments
        } else {
            if self.content.is_empty() {
                return Err(ProviderError::InvalidResponse(
                    "provider final content is missing",
                ));
            }
            self.content
        };
        match self.finish_reason.as_deref() {
            Some("stop") => {}
            Some("length") => {
                return Err(ProviderError::InvalidResponse(
                    "provider output reached token limit",
                ));
            }
            Some("content_filter") => {
                return Err(ProviderError::InvalidResponse(
                    "provider output was filtered",
                ));
            }
            Some("repetition_truncation") => {
                return Err(ProviderError::InvalidResponse(
                    "provider output repetition was truncated",
                ));
            }
            Some("tool_calls") if self.expected_tool_name.is_some() => {}
            Some("tool_calls") => {
                return Err(ProviderError::InvalidResponse(
                    "provider tool calls are unsupported",
                ));
            }
            Some(_) | None => {
                return Err(ProviderError::InvalidResponse(
                    "stream finish reason is invalid",
                ));
            }
        }
        let mut body =
            json!({"choices":[{"message":{"content":output_content},"finish_reason":"stop"}]});
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

#[cfg(test)]
mod tests {
    use super::*;

    fn strict_issue(error: ProviderError) -> StrictFunctionCallIssue {
        assert_eq!(error.code(), "provider_response_invalid");
        assert!(!error.to_string().contains("arguments"));
        error.strict_function_call_issue().unwrap()
    }
    #[test]
    fn aggregates_content_and_usage_without_retaining_reasoning() {
        let mut a = StreamResponseAggregator::new();
        a.push_data(r#"{"id":"x","model":"m","choices":[{"index":0,"delta":{"reasoning_content":"private","content":"{\"a\":"}}]}"#).unwrap();
        a.push_data(
            r#"{"choices":[{"index":0,"delta":{"content":"1}"}}],"usage":{"prompt_tokens":2}}"#,
        )
        .unwrap();
        a.push_data(r#"{"choices":[{"index":0,"finish_reason":"stop","delta":{}}]}"#)
            .unwrap();
        a.push_data("[DONE]").unwrap();
        let body = a.finish().unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "{\"a\":1}");
        assert_eq!(body["usage"]["prompt_tokens"], 2);
        assert!(!body.to_string().contains("private"));
    }
    #[test]
    fn rejects_nonstop_or_eof_and_accepts_usage_only() {
        let mut a = StreamResponseAggregator::new();
        a.push_data(r#"{"usage":{"completion_tokens":3},"choices":[]}"#)
            .unwrap();
        assert!(a.finish().is_err());
        let mut b = StreamResponseAggregator::new();
        b.push_data(r#"{"choices":[{"index":0,"finish_reason":"length","delta":{}}]}"#)
            .unwrap();
        b.push_data("[DONE]").unwrap();
        assert!(b.finish().is_err());
    }

    #[test]
    fn stop_eof_is_success_but_done_without_stop_is_not() {
        let mut eof = StreamResponseAggregator::new();
        eof.push_data(r#"{"choices":[{"index":0,"delta":{"content":"{}"}}]}"#)
            .unwrap();
        eof.push_data(r#"{"choices":[{"index":0,"finish_reason":"stop","delta":{}}]}"#)
            .unwrap();
        assert!(eof.finish().is_ok());

        let mut done = StreamResponseAggregator::new();
        done.push_data(r#"{"choices":[{"index":0,"delta":{"content":"{}"}}]}"#)
            .unwrap();
        done.push_data(r#"{"choices":[{"index":0,"finish_reason":"stop","delta":{}}]}"#)
            .unwrap();
        done.push_data("[DONE]").unwrap();
        assert!(done.finish().is_ok());

        let mut only_done = StreamResponseAggregator::new();
        only_done.push_data("[DONE]").unwrap();
        assert!(only_done.finish().is_err());
    }

    #[test]
    fn strict_function_call_aggregates_fragmented_name_and_arguments() {
        let mut a = StreamResponseAggregator::for_function_call("semantic_memory_observation");
        a.push_data(
            &json!({"choices":[{"index":0,"delta":{"content":null,"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"semantic_memory_","arguments":"{\"ok\":"}}]}}]}).to_string(),
        )
        .unwrap();
        a.push_data(
            &json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"observation","arguments":"true}"}}]}}]}).to_string(),
        )
        .unwrap();
        a.push_data(
            &json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}).to_string(),
        )
        .unwrap();
        a.push_data("[DONE]").unwrap();
        let body = a.finish().unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "{\"ok\":true}");
        assert!(!body.to_string().contains("call-1"));
    }

    #[test]
    fn strict_function_call_rejects_fallback_wrong_multi_mixed_and_truncated_responses() {
        let content_fallback = || {
            let mut a = StreamResponseAggregator::for_function_call("expected");
            a.push_data(
                r#"{"choices":[{"index":0,"delta":{"content":"{}"},"finish_reason":"stop"}]}"#,
            )
            .unwrap();
            a.finish().is_err()
        };
        assert!(content_fallback());

        let mut wrong_name = StreamResponseAggregator::for_function_call("expected");
        wrong_name.push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"other","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert!(wrong_name.finish().is_err());

        let mut multiple = StreamResponseAggregator::for_function_call("expected");
        assert!(multiple.push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"one","type":"function","function":{"name":"expected","arguments":"{}"}},{"index":1,"id":"two","type":"function","function":{"name":"expected","arguments":"{}"}}]}}]}"#).is_err());

        let mut mixed = StreamResponseAggregator::for_function_call("expected");
        mixed.push_data(r#"{"choices":[{"index":0,"delta":{"content":"text","tool_calls":[{"index":0,"id":"one","type":"function","function":{"name":"expected","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert!(mixed.finish().is_err());

        let mut truncated = StreamResponseAggregator::for_function_call("expected");
        truncated.push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"one","type":"function","function":{"name":"expected","arguments":"{"}}]},"finish_reason":"length"}]}"#).unwrap();
        assert!(truncated.finish().is_err());
    }

    #[test]
    fn strict_function_call_reports_only_bounded_protocol_categories() {
        let mut no_tool = StreamResponseAggregator::for_function_call("expected");
        no_tool
            .push_data(r#"{"choices":[{"index":0,"delta":{"content":"fallback"},"finish_reason":"stop"}]}"#)
            .unwrap();
        assert_eq!(
            strict_issue(no_tool.finish().unwrap_err()),
            StrictFunctionCallIssue::NoToolCall
        );

        let mut wrong_finish = StreamResponseAggregator::for_function_call("expected");
        wrong_finish
            .push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call","type":"function","function":{"name":"expected","arguments":"{}"}}]},"finish_reason":"stop"}]}"#)
            .unwrap();
        assert_eq!(
            strict_issue(wrong_finish.finish().unwrap_err()),
            StrictFunctionCallIssue::WrongFinishReason
        );

        let mut missing_id = StreamResponseAggregator::for_function_call("expected");
        missing_id
            .push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"type":"function","function":{"name":"expected","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        assert_eq!(
            strict_issue(missing_id.finish().unwrap_err()),
            StrictFunctionCallIssue::MissingToolCallId
        );

        let mut invalid_choice = StreamResponseAggregator::for_function_call("expected");
        let choice_error = invalid_choice
            .push_data(r#"{"choices":[{"index":1,"delta":{}}]}"#)
            .unwrap_err();
        assert_eq!(
            strict_issue(choice_error),
            StrictFunctionCallIssue::InvalidChoice
        );

        let mut mixed = StreamResponseAggregator::for_function_call("expected");
        mixed
            .push_data(r#"{"choices":[{"index":0,"delta":{"content":"text","tool_calls":[{"index":0,"id":"call","type":"function","function":{"name":"expected","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        assert_eq!(
            strict_issue(mixed.finish().unwrap_err()),
            StrictFunctionCallIssue::MixedMessageContent
        );

        let mut invalid_type = StreamResponseAggregator::for_function_call("expected");
        assert_eq!(
            strict_issue(
                invalid_type
                    .push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"type":7,"id":"call","function":{"name":"expected","arguments":"{}"}}]}}]}"#)
                    .unwrap_err()
            ),
            StrictFunctionCallIssue::InvalidToolDelta
        );
    }

    #[test]
    fn strict_function_call_rejects_changed_id_choice_and_duplicate_finish() {
        let mut changed_id = StreamResponseAggregator::for_function_call("expected");
        changed_id.push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"one","type":"function","function":{"name":"expected","arguments":"{"}}]}}]}"#).unwrap();
        assert!(changed_id.push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"two","function":{"arguments":"}"}}]}}]}"#).is_err());

        let mut missing_id = StreamResponseAggregator::for_function_call("expected");
        missing_id.push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"type":"function","function":{"name":"expected","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert!(missing_id.finish().is_err());

        let mut cross_choice = StreamResponseAggregator::for_function_call("expected");
        assert!(
            cross_choice
                .push_data(r#"{"choices":[{"index":0,"delta":{}},{"index":1,"delta":{}}]}"#)
                .is_err()
        );

        let mut finish = StreamResponseAggregator::for_function_call("expected");
        finish.push_data(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"one","type":"function","function":{"name":"expected","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert!(
            finish
                .push_data(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#)
                .is_err()
        );
    }

    #[test]
    fn strict_function_call_caps_argument_bytes_and_fragment_count() {
        let huge = "x".repeat(MAX_FUNCTION_ARGUMENT_BYTES + 1);
        let mut bytes = StreamResponseAggregator::for_function_call("expected");
        let event = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"expected","arguments":huge}}]}}]}).to_string();
        assert!(bytes.push_data(&event).is_err());

        let mut fragments = StreamResponseAggregator::for_function_call("expected");
        let event = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":""}}]}}]}"#;
        for _ in 0..MAX_FUNCTION_CALL_FRAGMENTS {
            fragments.push_data(event).unwrap();
        }
        assert!(fragments.push_data(event).is_err());
    }

    #[test]
    fn null_reasoning_is_ignored_but_non_string_reasoning_fails() {
        let mut valid = StreamResponseAggregator::new();
        valid
            .push_data(
                r#"{"choices":[{"index":0,"delta":{"reasoning_content":null,"content":"{}"}}]}"#,
            )
            .unwrap();
        valid
            .push_data(r#"{"choices":[{"index":0,"finish_reason":"stop","delta":{}}]}"#)
            .unwrap();
        assert!(valid.finish().is_ok());
        let mut invalid = StreamResponseAggregator::new();
        assert!(
            invalid
                .push_data(r#"{"choices":[{"index":0,"delta":{"reasoning_content":3}}]}"#)
                .is_err()
        );
    }
}
