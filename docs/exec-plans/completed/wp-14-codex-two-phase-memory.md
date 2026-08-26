# WP-14 Codex-style two-phase memory redesign

- Objective: replace note-quote-as-final-memory with a Codex-style Phase 1
  distillation and Phase 2 global consolidation pipeline.
- Owner: Codex
- Created: 2026-08-25
- Last updated: 2026-08-26
- Status: complete

## Purpose and user-visible result

MCP Vault will no longer materialize an extracted source quote directly as a
durable memory. A note revision first becomes a sourced raw-memory input. A
separately configured consolidation model then merges new raw inputs with the
current global memory, removes low-signal and duplicate material, resolves
conflicts, and writes concise semantic memories such as “项目后续统一使用 Rust。”

The original note revision, exact quote, path, and line range remain supporting
evidence. Normal `recall` returns the consolidated semantic content without
sources by default; an explicit detail/read operation returns evidence and
provenance. Existing automatically extracted quote-as-content memories are not
used as consolidation input and are regenerated from their current source
notes.

Admin presents two observable background phases, separate extraction and
consolidation model bindings, raw-input/consolidation readiness, and one
incremental or explicit full-regeneration action. There is no human review
inbox and no direct automatic promotion from Phase 1.

## Governing requirements

- `AGENTS.md`: Vault isolation, canonical Markdown portability, application
  service boundaries, untrusted LLM validation, provenance, normal recall
  without a live LLM, and safe Vault Core writes remain non-negotiable.
- `docs/product-requirements.md`: transparent sourced memory, durable jobs,
  rebuildable projections, semantic recall, and ordinary-note retrieval.
- `docs/architecture.md`: Memory and Provider application boundaries, modular
  monolith, transactional jobs/outbox, and Vault Core canonical writes.
- `docs/interfaces.md`: MCP/Admin DTOs and model-role separation.
- `docs/security.md`: note text, generated raw memory, consolidation output,
  and Provider responses are untrusted and secret-redacted.
- ADR-0001, ADR-0002, ADR-0006, ADR-0007, ADR-0011, ADR-0013.
- ADR-0016 supersedes ADR-0014/ADR-0015 where they made validated source text
  itself the automatically materialized memory; exact evidence and marker-free
  Vault-level source admission remain in force.
- OpenAI official Codex memory documentation: local memory files contain
  summaries, durable entries, recent inputs, and supporting evidence; memory
  generation runs after eligible inputs become idle; `extract_model` and
  `consolidation_model` are separate.
- OpenAI long-term-memory architecture example: session/raw memory is a staging
  area, consolidation performs deduplication/conflict resolution/forgetting,
  and only curated global memory is injected.
- Codex source reference: `memories/write/src/phase1.rs`,
  `memories/write/src/phase2.rs`, `state/memory_migrations/0001_memories.sql`,
  and the stage-one/consolidation templates in the sibling Codex checkout.

## Current repository state

- Migration `0010_codex_two_phase_memory.sql` owns Vault-scoped Stage 1 rows,
  prepared Phase 2 proposals/snapshots, committed generation/reset state, and
  one active consolidation job per Vault. Migration 0007's candidate table is
  retained only as prerelease upgrade input.
- `MemoryService::extract_note_with_options` emits semantic raw memory plus
  transient exact evidence validation and never writes final memory. Explicit
  `remember` uses the same staging path and returns raw/job identities.
- `MemoryService::consolidate` uses the separate
  `memory_consolidation` binding, validates complete actions/dispositions,
  persists a recoverable proposal, applies through Vault Core, commits exact
  raw hashes, and writes complete paginated global/raw/source artifacts.
- Workers expose `memory.extract`, `memory.consolidate`, and
  `memory.reset_legacy`. Consolidation drains successive 256-input generations;
  startup/periodic recovery admits stranded dirty inputs and delayed legacy
  re-extraction.
- Admin/API/MCP paths have no candidate-review operation. The Chinese Memory
  page shows both model roles, Stage 1/Phase 2 state, durable progress, final
  semantic content, source locations, and lifecycle deletion/archive actions.
- Normal `recall` remains query-time LLM-free and defaults sources off while
  `get_memory` retains provenance detail.
- Focused migration, isolation, recovery, pagination, multi-batch, MCP, Admin,
  and frontend tests pass. Full workspace/release validation remains pending.

## Scope

Included:

- Phase 1 note distillation with a separately persisted `raw_memory`, source
  summary, stable source slug, structured evidence references, and no-output
  coverage;
- Phase 1 support for explicit Agent/Admin remember inputs without direct final
  materialization;
- Phase 2 Vault-scoped consolidation using the `memory_consolidation` binding;
- deduplication, conflict/supersession, forgetting, source aggregation, and
  semantic final-memory content;
- canonical Codex-style generated artifacts under the managed memory root:
  `MEMORY.md`, `memory_summary.md`, `raw_memories.md`, and
  `source_summaries/`;
- final per-memory/source projections and FTS rebuilt from validated Phase 2
  output, while canonical Markdown remains portable;
- durable job ownership, watermark/selection state, retries, cancellation,
  restart recovery, and progress for both phases;
- migration/reset of prerelease quote-as-content extracted memories and clean
  regeneration from current notes;
- Admin/API/MCP behavior, Chinese UI, docs, and local-fake tests.

Not included:

- copying Codex's thread/rollout admission rules verbatim; MCP Vault's source
  unit is a Vault note revision, not a Codex conversation;
- giving a consolidation model shell/network access or bypassing the shared
  Provider transport;
- query-time LLM recall;
- making raw-memory SQLite rows the only knowledge copy;
- weakening Vault, listener, authentication, or secret boundaries.

## Invariants and risks

- Every Stage 1 row, Phase 2 job, final memory, source link, artifact, query,
  audit event, and cache remains Vault-scoped.
- Phase 1 never writes final memory or final FTS rows.
- A final memory must reference at least one current or retained historical
  supporting source. Consolidation may paraphrase but may not invent an
  unsupported memory.
- Generated semantic content and evidence references are independently
  validated. A valid JSON shape is not proof of factual support.
- Final artifacts are written through Vault Core managed-path operations and
  retain revision/history/outbox behavior.
- Interrupted Phase 1 leaves the note eligible. Interrupted Phase 2 does not
  advance input selection/watermark or replace the current global memory.
- Only one Phase 2 consolidation owns a Vault at a time. Provider calls occur
  outside SQL transactions and async locks.
- Old extracted final memories must not silently survive as consolidation
  truth. Reset is auditable and recoverable from Vault history/backups.
- Explicit memories are not discarded: they are converted to sourced raw
  inputs and pass through Phase 2 before becoming final global memory.
- Model-generated raw/final memory and summaries may contain secrets. Apply
  redaction before persistence and never log bodies.
- Consolidation can over-prune or merge incorrectly. Preserve source summaries,
  raw memories, Vault revisions, job diagnostics, and explicit regeneration.

## Proposed design

### Source and phase flow

```text
note revision / explicit remember
    -> memory.extract (Phase 1, memory_extraction model)
    -> memory_stage1_outputs
       raw_memory + source_summary + source_slug + evidence refs
    -> generated raw_memories.md + source_summaries/*.md
    -> memory.consolidate (Phase 2, memory_consolidation model)
    -> validate complete consolidation proposal and source references
    -> write MEMORY.md + memory_summary.md through Vault Core
    -> replace final memory/source/relation/FTS projection
    -> mark selected Phase 1 inputs consolidated
```

Automatic note events are debounced by file identity/revision. A successful
Phase 1 no-op persists an empty/no-output row so an unchanged note is not billed
again. Updating or withdrawing a note marks its Stage 1 input dirty and queues
Phase 2 so unsupported global memory can be revised or forgotten.

### Phase 1 contract

The Stage 1 structured result is one object:

```json
{
  "source_summary": "Detailed evidence-aware summary for later consolidation",
  "source_slug": "stable-source-slug",
  "raw_memory": "Concise consolidation-ready semantic notes",
  "evidence": [
    {
      "quote": "我决定以后项目统一使用 Rust。",
      "start_line": 10,
      "end_line": 10
    }
  ]
}
```

All fields are required; no-op uses empty strings and an empty evidence array.
Each evidence quote must occur exactly inside the declared current note lines.
`raw_memory` is not required to equal the quote and is never materialized as
final memory. Structural envelope repair remains limited to prompt-constrained
Providers and full validation still runs afterward.

### Phase 1 state

Migration 0010 creates `memory_stage1_outputs` with:

- `id`, `vault_id`, `source_type`, and stable `source_key`;
- optional note file/path/revision;
- extraction profile hash and prompt/pipeline versions;
- `raw_memory`, `source_summary`, `source_slug`, bounded `evidence_json`;
- output hash, status `ready|no_output|withdrawn`;
- generated/updated/last-used timestamps and usage count;
- `selected_for_phase2` and selected source revision/hash;
- Vault/file composite foreign keys and one current row per source key.

The row itself is operational and rebuildable. `raw_memories.md` and
`source_summaries/*.md` provide portable generated inputs. The original note
and its Vault history remain immutable supporting evidence.

### Phase 2 contract

Phase 2 loads a bounded set of dirty Stage 1 inputs, current active global
memory entries, and their source references. The consolidation model returns a
complete validated proposal containing:

- concise `memory_summary` optimized for future-agent context;
- final semantic memory entries with stable existing IDs when retained;
- memory type, temporal status, tags/entities, and normalized content;
- referenced Stage 1 input IDs/evidence indexes for every entry;
- explicit keep/create/update/supersede/archive/drop decisions and reasons.

Local code rejects unknown memory/source IDs, missing evidence, cross-Vault
references, unsupported statuses/types, duplicate actions, invalid temporal
ranges, and output exceeding configured bounds. It deterministically computes
hashes, IDs for new entries, relation rows, and Markdown rendering. A rejected
consolidation does not mutate current global memory or mark raw inputs selected.

### Canonical artifacts and projections

The managed memory root contains:

- `MEMORY.md`: retrieval-oriented consolidated semantic memory with source
  reference IDs, not raw note quotations as content;
- `memory_summary.md`: versioned compact cross-memory summary;
- `raw_memories.md`: deterministic inventory of current Stage 1 outputs;
- `source_summaries/<source-slug>.md`: detailed supporting summary and evidence
  pointers for one current source;
- optional per-entry managed records only if needed for existing public
  resource compatibility; they are rendered from the same Phase 2 proposal and
  are not an independent memory authority.

`memories`, `memory_sources`, `memory_relations`, and `memory_fts` remain the
Vault-scoped query projection. They are replaced only after canonical artifact
validation and can be rebuilt. `memory_sources` references note revision and
line ranges; evidence text is read from Vault history on explicit detail.

### Jobs and recovery

- `memory.extract`: Phase 1 per-source or full-Vault admission, default
  incremental with explicit full regeneration.
- `memory.consolidate`: one active Vault job, claimed with the existing durable
  lease; consumes dirty Stage 1 rows in bounded batches.
- `memory.reset_legacy`: one auditable upgrade/reset job that removes
  current quote-as-content extracted artifacts/projections through Vault Core,
  retains history/backups, converts explicit memories to raw inputs, clears
  obsolete candidates/evaluation state, and queues Phase 1 + Phase 2.

Phase 2 writes a prepared proposal to job progress/state, validates ownership,
materializes managed artifacts, replaces projections, then marks input rows
selected. Recovery before projection commit leaves the old global memory
active; recovery after canonical writes reconciles/rebuilds the projection from
the validated generation marker.

### Public behavior

- `remember` becomes durable staging admission and returns accepted raw-input/
  consolidation job information rather than claiming immediate final memory.
- `recall.include_sources` defaults to false. `get_memory`/resource detail can
  return sources and evidence.
- Admin shows Phase 1 processed/no-op/failed counts, pending raw inputs, Phase 2
  status, final-memory count, and last successful consolidation.
- Extraction and consolidation model bindings are both first-class readiness
  requirements; Phase 1 may accumulate safely if Phase 2 is temporarily
  unconfigured.
- The old candidate review/reset UI and direct-promotion wording are removed.

## Work breakdown

1. Amend specifications and add ADR-0016. Replace direct-promotion language in
   requirements, architecture, interfaces, security, data model, operations,
   testing, traceability, and Admin UX documents.
2. Replace unshipped migration 0010 with Stage 1/consolidation/reset state and
   update migration fixtures/version assertions.
3. Add State repository DTOs and Vault-scoped SQL for Stage 1 upsert/no-op/
   withdrawal, dirty selection, selection commit, consolidation status, and
   legacy reset inventory.
4. Replace automatic extraction DTO/prompt/schema/service flow with Codex-style
   raw memory + source summary + evidence. Remove automatic calls to `remember`.
5. Add Phase 2 consolidation DTO/schema/prompt, source-reference validation,
   dedup/conflict/forget application, canonical artifact rendering, and final
   projection replacement through Vault Core.
6. Add `memory.consolidate` and legacy-reset workers with cancellation,
   ownership, checkpoints, bounded batches, redacted logs, and recovery.
7. Change explicit remember and MCP/Admin DTOs to stage input; change recall
   defaults and evidence-detail behavior while retaining older protocol fields
   only where compatibility is truthful.
8. Replace Admin memory UI with two-phase status/actions, separate model
   readiness, final semantic memory detail, and explicit evidence drill-down.
9. Add migration, Vault-isolation, local-fake Provider, crash-recovery,
   no-output, deduplication, contradiction, forgetting, evidence, recall, MCP,
   Admin, and frontend tests.
10. Run all repository checks, update checksums and this plan, and document
    upgrade/rollback/image behavior.

## Progress

- [x] 2026-08-25 — Confirm the architectural mismatch from OpenAI official
  docs, the OpenAI long-term-memory example, Codex Phase 1/Phase 2 source, and
  the deployment log.
- [x] 2026-08-25 — Record the replacement architecture before implementation.
- [x] 2026-08-26 — Replace current specifications and adopt ADR-0016; retain
  superseded ADR/plan text only as explicitly marked history.
- [x] 2026-08-26 — Replace migration 0010 and add Vault-scoped Stage 1,
  prepared-proposal, generation, singleton-job, and reset state.
- [x] 2026-08-26 — Implement Phase 1 semantic raw-memory/no-output staging,
  transient exact-quote validation, explicit remember admission, redaction,
  coverage, and source withdrawal.
- [x] 2026-08-26 — Implement Phase 2 consolidation, complete reference/action
  validation, revision snapshots, prepared-proposal reuse, canonical artifacts,
  final projections, bounded multi-generation draining, and LLM-free recall.
- [x] 2026-08-26 — Implement Worker/Admin/MCP/Chinese-UI behavior, automatic
  legacy reset/regeneration, source drill-down, and candidate-review removal.
- [x] 2026-08-26 — Complete automated validation, public HTTP smoke, official
  MCP conformance scenarios, checksums, and operational documentation.

## Decisions

- Do not retain a hybrid “exact quote may directly promote” path. Exact quotes
  are supporting evidence only for automatic note extraction.
- The input-unit difference is intentional: Codex uses eligible idle rollout
  threads; MCP Vault uses Vault-scoped note revisions and explicit remember
  admissions. After admission, both use raw staging followed by global
  consolidation.
- Preserve MCP Vault's mandatory Vault/provenance/history/security boundaries
  even where Codex's local single-user implementation can rely on one home
  directory and a global lock.
- Use the existing `memory_extraction` and `memory_consolidation` model roles.
  They must not collapse into one binding.
- A consolidation model proposes semantic content; local code owns identifiers,
  source validation, lifecycle transitions, canonical writes, projections, and
  job commits.
- Existing `origin = extracted` final content is test-era output and must be
  regenerated. Explicit memories are staged as raw inputs rather than silently
  discarded or retained as an alternate final path.

## Surprises and discoveries

- The UI already exposes a `memory_consolidation` role, but no service or worker
  resolves it.
- The current `proposals_rejected` counter conflates source-evidence mismatch,
  durability policy, and downstream validation. This exposed the deeper issue:
  Phase 1 is making final-lifecycle decisions instead of producing raw input.
- Deployment log job `01a037dc-25db-7e12-a277-f0d28e93630e` had one isolated
  root-schema failure at note 23 and continued beyond note 44. The model-output
  defect is real but is not the reason to retain the direct-promotion design.
- Codex's Phase 1 table stores raw memory and summary but no final memory row;
  Phase 2 has a separate model, exclusive job ownership, selected-input state,
  artifact validation, and only then advances its watermark.
- A 200-row repository page and the 256-input model context had accidentally
  become canonical-artifact cutoffs. Final and raw artifact generation now
  paginates complete sets independently from the bounded Provider context;
  201-row regression fixtures cover both layers.
- Enqueueing a Phase 2 follow-up before the current singleton reached its
  terminal transition only returned that same active job. The handler now
  drains every bounded batch itself, while startup/periodic admission closes
  the narrow new-input-versus-job-completion race. A 257-input Worker fixture
  proves two committed generations complete in one durable job.
- Vault Core file mutation and memory-projection replacement are separate
  durable boundaries. Prepared proposals now include exact raw/current
  snapshots and stable operation timestamps; byte-identical existing managed
  files are adopted after a file-write/projection interruption. Partial global-
  artifact failure and file-before-projection recovery have focused tests.

## Validation

Required commands:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/release/check-migrations.sh
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
git diff --check
```

Local-fake contract tests must prove no paid endpoint is called in CI. Add
explicit tests for two Vaults, note-update/withdrawal, Phase 1 no-op coverage,
Phase 1 failure isolation, Phase 2 lease/restart, unknown/cross-Vault source
references, duplicate/conflict/forgetting decisions, artifact validation,
legacy extracted reset, semantic content differing from evidence, default
source-free recall, and source detail.

Validation recorded during implementation on 2026-08-26:

- targeted Memory integration and recovery tests pass;
- 201 final + 201 raw artifact pagination regression passes;
- 257-input multi-batch consolidation Worker regression passes;
- targeted Memory/State/Server all-feature Clippy passes with warnings denied;
- Admin lint and 22 frontend tests pass with `CI=true`;
- `cargo fmt --all --check`, full-workspace warnings-denied Clippy, full
  all-feature tests, and `cargo doc --workspace --no-deps` pass;
- Admin lint, 22 tests, and production build pass with `CI=true`;
- migration fixture checks, docs/workspace checks, `SHA256SUMS`, and
  `git diff --check` pass;
- the real HTTP fixture passes MCP discovery/Origin policy, Admin-plane
  separation, revision conflict, and 50 concurrent WebDAV PUTs;
- official MCP `2026-07-28` stateless, tools, resources, header, DNS-rebinding,
  and caching scenarios pass the reviewed baseline. The unadvertised
  `prompts/list` caching check remains the existing expected failure.

## Rollback and recovery

Migration 0010 is forward-only once released. Retain a verified schema-9 state
and Vault backup before upgrade. Rolling back to a pre-two-phase binary requires
restoring both together; do not manually splice old extracted projections into
new consolidated artifacts.

The legacy-reset job uses Vault Core deletions so revision history and backups
remain available. Each current-file deletion/projection transition is
idempotent. A partial reset resumes by inventorying the remaining legacy
records; source notes and already staged explicit raw inputs remain available.

Phase 2 never advances selected-input state when output validation, canonical
write, ownership confirmation, or projection replacement fails. Operators can
retry without losing raw memories. Current global artifacts remain the recall
source until a complete new generation commits.

## Outcomes

The candidate/direct-promotion memory path has been replaced end to end by
Codex-style raw staging plus global consolidation. Existing quote-shaped and
explicit prerelease final records migrate through one safe reset; normal UI and
MCP operation require no human review. Bounded generation, complete artifact
pagination, delayed-configuration admission, partial-write recovery, and
source-detail behavior now have automated evidence. All local required checks
and applicable public-protocol gates pass, so this plan can move to
`completed/`.
