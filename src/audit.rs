// Copyright 2025 Salesforce, Inc. All rights reserved.

//! Audit log entry structures for regulatory compliance tracing.

use crate::validator::ValidationResult;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct InjectionAuditEntry {
    pub event: &'static str,
    pub trace_id: String,
    /// LLM API format: "ChatCompletions", "Completions", "AnthropicMessages"
    pub llm_format: String,
    pub prompt_hash: String,
    pub input_preview: String,
    pub config_hash: String,
}

#[derive(Debug, Serialize)]
pub struct ValidationAuditEntry {
    pub event: &'static str,
    pub trace_id: String,
    pub compliance_status: String,
    pub compliance_percentage: f64,
    pub total_required: usize,
    pub valid_fields: Vec<String>,
    pub missing_fields: Vec<String>,
    pub invalid_fields: Vec<InvalidFieldLog>,
    pub config_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct InvalidFieldLog {
    pub field: String,
    pub reason: String,
}

impl InjectionAuditEntry {
    pub fn new(
        trace_id: &str,
        llm_format: &str,
        prompt_hash: &str,
        input_text: &str,
        config_hash: &str,
    ) -> Self {
        let preview = if input_text.len() > 200 {
            format!("{}...", &input_text[..200])
        } else {
            input_text.to_string()
        };
        Self {
            event: "llm_explainability_injection",
            trace_id: trace_id.to_string(),
            llm_format: llm_format.to_string(),
            prompt_hash: prompt_hash.to_string(),
            input_preview: preview,
            config_hash: config_hash.to_string(),
        }
    }
}

impl ValidationAuditEntry {
    pub fn new(
        trace_id: &str,
        result: &ValidationResult,
        config_hash: &str,
        metadata: Option<&Value>,
        log_full_metadata: bool,
        minimum_compliance_percentage: u8,
    ) -> Self {
        let compliance_status = if result.is_compliant(minimum_compliance_percentage) {
            "compliant"
        } else {
            "non_compliant"
        };
        Self {
            event: "llm_explainability_validation",
            trace_id: trace_id.to_string(),
            compliance_status: compliance_status.to_string(),
            compliance_percentage: result.compliance_percentage,
            total_required: result.total_required,
            valid_fields: result.valid_fields.clone(),
            missing_fields: result.missing_fields.clone(),
            invalid_fields: result
                .invalid_fields
                .iter()
                .map(|f| InvalidFieldLog {
                    field: f.field.clone(),
                    reason: f.reason.clone(),
                })
                .collect(),
            config_hash: config_hash.to_string(),
            metadata: if log_full_metadata {
                metadata.cloned()
            } else {
                None
            },
        }
    }
}

/// Formats a human-readable block message from a template.
pub fn format_block_message(
    template: &str,
    missing: &[String],
    invalid: &[crate::validator::InvalidField],
    compliance_pct: f64,
    trace_id: &str,
) -> String {
    let missing_str = if missing.is_empty() {
        "none".to_string()
    } else {
        missing.join(", ")
    };
    let invalid_str = if invalid.is_empty() {
        "none".to_string()
    } else {
        invalid.iter().map(|f| f.field.as_str()).collect::<Vec<_>>().join(", ")
    };
    template
        .replace("{missing_fields}", &missing_str)
        .replace("{invalid_fields}", &invalid_str)
        .replace("{compliance_percentage}", &format!("{:.1}", compliance_pct))
        .replace("{trace_id}", trace_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::InvalidField;

    #[test]
    fn test_injection_audit_truncates_preview() {
        let long_text = "x".repeat(300);
        let entry = InjectionAuditEntry::new("trace-1", "ChatCompletions", "hash1", &long_text, "cfg");
        assert_eq!(entry.input_preview.len(), 203);
        assert!(entry.input_preview.ends_with("..."));
    }

    #[test]
    fn test_injection_audit_short_preview() {
        let entry = InjectionAuditEntry::new("t1", "AnthropicMessages", "h", "short msg", "c");
        assert_eq!(entry.input_preview, "short msg");
    }

    #[test]
    fn test_format_block_message_replaces_placeholders() {
        let msg = format_block_message(
            "Missing: {missing_fields}. Invalid: {invalid_fields}. Pct: {compliance_percentage}. Trace: {trace_id}",
            &["field_a".to_string(), "field_b".to_string()],
            &[InvalidField { field: "field_c".to_string(), reason: "bad".to_string() }],
            66.7,
            "trace-xyz",
        );
        assert!(msg.contains("field_a, field_b"));
        assert!(msg.contains("field_c"));
        assert!(msg.contains("66.7"));
        assert!(msg.contains("trace-xyz"));
    }

    #[test]
    fn test_format_block_message_empty_collections() {
        let msg = format_block_message(
            "Missing: {missing_fields}. Invalid: {invalid_fields}.",
            &[],
            &[],
            100.0,
            "t1",
        );
        assert_eq!(msg, "Missing: none. Invalid: none.");
    }

    #[test]
    fn test_validation_audit_compliant() {
        let result = ValidationResult {
            valid_fields: vec!["a".to_string()],
            missing_fields: vec![],
            invalid_fields: vec![],
            compliance_percentage: 100.0,
            total_required: 1,
        };
        let entry = ValidationAuditEntry::new("t1", &result, "cfg", None, false, 100);
        assert_eq!(entry.compliance_status, "compliant");
        assert_eq!(entry.event, "llm_explainability_validation");
        assert!(entry.metadata.is_none());
    }

    #[test]
    fn test_validation_audit_non_compliant() {
        let result = ValidationResult {
            valid_fields: vec![],
            missing_fields: vec!["x".to_string()],
            invalid_fields: vec![],
            compliance_percentage: 0.0,
            total_required: 1,
        };
        let meta = serde_json::json!({"partial": true});
        let entry = ValidationAuditEntry::new("t2", &result, "cfg", Some(&meta), true, 100);
        assert_eq!(entry.compliance_status, "non_compliant");
        assert!(entry.metadata.is_some());
    }
}