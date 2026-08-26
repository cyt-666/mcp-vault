# ADR-0012: Compose Provider Presets Behind the Shared Secure Transport

- Status: Accepted
- Date: 2026-08-24

## Context

ADR-0010 established project-owned Provider adapters and one SSRF-safe bounded
transport. Deployment testing then showed that treating every
OpenAI-compatible endpoint as one dialect is not interoperable. DeepSeek,
MiMo, GLM, Kimi, Gemini, and Qwen differ in structured-output mode,
token-limit field, thinking control, temperature behavior, API-root path, and
model-list support.

The same model ID may also be served by an official endpoint, Ollama, vLLM, or
a proxy. Inferring a vendor solely from a model name can send unsupported
extensions or secrets to the wrong protocol surface.

## Decision

MCP Vault adds first-class Provider kinds for DeepSeek, Xiaomi MiMo, Zhipu GLM,
Moonshot/Kimi, Google Gemini, and Alibaba Qwen/DashScope. They share the
OpenAI-compatible adapter and `ProviderTransport`, but select a documented
provider preset.

Per-model settings keep these concerns independent:

- compatibility preset;
- structured-output mode;
- token-limit field;
- thinking mode;
- bounded one-call generation-token limit.

`auto` resolves from the first-class Provider kind. For legacy generic rows it
may recognize an exact official API host, but it never selects a vendor from a
model name. Proxy domains require an explicit first-class type or model preset.

The configured Base URL is the exact API root. Relative endpoint suffixes are
appended directly; `/v1` is supplied only for backward-compatible host-only
URLs. Every returned structured value is parsed and schema-validated locally.

Provider SDKs may be adopted only if their network execution can be injected
behind the existing transport policy. At this decision date OpenAI publishes
no official Rust SDK, and the evaluated community `openai_rust_sdk` owns an
independent client and requires a newer Rust toolchain, so it is not adopted.

## Consequences

Positive:

- supported vendors are visible Admin/API configuration rather than hidden
  model-name heuristics;
- provider differences remain testable as pure request translation plus local
  fake HTTP contracts;
- new vendors can add a preset without changing memory, MCP, or Admin business
  logic;
- SSRF, secrets, limits, retries, and redaction remain identical across
  providers;
- Zhipu, Gemini, and DashScope versioned API roots are no longer corrupted by
  an inserted `/v1` segment.

Costs:

- official provider documentation must be reverified as models and
  compatibility layers evolve;
- a local fake proves serialization, not paid-account availability;
- some model families vary within one vendor, so operators may need a typed
  per-model override;
- first-class provider and model settings expand the Admin configuration
  surface and require Chinese explanations.

## Rejected alternatives

- One fixed OpenAI Chat Completions request for every compatible endpoint.
- A separate unrestricted `reqwest`/SDK client for each vendor.
- Selecting provider behavior from model ID alone.
- Trusting provider JSON mode without local schema and memory-policy checks.
- Logging provider response bodies to diagnose compatibility failures.
