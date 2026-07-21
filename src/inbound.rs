// Copyright 2025 Salesforce, Inc. All rights reserved.

//! Inbound request filter: prompt injection for explainability requirements.

use crate::audit::InjectionAuditEntry;
use crate::config::{
    PolicyConfig, AUDIT_LOG_INJECTED_PROMPT, PROMPT_INJECTION_MODE, SCOPE_SKIP_STREAMING,
};
use crate::models::{detect_llm_format, extract_input_text_from_request, LlmApiFormat};
use crate::prompt_generator::generate_prompt;

use sha2::{Digest, Sha256};
use serde_json::Value;

pub struct InboundResult {
    pub trace_id: String,
    pub modified_body: Option<Vec<u8>>,
}

pub fn process_inbound(body: &[u8], path: &str, config: &PolicyConfig) -> Option<InboundResult> {
    // Check path inclusions (empty = all paths)
    if !config.scope_enforce_for_paths.is_empty()
        && !config
            .scope_enforce_for_paths
            .iter()
            .any(|p| path.contains(p.as_str()))
    {
        pdk::logger::warn!(
            "[llm-explainability-enforcement] path '{}' not in scope_enforce_for_paths — skipping",
            path
        );
        return None;
    }

    // Parse the body as JSON
    let mut request_value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            pdk::logger::warn!(
                "[llm-explainability-enforcement] request body is not valid JSON — skipping"
            );
            return None;
        }
    };

    // Detect LLM format
    let format = detect_llm_format(&request_value);
    if format == LlmApiFormat::Unknown {
        pdk::logger::warn!(
            "[llm-explainability-enforcement] unknown LLM API format — skipping"
        );
        return None;
    }

    // Skip streaming requests (hardcoded: always true)
    if SCOPE_SKIP_STREAMING {
        let is_streaming = request_value
            .get("stream")
            .and_then(|s| s.as_bool())
            .unwrap_or(false);
        if is_streaming {
            pdk::logger::warn!(
                "[llm-explainability-enforcement] streaming request (stream=true) — skipping"
            );
            return None;
        }
    }

    // Generate trace ID
    let trace_id = generate_trace_id(body, path);

    // Generate compliance prompt and inject
    let prompt_text = generate_prompt(config);
    let prompt_hash = compute_sha256(&prompt_text);
    let config_hash = config.config_hash();

    if AUDIT_LOG_INJECTED_PROMPT {
        let input_text = extract_input_text_from_request(&request_value);
        let preview = if input_text.len() > 200 {
            format!("{}...", &input_text[..200])
        } else {
            input_text
        };
        let audit = InjectionAuditEntry::new(
            &trace_id,
            &format!("{:?}", format),
            &prompt_hash,
            &preview,
            &config_hash,
        );
        if let Ok(json_str) = serde_json::to_string(&audit) {
            pdk::logger::info!("{}", json_str);
        }
    }

    let modified_body = inject_prompt(&mut request_value, &prompt_text, &format);

    Some(InboundResult {
        trace_id,
        modified_body: Some(modified_body),
    })
}

fn inject_prompt(request: &mut Value, prompt_text: &str, format: &LlmApiFormat) -> Vec<u8> {
    // Hardcoded to system_message — correct for all LLM providers
    let _ = PROMPT_INJECTION_MODE; // ensures constant is referenced
    inject_as_system_message(request, prompt_text, format);
    serde_json::to_vec(request).unwrap_or_default()
}

fn inject_as_system_message(request: &mut Value, prompt_text: &str, format: &LlmApiFormat) {
    match format {
        LlmApiFormat::ResponsesApi => {
            let existing = request.get("instructions").cloned();
            let new_val = match existing {
                Some(Value::String(s)) if !s.is_empty() => {
                    Value::String(format!("{}\n\n{}", s, prompt_text))
                }
                _ => Value::String(prompt_text.to_string()),
            };
            if let Some(obj) = request.as_object_mut() {
                obj.insert("instructions".to_string(), new_val);
            }
        }
        LlmApiFormat::AnthropicMessages => {
            let existing = request.get("system").cloned();
            let new_val = match existing {
                Some(Value::String(s)) => Value::String(format!("{}\n\n{}", s, prompt_text)),
                Some(Value::Array(mut arr)) => {
                    arr.insert(0, serde_json::json!({"type": "text", "text": prompt_text}));
                    Value::Array(arr)
                }
                _ => Value::String(prompt_text.to_string()),
            };
            if let Some(obj) = request.as_object_mut() {
                obj.insert("system".to_string(), new_val);
            }
        }
        LlmApiFormat::ChatCompletions => {
            if let Some(messages) = request.get_mut("messages").and_then(|m| m.as_array_mut()) {
                let mut found = false;
                for msg in messages.iter_mut() {
                    if msg.get("role").and_then(|r| r.as_str()) == Some("system") {
                        if let Some(content) = msg.get_mut("content") {
                            let updated = match content.as_str() {
                                Some(s) => format!("{}\n\n{}", s, prompt_text),
                                None => prompt_text.to_string(),
                            };
                            *content = Value::String(updated);
                        }
                        found = true;
                        break;
                    }
                }
                if !found {
                    messages.insert(
                        0,
                        serde_json::json!({"role": "system", "content": prompt_text}),
                    );
                }
            }
        }
        LlmApiFormat::Completions | LlmApiFormat::Unknown => {
            if let Some(prompt) = request.get_mut("prompt") {
                if let Some(s) = prompt.as_str() {
                    *prompt = Value::String(format!("{}\n\n{}", prompt_text, s));
                } else {
                    *prompt = Value::String(prompt_text.to_string());
                }
            } else if let Some(obj) = request.as_object_mut() {
                obj.insert("prompt".to_string(), Value::String(prompt_text.to_string()));
            }
        }
    }
}

fn generate_trace_id(body: &[u8], path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body);
    hasher.update(path.as_bytes());
    let time_bytes = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos().to_le_bytes())
        .unwrap_or([0u8; 16]);
    hasher.update(&time_bytes);
    let hash = hasher.finalize();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]),
        u16::from_be_bytes([hash[4], hash[5]]),
        u16::from_be_bytes([hash[6], hash[7]]),
        u16::from_be_bytes([hash[8], hash[9]]),
        u64::from_be_bytes([0, 0, hash[10], hash[11], hash[12], hash[13], hash[14], hash[15]])
    )
}

fn compute_sha256(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_config() -> PolicyConfig {
        serde_json::from_str(
            r#"{"explainability_fields": [
                {"field": "decision_outcome", "field_type": "string", "required": true, "allowed_values": ["approved", "rejected"]},
                {"field": "confidence_score", "field_type": "number", "required": true, "validation_min": 0.0, "validation_max": 1.0}
            ]}"#,
        ).unwrap()
    }

    fn chat_body(content: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"model": "gpt-4o", "messages": [{"role": "user", "content": content}]})).unwrap()
    }

    fn responses_body(input: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"model": "gpt-4o-mini", "input": input})).unwrap()
    }

    fn anthropic_body(content: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"model": "claude-3-5-sonnet-20241022", "max_tokens": 1024, "messages": [{"role": "user", "content": content}]})).unwrap()
    }

    #[test]
    fn test_chat_injects_system_message() {
        let config = test_config();
        let result = process_inbound(&chat_body("Approve loan APP-001"), "/", &config).unwrap();
        assert!(!result.trace_id.is_empty());
        let modified: serde_json::Value = serde_json::from_slice(&result.modified_body.unwrap()).unwrap();
        assert_eq!(modified["messages"][0]["role"], "system");
        assert!(modified["messages"][0]["content"].as_str().unwrap().contains("MANDATORY COMPLIANCE"));
    }

    #[test]
    fn test_responses_api_injects_instructions() {
        let config = test_config();
        let result = process_inbound(&responses_body("What documents for home loan?"), "/", &config).unwrap();
        let modified: serde_json::Value = serde_json::from_slice(&result.modified_body.unwrap()).unwrap();
        assert!(modified["instructions"].as_str().unwrap().contains("MANDATORY COMPLIANCE"));
        assert_eq!(modified["input"].as_str().unwrap(), "What documents for home loan?");
    }

    #[test]
    fn test_anthropic_injects_system_field() {
        let config = test_config();
        let result = process_inbound(&anthropic_body("Process claim"), "/", &config).unwrap();
        let modified: serde_json::Value = serde_json::from_slice(&result.modified_body.unwrap()).unwrap();
        assert!(modified["system"].as_str().unwrap().contains("MANDATORY COMPLIANCE"));
    }

    #[test]
    fn test_streaming_skipped() {
        let config = test_config();
        let body = serde_json::to_vec(&json!({"model": "gpt-4o", "stream": true, "messages": [{"role": "user", "content": "hi"}]})).unwrap();
        assert!(process_inbound(&body, "/", &config).is_none());
    }

    #[test]
    fn test_unknown_format_skipped() {
        let config = test_config();
        let body = serde_json::to_vec(&json!({"key": "value"})).unwrap();
        assert!(process_inbound(&body, "/", &config).is_none());
    }

    #[test]
    fn test_path_filter() {
        let mut config = test_config();
        config.scope_enforce_for_paths = vec!["/v1/responses".to_string()];
        assert!(process_inbound(&chat_body("hi"), "/v1/chat/completions", &config).is_none());
        assert!(process_inbound(&responses_body("hi"), "/v1/responses", &config).is_some());
    }

    #[test]
    fn test_existing_system_message_appended() {
        let config = test_config();
        let body = serde_json::to_vec(&json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a financial advisor."},
                {"role": "user", "content": "Approve loan"}
            ]
        })).unwrap();
        let result = process_inbound(&body, "/", &config).unwrap();
        let modified: serde_json::Value = serde_json::from_slice(&result.modified_body.unwrap()).unwrap();
        let msgs = modified["messages"].as_array().unwrap();
        assert_eq!(msgs.iter().filter(|m| m["role"] == "system").count(), 1);
        assert!(msgs[0]["content"].as_str().unwrap().contains("financial advisor"));
        assert!(msgs[0]["content"].as_str().unwrap().contains("MANDATORY COMPLIANCE"));
    }
}