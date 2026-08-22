# ADR-0010: Project-Owned Provider Adapters and Rebuildable Vector Fallback

- Status: Accepted
- Date: 2026-08-21

## Context

MCP Vault must support several LLM and embedding providers, local endpoints,
optional local models, and future vector backends without allowing provider
SDKs, remote availability, or a pre-1.0 vector extension to become a
canonical-data dependency.

Provider requests also cross a trust boundary. Base URLs, redirects, response
schemas, dimensions, credentials, and privacy policy must be validated in one
application-owned boundary rather than independently by protocol handlers.

## Decision

WP-10 introduces a project-owned ProviderService and adapter traits for:

- OpenAI Responses structured generation;
- OpenAI-compatible chat/structured generation and embeddings;
- Anthropic Messages structured generation;
- optional FastEmbed local embeddings.

Adapters use one bounded SSRF-safe HTTP transport. Redirects are disabled by
default, DNS results are validated before connection, public/HTTP/private
endpoint policy is explicit, requests are size/time/concurrency bounded, and
only transient failures are retried.

Provider definitions and model records are global operational state. Model
bindings resolve a Vault-specific override before the global default.
Provider credentials are installation-scoped encrypted secrets owned by the
provider record; plaintext is never persisted or returned.

Vector persistence is behind a VectorIndex trait. The mandatory backend is a
Vault-scoped SQLite exact-cosine store using normalized f32 vector BLOBs.
Embedding metadata and vectors are derived and rebuildable. A future
sqlite-vec implementation may be added behind the same trait and cannot
replace the exact fallback as the only copy.

## Consequences

Positive:

- provider failure cannot block canonical file operations or lexical search;
- adapter contracts are testable with local fake HTTP servers;
- model/dimension/Vault partition checks happen before vector persistence;
- provider SDK or vector-extension changes do not leak into MCP/Admin APIs;
- local semantic capability can be enabled without remote credentials.

Costs:

- separate response translators are required for Responses, Chat Completions,
  and Anthropic Messages;
- exact cosine is slower than a native vector extension for large corpora;
- model download/cache lifecycle needs explicit Admin/operations work;
- WP-11 must provide source resolvers for actual memory re-embedding.

## Rejected alternatives

- Calling provider SDKs directly from MCP, memory, or Admin handlers.
- Persisting API keys in provider rows or settings JSON.
- Following arbitrary provider redirects.
- Making sqlite-vec or an external vector database the only vector storage.
- Sending an MCP bearer token upstream to a configured provider.
