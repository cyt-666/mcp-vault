# ADR-0023: Preserve source language and persist multilingual retrieval metadata

- Status: Accepted
- Date: 2026-09-03
- Amends: ADR-0007, ADR-0010, ADR-0013, and ADR-0016

## Context

Phase 1 and Phase 2 use English system prompts without an output-language
contract. Providers may therefore turn Chinese source material into English
durable memories. Normal `recall` forwards the caller's natural-language query
unchanged, builds a whitespace-delimited FTS expression, and optionally adds a
query embedding. A Chinese request can consequently miss an English memory
when memory embeddings are unavailable or not reliably cross-lingual. The same
FTS shape also treats an unspaced Chinese sentence as one token, so paraphrases
can miss even when query and memory use the same language.

Relying only on query-time translation would add a live LLM dependency to
normal recall. Storing bilingual memory bodies would make canonical facts
noisy and duplicate semantic content. Requiring a particular embedding model
would violate the configured Provider boundary and the lexical degradation
contract.

## Decision

Canonical memory content remains one concise proposition in the primary
language of its supporting source. Phase 1 preserves the note's primary
language, and Phase 2 preserves the language of current memory on updates while
using the supporting raw inputs' primary language for creates.

MCP Vault maintains Vault-scoped, derived multilingual retrieval metadata for
each eligible memory/content hash. The metadata contains a validated BCP-47
source language and bounded search aliases for the source language, Simplified
Chinese (`zh-Hans`), and English (`en`). Aliases are search-only projections:
they are not facts, provenance, confidence, or canonical memory body content.
They are never returned by normal recall and may be regenerated.

Alias generation runs as a separate durable `memory.enrich_retrieval` job using
the existing `memory_consolidation` binding. New or changed memories admit the
job automatically. Existing active, stale, and superseded memories are
backfilled only after an authenticated Admin explicitly requests it. Normal
recall never waits for this job and never invokes an LLM.

An Admin backfill may make an equivalence-preserving language rewrite of an
existing memory when current exact sources provide a bounded language sample.
It keeps the memory ID, lifecycle, type, provenance, relationships, validity,
and audit/history boundaries. If current source evidence is unavailable, the
job records source language `und`, adds only `zh-Hans`/`en` aliases, and does
not rewrite the body.

Memory FTS indexes deterministic Latin tokens and overlapping Han bigrams from
canonical content and aliases. Natural-language terms form an escaped OR query
rather than an all-terms AND query. Stored aliases therefore provide offline
cross-language candidate generation after their asynchronous enrichment has
completed.

Vector search scopes candidates by object type before Top-K selection. Recall
weights reciprocal rank by the non-negative cosine value instead of discarding
the actual similarity. A fixed positive similarity cutoff is not imposed,
because recall favors avoiding cross-language false negatives and embedding
score distributions differ by model.

## Consequences

- Cross-language recall can work while the embedding Provider is unavailable,
  once persisted aliases cover the current memory hash.
- Alias coverage is asynchronous and explicit in recall/Admin diagnostics;
  missing coverage degrades retrieval but never hides or corrupts canonical
  memory.
- Existing backfill incurs bounded paid model calls only after Admin
  confirmation. New memory enrichment adds a separate batch call whose failure
  cannot roll back Phase 2.
- Search metadata is derived SQLite state and can be rebuilt. Canonical memory
  bodies and their revisions remain portable Markdown.
- Source-language rewrites create ordinary canonical revisions through Vault
  Core and can be restored from history.

## Rejected alternatives

### Store every memory body bilingually

Rejected because it duplicates facts, increases recall tokens, and makes
canonical Markdown harder for people to read and edit.

### Translate each recall query with an LLM

Rejected because normal recall must remain low-latency and must not require a
live LLM request.

### Depend only on multilingual embeddings

Rejected because Provider availability and model capability are optional and
the service must retain an explainable local degradation path.

### Run a destructive memory-pipeline cutover

Rejected because this change can preserve existing canonical memory and Stage
1 state. An Admin-confirmed in-place backfill is cheaper and keeps identifiers,
history, and provenance.
