# ADR-0006: LLM and embeddings are pluggable enrichment

- Status: Accepted
- Date: 2026-08-19

## Context

LLMs improve summaries, topics, and memory extraction. Embeddings improve semantic retrieval. However, providers add cost, latency, privacy exposure, availability risk, and model churn.

The external MCP client already contains an LLM, so the server should not become another interactive Agent for ordinary queries.

## Decision

LLM, embedding, reranker, and vector backends are pluggable.

Core capabilities remain available without remote AI:

- WebDAV;
- file operations and history;
- metadata/link index;
- FTS search;
- explicit memory;
- lexical recall.

LLMs run primarily in asynchronous enrichment jobs. Normal recall does not require a live LLM request.

Provider/model versions are recorded with derived output. Provider failures degrade rather than fail core readiness.

## Consequences

Positive:

- local/private operation;
- provider choice and replacement;
- fast predictable recall;
- no API key required for basic service.

Costs:

- more adapter/configuration/job code;
- non-LLM mode has less semantic enrichment;
- model changes require rebuild/re-embedding.

## Rejected alternatives

- Mandatory cloud LLM.
- Call an LLM and scan the entire Vault on every `recall`.
- Bind the data model to one provider’s API.
