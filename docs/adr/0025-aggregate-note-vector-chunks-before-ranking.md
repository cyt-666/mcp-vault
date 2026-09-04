# ADR-0025: Aggregate note-vector chunks before note ranking

- Status: Accepted
- Date: 2026-09-04
- Refines: ADR-0010, ADR-0013, ADR-0023, and ADR-0024

## Context

One ordinary note may own up to 128 deterministic `text-v2` embedding chunks.
The current semantic path asks the vector backend for the final note candidate
limit, even though the backend ranks chunk rows. Several high-scoring chunks
from one long note can therefore consume the bounded pool before the Index
service deduplicates by File ID. Duplicate chunks also advance semantic rank,
and note scoring records cosine only as diagnostics while adding a positive
rank-only contribution, including for negative similarities.

The Provider boundary cannot decide whether a chunk is current: only the Index
service can reconstruct the current chunk key/hash from the Vault-scoped note
projection. Aggregating blindly in the vector backend could select a stale
winning chunk and hide a lower-scoring current chunk for the same note.

## Decision

Ordinary-note semantic retrieval treats vector rows as an over-fetched,
bounded candidate pool and performs current-source validation plus object
aggregation in the Index application service. The candidate limit is
`min(10_000, requested_note_pool * MAX_NOTE_EMBEDDING_CHUNKS)`. Vault, model,
dimension, and exact `object_type = note` filtering remain below this boundary.

Candidates are processed in descending cosine order. Negative similarities are
discarded. A candidate must match the current `FileId`, chunk key, and chunk
content hash. The first valid, in-scope candidate for a File ID is that note's
winning chunk; subsequent chunks for the same note neither consume a result
slot nor advance semantic rank. Equal vector scores use object ID, chunk key,
then embedding ID as deterministic tie breakers.

The semantic contribution is the non-negative cosine multiplied by its
reciprocal-rank weight. Existing score-breakdown keys remain stable:
`semantic_cosine` is the raw winning cosine and `semantic_rrf` is the actual
weighted contribution added to the fused score. Lexical matches keep their
lexical snippet in hybrid mode; otherwise the winning semantic chunk supplies
the snippet.

## Consequences

- A long note appears at most once and cannot gain or consume rank merely by
  owning more chunks.
- Existing note vectors remain valid; deployment requires no vector rebuild,
  migration, or additional Provider request.
- `search_notes` and `recall.related_notes` share the corrected Index-service
  behavior without changing their wire schema.
- Exact-cosine candidate work remains bounded by the existing 10,000-row
  fallback limit. Native/ANN scaling beyond that cap is separate work.
- Scores and ordering may change because note semantics now match the
  documented cosine-weighted object-ranking contract.

## Rejected alternatives

### Sum or average every matching chunk

Rejected because summing rewards long notes while averaging dilutes a focused
match with unrelated sections.

### Aggregate only inside the vector backend

Rejected because the backend cannot validate current note chunk hashes and
could hide a current chunk behind a stale higher-scoring row.

### Rebuild one whole-note vector

Rejected because it loses section-level retrieval, adds paid Provider work,
and does not solve current-source validation or hybrid ranking.
