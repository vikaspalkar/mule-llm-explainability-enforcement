# Changelog

## 1.1.5 (2026-07-21) — STABLE RELEASE

### Fixed — Reverted request body buffering permanently

v1.1.4 tested `into_headers_body_state().await` in the request filter with the
Chat Completions endpoint. Result: **same hang confirmed on ALL endpoints** for
this Omni Gateway LLM proxy deployment. Body buffering is architecturally
incompatible with this proxy on both request and response sides.

**v1.1.5 is identical to v1.1.3** — headers-only on both sides. This is the
permanently stable implementation for Omni Gateway LLM proxies.

### Final Architecture Decision

| Side | State Machine | Body Access | Can Inject? |
|------|--------------|-------------|-------------|
| Request | `into_headers_state()` | ❌ No | ❌ No |
| Response (default) | `into_headers_state()` | ❌ No | N/A |

**WASM body modification is not possible on this proxy.** System prompt injection
must be configured natively in the Omni Gateway LLM proxy's System Prompt /
Instructions field in API Manager.

### What v1.1.5 provides

1. `X-Explainability-Trace-Id` on every request and response
2. Per-request audit log: `[llm-explainability-enforcement] trace=xxx Request tagged`
3. Response streams through unchanged with trace ID
4. All downstream policies (OOTB message logging, etc.) execute normally
5. Complete audit trail when combined with message logging policy

---

## 1.1.4 (2026-07-21)

### Fixed — Restored request body injection for Chat Completions endpoint

The v1.1.3 headers-only request filter prevented the compliance system prompt
from being injected into LLM requests. This was because `into_headers_state()`
cannot read or modify the request body.

**Root cause of previous hangs:** Request body buffering hangs specifically on
the **OpenAI Responses API** endpoint (`/vp-openai/responses`) which the Omni
Gateway LLM proxy treats as an SSE streaming pipeline. The **Chat Completions**
endpoint (`/vp-openai/chat/completions`) is standard HTTP — request body
buffering works correctly.

**What changed:**
- Request filter: restored `into_headers_body_state().await` + `process_inbound()`
- Response filter: still headers-only when `scope_response_validation_enabled: false`

### Required endpoint change

| Old (hangs) | New (works) |
|-------------|-------------|
| `POST /vp-openai/responses` | `POST /vp-openai/chat/completions` |
| `{"model":"...", "input":"..."}` | `{"model":"...", "messages":[{"role":"user","content":"..."}]}` |

### What you get with v1.1.4 + Chat Completions + validation=false

1. ✅ Compliance system message injected into `messages[]` (GPT-4o mini will include `explainability_metadata`)
2. ✅ `X-Explainability-Trace-Id` on request and response
3. ✅ `llm_explainability_injection` audit log per request
4. ✅ Response body streams through unchanged (no hang)
5. ✅ Message logging policy captures the full response with metadata

---

## 1.1.2 (2026-07-20)

### Fixed — Reverted v1.1.1 request filter; real root cause identified

The v1.1.1 headers-only request fix was incorrect. The AI Semantic Cache policy
(github.com/P4A-Policies-for-Agents/ai-semantic-cache) confirmed that
`into_headers_body_state().await` DOES work on Omni Gateway LLM proxy types.
The semantic cache uses `assetTypes: llm` + `into_headers_body_state()` and works.

**Real root cause:** The Omni Gateway LLM proxy treats the **OpenAI Responses API**
endpoint (`/v1/responses`) as an SSE streaming pipeline. This endpoint never signals
`end_of_stream`, causing any `into_headers_body_state().await` to hang forever.

The **Chat Completions** endpoint (`/v1/chat/completions`) is NOT a streaming pipeline
and body buffering works correctly — exactly as the semantic cache policy demonstrates.

### Action Required: Switch from Responses API to Chat Completions

**Old (broken) request format — causes 502/504:**
```bash
POST /vp-openai/responses
{"model": "gpt-4o-mini", "input": "What documents..."}
```

**New (correct) request format — works fully:**
```bash
POST /vp-openai/chat/completions
{"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "What documents..."}]}
```

With the Chat Completions endpoint:
- Request body injection (system prompt) ✅
- Response body validation and blocking ✅
- No hanging, no 502, no 504 ✅

### Restored

- Request filter: `into_headers_body_state().await` restored (full body injection)
- Full `process_inbound()` call restored (system message injection into messages[])
- `scope_response_validation_enabled: true` works safely on Chat Completions responses

---

## 1.1.1 (2026-07-20) — superseded by v1.1.2

Incorrect fix: switched request filter to headers-only based on wrong diagnosis.
Use v1.1.2 instead.

---

## 1.1.0 (2026-07-20)

### Changed — Simplified Configuration (25 fields → 6 fields)

Removed 19 configurable fields that were either always-correct defaults or provider-agnostic constants. All removed values are hardcoded as Rust `pub const` in `src/config.rs` and are correct for all LLM providers (OpenAI GPT-4/5, Anthropic Claude, Google Gemini, self-hosted LLaMA, etc.).

**Fields REMOVED from configuration screen** (hardcoded):

| Field | Hardcoded to |
|-------|-------------|
| `prompt_injection_enabled` | `true` (always inject) |
| `prompt_injection_mode` | `system_message` (works for all providers) |
| `prompt_response_format` | `json_block` (universal markdown standard) |
| `prompt_response_wrapper_key` | `explainability_metadata` |
| `validation_extraction_strategy` | `json_block` |
| `validation_block_status_code` | `422` |
| `validation_block_message` | default template |
| All 7 `audit_*` fields | all `true` / standard header names |
| `scope_skip_streaming` | `true` |
| `scope_trigger_keywords` | `[]` (always enforce) |
| `scope_exclude_paths` | `[]` (no exclusions) |

**Fields KEPT (6 total):**

1. `explainability_fields` *(required)* — the compliance field definitions
2. `validation_on_failure` — `log_only` (default), `flag`, `block`
3. `validation_minimum_compliance_percentage` — for gradual provider rollout
4. `prompt_custom_preamble` — domain context for the LLM
5. `scope_enforce_for_paths` — target specific endpoints
6. `scope_response_validation_enabled` — injection-only vs full validation

### Policy Configuration Screen — Minimum Required

To activate the policy, you only need to fill in `explainability_fields`.
Everything else has sensible defaults. The complete minimal config for Omni Gateway:

| Field | Value |
|-------|-------|
| `explainability_fields` | Your compliance field definitions |
| `validation_on_failure` | `log_only` (start here, switch to `block` later) |
| `scope_response_validation_enabled` | `false` (leave unchecked — avoids 504) |
| Everything else | leave as defaults |

---

## 1.0.3 (2026-07-20)

### Fixed
- **Critical — 504 gateway timeout**: `into_headers_body_state().await` was called unconditionally whenever `trace_id` was set. The Omni Gateway LLM proxy returns responses as chunked transfer encoding — PDK's body buffering never completes because `end_of_stream` is never signalled, hanging the gateway and producing a 504 timeout. This is the same bug the A2A policy had with `message/stream` SSE responses.

### Added
- **`scope_response_validation_enabled` config field** (default: `false`): Controls whether the response body is buffered for metadata validation.
  - `false` (default, safe for all Omni Gateway LLM proxies): Inbound injection still works. Response streams through unchanged. Only `X-Explainability-Trace-Id` header is added to the response. Use direct OpenAI API testing to verify LLM compliance.
  - `true`: Full outbound field validation enabled. Only set this if your upstream returns a finite response with `Content-Length` set (e.g., self-hosted LLMs or direct OpenAI integration without chunked encoding).

### How to fix the 504

In the policy configuration screen, leave `scope_response_validation_enabled` **unchecked/false** (it defaults to false). This puts the policy in **injection-only mode**: the `instructions` field is injected into every LLM request, but the response is not buffered.

The verified workflow:
1. Policy injects `instructions` → LLM follows compliance instructions
2. LLM returns compliant response with `explainability_metadata` (confirmed via direct API test)
3. Response streams through with `X-Explainability-Trace-Id` header
4. Audit logs show `llm_explainability_injection` event per request

---

## 1.0.2 (2026-07-20)

### Fixed
- **OpenAI Responses API (`input` field) not recognized**: Added `ResponsesApi` as a new LLM API format. The Responses API (`POST /v1/responses`) uses `"input"` instead of `"messages"`, which was being detected as `Unknown` and silently skipped — causing no per-request audit logs and no prompt injection. This was the root cause of the 502 and missing audit logs observed when using the Omni Gateway LLM proxy with the `responses` endpoint.
- **Responses API injection target**: The Responses API uses `instructions` (not `system` messages) for system-level prompts. The policy now correctly injects into the top-level `instructions` field when using `system_message` injection mode. Appends to existing `instructions` content if already present.
- **Responses API response extraction**: The Responses API response format uses `output[*].content[*].text` with `type: "output_text"`. The outbound validator now correctly extracts text from this format.
- **Responses API `status` field for finality check**: Added `status: "completed"` detection (Responses API) alongside the existing `finish_reason` (Chat Completions) and `stop_reason` (Anthropic) checks.

### Added
- `LlmApiFormat::ResponsesApi` variant
- Test fixtures: `request_responses_api.json`, `response_responses_api_compliant.json`
- Unit tests for Responses API injection and instructions field appending

### Root cause of 502 error
The 502 was caused by two issues working together:
1. The request format (`input` field) was not recognized → policy passed through unchanged without injecting `instructions`
2. The upstream LLM backend may have encountered an authentication or routing error independent of the policy

The policy fix resolves issue 1. For issue 2, verify the LLM proxy's upstream authentication and endpoint configuration in API Manager.

---

## 1.0.1 (2026-07-20)

### Fixed
- **Policy not visible for LLM proxies in API Manager**: Changed `metadata/capabilities/assetTypes` in `definition/gcl.yaml` from `http` to `llm,http`. LLM proxy API instances in Omni Gateway are registered under the `llm` asset type; using `http` alone caused the policy to be hidden from the LLM proxy policy picker. The fix adds `llm` while keeping `http` for backward compatibility with standard HTTP APIs.

### How to apply the fix
Rebuild and re-publish:
```bash
cd llm-explainability-enforcement
make build
make release
```
Then in API Manager, refresh the policy list for your LLM proxy instance — the policy will now appear.

---

## 1.0.0 (2026-07-20)

### Added
- Initial release of LLM Explainability Enforcement Policy
- **Multi-format LLM support**: OpenAI Chat Completions, OpenAI Completions (legacy), and Anthropic Messages API
- **Inbound prompt injection** with five injection modes:
  - `system_message` (default) — injects as system prompt, most effective for compliance
  - `user_message_prepend` — prepends instructions to first user message
  - `user_message_append` — appends instructions to last user message
  - `prompt_prefix` — prepends to Completions API `prompt` field
  - `prompt_suffix` — appends to Completions API `prompt` field
- **Outbound response validation** with metadata extraction from:
  - JSON code fences (```json ... ```)
  - Plain code fences (``` ... ```)
  - Inline JSON within natural language text
  - Tagged sections ([EXPLAINABILITY_START] ... [EXPLAINABILITY_END])
- **Three enforcement modes**: `block`, `flag`, `log_only`
- **Streaming request detection** — automatic skip when `stream: true` in request body
- **Anthropic system field injection** — correctly appends to `system` string or array
- **OpenAI system message deduplication** — appends to existing system message rather than creating duplicate
- Configurable compliance thresholds for gradual rollout
- Trace ID generation and propagation for end-to-end audit correlation
- SHA-256 config hashing for audit traceability
- Support for conditional field requirements (`required_when`)
- Multiple extraction strategies: `json_block` and `tagged_section`
- Comprehensive audit logging with injection and validation events
- Response error format using OpenAI-compatible error structure
- Full behaviour matrix: block×compliant, block×non-compliant, flag×compliant, flag×non-compliant, log_only×compliant, log_only×non-compliant