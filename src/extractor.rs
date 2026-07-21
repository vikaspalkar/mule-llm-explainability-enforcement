// Copyright 2025 Salesforce, Inc. All rights reserved.

//! Extracts explainability metadata JSON from LLM response text.

use regex::Regex;
use serde_json::Value;

pub fn extract_metadata(full_text: &str, wrapper_key: &str, strategy: &str) -> Option<Value> {
    match strategy {
        "json_block" => extract_from_json_block(full_text, wrapper_key),
        "tagged_section" => extract_from_tagged_section(full_text, wrapper_key),
        _ => extract_from_json_block(full_text, wrapper_key),
    }
}

fn extract_from_json_block(text: &str, wrapper_key: &str) -> Option<Value> {
    if let Some(val) = try_code_fence_with_lang(text, wrapper_key) {
        return Some(val);
    }
    if let Some(val) = try_code_fence_no_lang(text, wrapper_key) {
        return Some(val);
    }
    if let Some(val) = try_find_json_object(text, wrapper_key) {
        return Some(val);
    }
    if let Some(val) = try_parse_full_text(text, wrapper_key) {
        return Some(val);
    }
    None
}

fn extract_from_tagged_section(text: &str, wrapper_key: &str) -> Option<Value> {
    let start_tag = "[EXPLAINABILITY_START]";
    let end_tag = "[EXPLAINABILITY_END]";
    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let content = text[content_start..content_start + end_idx].trim();
    let parsed: Value = serde_json::from_str(content).ok()?;
    extract_wrapper_value(&parsed, wrapper_key)
}

fn try_code_fence_with_lang(text: &str, wrapper_key: &str) -> Option<Value> {
    let re = Regex::new(r"```json\s*\n([\s\S]*?)\n\s*```").ok()?;
    for cap in re.captures_iter(text) {
        if let Some(content) = cap.get(1) {
            if let Some(val) = parse_and_extract(content.as_str(), wrapper_key) {
                return Some(val);
            }
        }
    }
    None
}

fn try_code_fence_no_lang(text: &str, wrapper_key: &str) -> Option<Value> {
    let re = Regex::new(r"```\s*\n([\s\S]*?)\n\s*```").ok()?;
    for cap in re.captures_iter(text) {
        if let Some(content) = cap.get(1) {
            if let Some(val) = parse_and_extract(content.as_str(), wrapper_key) {
                return Some(val);
            }
        }
    }
    None
}

fn try_find_json_object(text: &str, wrapper_key: &str) -> Option<Value> {
    let key_pattern = format!("\"{}\"", wrapper_key);
    let key_idx = text.find(&key_pattern)?;
    let before = &text[..key_idx];
    let open_brace = before.rfind('{')?;
    let from_open = &text[open_brace..];
    if let Some(json_str) = find_balanced_json(from_open) {
        if let Some(val) = parse_and_extract(json_str, wrapper_key) {
            return Some(val);
        }
    }
    None
}

fn try_parse_full_text(text: &str, wrapper_key: &str) -> Option<Value> {
    parse_and_extract(text.trim(), wrapper_key)
}

fn parse_and_extract(json_str: &str, wrapper_key: &str) -> Option<Value> {
    if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
        return extract_wrapper_value(&parsed, wrapper_key);
    }
    let cleaned = strip_trailing_commas(json_str);
    if let Ok(parsed) = serde_json::from_str::<Value>(&cleaned) {
        return extract_wrapper_value(&parsed, wrapper_key);
    }
    None
}

fn extract_wrapper_value(parsed: &Value, wrapper_key: &str) -> Option<Value> {
    if let Some(obj) = parsed.as_object() {
        if let Some(metadata) = obj.get(wrapper_key) {
            return Some(metadata.clone());
        }
    }
    if parsed.is_object() {
        return Some(parsed.clone());
    }
    None
}

fn find_balanced_json(text: &str) -> Option<&str> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in text.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => { escape_next = true; }
            '"' => { in_string = !in_string; }
            '{' if !in_string => { depth += 1; }
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_trailing_commas(json: &str) -> String {
    let re = Regex::new(r",(\s*[}\]])").unwrap_or_else(|_| Regex::new(r"$^").unwrap());
    re.replace_all(json, "$1").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_from_json_code_fence() {
        let text = r#"Here is the decision.

```json
{
  "explainability_metadata": {
    "decision_outcome": "approved",
    "confidence_score": 0.95
  }
}
```"#;
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_some());
        let meta = result.unwrap();
        assert_eq!(meta["decision_outcome"], "approved");
        assert_eq!(meta["confidence_score"], 0.95);
    }

    #[test]
    fn test_extract_from_code_fence_no_lang_tag() {
        let text = "Response text.\n\n```\n{\"explainability_metadata\": {\"outcome\": \"ok\"}}\n```";
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_some());
        assert_eq!(result.unwrap()["outcome"], "ok");
    }

    #[test]
    fn test_extract_inline_json() {
        let text = r#"The claim is approved. {"explainability_metadata": {"status": "approved"}} End."#;
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_some());
        assert_eq!(result.unwrap()["status"], "approved");
    }

    #[test]
    fn test_extract_tagged_section() {
        let text = "Text\n[EXPLAINABILITY_START]\n{\"explainability_metadata\": {\"a\": \"b\"}}\n[EXPLAINABILITY_END]\nMore text";
        let result = extract_metadata(text, "explainability_metadata", "tagged_section");
        assert!(result.is_some());
        assert_eq!(result.unwrap()["a"], "b");
    }

    #[test]
    fn test_no_metadata_returns_none() {
        let text = "This response has no metadata at all.";
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_none());
    }

    #[test]
    fn test_malformed_json_returns_none() {
        let text = "```json\n{\"explainability_metadata\": {\"broken\": \n```";
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_none());
    }

    #[test]
    fn test_trailing_comma_handled() {
        let text = "```json\n{\"explainability_metadata\": {\"x\": 1,}}\n```";
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_some());
    }

    #[test]
    fn test_unicode_values() {
        let text = r#"```json
{"explainability_metadata": {"factors": ["निर्णय कारक", "अनुपालन"]}}
```"#;
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_some());
        let meta = result.unwrap();
        let factors = meta["factors"].as_array().unwrap();
        assert_eq!(factors[0], "निर्णय कारक");
    }

    #[test]
    fn test_full_text_is_json() {
        let text = r#"{"explainability_metadata": {"x": 1}}"#;
        let result = extract_metadata(text, "explainability_metadata", "json_block");
        assert!(result.is_some());
        assert_eq!(result.unwrap()["x"], 1);
    }
}