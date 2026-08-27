# Provider Compatibility

## 1. Scope and boundary

MCP Vault supports first-class Provider records for OpenAI, Anthropic,
DeepSeek, Xiaomi MiMo, Zhipu GLM, Moonshot/Kimi, Google Gemini, and Alibaba
Qwen/DashScope. The six latter integrations use their official
OpenAI-compatible HTTP surfaces, but “OpenAI-compatible” does not mean every
request field has the same semantics.

Provider-specific code translates a typed model configuration into a wire
request. Every request still passes through the shared `ProviderTransport` for
DNS resolution, SSRF policy, redirect denial, encrypted-secret injection,
bounded concurrency, request/response size limits, deadlines, redaction, and
cost-safe retry behavior. A preset is not allowed to construct its own HTTP
client.

The Admin `provider_type` values are:

```text
openai_responses
openai_compatible
anthropic_messages
deepseek
xiaomi_mimo
zhipu_glm
moonshot_kimi
google_gemini
alibaba_qwen
embedding_http
```

## 2. Verified compatibility matrix

The following contracts were verified against the linked primary
documentation on 2026-08-24. “Default” describes MCP Vault's request preset,
not every model the vendor may publish later.

| Provider type | Recommended API root | Structured output default | Token-limit field | Thinking default/control | Model list |
|---|---|---|---|---|---|
| `openai_responses` | `https://api.openai.com/v1/` | Responses `json_schema` | `max_output_tokens` | Provider/model default | Provider API |
| `anthropic_messages` | `https://api.anthropic.com/v1/` | Prompt schema plus local validation | `max_tokens` | Provider/model default | Optional/manual |
| `deepseek` | `https://api.deepseek.com/v1/` | `json_object` plus schema prompt | `max_tokens` | `thinking.type`, default enabled | OpenAI-compatible when available |
| `xiaomi_mimo` | `https://api.xiaomimimo.com/v1/` | `json_object` plus schema prompt | `max_completion_tokens` | `thinking.type`, default enabled | OpenAI-compatible |
| `zhipu_glm` | `https://open.bigmodel.cn/api/paas/v4/` | `json_object` plus schema prompt | `max_tokens` | Optional `thinking.type` | Optional/manual |
| `moonshot_kimi` | `https://api.moonshot.ai/v1/` | strict `json_schema` | `max_completion_tokens` | Model default; K2 supports `thinking.type`, K3 always thinks | `/v1/models` |
| `google_gemini` | `https://generativelanguage.googleapis.com/v1beta/openai/` | strict `json_schema` | `max_tokens` compatibility field | Model default; explicit control maps to `reasoning_effort` | compatibility `/models` |
| `alibaba_qwen` | region/workspace `.../compatible-mode/v1/` | `json_object` plus schema prompt | `max_completion_tokens` | Model default; explicit control uses `enable_thinking` | compatibility `/models` where offered |
| `openai_compatible` | exact endpoint API root | strict `json_schema` | `max_tokens` | no vendor extension | best effort/manual |

The generic type does not infer a vendor from a model name. A local Ollama
model named `qwen3` is not necessarily a DashScope endpoint. For compatibility
with Provider rows created before first-class types existed, `auto` recognizes
only exact official API hosts. A proxy or reseller domain must use a
first-class Provider type or an explicit model preset.

The configured Base URL is an API root. MCP Vault appends
`chat/completions`, `models`, or `embeddings` directly. It adds `/v1/` only
when the configured URL has no path, preserving old host-only generic
configuration. It never inserts `/v1` inside Zhipu's `/api/paas/v4/`, Gemini's
`/v1beta/openai/`, or DashScope's `/compatible-mode/v1/` path.

## 3. Typed per-model overrides

`ModelSettings` keeps the axes independent so adding a provider does not create
a combinatorial set of ad hoc request builders:

```json
{
  "openai_compatibility_preset": "auto",
  "openai_structured_output_mode": "auto",
  "openai_token_limit_field": "auto",
  "openai_thinking_mode": "auto",
  "generation_token_limit": null
}
```

Allowed compatibility presets are `auto`, `generic`, `deepseek`,
`xiaomi_mimo`, `zhipu_glm`, `moonshot_kimi`, `google_gemini`, and
`alibaba_qwen`. Structured-output overrides are `auto`,
`strict_json_schema`, `json_object`, and `prompt_only`. Token-field overrides
are `auto`, `max_tokens`, and `max_completion_tokens`. Thinking is `auto`,
`enabled`, or `disabled` and is translated only for a preset with a documented
control.

`generation_token_limit` is the maximum generated-token request for one model
call, not a currency balance or a whole extraction-job quota. Reasoning-first
presets default to a bounded 32,768 tokens; ordinary profiles use the caller's
8,192-token extraction request. An operator value and a lower recorded model
capability both clamp the effective limit. Some providers count reasoning and
visible output together; the Admin UI therefore describes the value as a
per-note upper bound rather than promising a visible JSON size.

Every returned value is parsed and validated locally against MCP Vault's JSON
Schema subset. Provider JSON mode or SDK parsing never bypasses Phase 1
source/evidence validation or Phase 2 reference, deduplication, conflict,
forgetting, and revision-snapshot checks.

## 4. Provider-specific notes

### 4.1 DeepSeek

DeepSeek JSON Output requires `response_format={"type":"json_object"}`, the
word JSON and an expected shape in the prompt, and a sufficient `max_tokens`.
Current V4 thinking defaults enabled, uses `thinking.type`, returns
`reasoning_content`, and ignores sampling parameters while thinking. MCP Vault
therefore omits extraction temperature while thinking is active.

### 4.2 Xiaomi MiMo

MiMo v2.5 JSON mode requires `json_object` and a complete format instruction.
`max_completion_tokens` includes reasoning plus final content. Thinking remains
enabled by default; the operator may explicitly disable it or change the
per-call generation bound. Xiaomi's current structured-output guide explicitly
states that JSON Object mode guarantees valid JSON syntax, not the requested
field hierarchy, and recommends a complete structure template. MCP Vault
therefore includes both the full JSON Schema and the exact Codex Phase 1
three-field root template in the prompt and still validates the result locally.
If Phase 1 returns valid `raw_memory` but omits only auxiliary
`rollout_summary`, MCP Vault copies `raw_memory` verbatim into that field and
reruns the full validator. Unknown or empty objects still fail without issuing
an automatic second paid request. Source provenance is derived locally; MiMo is
never asked for evidence line coordinates. This is `memory-stage1-v4`.

Phase 2 likewise includes an exact compact root template, but does not ask MiMo
to generate or copy durable identifiers, evidence indexes, or mechanically
duplicated raw dispositions. The request labels raw inputs and current memories
with small request-local integers. MiMo returns semantic actions, those bounded
indexes, and explicit discard indexes. MCP Vault maps them back to the captured
snapshot, allocates every create ID locally, expands ready inputs to validated
evidence, and derives `used`, `no_output`, and `withdrawn` state. An out-of-range
reference is reported as a redacted `memory_phase2_*_index_invalid` code; the
Provider response body is never retained. This is the
`memory-consolidation-v4` contract. Older prepared proposals are rejected before
parsing, and revision-only projection drift does not invalidate unchanged
semantic memory.

### 4.3 Zhipu GLM

The API root ends at `/api/paas/v4/`; adding another `/v1` is invalid. GLM
structured output uses JSON Object mode and a prompt-defined structure. The
OpenAI-compatibility guide excludes temperature zero, so MCP Vault omits its
usual deterministic `0.0` value for the GLM preset.

### 4.4 Moonshot/Kimi

Kimi supports both JSON Object and strict JSON Schema. MCP Vault selects strict
schema by default. `max_tokens` is deprecated in favor of
`max_completion_tokens`. Kimi K3 always thinks and cannot be configured as
disabled; K2 variants use the documented `thinking` object when explicitly
overridden.

### 4.5 Google Gemini

MCP Vault uses Google's OpenAI-compatibility endpoint, not the native
GenerateContent/Interactions API. The compatibility layer documents standard
OpenAI structured parsing and `reasoning_effort`; it remains beta. Native-only
Gemini tools and thought-signature workflows are outside this adapter. An
explicit disabled mode is rejected for models whose official documentation
says thinking cannot be disabled.

### 4.6 Alibaba Qwen/DashScope

DashScope endpoints vary by region and workspace; the exact Base URL shown by
the provider console is authoritative. JSON Object mode is the broad default,
while strict JSON Schema can be selected for models documented to support it.
`max_tokens` is being deprecated in favor of `max_completion_tokens`.
Thinking control is the non-standard top-level `enable_thinking` boolean.

## 5. Test and release evidence

CI uses local fake providers and never paid APIs. Unit tests assert the exact
redacted request body for every preset. A transport-backed integration test
creates all six first-class Provider kinds and verifies they pass through the
same bounded HTTP boundary. Admin tests cover type acceptance, Chinese preset
selection, official Base URL templates, and typed model settings.

These tests prove serialization and internal policy, not live account access.
Before advertising a release as live-verified for a provider, run one
non-sensitive structured extraction against that provider's current official
endpoint, record the model ID and date, and retain only sanitized status/usage
evidence.

## 6. Primary references

- OpenAI official SDK/OpenAPI list: https://github.com/openai/openai-openapi
- OpenAI Structured Outputs schema rules: https://developers.openai.com/api/docs/guides/structured-outputs
- DeepSeek JSON Output: https://api-docs.deepseek.com/guides/json_mode
- DeepSeek thinking mode: https://api-docs.deepseek.com/guides/thinking_mode
- Xiaomi MiMo structured output: https://mimo.mi.com/docs/en-US/quick-start/usage-guide/text-generation/structured-output
- Xiaomi MiMo Chat API: https://mimo.mi.com/docs/en-US/api/chat/openai-api
- Zhipu structured output: https://docs.bigmodel.cn/cn/guide/capabilities/struct-output
- Zhipu OpenAI compatibility: https://docs.bigmodel.cn/cn/guide/develop/openai/introduction
- Kimi Chat API: https://platform.kimi.ai/docs/api/chat
- Kimi API overview: https://platform.kimi.ai/docs/api/overview
- Gemini OpenAI compatibility: https://ai.google.dev/gemini-api/docs/openai
- Alibaba Qwen OpenAI-compatible Chat: https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions
- Alibaba structured output: https://help.aliyun.com/zh/model-studio/qwen-structured-output
- Alibaba thinking models: https://help.aliyun.com/zh/model-studio/deep-thinking
