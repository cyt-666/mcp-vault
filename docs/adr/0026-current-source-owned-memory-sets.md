# ADR-0026: Use current source-owned memory sets

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR-0016's two-phase global consolidation architecture,
  ADR-0017's destructive generation reset, ADR-0022's source-health state
  graph, and ADR-0007's model-readable lifecycle/history behavior.
- Amends: ADR-0015 automatic-memory classification and ADR-0023 eligibility.
- Retains: ADR-0001/0007 portable canonical Markdown and provenance, ADR-0013
  separately typed related-note cues, ADR-0023 persisted multilingual search
  aliases, and ADR-0024/0025 vector/chunk validity and object aggregation.

## Context

The prerelease memory system currently maintains two generative phases,
Stage 1 dispositions, a global semantic action protocol, seven final lifecycle
states, supersession relations, continuous per-source health, multiple
aggregate/raw/summary artifacts, and historical read filters. That machinery
does not make ordinary memory management predictable. Deleting a memory
normally archives it, a caller with historical options can retrieve it again,
and a changed source remains represented as stale history. A global model must
also decide create/update/archive/supersede actions that application code can
derive more safely from source ownership.

The same implementation has independent retrieval defects. Recent or important
records can enter ranking without query evidence, any non-negative cosine can
be treated as relevant, chunk windows lose local section context and may sample
long input, and response budgeting does not constrain the first item or try a
shorter item after an oversized one.

Ordinary notes are already canonical and carry stable Vault-scoped File IDs and
content hashes. Explicit memories already have canonical Markdown. Those two
ownership facts provide a simpler consistency boundary than a global history
and consolidation graph.

## Decision

MCP Vault has two current memory ownership classes.

1. A note-derived memory item belongs to exactly one source note. All items for
   that note form one current memory set identified by Vault and stable File ID,
   bound to the exact current source-content hash. The set is materialized as
   one managed Markdown file and replaced in full after a successful bounded
   extraction.
2. An explicit memory belongs to the authenticated user/Agent assertion. It is
   immediately materialized as its own Markdown record and does not disappear
   when any source note changes or is deleted. Attaching a note reference does
   not change this ownership. Converting a derived item to explicit ownership
   is a separate authorized operation.

There is no model-readable memory history. Normal and detail reads, list,
recall, resources, known IDs, indexes, vectors, and context summaries expose
only current published data. A successful forget operation deletes the current
canonical memory and its projections rather than transitioning to archived.
It returns identifiers and effects, not deleted content. Retained Vault Core
revision history, audit, and offline backups are separate recovery controls and
are never memory query sources.

Deleting one note-derived item revision-rewrites its owning set and pauses
automatic extraction for that source. A normal note write does not silently
undo the pause; an authenticated explicit resume/regenerate action does. This
is one source-level control, not a semantic blacklist.

Source events use stable identity and exact hashes:

- a content-hash change immediately makes the old set ineligible, advances its
  set revision, and coalesces extraction for the newest hash;
- a same-File-ID move with unchanged content only updates the current path;
- deleting the source deletes the derived set and invalidates pending work;
- recreating the same path with a new File ID creates a new source;
- an explicit Vault Core tombstone restoration may retain its File ID, but
  deletion of the prior memory set is still final and re-extraction allocates a
  fresh set and item identities;
- identical-content writes do not regenerate.

Automatic extraction is one Provider call whose core result is:

```json
{"memories":[{"content":"...","kind":"knowledge","tags":["..."]}]}
```

Only the `memories` array and each non-empty `content` are required. Optional
kind/tag defects are normalized or dropped without losing valid content.
Missing, ambiguous, truncated, or partially valid roots fail the operation; an
explicit valid empty array is a successful empty replacement. The model never
chooses IDs, paths, source coordinates, permissions, status transitions,
confidence, importance, validity, or write actions. Server-generated item IDs
are persisted in one prepared source snapshot so recovery does not repeat a
possibly billable call.

Extraction may retain important knowledge, methods, experiments, project
state, decisions, preferences, and progress. It must preserve the subject,
scope, conditions, negation, uncertainty, and adoption state. Tutorial or
assistant-proposed content does not prove that the owner knows, accepts, or
uses it. This supersedes ADR-0015's local rule excluding general knowledge;
quality is enforced through versioned prompts, deterministic validation, and
labeled evaluation rather than a fixed type allow-list.

Publishing a set checks the current Vault/File ID/source hash, source pause
flag, and expected set revision immediately before the canonical/projection
handoff. Only the published current set is queryable. Source change fails
closed: the old set is not reopened when extraction fails. A manual
regeneration of an unchanged source may retain its old set until a complete
replacement succeeds. One source has at most one prepared publication
snapshot, reused idempotently across crashes.

Explicit metadata remains optional and preserves caller intent end to end.
Omitted update fields retain their value, explicit values replace them, and
explicit clears remove nullable/list values. `created_at` and `updated_at` do
not invent fact-validity dates. Missing confidence/importance is unknown, not
an implicit probability; legacy numeric values may be retained as imported
metadata but do not gain trust merely by existing.

Recall remains a local indexed operation and retains separately typed ordinary
note cues. Candidate generation and relevance acceptance are distinct. A
candidate must have calibrated semantic evidence or strong normalized lexical/
entity evidence before rank, recency, or importance boosts apply. Unrelated
queries may return no results. Public fused `score`, raw BM25/cosine, and each
RRF contribution keep distinct meanings. Vectors must match Vault, object
type, model/profile/dimension, current object/source hash, and exact embedding
input hash. Section-aware chunks carry local headings, cover supported input
from start to end or report an explicit limit, and contribute once per object.
All returned text and metadata share one estimated output budget; oversized
items are skipped while later items are still considered.

## Migration and recovery

The forward schema migration adds current ownership, source sets, extraction
pause/version state, one prepared snapshot, and migration-report state. It does
not delete canonical files. Existing memory becomes unavailable to model reads
until classified. Reliably source-less explicit Agent/Admin assertions can be
preserved as explicit memory. Reliably single-note-derived records are
regenerated from the current note. Cross-source or mixed records remain in a
preflight report until an authenticated operator chooses preservation,
explicit extraction, or deletion; origin alone is insufficient proof.

Canonical cleanup and conversion run only after a backup/preflight confirmation
and use Vault Core. Old `MEMORY.md`, `memory_summary.md`, `raw_memories.md`,
source summaries, Stage 1 rows, consolidation proposals, source-health rows,
supersession relations, and legacy jobs never become current-query inputs after
the cutover. The old tables may temporarily remain as isolated migration input
but are not a second runtime engine.

A daily interrupted publication resumes or rejects only its one persisted
source snapshot. It never replays output invalidated by deletion, pause, a new
source hash, or a new set revision. Filesystem/SQLite reconciliation may
temporarily expose no set; it never exposes a mixed old/new set.

## Consequences

Positive:

- forgetting, source changes, and regeneration have direct observable rules;
- automatic work is isolated per source and no global model action ledger can
  mutate unrelated memory;
- explicit memory is available immediately and preserves user metadata;
- old content cannot be recovered through a protocol option or known ID;
- Provider failure affects only regeneration, not current explicit memory,
  note search, WebDAV, or canonical writes;
- the number of long-lived business concepts and model-owned decisions drops
  substantially.

Costs:

- duplicate facts from different notes remain separately owned; retrieval may
  suppress near-duplicate presentation but cannot merge ownership;
- source edits may create a temporary memory gap;
- deleting one generated item pauses that note's future extraction until an
  explicit resume;
- the upgrade needs a visible preflight because legacy global consolidation
  can mix explicit and multiple note inputs;
- public prerelease memory schemas and Admin controls change.

## Rejected alternatives

### Keep lifecycle history and only change default filters

Rejected because known IDs, resources, history flags, aggregate artifacts, and
stale vectors would remain bypasses around deletion.

### Keep Phase 2 but constrain its actions

Rejected because source ownership already determines replacement/deletion and
the global model still adds unrelated concurrency, recovery, and attribution
risks.

### Persist every source version as a set generation

Rejected because revision history/backups already provide recovery and those
generations would recreate model-readable historical memory.

### Automatically bind moved/recreated sources by content or semantics

Rejected because same-File-ID moves are already stable and delete/recreate must
not guess identity. Semantic, filename, ambiguous-hash, and cross-Vault matches
are not authorization or ownership proof.

### Treat an extraction failure as an empty result

Rejected because malformed/truncated Provider output must not delete a valid
current unchanged-source set or disguise paid-call failure as a semantic
decision.
