# Long-Term Memory System

This document is the normative memory design for MCP Vault v2.1. ADR 0026
supersedes the prerelease lifecycle, candidate, two-phase consolidation, and
source-health designs. Their tables may remain as non-destructive migration
input, but no MCP, Admin, resource, embedding, or recall path treats them as
current memory.

## 1. Contract

The model-visible memory domain has one state: **current**. A successful delete
physically removes the current memory projection and its canonical current
Markdown. Revision history and backups remain governed by Vault retention, but
they are not addressable through memory IDs and are never searched by a model
route.

There are exactly two ownership modes:

- `explicit`: a user, Agent, Admin, or importer owns one independent memory;
- `note_derived`: one item in the single current set owned by one source File
  ID.

There is no archive, restore, supersede, candidate approval, raw-memory inbox,
global consolidation generation, or query-time memory model.

## 2. Canonical and operational data

Current knowledge is materialized in the Vault reserved namespace:

```text
.mcp-vault/memory/current/explicit/{memory_id}.md
.mcp-vault/memory/current/sources/{source_file_id}.md
```

The exact reserved root is configuration-owned. Explicit files contain the
caller-supplied proposition and optional metadata. A source-set file contains
the source File ID, source path, full source content hash, set revision, pause
flag, extraction profile, and every current item in deterministic order.

SQLite owns operational coordination:

- `memory_current_items` and `memory_current_sources` are current projections;
- `memory_note_sets` stores one row per source File ID;
- `memory_note_set_snapshots` stores validated, prepared whole-set writes;
- `memory_current_idempotency` and
  `memory_current_explicit_reservations` make explicit commands retryable;
- `memory_current_fts` and vectors are rebuildable retrieval projections;
- legacy memory tables are migration input only.

Every row, query, reservation, snapshot, FTS row, vector, job, and audit entry
is Vault-scoped. The repository makes a row readable only when its canonical
file is current. A note-derived row additionally requires a live source entry
with the exact stored full-content hash.

## 3. Source-owned extraction

Automatic extraction operates on one Markdown source note at a time:

```text
Vault Core read
  -> stable File ID + exact revision + full content hash
  -> one structured generation call
  -> validate {"memories":[...]}
  -> prepare complete replacement snapshot
  -> atomically write canonical set Markdown
  -> atomically replace the source's projection
  -> schedule rebuildable embeddings
```

The model may propose only:

```json
{
  "memories": [
    {
      "content": "The Alpha team uses Rust 1.95 for backend builds.",
      "kind": "decision",
      "tags": ["backend"]
    }
  ]
}
```

`kind` and `tags` are optional. The service owns IDs, source identity,
provenance, revisions, actions, history, canonical paths, confidence,
importance, and database state. Invalid/unknown kinds and invalid, duplicate,
or excess tags are dropped with a bounded content-free warning; they never
discard otherwise valid content. Missing/invalid required content, a missing
or oversized `memories` array, and ambiguous root structure fail the complete
replacement. Duplicate normalized propositions are collapsed and secret-like
text is redacted before publication.

The extraction prompt requires complete useful coverage while preserving the
exact subject, scope, condition, exception, date, uncertainty, negation, and
non-adoption status. Durable technical or reference knowledge is allowed; the
content need not be autobiographical. Note text is untrusted evidence and
cannot instruct the extractor.

An empty array is a valid current empty set. A replacement is all-or-nothing;
there is no `supersedes` edge and no partial merge. Exact normalized items may
retain their stable item ID while their revision advances. Removed items cease
to exist in the current projection.

### Source identity rules

- Same File ID and same content hash: a move updates navigation metadata and
  canonical set Markdown without calling a model.
- Same File ID and changed hash: the old set becomes unreadable immediately,
  before extraction is queued or completed.
- Deleted source: the old set is unreadable immediately.
- A new File ID is a new owner even if its path or bytes resemble an old note.
- If Vault Core explicitly recreates a deleted path by restoring its tombstone
  and retains the File ID for note-history continuity, the deleted memory set
  still does not return: the next extraction allocates a fresh set and item
  identities.
- No heuristic path/content scan may rebind ownership.

### Crash recovery

A validated full-set snapshot is committed before its canonical write. A retry
adopts only byte-identical canonical output, then atomically publishes the
projection and marks the snapshot applied. If source revision/hash or expected
set revision changed, the proposal conflicts and must be regenerated. This
prevents both half-published sets and stale model output from becoming current.

## 4. Explicit remember and update

`remember` directly creates an `explicit` current memory. It does not invoke an
extraction or consolidation model. Omitted `kind`, `importance`, and
`confidence` stay absent; the server never invents default semantic scores.
Tags, entities, validity, provenance coordinates, and caller metadata are
preserved after validation and secret redaction.

An idempotency key is bound to a request hash. The service reserves the stable
memory ID and creation time before writing the canonical file. A retry may
adopt only the exact canonical bytes left by the same reservation; using the
key for different input fails closed.

Updates are explicit-memory-only and require the expected item revision.
Canonical bytes are written through Vault Core before the current projection
advances. A retry after an interrupted projection commit adopts only exact
already-written bytes. Note-derived content is updated by editing/re-extracting
its owning note, not by an item patch.

## 5. Delete and pause

`forget_memory` means delete, not archive:

- explicit: delete its canonical current Markdown and current projection;
- note-derived: prepare and publish the owning set without that item, then set
  `extraction_paused=true` for the source.

The response contains only deletion metadata, never the deleted body. The
deleted ID is immediately unavailable to get, list, recall, MCP resources, and
embedding-source resolution. Manual deletion cannot be undone by a background
re-extraction. An authenticated Admin must explicitly resume the source; the
resume is set-revision-aware and queues a forced one-call replacement.

## 6. Recall

Recall is current-only and performs no live generation or reranking LLM call.
It combines:

- FTS candidates with deterministic lexical-overlap gating;
- exact context entity/tag matches;
- optional current vectors whose model, projection version, profile hash,
  source content hash, chunk identity, and input hash all match;
- optional ordinary-note results from the index service.

Candidate admission is relevance-gated. Recency alone cannot cause a memory to
be returned. Semantic raw cosine is retained as `semantic_cosine` for
diagnostics; the public `score` is a calibrated fusion score and must not be
described as cosine similarity. Missing/unavailable vectors degrade to local
retrieval instead of widening relevance.

The output budget accounts for the whole returned object, including body,
metadata, and optional provenance. An oversized candidate is skipped even when
it ranks first, allowing later fitting candidates to be returned. The first
item is not exempt. `available_*`, `truncated`, and degradation codes describe
the bounded result without exposing hidden history.

Ordinary-note semantic indexing preserves heading, paragraph, list, code-block,
and table-row adjacency in its plain-text projection. It packs overlapping
UTF-8 byte-bounded windows, prefers a nearby block/line/sentence boundary,
attaches the nearest local heading as context, and covers the document
sequentially through the tail. An explicit coverage diagnostic is required if
a configured safety limit is ever reached. Vector hits are aggregated per
note/current-memory object before ranking so a long object cannot occupy
multiple result slots.

## 7. Embedding freshness

Every vector stores:

- provider/model identity and embedding dimension;
- projection version;
- source object type, ID, chunk key, and content hash;
- `profile_hash`, derived from non-secret provider/model/settings/capability
  inputs and projection version;
- `input_hash`, derived from the exact prepared text plus the source identity.

Scheduling, status, source resolution, and recall all apply the same freshness
predicate. Changing a source, preprocessing rule, model binding, endpoint, or
relevant settings makes the old vector ineligible. Vectors are never the only
copy of knowledge and can always be rebuilt.

## 8. Migration from prerelease memory

Migration is never automatic or destructive. Authenticated Admin flow is:

1. take a backup and run `POST /memory/migration/preflight`;
2. inspect content-free counts and IDs for safe explicit, note-derived, mixed,
   unsupported, and historical rows; the returned confirmation hash also binds
   every legacy field that apply would consume;
3. run `POST /memory/migration/execute` with confirmation
   `MIGRATE_MEMORY_V2_1`;
4. regenerate note-derived memory from current source notes when extraction is
   configured;
5. retain legacy tables/history until the operator separately retires them.

Unambiguous active explicit/import rows preserve their original Memory ID and
validated optional metadata. Note-derived rows are regenerated under File-ID
ownership. Mixed or unsupported provenance is reported and never guessed.
Non-active legacy lifecycle rows are historical and remain outside all current
model paths. Migration does not delete legacy rows.
Execute recomputes the classified-state digest while holding the per-Vault
memory write lock and fails before its first canonical write if the reviewed
preflight changed, including changes that leave all classification counts
unchanged.

## 9. Jobs and control plane

Production registers only the memory jobs needed by v2.1:

- `memory.extract`: one-call source-set extraction/backfill;
- `memory.source_reconcile`: event-driven File-ID/hash/path reconciliation;
- `embedding.rebuild`: rebuild note or memory vectors.

Startup retires obsolete prerelease memory jobs without deleting user data.
The Admin API exposes current CRUD, extraction status/run, source resume,
migration preflight/execute, and embedding status/rebuild. It exposes no
archive/restore/merge, candidate inbox, source-health audit, multilingual
backfill, consolidation, or pipeline-reset route.

## 10. Acceptance

The deterministic regression corpus contains at least 40 retrieval queries,
including 10 no-answer/hard-negative cases, and at least 15 labeled generation
cases. Reports include corpus fingerprint, pipeline version, Recall@5, MRR@5,
no-answer false-return rate, support precision, fact coverage, subject errors,
condition/negation errors, type errors, duplicates, and per-case details.

Deterministic fake outputs validate wiring and accounting, not real model
quality. Real-provider evaluation is opt-in only and requires explicit data and
cost authorization. Required integration coverage includes Vault isolation,
hash invalidation, move-without-model, whole-set replacement, delete/pause,
explicit resume, exact-vector freshness, output budgets, and crash adoption.
