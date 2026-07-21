// Copyright 2025 Salesforce, Inc. All rights reserved.

//! LLM Explainability Enforcement Policy — v1.1.8
//!
//! REQUEST: Two-step PDK (v1.1.6 confirmed working)
//!   Step 1: into_headers_state() → set trace ID, remove Content-Length
//!   Step 2: into_body_state()    → inject compliance system prompt
//!
//! RESPONSE:
//!   scope_response_validation_enabled=false (default): headers-only (safe, v1.1.6)
//!   scope_response_validation_enabled=true:  two-step response
//!     Step 1: into_headers_state() → set trace ID, remove Content-Length
//!     Step 2: into_body_state()    → read body, validate, replace body if non-compliant
//!   Note: :status cannot be changed in body state — block mode replaces body
//!   with error JSON but HTTP status remains 200.

mod audit;
mod config;
mod extractor;
mod inbound;
mod models;
mod outbound;
mod prompt_generator;
mod validator;

use anyhow::{anyhow, Result};
use pdk::hl::*;
use pdk::logger;
use pdk::script::PayloadBinding;
use sha2::{Digest, Sha256};

use crate::audit::{format_block_message, ValidationAuditEntry};
use crate::config::{
    PolicyConfig, AUDIT_ADD_COMPLIANCE_HEADER, AUDIT_COMPLIANCE_HEADER_NAME,
    AUDIT_LOG_FULL_METADATA, AUDIT_LOG_RESULTS, AUDIT_TRACE_ID_HEADER,
    PROMPT_RESPONSE_WRAPPER_KEY, VALIDATION_BLOCK_MESSAGE, VALIDATION_BLOCK_STATUS_CODE,
    VALIDATION_EXTRACTION_STRATEGY,
};
use crate::extractor::extract_metadata;
use crate::models::{
    detect_llm_format, extract_text_from_llm_response, is_llm_error_response, LlmApiFormat,
    LlmErrorResponse,
};
use crate::prompt_generator::generate_prompt;
use crate::validator::validate_metadata;

const POLICY_NAME: &str = "llm-explainability-enforcement";
const CONTENT_LENGTH_HEADER: &str = "content-length";
const CONTENT_TYPE_HEADER: &str = "content-type";
const APPLICATION_JSON: &str = "application/json";

struct RequestContext {
    trace_id: String,
    validate_response: bool,
}

#[entrypoint]
async fn configure(launcher: Launcher, Configuration(bytes): Configuration) -> Result<()> {
    let raw_config = String::from_utf8_lossy(&bytes);
    logger::info!("[{}] Received configuration ({} bytes): {}", POLICY_NAME, bytes.len(), &raw_config[..raw_config.len().min(500)]);

    let config: PolicyConfig = serde_json::from_slice(&bytes).map_err(|err| {
        anyhow!("[{}] Failed to parse configuration. Cause: {}", POLICY_NAME, err)
    })?;

    logger::info!("[{}] Parsed {} explainability_fields", POLICY_NAME, config.explainability_fields.len());

    if let Err(e) = config.validate() {
        logger::warn!("[{}] Configuration issue: {}", POLICY_NAME, e);
    }

    logger::info!(
        "[{}] Policy configured. Mode: {}. Response validation: {}. Hash: {}",
        POLICY_NAME, config.validation_on_failure, config.scope_response_validation_enabled, config.config_hash()
    );

    let filter = on_request(|request_state| request_filter(request_state, &config))
        .on_response(|response_state, request_data| response_filter(response_state, request_data, &config));

    launcher.launch(filter).await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// REQUEST FILTER — two-step PDK (v1.1.6 confirmed pattern)
// ─────────────────────────────────────────────────────────────────────────────
async fn request_filter(request_state: RequestState, config: &PolicyConfig) -> Flow<RequestContext> {
    if config.explainability_fields.is_empty() {
        logger::warn!("[{}] explainability_fields is empty — INACTIVE.", POLICY_NAME);
        let _state = request_state.into_headers_state().await;
        return Flow::Continue(RequestContext { trace_id: String::new(), validate_response: false });
    }

    // Step 1: headers
    let headers_state = request_state.into_headers_state().await;
    let handler = headers_state.handler();

    if !handler.header(CONTENT_TYPE_HEADER).unwrap_or_default().starts_with(APPLICATION_JSON) {
        return Flow::Continue(RequestContext { trace_id: String::new(), validate_response: false });
    }

    let path = handler.header(":path").unwrap_or_default();

    if !config.scope_enforce_for_paths.is_empty()
        && !config.scope_enforce_for_paths.iter().any(|p| path.contains(p.as_str()))
    {
        return Flow::Continue(RequestContext { trace_id: String::new(), validate_response: false });
    }

    let trace_id = generate_trace_id_from_path(&path);
    handler.set_header(AUDIT_TRACE_ID_HEADER, &trace_id);
    handler.remove_header(CONTENT_LENGTH_HEADER);

    // Step 2: body
    let body_state = headers_state.into_body_state().await;

    if body_state.contains_body() {
        let body_bytes = body_state.as_bytes();
        let body_handler = body_state.handler();
        if let Some(modified) = inject_system_prompt(&body_bytes, config) {
            if let Err(e) = body_handler.set_body(&modified) {
                logger::warn!("[{}] trace={} set_body failed: {:?}", POLICY_NAME, trace_id, e);
            } else {
                logger::info!("[{}] trace={} Compliance system prompt injected. Path: {}", POLICY_NAME, trace_id, path);
            }
        } else {
            logger::warn!("[{}] trace={} Could not inject — unknown format or streaming", POLICY_NAME, trace_id);
        }
    }

    Flow::Continue(RequestContext { trace_id, validate_response: config.scope_response_validation_enabled })
}

// ─────────────────────────────────────────────────────────────────────────────
// RESPONSE FILTER
// ─────────────────────────────────────────────────────────────────────────────
async fn response_filter(
    response_state: ResponseState,
    request_data: RequestData<RequestContext>,
    config: &PolicyConfig,
) {
    let context = match request_data {
        RequestData::Continue(ctx) => ctx,
        _ => { let _s = response_state.into_headers_state().await; return; }
    };

    if context.trace_id.is_empty() {
        let _s = response_state.into_headers_state().await;
        return;
    }

    if !context.validate_response {
        // v1.1.6 safe path: headers-only
        let state = response_state.into_headers_state().await;
        let handler = state.handler();
        handler.set_header(AUDIT_TRACE_ID_HEADER, &context.trace_id);
        logger::info!("[{}] trace={} Response passed through (injection-only mode)", POLICY_NAME, context.trace_id);
        return;
    }

    // Two-step response: try to read body without combined headers+body state
    // Step 1: response headers
    let resp_headers_state = response_state.into_headers_state().await;
    let resp_handler = resp_headers_state.handler();

    resp_handler.set_header(AUDIT_TRACE_ID_HEADER, &context.trace_id);

    if !resp_handler.header(CONTENT_TYPE_HEADER).unwrap_or_default().starts_with(APPLICATION_JSON) {
        return;
    }

    // Remove Content-Length — we may replace the body
    resp_handler.remove_header(CONTENT_LENGTH_HEADER);

    // Step 2: response body
    let resp_body_state = resp_headers_state.into_body_state().await;

    if !resp_body_state.contains_body() {
        return;
    }

    let body_bytes = resp_body_state.as_bytes();
    let body_handler = resp_body_state.handler();

    let response_json: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(_) => {
            logger::warn!("[{}] trace={} response not valid JSON", POLICY_NAME, context.trace_id);
            return;
        }
    };

    if is_llm_error_response(&response_json) {
        return;
    }

    let full_text = extract_text_from_llm_response(&response_json);
    let metadata = if full_text.is_empty() {
        None
    } else {
        extract_metadata(&full_text, PROMPT_RESPONSE_WRAPPER_KEY, VALIDATION_EXTRACTION_STRATEGY)
    };

    // Log explainability metadata
    match &metadata {
        Some(meta) => logger::info!("[{}] trace={} explainability_metadata: {}", POLICY_NAME, context.trace_id, meta),
        None => logger::warn!("[{}] trace={} explainability_metadata: NOT FOUND", POLICY_NAME, context.trace_id),
    }

    let validation_result = validate_metadata(metadata.as_ref(), &config.explainability_fields, metadata.as_ref());
    let compliant = validation_result.is_compliant(config.validation_minimum_compliance_percentage);
    let config_hash = config.config_hash();

    if AUDIT_LOG_RESULTS {
        let audit = ValidationAuditEntry::new(
            &context.trace_id, &validation_result, &config_hash,
            metadata.as_ref(), AUDIT_LOG_FULL_METADATA, config.validation_minimum_compliance_percentage,
        );
        if let Ok(json_str) = serde_json::to_string(&audit) {
            logger::info!("{}", json_str);
        }
    }

    if compliant {
        // Note: cannot set headers in body state — compliance header skipped here
        logger::info!("[{}] trace={} Response COMPLIANT", POLICY_NAME, context.trace_id);
        return;
    }

    match config.validation_on_failure.as_str() {
        "block" => {
            let message = format_block_message(
                VALIDATION_BLOCK_MESSAGE,
                &validation_result.missing_fields,
                &validation_result.invalid_fields,
                validation_result.compliance_percentage,
                &context.trace_id,
            );
            let error_data = serde_json::json!({
                "trace_id": context.trace_id,
                "compliance_status": "non_compliant",
                "compliance_percentage": validation_result.compliance_percentage,
                "missing_fields": validation_result.missing_fields,
                "invalid_fields": validation_result.invalid_fields.iter()
                    .map(|f| serde_json::json!({"field": &f.field, "reason": &f.reason}))
                    .collect::<Vec<_>>(),
                "policy_version": "1.1.8",
                "config_hash": config_hash
            });
            let error_response = LlmErrorResponse::compliance_error(message, error_data);
            let error_body = serde_json::to_vec(&error_response).unwrap_or_default();

            if let Err(e) = body_handler.set_body(&error_body) {
                logger::error!("[{}] trace={} Failed to set block response: {:?}", POLICY_NAME, context.trace_id, e);
            } else {
                logger::info!(
                    "[{}] trace={} Response BLOCKED (body replaced) — missing: {:?}",
                    POLICY_NAME, context.trace_id, validation_result.missing_fields
                );
            }
            // Note: HTTP :status cannot be changed here (body state).
            // Client receives 200 with error JSON body.
            // Use flag mode if HTTP 422 status is required.
        }
        "flag" => {
            let missing_str = validation_result.missing_fields.join(",");
            let mut status_val = "non_compliant".to_string();
            if !missing_str.is_empty() { status_val.push_str(&format!(";missing={}", missing_str)); }
            // Note: cannot set headers in body state
            logger::info!("[{}] trace={} Response NON-COMPLIANT (flag): {}", POLICY_NAME, context.trace_id, status_val);
        }
        _ => {
            // log_only: already logged
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn inject_system_prompt(body: &[u8], config: &PolicyConfig) -> Option<Vec<u8>> {
    let mut req: serde_json::Value = serde_json::from_slice(body).ok()?;
    let format = detect_llm_format(&req);
    if format == LlmApiFormat::Unknown { return None; }
    if req.get("stream").and_then(|s| s.as_bool()).unwrap_or(false) { return None; }

    let prompt = generate_prompt(config);

    match format {
        LlmApiFormat::ResponsesApi => {
            let existing = req.get("instructions").cloned();
            let new_val = match existing {
                Some(serde_json::Value::String(s)) if !s.is_empty() =>
                    serde_json::Value::String(format!("{}\n\n{}", s, prompt)),
                _ => serde_json::Value::String(prompt),
            };
            req.as_object_mut()?.insert("instructions".to_string(), new_val);
        }
        LlmApiFormat::AnthropicMessages => {
            let existing = req.get("system").cloned();
            let new_val = match existing {
                Some(serde_json::Value::String(s)) => serde_json::Value::String(format!("{}\n\n{}", s, prompt)),
                Some(serde_json::Value::Array(mut arr)) => {
                    arr.insert(0, serde_json::json!({"type": "text", "text": prompt}));
                    serde_json::Value::Array(arr)
                }
                _ => serde_json::Value::String(prompt.clone()),
            };
            req.as_object_mut()?.insert("system".to_string(), new_val);
        }
        LlmApiFormat::ChatCompletions => {
            let messages = req.get_mut("messages")?.as_array_mut()?;
            let mut found = false;
            for msg in messages.iter_mut() {
                if msg.get("role").and_then(|r| r.as_str()) == Some("system") {
                    if let Some(content) = msg.get_mut("content") {
                        let updated = match content.as_str() {
                            Some(s) => format!("{}\n\n{}", s, prompt),
                            None => prompt.clone(),
                        };
                        *content = serde_json::Value::String(updated);
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                messages.insert(0, serde_json::json!({"role": "system", "content": prompt}));
            }
        }
        LlmApiFormat::Completions | LlmApiFormat::Unknown => {
            if let Some(p) = req.get_mut("prompt") {
                if let Some(s) = p.as_str() {
                    *p = serde_json::Value::String(format!("{}\n\n{}", prompt, s));
                }
            } else {
                req.as_object_mut()?.insert("prompt".to_string(), serde_json::Value::String(prompt));
            }
        }
    }

    serde_json::to_vec(&req).ok()
}

fn generate_trace_id_from_path(path: &str) -> String {
    let mut hasher = Sha256::new();
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