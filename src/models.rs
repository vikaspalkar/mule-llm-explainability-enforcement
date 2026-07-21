// Copyright 2025 Salesforce, Inc. All rights reserved.

//! LLM API protocol data structures.
//!
//! Supports:
//! - OpenAI Chat Completions API (`/v1/chat/completions`)
//! - OpenAI Responses API (`/v1/responses`) — new Responses API with `input` field
//! - OpenAI Completions API (`/v1/completions`) — legacy `prompt` field
//! - Anthropic Messages API (`/v1/messages`)
//! - Any OpenAI-compatible endpoint

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Detected LLM API format from the request body.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmApiFormat {
    /// OpenAI Chat Completions: has `messages` array with role/content
    ChatCompletions,
    /// OpenAI Responses API (new, 2025+): has `input` field (string or array)
    ResponsesApi,
    /// OpenAI Completions (legacy): has `prompt` field
    Completions,
    /// Anthropic Messages: has `messages` array + optional top-level `system` string
    AnthropicMessages,
    /// Unknown — not a recognized LLM request
    Unknown,
}

/// Represents a single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    /// Content can be a plain string or an array of content parts.
    pub content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// Create a system message with the given text.
    pub fn system(text: String) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(Value::String(text)),
            name: None,
        }
    }

    /// Extract all text from this message's content.
    pub fn extract_text(&self) -> String {
        match &self.content {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| {
                    let is_text = p
                        .get("type")
                        .and_then(|t| t.as_str())
                        .map(|t| t == "text")
                        .unwrap_or(false);
                    if is_text {
                        p.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        }
    }
}

/// Detects LLM API format from a raw request body value.
///
/// Detection order (most specific first):
/// 1. Anthropic: `messages` array + claude model OR top-level `system` string
/// 2. Chat Completions: `messages` array
/// 3. Responses API: `input` field (string or array) — OpenAI Responses API (2025+)
/// 4. Completions (legacy): `prompt` field
/// 5. Unknown
pub fn detect_llm_format(body: &Value) -> LlmApiFormat {
    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or("");
    let has_messages = body.get("messages").and_then(|m| m.as_array()).is_some();
    let has_prompt = body.get("prompt").is_some();
    let has_input = body.get("input").is_some(); // OpenAI Responses API
    let has_top_level_system = body
        .get("system")
        .map(|s| s.is_string() || s.is_array())
        .unwrap_or(false);

    // Anthropic: messages array AND (claude model OR top-level system field)
    if has_messages && (model.starts_with("claude") || has_top_level_system) {
        return LlmApiFormat::AnthropicMessages;
    }

    // OpenAI Chat Completions: messages array
    if has_messages {
        return LlmApiFormat::ChatCompletions;
    }

    // OpenAI Responses API (2025+): input field (no messages array)
    // The Responses API uses "input" instead of "messages"
    if has_input {
        return LlmApiFormat::ResponsesApi;
    }

    // OpenAI Completions (legacy): prompt field
    if has_prompt {
        return LlmApiFormat::Completions;
    }

    LlmApiFormat::Unknown
}

/// Extracts all user-visible text from an LLM request body for keyword matching / audit preview.
pub fn extract_input_text_from_request(body: &Value) -> String {
    let mut parts = Vec::new();

    // Chat / Anthropic: messages[*] where role == "user" or "human"
    if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role == "user" || role == "human" {
                let text = extract_content_value(msg.get("content"));
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
    }

    // OpenAI Responses API: input field
    if let Some(input) = body.get("input") {
        match input {
            Value::String(s) => parts.push(s.clone()),
            Value::Array(arr) => {
                for item in arr {
                    let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    if role == "user" || role.is_empty() {
                        let text = extract_content_value(item.get("content"));
                        if !text.is_empty() {
                            parts.push(text);
                        } else if let Some(s) = item.as_str() {
                            parts.push(s.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Completions: prompt field
    if let Some(prompt) = body.get("prompt") {
        match prompt {
            Value::String(s) => parts.push(s.clone()),
            Value::Array(arr) => {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        parts.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    parts.join(" ")
}

/// Extracts the full text content from an LLM API response.
///
/// Supports:
/// - OpenAI Chat Completions: `choices[*].message.content`
/// - OpenAI Responses API: `output[*].content[*].text` where type == "output_text"
/// - OpenAI Completions (legacy): `choices[*].text`
/// - Anthropic Messages: `content[*].text`
pub fn extract_text_from_llm_response(response: &Value) -> String {
    let mut texts = Vec::new();

    // ── OpenAI Responses API: output[*].content[*].text ──────────────────────
    if let Some(output) = response.get("output").and_then(|o| o.as_array()) {
        for item in output {
            // output items with type == "message" contain content blocks
            if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                for block in content {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if block_type == "output_text" || block_type == "text" {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                texts.push(text.to_string());
                            }
                        }
                    }
                }
            }
            // Some Responses API items have text directly
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    texts.push(text.to_string());
                }
            }
        }
    }

    // ── OpenAI Chat Completions + Completions: choices[*] ────────────────────
    if texts.is_empty() {
        if let Some(choices) = response.get("choices").and_then(|c| c.as_array()) {
            for choice in choices {
                // Chat Completions: choices[*].message.content
                if let Some(message) = choice.get("message") {
                    let text = extract_content_value(message.get("content"));
                    if !text.is_empty() {
                        texts.push(text);
                    }
                }
                // Completions (legacy): choices[*].text
                if let Some(text) = choice.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        texts.push(text.to_string());
                    }
                }
            }
        }
    }

    // ── Anthropic Messages: content[*] ───────────────────────────────────────
    if texts.is_empty() {
        if let Some(content) = response.get("content") {
            match content {
                Value::Array(parts) => {
                    for part in parts {
                        let is_text = part
                            .get("type")
                            .and_then(|t| t.as_str())
                            .map(|t| t == "text")
                            .unwrap_or(false);
                        if is_text {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                if !text.is_empty() {
                                    texts.push(text.to_string());
                                }
                            }
                        }
                    }
                }
                Value::String(s) if !s.is_empty() => {
                    texts.push(s.clone());
                }
                _ => {}
            }
        }
    }

    texts.join("\n")
}

/// Returns true if the LLM response is a final (non-streaming) response.
pub fn is_final_llm_response(response: &Value) -> bool {
    // OpenAI Responses API: `status` field
    // "completed" = final, "in_progress" = streaming, "failed" = error
    if let Some(status) = response.get("status").and_then(|s| s.as_str()) {
        return status == "completed" || status == "failed";
    }

    // OpenAI Chat Completions: choices[0].finish_reason
    if let Some(finish_reason) = response
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("finish_reason"))
    {
        return !finish_reason.is_null();
    }

    // Anthropic: stop_reason
    if let Some(stop_reason) = response.get("stop_reason") {
        return !stop_reason.is_null();
    }

    // No finish indicator — treat as final
    true
}

/// Checks whether the response represents an error from the LLM provider.
pub fn is_llm_error_response(response: &Value) -> bool {
    // OpenAI error format: {"error": {...}}
    if response.get("error").is_some() {
        return true;
    }
    // Anthropic error format: {"type": "error", "error": {...}}
    if response.get("type").and_then(|t| t.as_str()) == Some("error") {
        return true;
    }
    // OpenAI Responses API failed status
    if response.get("status").and_then(|s| s.as_str()) == Some("failed") {
        return true;
    }
    false
}

/// Helper: extract text from an `Option<&Value>` that may be a string or array of content parts.
pub fn extract_content_value(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                let is_text = p
                    .get("type")
                    .and_then(|t| t.as_str())
                    .map(|t| t == "text")
                    .unwrap_or(false);
                if is_text {
                    p.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Error response body for blocked LLM responses (OpenAI error format).
#[derive(Debug, Clone, Serialize)]
pub struct LlmErrorResponse {
    pub error: LlmErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: String,
    pub data: Option<Value>,
}

impl LlmErrorResponse {
    pub fn compliance_error(message: String, data: Value) -> Self {
        Self {
            error: LlmErrorDetail {
                message,
                error_type: "explainability_compliance_error".to_string(),
                code: "explainability_non_compliant".to_string(),
                data: Some(data),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_detect_responses_api_string_input() {
        let body = json!({"model": "gpt-4o-mini", "input": "What documents are needed?"});
        assert_eq!(detect_llm_format(&body), LlmApiFormat::ResponsesApi);
    }

    #[test]
    fn test_detect_responses_api_array_input() {
        let body = json!({"model": "gpt-4o-mini", "input": [{"role": "user", "content": "Hello"}]});
        assert_eq!(detect_llm_format(&body), LlmApiFormat::ResponsesApi);
    }

    #[test]
    fn test_detect_openai_chat_format() {
        let body = json!({"model": "gpt-4o", "messages": [{"role": "user", "content": "hello"}]});
        assert_eq!(detect_llm_format(&body), LlmApiFormat::ChatCompletions);
    }

    #[test]
    fn test_detect_openai_completions_format() {
        let body = json!({"model": "gpt-3.5-turbo-instruct", "prompt": "Once upon a time"});
        assert_eq!(detect_llm_format(&body), LlmApiFormat::Completions);
    }

    #[test]
    fn test_detect_anthropic_claude_model() {
        let body = json!({"model": "claude-3-5-sonnet-20241022", "messages": [{"role": "user", "content": "hello"}], "max_tokens": 1024});
        assert_eq!(detect_llm_format(&body), LlmApiFormat::AnthropicMessages);
    }

    #[test]
    fn test_detect_anthropic_system_field() {
        let body = json!({"model": "some-model", "system": "You are helpful.", "messages": [{"role": "user", "content": "hello"}]});
        assert_eq!(detect_llm_format(&body), LlmApiFormat::AnthropicMessages);
    }

    #[test]
    fn test_detect_unknown_format() {
        let body = json!({"key": "value"});
        assert_eq!(detect_llm_format(&body), LlmApiFormat::Unknown);
    }

    #[test]
    fn test_extract_text_from_responses_api_response() {
        // OpenAI Responses API response format
        let response = json!({
            "id": "resp_abc",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "Here is the answer.\n\n```json\n{\"explainability_metadata\": {\"decision_outcome\": \"approved\"}}\n```"}]
            }],
            "status": "completed"
        });
        let text = extract_text_from_llm_response(&response);
        assert!(text.contains("Here is the answer"));
        assert!(text.contains("explainability_metadata"));
    }

    #[test]
    fn test_extract_text_from_chat_completion_response() {
        let response = json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello world"}, "finish_reason": "stop"}]
        });
        assert_eq!(extract_text_from_llm_response(&response), "Hello world");
    }

    #[test]
    fn test_extract_text_from_anthropic_response() {
        let response = json!({
            "content": [{"type": "text", "text": "Hi there!"}],
            "stop_reason": "end_turn"
        });
        assert_eq!(extract_text_from_llm_response(&response), "Hi there!");
    }

    #[test]
    fn test_is_final_responses_api_completed() {
        let response = json!({"status": "completed", "output": []});
        assert!(is_final_llm_response(&response));
    }

    #[test]
    fn test_is_not_final_responses_api_in_progress() {
        let response = json!({"status": "in_progress", "output": []});
        assert!(!is_final_llm_response(&response));
    }

    #[test]
    fn test_is_final_response_openai() {
        let response = json!({"choices": [{"finish_reason": "stop", "message": {"content": "hi"}}]});
        assert!(is_final_llm_response(&response));
    }

    #[test]
    fn test_is_streaming_chunk_openai() {
        let response = json!({"choices": [{"finish_reason": null, "delta": {"content": "hi"}}]});
        assert!(!is_final_llm_response(&response));
    }

    #[test]
    fn test_extract_input_text_responses_api_string() {
        let body = json!({"model": "gpt-4o-mini", "input": "What documents do I need?"});
        let text = extract_input_text_from_request(&body);
        assert_eq!(text, "What documents do I need?");
    }

    #[test]
    fn test_extract_input_text_chat() {
        let body = json!({
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Approve claim CLM-001"}
            ]
        });
        let text = extract_input_text_from_request(&body);
        assert!(text.contains("Approve claim CLM-001"));
        assert!(!text.contains("helpful"));
    }
}