# ADR-0024: Bound embedding inputs and rebuild current-model vectors

- Status: Accepted
- Date: 2026-09-04
- Amends: ADR-0010, ADR-0013, and ADR-0023

## Context

The original note-vector projection bounded chunks by Unicode character count.
A chunk could contain 6,000 characters plus 2,048 context characters. Zhipu
`embedding-3` accepts no more than 3,072 tokens for one input, and a live Vault
note reproduced a non-retryable Provider failure while a short query succeeded.
One over-limit member rejects the complete embedding batch.

The Admin index rebuild is deliberately separate from its asynchronously
scheduled vector jobs. It may therefore complete while semantic coverage stays
empty. Failed jobs expose only `embedding_rebuild_failed`, and their persisted
model ID does not change when an Admin selects a different binding. Note model
binding schedules current chunks, but memory model binding has no equivalent
existing-record backfill.

## Decision

The Index application service owns deterministic versioned note-vector chunks.
The complete text passed for each `text-v2` input, including bounded metadata
context, is limited by UTF-8 bytes and snapped only at valid character
boundaries. Provider adapters do not hide an oversized logical source by
issuing multiple billable calls and pooling their vectors.

Embedding job identities include a project-owned projection version. An
incompatible derived-profile change therefore admits new jobs without mutating
or silently repurposing historical terminal jobs. Job payloads remain
reference-only and resolve current source text at execution.

Embedding workers retain stable redacted Provider error categories. Admin job
details may expose model identity, homogeneous source type, and source count,
but never source text, paths, object IDs, Provider response bodies, or secrets.

Selecting an effective note or memory embedding model schedules missing/stale
vectors in the selected Vault. Existing memory-vector scheduling reads current
active, stale, and superseded projections directly and never invokes memory
extraction or consolidation. Admin also exposes an explicit repeatable
memory-vector scheduling action and coverage view.

## Consequences

- Long multilingual notes generate more, smaller vector chunks and remain
  semantically searchable with lower-limit Providers.
- A chunk-profile upgrade invalidates only derived vector identity; canonical
  notes, memories, revisions, aliases, and FTS remain untouched.
- Model changes and explicit rebuilds use the newly selected model rather than
  retrying an old task with its persisted model ID.
- More chunks may increase embedding request count and vector storage. Bounded
  batches and the existing maximum-chunk cap contain that cost.
- The UTF-8 byte envelope is conservative because vendor tokenizers differ; it
  favors reliable offline rebuild over maximum per-request utilization.

## Rejected alternatives

### Retry the old job after changing the binding

Rejected because a durable job intentionally retains its original model ID.
Changing that payload on retry would make audit and deduplication semantics
false.

### Split oversized inputs inside the Provider adapter and average vectors

Rejected because it can turn one job attempt into several paid calls, makes
partial-call recovery ambiguous, and changes retrieval semantics invisibly.

### Re-run all memory extraction to obtain vectors

Rejected because vectors are rebuildable projections of current memory and do
not require paid semantic regeneration or canonical-memory changes.
