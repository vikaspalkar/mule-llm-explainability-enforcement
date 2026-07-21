# LLM Explainability Enforcement Policy

Enforces AI explainability and transparency requirements on LLM API responses. Automatically injects a structured compliance instruction into every LLM request and validates that responses include the required explainability metadata.

## What It Does

**Inbound (Request):** Automatically injects a compliance system message into LLM requests instructing the model to include structured `explainability_metadata` in its response. Supports OpenAI Chat Completions, Anthropic Messages, and OpenAI-compatible endpoints.

**Outbound (Response):** When enabled, reads the LLM response, extracts the `explainability_metadata` JSON block, validates it against your configured field rules, and enforces compliance through logging, flagging, or blocking.

## Key Features

- **Auto-injection** — generates and injects a structured system prompt from your field definitions
- **Field validation** — type checking, range validation, allowed values, conditional requirements
- **Three enforcement modes** — `log_only`, `flag`, `block`
- **Audit trail** — every request tagged with `X-Explainability-Trace-Id` for end-to-end correlation
- **Multi-provider** — OpenAI (Chat Completions), Anthropic Claude, and OpenAI-compatible LLMs
- **Two-step PDK architecture** — uses `into_headers_state() → into_body_state()` pattern (same as MuleSoft A2A Prompt Decorator)

## Quick Start

### 1. Apply the Policy

Add to your LLM proxy API instance in API Manager.

### 2. Minimum Configuration

```yaml
explainability_fields:
  - field: "decision_outcome"
    field_type: "string"
    required: true
    allowed_values: ["approved", "rejected"]
  - field: "confidence_score"
    field_type: "number"
    required: true
    validation_min: 0.0
    validation_max: 1.0
  - field: "reasoning"
    field_type: "array"
    required: true
    validation_min: 1

validation_on_failure: "log_only"
scope_response_validation_enabled: true
```

### 3. Test

```bash
curl -X POST https://<gateway>/vp-openai/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Approve loan APP-001?"}]}'
```

Check logs for:
```
explainability_metadata: {"decision_outcome":"approved","confidence_score":0.87,"reasoning":["..."]}
{"event":"llm_explainability_validation","compliance_status":"compliant",...}
```

## Configuration

| Field | Default | Description |
|-------|---------|-------------|
| `explainability_fields` | — | **Required.** Field definitions for compliance metadata |
| `validation_on_failure` | `log_only` | `log_only` / `flag` / `block` |
| `validation_minimum_compliance_percentage` | `100` | Min % of fields that must be valid |
| `prompt_custom_preamble` | `""` | Domain context before the compliance instruction |
| `scope_enforce_for_paths` | `[]` | Paths to enforce on (empty = all) |
| `scope_response_validation_enabled` | `false` | Enable response body reading |

## Endpoint Compatibility

| Endpoint | Injection | Validation |
|----------|-----------|------------|
| `/chat/completions` | ✅ | ✅ (with `scope_response_validation_enabled: true`) |
| `/responses` | ✅ | ❌ (SSE pipeline limitation) |
| Anthropic `/messages` | ✅ | Depends on proxy |

## Source

[GitHub Repository](https://github.com/P4A-Policies-for-Agents/llm-explainability-enforcement)