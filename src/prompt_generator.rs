// Copyright 2025 Salesforce, Inc. All rights reserved.

//! Auto-generates the explainability instruction prompt from field definitions.

use crate::config::{
    ExplainabilityField, PolicyConfig, PROMPT_RESPONSE_FORMAT, PROMPT_RESPONSE_WRAPPER_KEY,
};

pub fn generate_prompt(config: &PolicyConfig) -> String {
    let mut prompt = String::with_capacity(2048);

    if !config.prompt_custom_preamble.is_empty() {
        prompt.push_str(&config.prompt_custom_preamble);
        prompt.push_str("\n\n");
    }

    prompt.push_str("=== MANDATORY COMPLIANCE REQUIREMENT ===\n\n");
    prompt.push_str(
        "You MUST include structured explainability metadata in your response. \
         Non-compliant responses will be REJECTED by the governance gateway.\n\n",
    );

    prompt.push_str("REQUIRED FIELDS:\n\n");

    for (i, field) in config.explainability_fields.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. **{}** (type: {})",
            i + 1,
            field.field,
            field.field_type
        ));

        if field.required {
            prompt.push_str(" [REQUIRED]");
        } else if field.required_when_field.is_some() {
            prompt.push_str(&format!(
                " [REQUIRED WHEN {} = \"{}\"]",
                field.required_when_field.as_deref().unwrap_or(""),
                field.required_when_equals.as_deref().unwrap_or("")
            ));
        } else {
            prompt.push_str(" [OPTIONAL]");
        }

        prompt.push('\n');
        if !field.description.is_empty() {
            prompt.push_str(&format!("   Description: {}\n", field.description));
        }
        if !field.allowed_values.is_empty() {
            prompt.push_str(&format!("   Allowed values: {}\n", field.allowed_values.join(", ")));
        }
        if let Some(min) = field.validation_min {
            if field.field_type == "number" {
                prompt.push_str(&format!("   Minimum: {}\n", min));
            } else if field.field_type == "array" {
                prompt.push_str(&format!("   Minimum items: {}\n", min as u64));
            }
        }
        if let Some(max) = field.validation_max {
            if field.field_type == "number" {
                prompt.push_str(&format!("   Maximum: {}\n", max));
            } else if field.field_type == "array" {
                prompt.push_str(&format!("   Maximum items: {}\n", max as u64));
            }
        }
        prompt.push('\n');
    }

    prompt.push_str("RESPONSE FORMAT:\n\n");

    if PROMPT_RESPONSE_FORMAT == "json_block" {
        prompt.push_str(&format!(
            "Include the metadata as a JSON object wrapped in a code fence at the END of your response:\n\n\
             ```json\n{{\n  \"{}\": {{\n",
            PROMPT_RESPONSE_WRAPPER_KEY
        ));
        prompt.push_str(&generate_example_fields(&config.explainability_fields));
        prompt.push_str("  }\n}\n```\n\n");
    } else {
        prompt.push_str(&format!(
            "Include the metadata between tags:\n\n\
             [EXPLAINABILITY_START]\n{{\n  \"{}\": {{\n",
            PROMPT_RESPONSE_WRAPPER_KEY
        ));
        prompt.push_str(&generate_example_fields(&config.explainability_fields));
        prompt.push_str("  }\n}\n[EXPLAINABILITY_END]\n\n");
    }

    prompt.push_str(
        "IMPORTANT: Failure to include this metadata will result in your response being \
         BLOCKED by the governance policy. Include ALL required fields with valid values.\n",
    );

    prompt
}

fn generate_example_fields(fields: &[ExplainabilityField]) -> String {
    let mut example = String::new();
    for (i, field) in fields.iter().enumerate() {
        let value = generate_example_value(field);
        let comma = if i < fields.len() - 1 { "," } else { "" };
        example.push_str(&format!("    \"{}\": {}{}\n", field.field, value, comma));
    }
    example
}

fn generate_example_value(field: &ExplainabilityField) -> String {
    match field.field_type.as_str() {
        "string" => {
            if !field.allowed_values.is_empty() {
                format!("\"{}\"", field.allowed_values[0])
            } else {
                format!("\"<{}>\"", field.field)
            }
        }
        "number" => {
            if let (Some(min), Some(max)) = (field.validation_min, field.validation_max) {
                format!("{:.2}", (min + max) / 2.0)
            } else if let Some(min) = field.validation_min {
                format!("{:.2}", min)
            } else {
                "0.0".to_string()
            }
        }
        "boolean" => "true".to_string(),
        "array" => "[\"example_item\"]".to_string(),
        _ => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> PolicyConfig {
        serde_json::from_str(r#"{
            "explainability_fields": [
                {"field": "decision_outcome", "description": "Final decision", "field_type": "string", "required": true, "allowed_values": ["approved", "rejected"]},
                {"field": "confidence_score", "description": "Confidence level", "field_type": "number", "required": true, "validation_min": 0.0, "validation_max": 1.0},
                {"field": "reasoning", "field_type": "array", "required": true, "validation_min": 1}
            ],
            "prompt_custom_preamble": "REGULATORY NOTE: EU AI Act compliance required."
        }"#).unwrap()
    }

    #[test]
    fn test_prompt_contains_all_fields() {
        let config = test_config();
        let prompt = generate_prompt(&config);
        assert!(prompt.contains("decision_outcome"));
        assert!(prompt.contains("confidence_score"));
        assert!(prompt.contains("reasoning"));
    }

    #[test]
    fn test_prompt_contains_custom_preamble() {
        let config = test_config();
        let prompt = generate_prompt(&config);
        assert!(prompt.starts_with("REGULATORY NOTE: EU AI Act compliance required."));
    }

    #[test]
    fn test_prompt_contains_json_block() {
        let config = test_config();
        let prompt = generate_prompt(&config);
        assert!(prompt.contains("```json"));
        assert!(prompt.contains("explainability_metadata"));
    }

    #[test]
    fn test_prompt_no_preamble() {
        let mut config = test_config();
        config.prompt_custom_preamble = String::new();
        let prompt = generate_prompt(&config);
        assert!(prompt.starts_with("=== MANDATORY COMPLIANCE REQUIREMENT ==="));
    }

    #[test]
    fn test_prompt_contains_constraints() {
        let config = test_config();
        let prompt = generate_prompt(&config);
        assert!(prompt.contains("approved, rejected"));
        assert!(prompt.contains("Minimum: 0"));
        assert!(prompt.contains("Maximum: 1"));
        assert!(prompt.contains("Minimum items: 1"));
    }
}
