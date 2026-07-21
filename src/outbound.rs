// Copyright 2025 Salesforce, Inc. All rights reserved.

//! Outbound response filter: validates explainability metadata and enforces compliance.

use crate::audit::{format_block_message, ValidationAuditEntry};
use crate::config::{
    PolicyConfig, AUDIT_ADD_COMPLIANCE_HEADER, AUDIT_COMPLIANCE_HEADER_NAME,
    AUDIT_LOG_FULL_METADATA, AUDIT_LOG_RESULTS, AUDIT_TRACE_ID_HEADER,
    PROMPT_RESPONSE_WRAPPER_KEY, VALIDATION_BLOCK_MESSAGE, VALIDATION_BLOCK_STATUS_CODE,
    VALIDATION_EXTRACTION_STRATEGY,
};
use crate::extractor::extract_metadata;
use crate::models::{
    extract_text_from_llm_response, is_final_llm_response, is_llm_error_response, LlmErrorResponse,
};
use crate::validator::{validate_metadata, ValidationResult};

use serde_json::{json, Value};

pub struct OutboundResult {
    pub compliant: bool,
    pub headers_to_add: Vec<(String, String)>,
    pub replacement_body: Option<Vec<u8>>,
    pub replacement_status: Option<u16>,
    pub validation_result: Option<ValidationResult>,
}

pub fn process_outbound(body: &[u8], trace_id: &str, config: &PolicyConfig) -> OutboundResult {
    let response: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            pdk::logger::warn!(
                "[llm-explainability-enforcement] trace={} response body is not valid JSON — skipping",
                trace_id
            );
            return OutboundResult::pass_through();
        }
    };

    if is_llm_error_response(&response) {
        pdk::logger::warn!(
            "[llm-explainability-enforcement] trace={} upstream returned an LLM error — skipping",
            trace_id
        );
        return OutboundResult::pass_through();
    }

    if !is_final_llm_response(&response) {
        pdk::logger::warn!(
            "[llm-explainability-enforcement] trace={} streaming chunk — skipping",
            trace_id
        );
        return OutboundResult::pass_through();
    }

    let full_text = extract_text_from_llm_response(&response);

    let metadata = if full_text.is_empty() {
        pdk::logger::warn!(
            "[llm-explainability-enforcement] trace={} no text content — all fields missing",
            trace_id
        );
        None
    } else {
        extract_metadata(&full_text, PROMPT_RESPONSE_WRAPPER_KEY, VALIDATION_EXTRACTION_STRATEGY)
    };

    let validation_result = validate_metadata(
        metadata.as_ref(),
        &config.explainability_fields,
        metadata.as_ref(),
    );

    if AUDIT_LOG_RESULTS {
        let config_hash = config.config_hash();
        let audit = ValidationAuditEntry::new(
            trace_id,
            &validation_result,
            &config_hash,
            metadata.as_ref(),
            AUDIT_LOG_FULL_METADATA,
            config.validation_minimum_compliance_percentage,
        );
        if let Ok(json_str) = serde_json::to_string(&audit) {
            pdk::logger::info!("{}", json_str);
        }
    }

    handle_validation_result(&validation_result, trace_id, config)
}

fn handle_validation_result(
    result: &ValidationResult,
    trace_id: &str,
    config: &PolicyConfig,
) -> OutboundResult {
    let compliant = result.is_compliant(config.validation_minimum_compliance_percentage);

    if compliant {
        let mut headers = Vec::new();
        let is_log_only = config.validation_on_failure == "log_only";
        if AUDIT_ADD_COMPLIANCE_HEADER && !is_log_only {
            headers.push((AUDIT_COMPLIANCE_HEADER_NAME.to_string(), "compliant".to_string()));
        }
        headers.push((AUDIT_TRACE_ID_HEADER.to_string(), trace_id.to_string()));
        return OutboundResult {
            compliant: true,
            headers_to_add: headers,
            replacement_body: None,
            replacement_status: None,
            validation_result: Some(result.clone()),
        };
    }

    match config.validation_on_failure.as_str() {
        "block" => build_block_response(result, trace_id, config),
        "flag" => build_flag_response(result, trace_id),
        "log_only" => build_log_only_response(trace_id),
        _ => build_block_response(result, trace_id, config),
    }
}

fn build_block_response(result: &ValidationResult, trace_id: &str, config: &PolicyConfig) -> OutboundResult {
    let message = format_block_message(
        VALIDATION_BLOCK_MESSAGE,
        &result.missing_fields,
        &result.invalid_fields,
        result.compliance_percentage,
        trace_id,
    );
    let config_hash = config.config_hash();
    let error_data = json!({
        "trace_id": trace_id,
        "compliance_status": "non_compliant",
        "compliance_percentage": result.compliance_percentage,
        "required_fields_count": result.total_required,
        "valid_fields": result.valid_fields,
        "missing_fields": result.missing_fields,
        "invalid_fields": result.invalid_fields.iter()
            .map(|f| json!({"field": &f.field, "reason": &f.reason}))
            .collect::<Vec<_>>(),
        "policy_version": "1.1.0",
        "config_hash": config_hash
    });

    let error_response = LlmErrorResponse::compliance_error(message, error_data);
    let body = serde_json::to_vec(&error_response).unwrap_or_default();

    let mut headers = Vec::new();
    if AUDIT_ADD_COMPLIANCE_HEADER {
        headers.push((AUDIT_COMPLIANCE_HEADER_NAME.to_string(), "non_compliant".to_string()));
    }
    headers.push((AUDIT_TRACE_ID_HEADER.to_string(), trace_id.to_string()));

    OutboundResult {
        compliant: false,
        headers_to_add: headers,
        replacement_body: Some(body),
        replacement_status: Some(VALIDATION_BLOCK_STATUS_CODE),
        validation_result: Some(result.clone()),
    }
}

fn build_flag_response(result: &ValidationResult, trace_id: &str) -> OutboundResult {
    let missing_str = result.missing_fields.join(",");
    let invalid_str = result.invalid_fields.iter()
        .map(|f| f.field.as_str())
        .collect::<Vec<_>>()
        .join(",");

    let mut status_value = "non_compliant".to_string();
    if !missing_str.is_empty() {
        status_value.push_str(&format!(";missing={}", missing_str));
    }
    if !invalid_str.is_empty() {
        status_value.push_str(&format!(";invalid={}", invalid_str));
    }

    let mut headers = Vec::new();
    if AUDIT_ADD_COMPLIANCE_HEADER {
        headers.push((AUDIT_COMPLIANCE_HEADER_NAME.to_string(), status_value));
    }
    headers.push((AUDIT_TRACE_ID_HEADER.to_string(), trace_id.to_string()));

    OutboundResult {
        compliant: false,
        headers_to_add: headers,
        replacement_body: None,
        replacement_status: None,
        validation_result: Some(result.clone()),
    }
}

fn build_log_only_response(trace_id: &str) -> OutboundResult {
    OutboundResult {
        compliant: false,
        headers_to_add: vec![(AUDIT_TRACE_ID_HEADER.to_string(), trace_id.to_string())],
        replacement_body: None,
        replacement_status: None,
        validation_result: None,
    }
}

impl OutboundResult {
    fn pass_through() -> Self {
        Self {
            compliant: true,
            headers_to_add: Vec::new(),
            replacement_body: None,
            replacement_status: None,
            validation_result: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_config() -> PolicyConfig {
        // Use "block" mode in tests so we can assert on X-Explainability-Status headers.
        // Tests that need log_only or flag override validation_on_failure explicitly.
        serde_json::from_str(r#"{
            "explainability_fields": [
                {"field": "decision_outcome", "field_type": "string", "required": true, "allowed_values": ["approved", "rejected"]},
                {"field": "confidence_score", "field_type": "number", "required": true, "validation_min": 0.0, "validation_max": 1.0},
                {"field": "reasoning", "field_type": "array", "required": true, "validation_min": 1},
                {"field": "human_review_required", "field_type": "boolean", "required": true}
            ],
            "validation_on_failure": "block",
            "scope_response_validation_enabled": true
        }"#).unwrap()
    }

    fn compliant_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "choices": [{"message": {"role": "assistant", "content": "Approved.\n\n```json\n{\"explainability_metadata\": {\"decision_outcome\": \"approved\", \"confidence_score\": 0.92, \"reasoning\": [\"good credit\", \"stable income\"], \"human_review_required\": false}}\n```"}, "finish_reason": "stop"}]
        })).unwrap()
    }

    fn non_compliant_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "choices": [{"message": {"role": "assistant", "content": "Approved without metadata."}, "finish_reason": "stop"}]
        })).unwrap()
    }

    #[test]
    fn test_compliant_passes() {
        let config = test_config();
        let result = process_outbound(&compliant_body(), "trace-1", &config);
        assert!(result.compliant);
        assert!(result.headers_to_add.iter().any(|(k, v)| k == "X-Explainability-Status" && v == "compliant"));
    }

    #[test]
    fn test_non_compliant_blocked_by_default() {
        let mut config = test_config();
        config.validation_on_failure = "block".to_string();
        let result = process_outbound(&non_compliant_body(), "trace-2", &config);
        assert!(!result.compliant);
        assert!(result.replacement_body.is_some());
        assert_eq!(result.replacement_status, Some(422));
        let err: Value = serde_json::from_slice(&result.replacement_body.unwrap()).unwrap();
        assert_eq!(err["error"]["code"], "explainability_non_compliant");
    }

    #[test]
    fn test_flag_mode() {
        let mut config = test_config();
        config.validation_on_failure = "flag".to_string();
        let result = process_outbound(&non_compliant_body(), "trace-3", &config);
        assert!(!result.compliant);
        assert!(result.replacement_body.is_none());
        assert!(result.headers_to_add.iter().any(|(k, v)| k == "X-Explainability-Status" && v.starts_with("non_compliant;missing=")));
    }

    #[test]
    fn test_log_only_mode() {
        let mut config = test_config();
        config.validation_on_failure = "log_only".to_string();
        let result = process_outbound(&non_compliant_body(), "trace-4", &config);
        assert!(!result.compliant);
        assert!(result.replacement_body.is_none());
        assert!(!result.headers_to_add.iter().any(|(k, _)| k == "X-Explainability-Status"));
        assert!(result.headers_to_add.iter().any(|(k, _)| k == "X-Explainability-Trace-Id"));
    }

    #[test]
    fn test_responses_api_compliant() {
        let config = test_config();
        let body = serde_json::to_vec(&json!({
            "status": "completed",
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "Answer.\n\n```json\n{\"explainability_metadata\": {\"decision_outcome\": \"approved\", \"confidence_score\": 0.9, \"reasoning\": [\"x\"], \"human_review_required\": false}}\n```"}]}]
        })).unwrap();
        let result = process_outbound(&body, "trace-5", &config);
        assert!(result.compliant);
    }

    #[test]
    fn test_llm_error_passes_through() {
        let config = test_config();
        let body = serde_json::to_vec(&json!({"error": {"message": "Rate limit", "type": "rate_limit_error"}})).unwrap();
        let result = process_outbound(&body, "t", &config);
        assert!(result.compliant);
        assert!(result.headers_to_add.is_empty());
    }

    #[test]
    fn test_log_only_compliant_no_status_header() {
        let mut config = test_config();
        config.validation_on_failure = "log_only".to_string();
        let result = process_outbound(&compliant_body(), "t", &config);
        assert!(result.compliant);
        assert!(!result.headers_to_add.iter().any(|(k, _)| k == "X-Explainability-Status"));
        assert!(result.headers_to_add.iter().any(|(k, _)| k == "X-Explainability-Trace-Id"));
    }
}