// Copyright 2025 Salesforce, Inc. All rights reserved.

//! Simplified policy configuration — 6 user-configurable fields.
//! All other settings are hardcoded as constants appropriate for all LLM providers.

use serde::Deserialize;
use sha2::{Digest, Sha256};

// ── Hardcoded constants ───────────────────────────────────────────────────────
// Correct for ALL LLM providers (OpenAI, Anthropic, Gemini, future providers).

pub const PROMPT_INJECTION_MODE: &str = "system_message";
pub const PROMPT_RESPONSE_FORMAT: &str = "json_block";
pub const PROMPT_RESPONSE_WRAPPER_KEY: &str = "explainability_metadata";
pub const VALIDATION_EXTRACTION_STRATEGY: &str = "json_block";
pub const VALIDATION_BLOCK_STATUS_CODE: u16 = 422;
pub const VALIDATION_BLOCK_MESSAGE: &str =
    "LLM response does not meet explainability requirements. \
     Missing: {missing_fields}. Invalid: {invalid_fields}. Trace: {trace_id}";
pub const AUDIT_LOG_RESULTS: bool = true;
pub const AUDIT_LOG_FULL_METADATA: bool = true;
pub const AUDIT_LOG_INJECTED_PROMPT: bool = true;
pub const AUDIT_ADD_COMPLIANCE_HEADER: bool = true;
pub const AUDIT_COMPLIANCE_HEADER_NAME: &str = "X-Explainability-Status";
pub const AUDIT_TRACE_ID_HEADER: &str = "X-Explainability-Trace-Id";
pub const SCOPE_SKIP_STREAMING: bool = true;

// ── User-configurable fields (6 total) ───────────────────────────────────────

/// Simplified policy configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    /// REQUIRED. Field definitions for required explainability metadata.
    /// Each entry defines both the instruction to the LLM AND the validation rule.
    #[serde(alias = "explainabilityFields", alias = "explainability_fields", default)]
    pub explainability_fields: Vec<ExplainabilityField>,

    /// Action when LLM response fails validation.
    /// `log_only` (default) → safe for rollout. `flag` → adds header, passes through.
    /// `block` → returns HTTP 422 (full enforcement).
    #[serde(default = "default_log_only", alias = "validationOnFailure", alias = "validation_on_failure")]
    pub validation_on_failure: String,

    /// Minimum percentage of required fields that must be valid (0–100).
    /// Use 100 for proven LLMs. Lower to 80–90 when onboarding a new provider.
    #[serde(default = "default_100", alias = "validationMinimumCompliancePercentage", alias = "validation_minimum_compliance_percentage")]
    pub validation_minimum_compliance_percentage: u8,

    /// Optional domain context prepended to the generated compliance instruction.
    /// Example: "You are a regulated financial AI operating under RBI guidelines."
    #[serde(default, alias = "promptCustomPreamble", alias = "prompt_custom_preamble")]
    pub prompt_custom_preamble: String,

    /// API paths to enforce on. Empty = enforce on all JSON POST paths.
    /// Example: ["/v1/responses", "/v1/chat/completions"]
    #[serde(default, alias = "scopeEnforceForPaths", alias = "scope_enforce_for_paths")]
    pub scope_enforce_for_paths: Vec<String>,

    /// Enable response body buffering and validation.
    /// `false` (default) — injection-only mode. Safe for Omni Gateway LLM proxies
    /// that return chunked responses (avoids 504 timeout).
    /// `true` — full outbound field validation. Only for direct/self-hosted LLMs
    /// that return finite responses with Content-Length.
    #[serde(default, alias = "scopeResponseValidationEnabled", alias = "scope_response_validation_enabled")]
    pub scope_response_validation_enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExplainabilityField {
    pub field: String,
    #[serde(default)]
    pub description: String,
    #[serde(alias = "fieldType", alias = "field_type")]
    pub field_type: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, alias = "allowedValues", alias = "allowed_values")]
    pub allowed_values: Vec<String>,
    #[serde(default, alias = "validationMin", alias = "validation_min")]
    pub validation_min: Option<f64>,
    #[serde(default, alias = "validationMax", alias = "validation_max")]
    pub validation_max: Option<f64>,
    #[serde(default, alias = "requiredWhenField", alias = "required_when_field")]
    pub required_when_field: Option<String>,
    #[serde(default, alias = "requiredWhenEquals", alias = "required_when_equals")]
    pub required_when_equals: Option<String>,
}

impl PolicyConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.explainability_fields.is_empty() {
            return Err("explainability_fields must not be empty".to_string());
        }

        let field_names: Vec<&str> = self
            .explainability_fields
            .iter()
            .map(|f| f.field.as_str())
            .collect();

        for field in &self.explainability_fields {
            if field.field.is_empty() {
                return Err("field name must not be empty".to_string());
            }
            if !matches!(
                field.field_type.as_str(),
                "string" | "number" | "boolean" | "array"
            ) {
                return Err(format!(
                    "field '{}' has invalid type '{}'. Must be string, number, boolean, or array",
                    field.field, field.field_type
                ));
            }
            if let (Some(min), Some(max)) = (field.validation_min, field.validation_max) {
                if min > max {
                    return Err(format!(
                        "field '{}': validation_min ({}) must be <= validation_max ({})",
                        field.field, min, max
                    ));
                }
            }
            if let Some(ref when_field) = field.required_when_field {
                if !field_names.contains(&when_field.as_str()) {
                    return Err(format!(
                        "field '{}': required_when_field '{}' does not exist in explainability_fields",
                        field.field, when_field
                    ));
                }
            }
            if !field.allowed_values.is_empty() && field.field_type != "string" {
                return Err(format!(
                    "field '{}': allowed_values can only be used with string type",
                    field.field
                ));
            }
        }
        Ok(())
    }

    pub fn config_hash(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

impl serde::Serialize for PolicyConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("PolicyConfig", 3)?;
        state.serialize_field("explainability_fields", &self.explainability_fields)?;
        state.serialize_field("validation_on_failure", &self.validation_on_failure)?;
        state.serialize_field(
            "validation_minimum_compliance_percentage",
            &self.validation_minimum_compliance_percentage,
        )?;
        state.end()
    }
}

impl serde::Serialize for ExplainabilityField {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ExplainabilityField", 4)?;
        state.serialize_field("field", &self.field)?;
        state.serialize_field("field_type", &self.field_type)?;
        state.serialize_field("required", &self.required)?;
        state.serialize_field("allowed_values", &self.allowed_values)?;
        state.end()
    }
}

fn default_true() -> bool {
    true
}

fn default_log_only() -> String {
    "log_only".to_string()
}

fn default_100() -> u8 {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_json() -> &'static str {
        r#"{"explainability_fields": [{"field": "decision_outcome", "field_type": "string"}]}"#
    }

    #[test]
    fn test_defaults() {
        let config: PolicyConfig = serde_json::from_str(minimal_json()).unwrap();
        assert_eq!(config.validation_on_failure, "log_only");
        assert_eq!(config.validation_minimum_compliance_percentage, 100);
        assert!(config.prompt_custom_preamble.is_empty());
        assert!(config.scope_enforce_for_paths.is_empty());
        assert!(!config.scope_response_validation_enabled);
    }

    #[test]
    fn test_constants() {
        assert_eq!(PROMPT_INJECTION_MODE, "system_message");
        assert_eq!(PROMPT_RESPONSE_FORMAT, "json_block");
        assert_eq!(PROMPT_RESPONSE_WRAPPER_KEY, "explainability_metadata");
        assert_eq!(AUDIT_COMPLIANCE_HEADER_NAME, "X-Explainability-Status");
        assert_eq!(AUDIT_TRACE_ID_HEADER, "X-Explainability-Trace-Id");
        assert!(SCOPE_SKIP_STREAMING);
    }

    #[test]
    fn test_validate_empty_fields_fails() {
        let c: PolicyConfig = serde_json::from_str(r#"{"explainability_fields": []}"#).unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_validate_valid_config() {
        let c: PolicyConfig = serde_json::from_str(minimal_json()).unwrap();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn test_config_hash_deterministic() {
        let c: PolicyConfig = serde_json::from_str(minimal_json()).unwrap();
        assert_eq!(c.config_hash(), c.config_hash());
        assert_eq!(c.config_hash().len(), 64);
    }
}