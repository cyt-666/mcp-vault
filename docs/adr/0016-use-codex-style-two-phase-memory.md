# ADR-0016: Use a Codex-style two-phase memory pipeline

- Status: Accepted
- Date: 2026-08-25
- Supersedes: the direct automatic-promotion portions of ADR-0014 and ADR-0015
- Upgrade behavior amended by: ADR-0017

> ADR-0017 supersedes the prerelease preservation/conversion paragraph below.
> The two-phase architecture remains accepted; current cutovers discard all old
> memory state and jobs before fresh regeneration.

## Context

The prerelease automatic-memory implementation treated an exact quotation from
a note as both supporting evidence and final durable memory content. Exact
source validation prevented hallucinated quotations, but it produced a memory
store shaped like an excerpt collection rather than a curated semantic state.
It also forced per-note extraction to perform final deduplication, lifecycle,
and promotion decisions before seeing related evidence from other notes.

Codex local memories use separate phases. Eligible historical inputs first
produce raw memories and detailed summaries in Stage 1 state. A separate global
consolidation phase merges those inputs, deduplicates, resolves conflicts,
forgets low-signal/stale material, and writes curated memory artifacts. Official
OpenAI documentation likewise distinguishes summaries, durable entries, recent
inputs, and supporting evidence, and exposes separate extraction and
consolidation model settings.

MCP Vault already has separate `memory_extraction` and `memory_consolidation`
model roles, durable jobs, Vault Core managed Markdown, source/relation tables,
and query-time LLM-free recall. The missing boundary is Stage 1 versus final
global memory.

## Decision

MCP Vault will use the Codex two-phase memory architecture.

Phase 1 distills one eligible Vault note revision or explicit remember input
into a sourced raw-memory record containing semantic `raw_memory`, a detailed
summary, a stable slug, and application-derived source provenance. It may
produce a no-op. Phase 1 never creates or updates final `memories` rows, final
FTS, or final canonical memory content.

Phase 2 runs as a separate Vault-scoped durable job using the
`memory_consolidation` model binding. It consumes dirty Phase 1 records together
with current global memory and proposes a complete set of validated create,
update, keep, supersede, archive, and drop decisions. It performs semantic
normalization, deduplication, conflict resolution, temporal/lifecycle handling,
and forgetting. Local code validates all memory/source references and owns
identifiers, status transitions, canonical writes, and projection commits.

Canonical generated artifacts mirror the Codex layers under the Vault Core
managed memory root:

- `raw_memories.md` and `source_summaries/` preserve current Phase 1 inputs and
  supporting evidence routing;
- `MEMORY.md` contains curated semantic durable memory;
- `memory_summary.md` contains compact prompt/retrieval-oriented summary state.

The source unit differs from Codex only at admission: Codex processes idle
conversation rollouts, while MCP Vault processes Vault-scoped note revisions
and explicit remember inputs. Vault isolation, authentication, provenance,
revision history, secret redaction, Provider transport policy, and safe Vault
Core writes remain mandatory project boundaries.

Final `memory.content` is a model-consolidated semantic statement and is not
required to equal its supporting evidence. `memory_sources` separately points
to the supporting note identity, revision, path, and whole-source hash; explicit
or imported provenance may additionally include heading/line anchors. Normal
recall omits sources by default; explicit detail can retrieve them.

Existing prerelease final memories with `origin = extracted` are not trusted as
global-memory inputs because their content is the old quote representation.
Upgrade/reset removes their current managed artifacts and projections through
Vault Core while preserving revision history/backups, then regenerates from
current source notes. Existing explicit Agent/Admin memories become Phase 1
inputs, then their legacy current records are removed through Vault Core before
they pass through consolidation; they do not form an alternate direct final-
memory path.

### 2026-08-26 amendment (superseded): model selects ranges, local code derives text

The initial Phase 1 contract asked the model to count unnumbered Markdown lines
and echo an exact quotation. Deployment showed that a valid source statement
could be rejected merely because the model copied a character differently or
miscounted the line. That is not a useful trust boundary.

That pipeline sent a line-numbered untrusted source view and accepted
only bounded `start_line`/`end_line` selections. MCP Vault validates those
labels against the current Vault revision and derives the exact excerpt hash
from authoritative Markdown. The model never supplies canonical evidence text.
Pipeline generation 2 and extraction pipeline 9 force prerelease Stage 1 work
to restart under this contract rather than mixing evidence representations.

### 2026-08-27 amendment: Phase 2 models propose semantics, not bookkeeping

Live MiMo JSON Object output demonstrated that a syntactically valid model can
invent placeholder UUIDs and can inconsistently repeat operation, evidence, and
raw-disposition bookkeeping. Those fields are not semantic decisions and must
not become model-owned state.

The Phase 2 wire contract therefore contains semantic create/update/archive
actions, request-local integer references, explicit discards of dirty ready
inputs, and `memory_summary`. Durable Stage 1 and current-memory UUIDs are not
sent to the model. MCP Vault maps `input_index` and `memory_index` values back to
the exact captured snapshot, always allocates create UUIDv7 identifiers, expands
referenced ready inputs to all server-validated evidence anchors, derives `used`
from write references, and automatically dispositions `no_output` and
`withdrawn` inputs. Bounds, readiness, supersession, Vault ownership, content,
and revision snapshots remain strictly validated. Generated-output failures
expose only stable `memory_phase2_*` codes and never retain Provider text.

This is consolidation prompt version `memory-consolidation-v3`, not a Stage 1 or
canonical-file generation change. Applied proposals remain historical and the
unchanged typed prepared-proposal representation preserves crash recovery.

### 2026-08-27 amendment: match Codex Phase 1 and semantic Phase 2 concurrency

The evidence-range amendment above still delegated source bookkeeping that
upstream Codex Phase 1 does not request. Live MiMo processing then failed valid
notes with `memory_phase1_evidence_too_large` and
`memory_phase1_evidence_anchor_invalid`. Phase 1 now matches the Codex wire
contract exactly: `raw_memory`, `rollout_summary`, and `rollout_slug`. For note
admission, MCP Vault maps the rollout-named fields into source state and derives
file identity, path, exact revision, and normalized whole-source hash locally.
Automatic extraction never asks the model for a quote or line coordinate. This
supersedes the 2026-08-26 evidence-range mechanism for automatic notes; explicit
and imported sources may still carry caller-validated line/heading anchors.

Live Phase 2 diagnosis also showed that aggregate managed-file events admitted
full projection rebuilds, while every unchanged rebuild incremented every
memory revision. A prepared proposal saw identical content/status hashes but
revision increases of 136 and was retried five times as
`memory_consolidation_snapshot_changed`; the stale proposal then blocked Phase
1. This is operational feedback, not semantic concurrency.

Accordingly, projection rebuild admission is limited to canonical memory-record
paths and an already projected canonical revision is a no-op. Phase 2 uses
status/content hash as current-memory semantic snapshot identity and refreshes
the optimistic revision only when those semantics are unchanged. A changed
action target still conflicts. Reconciliation waits for unrelated active Phase
1 work, and a claimed Phase 2 job defers without consuming attempts. Prepared
proposals from older prompt contracts are rejected before parsing or apply.
These changes are `memory-stage1-v4`, extraction pipeline 10, and
`memory-consolidation-v4`.

## Consequences

- Automatic memory becomes coherent across notes and can express concise
  normalized semantics while retaining inspectable evidence.
- Extraction and consolidation have separate cost, readiness, progress,
  retries, model selection, and failure domains.
- Raw memory may accumulate while consolidation is unavailable without losing
  source work.
- Final memory is no longer updated immediately after each note event or
  explicit remember admission.
- Consolidation is a higher-risk LLM operation. Output must remain a proposal,
  reference valid sources, pass strict bounds/schema, and commit atomically at
  the job/application boundary.
- Reducing model-owned bookkeeping makes JSON Object providers more reliable
  without weakening provenance, lifecycle, or concurrency checks.
- Prepared proposals retain the exact raw/current-memory snapshot and stable
  operation identity required to recover partial Vault Core/projection writes
  without another Provider call. While one is prepared, competing memory
  mutations retry instead of changing its evidence underneath it.
- Provider context and actions remain bounded per generation, but one durable
  singleton job drains successive generations and canonical artifact rendering
  paginates the complete active/raw sets.
- Query-time recall remains LLM-free and reads only the last committed global
  memory projection/artifacts.
- Migration and UI are larger than a prompt adjustment; the old candidate
  review/direct-promotion workflow is removed rather than maintained in
  parallel.

## Rejected alternatives

### Keep exact quote as final content and merely improve labels/prompts

Rejected because it preserves the architectural error: evidence and memory are
still the same object, and no cross-source consolidation occurs.

### Add an optional consolidation pass after direct promotion

Rejected because it creates two competing final-memory authorities and makes
recall/lifecycle behavior dependent on which path wrote a record.

### Let Phase 2 write arbitrary files or SQL directly

Rejected because it bypasses Vault Core, schema validation, Vault scoping,
revision history, Provider boundaries, and deterministic recovery.

### Reuse old extracted quote memories as global inputs

Rejected because they encode the representation being replaced and would bias
the first consolidation toward preserving excerpt-shaped output.
