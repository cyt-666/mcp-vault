# WP-10 Provider, Embedding, and Vector Subsystem

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-21
- Updated: 2026-08-21

## Purpose and user-visible result

Add an optional, replaceable semantic-enrichment subsystem. Later Admin and
memory/index services can configure provider/model bindings, test providers,
generate structured LLM output, create embeddings, and query Vault-partitioned
vectors. Provider failures remain observable and retryable without blocking
canonical Vault reads, writes, WebDAV, or lexical search.

## Governing requirements

- AGENTS.md: provider secrets are encrypted; providers cannot own canonical
  file writes or SQL projections; all Vault data and vector rows are
  Vault-scoped; external input and provider responses are untrusted.
- Product requirements sections 3.3, 3.5, 3.6, 3.9, 4.2, and 6.
- Architecture sections 3, 4.5-4.6, 7.1, 9, 10.2, and 12.
- Interfaces sections 8, 10.3-10.5, and 11.
- Data model sections 4, 10, 14-15, and 19.
- Security sections 12-18.
- Memory system sections 15-20.
- Accepted ADR-0001, ADR-0002, ADR-0005, ADR-0006, and ADR-0008, plus the
  provider/vector decisions recorded during this plan.

## Starting repository state

- crates/providers now owns provider policy, adapters, transport, embedding
  orchestration, and the VectorIndex boundary.
- crates/state now owns provider/model/binding/health and embedding/vector
  repositories with strict Vault predicates.
- migrations/0006_provider_vector_state.sql is the forward-only provider/vector
  state migration.
- crates/auth already exposes installation-scoped encrypted secret storage and
  redaction-aware SecretString/MasterKeyRing APIs.
- crates/server/src/workers.rs has durable leased jobs and the WP-09 index
  handler, but no provider/embedding job handler.
- WP-12 owns Admin HTTP/UI routes; this package provides application services
  and state seams without exposing provider CRUD through a protocol handler.

## Scope

### Included

- Add typed Provider/Model/Embedding IDs and provider policy values.
- Add migration 0006_provider_vector_state.sql for providers, models,
  global/Vault model bindings, provider health, embedding metadata, and
  Vault-scoped exact-vector storage.
- Add SQLx provider/model/binding/health/embedding repositories with strict
  Vault predicates, binding inheritance, dimension/model checks, and
  coverage queries.
- Implement ProviderService with typed configuration, encrypted
  installation secrets through AuthService, model discovery/test seams,
  health records, and redacted errors.
- Implement SSRF-safe HTTP transport with explicit provider mode, DNS/IP
  checks, no unsafe redirects, request/response bounds, timeout, retry, and
  bounded concurrency.
- Implement OpenAI Responses/compatible structured generation, Anthropic
  Messages generation, OpenAI-compatible embeddings, and an optional local
  FastEmbed adapter.
- Implement VectorIndex, exact cosine similarity, SQLite-backed vector
  persistence, dimension/model/Vault partition enforcement, and deterministic
  ranking.
- Add durable embedding/re-embedding job admission and a generic source
  resolver seam for later index/memory services.
- Add local fake-provider contract, invalid response/auth/rate-limit/timeout/
  redirect, policy, dimension, inheritance, outage, vector isolation, and
  model-change re-embedding tests.
- Update provider/data-model/security/operations documentation and checksums.

### Not included

- Admin HTTP/UI routes and React provider pages (WP-12).
- Memory extraction, canonical memory materialization, or memory recall
  orchestration (WP-11).
- Query-time semantic MCP tools; WP-09 lexical search remains the safe
  default until later integration.
- Real paid provider calls in tests or CI.

## Invariants and risks

- Provider configuration is operational state; embeddings and vectors are
  derived and rebuildable. No provider or vector row is canonical knowledge.
- Every embedding/vector query requires a VaultContext. Provider definitions
  are global configuration, while Vault model bindings and data projections
  are explicitly scoped.
- A remote request is made only after endpoint policy validation, DNS/IP
  checks, Vault privacy-mode checks, request-size checks, and bounded
  concurrency acquisition.
- Authorization headers and secret material never enter logs, errors, test
  snapshots, or API response DTOs.
- Provider responses are untrusted: status, content type, body size, JSON
  shape, structured schema, embedding dimensions, and model identity are
  validated before persistence.
- Retry is limited to transport/timeouts, 408, 429, and 5xx. Invalid
  credentials, unsupported models, schema failures, SSRF, and dimension
  mismatches are terminal until configuration changes.
- A provider outage cannot fail a canonical write or make lexical search
  unavailable.
- Re-embedding jobs carry only Vault/object/model references and bounded
  progress metadata; source content is resolved by the owning application
  service, not copied into durable job payloads.

## Proposed design

Application service -> ProviderService/EmbeddingService
  -> state provider/model/binding/embedding repositories
  -> AuthService installation-secret decrypt boundary
  -> provider adapter and SSRF-safe HTTP transport
  -> VectorIndex exact SQLite fallback
  -> durable embedding.rebuild job

crates/providers owns typed provider DTOs, policy, adapters, transport,
structured-output validation, embedding orchestration, vector interfaces, and
provider errors. It may depend on domain/state/auth and must not depend on
Axum, MCP, WebDAV, indexer, memory, or the frontend.

crates/state owns all SQL and migration conversions. Provider rows are global
configuration; model bindings have nullable global scope plus an optional
Vault override. Embedding metadata and vector bytes always carry Vault
identity. The exact backend stores normalized f32 vectors in SQLite as a
rebuildable BLOB; the VectorIndex trait keeps a future sqlite-vec
implementation replaceable.

Provider adapters share bounded transport but translate their own contracts:
OpenAI Responses structured JSON, OpenAI-compatible chat/embeddings,
Anthropic Messages, and optional FastEmbed blocking inference.

ProviderService resolves a model binding using Vault override first and global
fallback, reads the provider secret only at the request boundary, applies
ProviderMode, and records redacted health/test results. It never persists raw
prompts, note bodies, or provider responses.

## Work breakdown

1. Add this plan, inspect current provider/auth/state/worker seams, and record
   the provider/vector ADR.
2. Add domain IDs, migration 0006, state records/repositories, migration and
   Vault-isolation tests.
3. Implement provider configuration/secret/health services and binding
   inheritance with typed validation tests.
4. Implement SSRF-safe transport, retry/concurrency/privacy policy, adapter
   contracts, and local fake-provider tests.
5. Implement embedding orchestration, exact SQLite vector index, optional
   FastEmbed path, dimension/model checks, and vector isolation tests.
6. Add durable embedding/re-embedding job admission/source-resolver seam and
   model-change coverage.
7. Update docs/checksums, run Rust/frontend/docs acceptance checks, and archive
   this plan only after all WP-10 checks pass.

## Progress

- [x] 2026-08-21 — Read root and ordered specifications plus provider,
  security, memory, admin, data-model, and worker seams.
- [x] 2026-08-21 — Create WP-10 ExecPlan before implementation.
- [x] 2026-08-21 — Add migration/state/provider configuration repositories,
  including revisioned global/Vault bindings, health, embedding metadata, and
  exact SQLite vector BLOB state.
- [x] 2026-08-21 — Implement provider policy, encrypted installation-secret
  resolution, SSRF-safe transport, retries, bounded concurrency, and OpenAI /
  Anthropic / FastEmbed adapter contracts.
- [x] 2026-08-21 — Implement embeddings, Vault/model/dimension-partitioned
  vectors, deterministic exact cosine search, coverage, and reference-only
  re-embedding admission/resolution seams.
- [x] 2026-08-21 — Add local fake-provider contract tests, schema/auth/rate
  limit/timeout/redirect/content-type checks, dimension and model-change
  coverage, Vault isolation tests, and provider/security/operations docs.
- [x] 2026-08-21 — Run formatting, Clippy, workspace Rust tests, Admin
  frontend checks, documentation checks, and checksum validation.

## Decisions

- Use a project-owned adapter/transport layer rather than exposing an SDK or
  raw HTTP client to callers. OpenAI-compatible providers share transport but
  retain separate response translators.
- Use reqwest with redirects disabled and validated DNS pinning for HTTP
  transport. Provider base URLs are never accepted from MCP requests.
- Use SQLite BLOB exact-cosine storage as the mandatory fallback. Native
  sqlite-vec remains an optional future backend behind VectorIndex; no
  extension becomes the only vector copy.
- Use FastEmbed 5.17.4 behind the fastembed feature, with model downloads
  disabled in normal tests and inference isolated on spawn_blocking.
- Use global provider definitions plus global/Vault binding inheritance.
  Provider secrets are installation-scoped encrypted records owned by the
  provider ID, never plaintext provider rows.

## Surprises and discoveries

- The provider crate and memory crate are currently stubs, while auth already
  provides the correct encrypted installation-secret boundary.
- The workspace has no native sqlite-vec dependency; the mandatory exact
  SQLite backend is implemented first behind the internal trait.
- Provider API contracts vary materially: Responses, Chat Completions, and
  Anthropic Messages need separate response extraction and validation.

## Validation

Commands:

    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test -p mcp-vault-providers --all-features --test providers -- --test-threads=1
    cargo test -p mcp-vault-state --all-features
    cargo test -p mcp-vault-server --all-features
    cargo test --workspace --all-features
    pnpm --dir frontend/admin lint
    pnpm --dir frontend/admin test
    pnpm --dir frontend/admin build
    bash scripts/check-docs.sh
    shasum -a 256 -c SHA256SUMS

All provider tests use local fakes. Official paid-provider calls, external
MCP conformance, and Obsidian Litmus remain release-environment checks.

## Rollback and recovery

Migration 0006 is forward-only. Removing embedding/vector rows or rebuilding
them never removes Markdown, revisions, memories, or provider secrets.
Interrupted provider jobs remain leased/reclaimable through the existing job
repository. A provider outage leaves source operations committed and records
a retryable job/health failure. If optional FastEmbed initialization fails,
only that local model path is disabled.

## Outcomes

WP-10 is complete. The service now has a replaceable ProviderService boundary
with encrypted provider credentials, global provider/model state, Vault-first
model binding inheritance, redacted health state, local and remote privacy
policy, SSRF-safe bounded HTTP transport, structured-output validation, HTTP
embeddings, optional FastEmbed local embeddings, and deterministic model
discovery seams.

Embedding metadata and exact-cosine f32 vectors are persisted with strict
Vault/model/dimension predicates. Coverage and vector deletion are exposed as
derived-state operations. Re-embedding admission stores only Vault/object/
chunk/model references in durable `embedding.rebuild` jobs; the source resolver
remains an explicit seam for WP-11's canonical note/memory services.

The mandatory backend is the rebuildable SQLite BLOB fallback. Native
`sqlite-vec` is intentionally not introduced in this package, so no external
extension becomes the only vector copy. FastEmbed is compiled and policy
tested under `--all-features`; model download/runtime inference remains an
operator-environment concern and is not performed in CI.

Admin HTTP/UI provider CRUD remains WP-12, and the server worker registration
for source-specific re-embedding remains with the owning index/memory package.
These are deliberate boundaries, not missing protocol behavior in WP-10.
