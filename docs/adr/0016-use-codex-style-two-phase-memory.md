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
source summary, a stable slug, and exact evidence references. It may produce a
no-op. Phase 1 never creates or updates final `memories` rows, final FTS, or
final canonical memory content.

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
to the supporting note identity, revision, path, and line range. Normal recall
omits sources by default; explicit detail can retrieve them.

Existing prerelease final memories with `origin = extracted` are not trusted as
global-memory inputs because their content is the old quote representation.
Upgrade/reset removes their current managed artifacts and projections through
Vault Core while preserving revision history/backups, then regenerates from
current source notes. Existing explicit Agent/Admin memories become Phase 1
inputs, then their legacy current records are removed through Vault Core before
they pass through consolidation; they do not form an alternate direct final-
memory path.

### 2026-08-26 amendment: evidence ranges are selected, evidence text is derived

The initial Phase 1 contract asked the model to count unnumbered Markdown lines
and echo an exact quotation. Deployment showed that a valid source statement
could be rejected merely because the model copied a character differently or
miscounted the line. That is not a useful trust boundary.

The current pipeline sends a line-numbered untrusted source view and accepts
only bounded `start_line`/`end_line` selections. MCP Vault validates those
labels against the current Vault revision and derives the exact excerpt hash
from authoritative Markdown. The model never supplies canonical evidence text.
Pipeline generation 2 and extraction pipeline 9 force prerelease Stage 1 work
to restart under this contract rather than mixing evidence representations.

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
