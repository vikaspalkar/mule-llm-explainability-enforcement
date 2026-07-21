# LLM Explainability Enforcement Policy

## Overview

A MuleSoft Flex Gateway / Omni Gateway custom policy that enforces AI explainability and transparency requirements on LLM API responses. The policy:

1. **Injects** a structured compliance instruction into every LLM request as a system message
2. **Validates** that the LLM response contains the required explainability metadata fields
3. **Enforces** compliance through blocking, flagging, or audit logging

Compatible with **OpenAI Chat Completions**, **Anthropic Messages**, and any OpenAI-compatible endpoint.

> **Endpoint Note:** The Chat Completions endpoint (`/v1/chat/completions`) provides full functionality — request injection + response validation. The Responses API endpoint (`/v1/responses`) supports request injection only; response body validation is not available due to the SSE streaming architecture of the Omni Gateway LLM proxy.

---

## How It Works

```
┌──────────┐         ┌──────────────────────────────────┐         ┌────────────┐
│  Client  │──POST──▶│  Omni Gateway                    │──POST──▶│  LLM API   │
│          │         │  ┌──────────────────────────────┐ │         │ (OpenAI,   │
│          │◀────────│  │ LLM Explainability Policy    │ │◀────────│  Anthropic)│
│          │         │  │ ① Inject system prompt       │ │         └────────────┘
└──────────┘         │  │ ② Validate response metadata │ │
                     │  │ ③ Log / Block / Flag         │ │
                     │  └──────────────────────────────┘ │
                     └──────────────────────────────────┘
```

**REQUEST:** The policy injects a structured compliance instruction as a system message telling the LLM to include `explainability_metadata` in its response.

**RESPONSE:** When enabled, the policy reads the LLM response, extracts the `explainability_metadata` JSON block, validates each field, and enforces based on the configured mode.

---

## Prerequisites

- Flex Gateway / Omni Gateway 1.13+
- Rust toolchain with `wasm32-wasip1` target
- PDK 1.9.0
- Anypoint CLI v4 with PDK plugin

---

## Installation

```bash
# 1. Install tools
npm install -g anypoint-cli-v4-public
anypoint-cli-v4 plugins:install anypoint-pdk-plugin
cargo install cargo-anypoint@1.8.0
rustup target add wasm32-wasip1

# 2. Authenticate
anypoint-cli-v4 account:login
cargo login $(anypoint-cli-v4 pdk get-token)

# 3. Update group_id in Cargo.toml and definition/exchange.json
#    Replace 37289ac4-... with your organization's business group ID

# 4. Build, test and publish
make setup && make test && make build && make release
```

---

## Configuration Reference

### `explainability_fields` *(required)*

Each entry defines both the instruction injected into the LLM AND the validation rule:

```yaml
explainability_fields:
  - field: "decision_outcome"
    description: "The final decision made by the LLM"
    field_type: "string"
    required: true
    allowed_values: ["approved", "rejected", "escalated"]

  - field: "confidence_score"
    description: "LLM confidence level (0.0-1.0)"
    field_type: "number"
    required: true
    validation_min: 0.0
    validation_max: 1.0

  - field: "reasoning"
    description: "Key factors behind the decision"
    field_type: "array"
    required: true
    validation_min: 1
```

### Other Configuration Fields

| Field | Default | Description |
|-------|---------|-------------|
| `validation_on_failure` | `log_only` | `log_only`, `flag`, or `block` |
| `validation_minimum_compliance_percentage` | `100` | 0–100 |
| `prompt_custom_preamble` | `""` | Context text before the compliance instruction |
| `scope_enforce_for_paths` | `[]` | Paths to enforce on (empty = all) |
| `scope_response_validation_enabled` | `false` | Enable response body reading |

---

## Enforcement Modes

| `validation_on_failure` | `scope_response_validation_enabled` | Behavior |
|------------------------|-------------------------------------|----------|
| `log_only` | `false` | Inject only. Trace ID on request/response. |
| `log_only` | `true` | Inject + extract + log metadata. Never block. |
| `flag` | `true` | Inject + log + add `X-Explainability-Status` header. |
| `block` | `true` | Inject + log + replace body with error JSON on non-compliance. |

---

## End-to-End Testing Example

This example uses the OpenAI Chat Completions endpoint through an Omni Gateway LLM proxy.

### Policy Configuration (API Manager)

```yaml
explainability_fields:
  - field: "decision_outcome"
    description: "The recommendation made"
    field_type: "string"
    required: true
    allowed_values: ["approved", "rejected", "escalated"]

  - field: "confidence_score"
    description: "Confidence level between 0 and 1"
    field_type: "number"
    required: true
    validation_min: 0.0
    validation_max: 1.0

  - field: "reasoning"
    description: "Key factors influencing the recommendation"
    field_type: "array"
    required: true
    validation_min: 1

validation_on_failure: "log_only"            # Start here — safe
scope_response_validation_enabled: true       # Enable response reading
```

### Test Request

```bash
curl -X POST https://<your-gateway>/vp-openai/chat/completions \
  -H "client_id: <your-client-id>" \
  -H "client_secret: <your-client-secret>" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [
      {"role": "user", "content": "Should we approve loan application APP-2024-001? The applicant has a credit score of 720 and annual income of $85,000."}
    ]
  }'
```

### Expected LLM Response (Compliant)

The policy injects a system message instructing the LLM to include metadata. The LLM response will end with:

```
...analysis text...

```json
{
  "explainability_metadata": {
    "decision_outcome": "approved",
    "confidence_score": 0.87,
    "reasoning": [
      "Credit score 720 exceeds minimum threshold of 650",
      "Income $85,000 supports the requested loan amount",
      "Debt-to-income ratio is within acceptable range"
    ]
  }
}
```
```

### Expected Policy Logs

```
[llm-explainability-enforcement] trace=a1b2c3d4-... Compliance system prompt injected. Path: /vp-openai/chat/completions
[llm-explainability-enforcement] trace=a1b2c3d4-... explainability_metadata: {"decision_outcome":"approved","confidence_score":0.87,"reasoning":["..."]}
{"event":"llm_explainability_validation","trace_id":"a1b2c3d4-...","compliance_status":"compliant","compliance_percentage":100.0,"valid_fields":["decision_outcome","confidence_score","reasoning"],...}
[llm-explainability-enforcement] trace=a1b2c3d4-... Response COMPLIANT
```

### Test Block Mode

Switch to block mode:
```yaml
validation_on_failure: "block"
```

To force a non-compliant response (for testing), temporarily add a field the LLM won't know about:
```yaml
  - field: "audit_token"
    field_type: "string"
    required: true
    # Not mentioned in prompt_custom_preamble → LLM won't include it → BLOCKED
```

Expected blocked response body:
```json
{
  "error": {
    "message": "LLM response does not meet explainability requirements. Missing: audit_token. Trace: a1b2c3d4-...",
    "type": "explainability_compliance_error",
    "code": "explainability_non_compliant",
    "data": {
      "missing_fields": ["audit_token"],
      "compliance_percentage": 75.0
    }
  }
}
```

> The HTTP status code remains `200`. Check `error.code == "explainability_non_compliant"` in your application.

---

## Audit Trail

Every request generates a trace ID (`X-Explainability-Trace-Id`) that links the request injection log, the response validation log, and the compliance result. This provides a complete chain of evidence for regulatory audits:

| Evidence | From |
|----------|------|
| Governance was enforced | `[llm-explainability-enforcement] trace=xxx Request tagged` |
| Compliance instruction sent | `trace=xxx Compliance system prompt injected` |
| LLM included metadata | `trace=xxx explainability_metadata: {...}` |
| Validation result | `{"event":"llm_explainability_validation","compliance_status":"compliant",...}` |
| Config in force | `config_hash` in every validation log entry |

---

## LLM Provider Compatibility

| Provider / Endpoint | Injection | Response Validation | Full Support |
|--------------------|-----------|---------------------|-------------|
| OpenAI Chat Completions (`/chat/completions`) | ✅ | ✅ | ✅ |
| OpenAI Responses API (`/responses`) | ✅ | ❌ (SSE pipeline) | Partial |
| Anthropic Claude (`/messages`) | ✅ | Depends on proxy | Varies |
| Gemini (OpenAI-compat) | ✅ | Depends on proxy | Varies |

---

## License

Copyright 2025 Salesforce, Inc. All rights reserved.