// Copyright 2025 Salesforce, Inc. All rights reserved.

//! Validates extracted metadata fields against configured rules.

use crate::config::ExplainabilityField;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ValidationResult {
    pub valid_fields: Vec<String>,
    pub missing_fields: Vec<String>,
    pub invalid_fields: Vec<InvalidField>,
    pub compliance_percentage: f64,
    pub total_required: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvalidField {
    pub field: String,
    pub reason: String,
}

impl ValidationResult {
    pub fn is_compliant(&self, minimum_percentage: u8) -> bool {
        self.compliance_percentage >= minimum_percentage as f64
    }
}

pub fn validate_metadata(
    metadata: Option<&Value>,
    fields: &[ExplainabilityField],
    all_metadata: Option<&Value>,
) -> ValidationResult {
    let mut valid_fields = Vec::new();
    let mut missing_fields = Vec::new();
    let mut invalid_fields = Vec::new();

    let active_required: Vec<&ExplainabilityField> = fields
        .iter()
        .filter(|f| is_field_required(f, all_metadata))
        .collect();

    let total_required = active_required.len();

    for field_def in &active_required {
        let field_value = metadata.and_then(|m| m.get(&field_def.field));
        match field_value {
            None => missing_fields.push(field_def.field.clone()),
            Some(value) => {
                if let Some(reason) = validate_field_value(value, field_def) {
                    invalid_fields.push(InvalidField {
                        field: field_def.field.clone(),
                        reason,
                    });
                } else {
                    valid_fields.push(field_def.field.clone());
                }
            }
        }
    }

    // Also validate optional fields that ARE present
    for field_def in fields.iter().filter(|f| !is_field_required(f, all_metadata)) {
        if let Some(value) = metadata.and_then(|m| m.get(&field_def.field)) {
            if let Some(reason) = validate_field_value(value, field_def) {
                invalid_fields.push(InvalidField {
                    field: field_def.field.clone(),
                    reason,
                });
            }
        }
    }

    let compliance_percentage = if total_required == 0 {
        100.0
    } else {
        (valid_fields.len() as f64 / total_required as f64) * 100.0
    };

    ValidationResult {
        valid_fields,
        missing_fields,
        invalid_fields,
        compliance_percentage,
        total_required,
    }
}

fn is_field_required(field: &ExplainabilityField, metadata: Option<&Value>) -> bool {
    if field.required {
        return true;
    }
    if let (Some(ref when_field), Some(ref when_value)) =
        (&field.required_when_field, &field.required_when_equals)
    {
        if let Some(meta) = metadata {
            if let Some(ref_value) = meta.get(when_field).and_then(|v| v.as_str()) {
                return ref_value == when_value;
            }
        }
    }
    false
}

fn validate_field_value(value: &Value, field_def: &ExplainabilityField) -> Option<String> {
    let type_valid = match field_def.field_type.as_str() {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        _ => true,
    };

    if !type_valid {
        let actual_type = match value {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Null => "null",
        };
        return Some(format!("expected {}, got {}", field_def.field_type, actual_type));
    }

    match field_def.field_type.as_str() {
        "number" => {
            if let Some(num) = value.as_f64() {
                if let Some(min) = field_def.validation_min {
                    if num < min {
                        return Some(format!("value {} is below minimum {}", num, min));
                    }
                }
                if let Some(max) = field_def.validation_max {
                    if num > max {
                        return Some(format!("value {} exceeds maximum {}", num, max));
                    }
                }
            }
        }
        "array" => {
            if let Some(arr) = value.as_array() {
                if let Some(min) = field_def.validation_min {
                    if (arr.len() as f64) < min {
                        return Some(format!("array has {} items, minimum is {}", arr.len(), min as u64));
                    }
                }
                if let Some(max) = field_def.validation_max {
                    if (arr.len() as f64) > max {
                        return Some(format!("array has {} items, maximum is {}", arr.len(), max as u64));
                    }
                }
            }
        }
        "string" => {
            if !field_def.allowed_values.is_empty() {
                if let Some(s) = value.as_str() {
                    if !field_def.allowed_values.contains(&s.to_string()) {
                        return Some(format!(
                            "value '{}' not in allowed set: [{}]",
                            s,
                            field_def.allowed_values.join(", ")
                        ));
                    }
                }
            }
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_fields() -> Vec<ExplainabilityField> {
        serde_json::from_value(json!([
            {"field": "decision_outcome", "field_type": "string", "required": true, "allowed_values": ["approved", "rejected"]},
            {"field": "confidence_score", "field_type": "number", "required": true, "validation_min": 0.0, "validation_max": 1.0},
            {"field": "reasoning", "field_type": "array", "required": true, "validation_min": 1},
            {"field": "human_review_required", "field_type": "boolean", "required": true},
            {"field": "appeal_pathway", "field_type": "string", "required": false, "required_when_field": "decision_outcome", "required_when_equals": "rejected"}
        ]))
        .unwrap()
    }

    #[test]
    fn test_fully_compliant() {
        let fields = make_fields();
        let meta = json!({"decision_outcome": "approved", "confidence_score": 0.85, "reasoning": ["risk_score"], "human_review_required": false});
        let result = validate_metadata(Some(&meta), &fields, Some(&meta));
        assert_eq!(result.compliance_percentage, 100.0);
        assert!(result.missing_fields.is_empty());
        assert!(result.invalid_fields.is_empty());
    }

    #[test]
    fn test_missing_required_fields() {
        let fields = make_fields();
        let meta = json!({"decision_outcome": "approved", "confidence_score": 0.85});
        let result = validate_metadata(Some(&meta), &fields, Some(&meta));
        assert!(result.missing_fields.contains(&"reasoning".to_string()));
        assert!(result.missing_fields.contains(&"human_review_required".to_string()));
        assert_eq!(result.compliance_percentage, 50.0);
    }

    #[test]
    fn test_invalid_number_above_max() {
        let fields = make_fields();
        let meta = json!({"decision_outcome": "approved", "confidence_score": 1.5, "reasoning": ["x"], "human_review_required": true});
        let result = validate_metadata(Some(&meta), &fields, Some(&meta));
        assert_eq!(result.invalid_fields[0].field, "confidence_score");
        assert!(result.invalid_fields[0].reason.contains("exceeds maximum"));
    }

    #[test]
    fn test_invalid_allowed_values() {
        let fields = make_fields();
        let meta = json!({"decision_outcome": "unknown", "confidence_score": 0.5, "reasoning": ["x"], "human_review_required": true});
        let result = validate_metadata(Some(&meta), &fields, Some(&meta));
        assert_eq!(result.invalid_fields[0].field, "decision_outcome");
        assert!(result.invalid_fields[0].reason.contains("not in allowed set"));
    }

    #[test]
    fn test_required_when_condition_met() {
        let fields = make_fields();
        let meta = json!({"decision_outcome": "rejected", "confidence_score": 0.3, "reasoning": ["x"], "human_review_required": true});
        let result = validate_metadata(Some(&meta), &fields, Some(&meta));
        assert!(result.missing_fields.contains(&"appeal_pathway".to_string()));
        assert_eq!(result.total_required, 5);
    }

    #[test]
    fn test_required_when_condition_not_met() {
        let fields = make_fields();
        let meta = json!({"decision_outcome": "approved", "confidence_score": 0.9, "reasoning": ["x"], "human_review_required": false});
        let result = validate_metadata(Some(&meta), &fields, Some(&meta));
        assert!(!result.missing_fields.contains(&"appeal_pathway".to_string()));
        assert_eq!(result.total_required, 4);
        assert_eq!(result.compliance_percentage, 100.0);
    }

    #[test]
    fn test_no_metadata_all_missing() {
        let fields = make_fields();
        let result = validate_metadata(None, &fields, None);
        assert_eq!(result.missing_fields.len(), 4);
        assert_eq!(result.compliance_percentage, 0.0);
    }
}