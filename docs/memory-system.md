# Long-Term Memory System

## 1. Purpose

MCP Vault memory is durable, sourced context for future Agent work. It is not
an excerpt collection, an opaque model store, or a synonym for note search.

The subsystem has two complementary outputs:

- consolidated long-term memory for preferences, decisions, constraints,
  project state, procedures, relationships, and significant events;
- `related_notes` retrieval cues for ordinary article knowledge that should
  remain in canonical notes instead of becoming global memory.

Normal recall is an indexed local operation. It never scans the Vault or calls
an LLM at query time.

## 2. Architecture

MCP Vault follows the Codex two-phase memory architecture. The admission unit
is a Vault note revision or explicit `remember` input rather than a Codex
conversation rollout.

```text
note revision / explicit remember
    │
    ▼
Phase 1: memory.extract
    memory_extraction model
    semantic raw_memory + source_summary + evidence anchors
    │
    ▼
memory_stage1_outputs
    ready / no_output / withdrawn
    │
    ▼
Phase 2: memory.consolidate
    memory_consolidation model
    deduplicate / merge / update / conflict resolution / forgetting
    │
    ▼
validated canonical artifacts through Vault Core
    MEMORY.md + memory_summary.md + per-record Markdown
    │
    ▼
Vault-scoped memory projections and LLM-free recall
```

Phase 1 never writes final `memories` or final memory FTS rows. Phase 2 never
accepts arbitrary Provider output as state: local code validates every action,
source reference, evidence index, memory ID, lifecycle transition, bound, and
optimistic revision before applying it.

## 3. Phase 1: source distillation

### 3.1 Note input

When automatic memory is enabled, an ordinary non-managed Markdown revision is
eligible without tags, frontmatter, marker files, path conventions, or special
folders. Managed memory artifacts are always excluded to prevent feedback
loops.

The extraction model returns one strict object:

```json
{
  "source_summary": "Detailed evidence-aware summary",
  "source_slug": "stable-source-slug",
  "raw_memory": "Concise consolidation-ready semantic information",
  "evidence": [
    {
      "start_line": 10,
      "end_line": 10
    }
  ]
}
```

MCP Vault sends an untrusted view whose lines carry stable `L<number>:` labels.
The model selects bounded start/end labels and never supplies the canonical
quotation. MCP Vault validates the range against the current revision, derives
the exact excerpt hash itself, and persists only file identity, path, revision,
and line range. `source_summary` is an auxiliary Phase 2 context field. If a
Provider omits only that field while returning a valid string `raw_memory`, MCP
Vault copies that semantic text verbatim as the summary and reruns the full
schema/evidence checks. This bounded repair never accepts an empty object or
manufactures evidence.

`raw_memory` is semantic model output and is allowed to differ from the source
wording. It is not final global memory. If the note contains no durable input,
Phase 1 stores `no_output` with empty semantic fields so an unchanged note is
not billed again.

### 3.2 Explicit remember input

MCP/Admin explicit remember is also a Phase 1 admission. The service validates
and normalizes any note provenance, stages the caller's semantic statement and
metadata, enqueues `memory.consolidate`, and returns:

```json
{
  "outcome": "staged",
  "memory": null,
  "raw_memory_id": "...",
  "consolidation_job_id": "..."
}
```

The call does not claim that recall has changed. The final memory becomes
visible only after Phase 2 commits. Idempotency keys map to the same current
raw input and reject conflicting reuse.

### 3.3 Coverage and re-extraction

One current Stage 1 row is keyed by Vault, source type, and stable source key.
For notes it records the exact file revision and an effective profile hash over
the extraction policy, prompt/pipeline, model binding, model settings, and
Provider settings.

Incremental extraction skips an unchanged successful `ready` or `no_output`
row before any Provider call. Explicit full re-extraction includes unchanged
rows. A failed forced extraction invalidates the profile for retry but does not
turn previously selected raw content into a false new Phase 2 input.

One malformed model output is a note-local failure. The durable full-Vault job
checkpoints it and continues later notes. A bounded consecutive-output circuit
prevents unlimited invalid paid calls while preserving the completed cursor.

## 4. Phase 2: global consolidation

Phase 2 receives a bounded set of dirty Stage 1 inputs, recent current raw
inputs, the last compact memory summary, and current active global memories.
The model proposes:

- `create`, `update`, `keep`, or `archive` actions;
- semantic content and memory type for writes;
- Stage 1/evidence references for every written memory;
- supersession targets;
- one `used`, `discarded`, or `withdrawn` disposition for every dirty input;
- a complete compact `memory_summary`.

Local validation enforces:

- every dirty input is dispositioned exactly once;
- a `used` input is referenced by a write action;
- discarded/withdrawn inputs are not used as support;
- note-derived writes reference locally validated evidence anchors;
- IDs and sources exist in the same Vault;
- one generation does not target and supersede the same memory;
- create/update content is bounded, non-empty, and secret-redacted;
- prepared update/keep/archive actions retain the exact base revision observed
  before the Provider call.

Only one non-terminal `memory.consolidate` job may exist per Vault. Provider
I/O happens before the Vault apply lock. Before canonical writes, the service
rechecks the generation, every raw output hash/status, and every current memory
revision/content hash. Before a proposal is persisted, a changed snapshot
causes a short retry with a new input hash. Once a proposal is prepared,
memory mutations briefly return a retryable conflict until that exact proposal
is applied or recovered, so its source/base revisions cannot drift underneath
partial writes.

Prepared proposals are persisted before application. Retrying the same input
reuses the validated proposal without another model call. Per-memory proposal
markers, Vault Core revision preconditions, idempotent proposal commit, and
selected-input hashes make partial application recoverable after restart. If a
managed record was written but its memory projection was not, recovery adopts
the byte-identical current file and completes the projection instead of
creating another Vault revision. One worker invocation drains successive
bounded 256-input generations until no dirty Stage 1 input remains; startup
and periodic reconciliation re-admit any durable input left by the narrow job-
completion race.

## 5. Canonical artifacts

All final mutations go through Vault Core managed operations:

```text
_mcp-vault/memory/
├── MEMORY.md
├── memory_summary.md
├── raw_memories.md
├── source_summaries/
│   └── <stage1-id>.md
└── records/
    └── YYYY/MM/<memory-id>.md
```

- `MEMORY.md` is the retrieval-oriented consolidated semantic state.
- `memory_summary.md` is the compact cross-memory summary from the committed
  generation.
- `raw_memories.md` and `source_summaries/` make the current staged semantic
  inputs inspectable without copying source quotations.
- `records/` retains one deterministic Markdown record per final memory for
  lifecycle/detail/resource compatibility.

Obsolete source-summary files are deleted through Vault Core when their Stage
1 row is withdrawn or regenerated. Revision history and backup retention remain
independent. Artifact generation paginates the complete active-memory and ready
Stage 1 sets; the bounded model context is never reused as an artifact cutoff.
SQLite projections are rebuildable from canonical final Markdown; the original
note/history remains canonical supporting evidence.

## 6. Final memory and provenance

The final body is a concise normalized semantic statement:

```text
Project services should use Rust for future implementation.
```

It is not required to equal this supporting evidence:

```text
I decided that future services use Rust.
```

`memory_sources` separately stores source type, file ID, path, note revision,
line range, excerpt hash, and Vault ID. Detail operations return source
metadata; normal recall omits sources by default to conserve context. Exact
evidence can be verified against the retained note revision through Vault
history permissions.

Memory types remain extensible text values:

```text
identity preference decision constraint fact project progress
event relationship procedure
```

Final lifecycle states are:

```text
active ──newer truth──▶ superseded
   │
   ├──source consolidation──▶ archived
   ├──manual archive────────▶ archived
   └──invalid managed file──▶ quarantined
```

There is no human-review candidate lifecycle in the normal architecture.
Legacy `memory_candidates` rows are removed by the pipeline cutover and are not
exposed by Admin or MCP.

## 7. Recall

```text
recall request
    → Vault/status/temporal filters
    → memory FTS/entity/tag/recent candidates
    → optional stored memory vectors
    → ordinary-note lexical/vector cues when vault:read is allowed
    → rank fusion and diversity
    → token-budgeted memories + related_notes
```

Normal recall reads only the latest committed projection/artifacts. It never
waits for pending Stage 1 work and never performs a live consolidation call.
`include_sources` defaults to `false`; `get_memory` and memory resources provide
provenance detail.

An MCP Host still decides when to call recall. Discovery instructions,
`vault://memory/context`, deterministic low-latency output, and the distinct
`related_notes` channel make proactive use more likely without pretending MCP
Vault receives every user message.

## 8. Source changes and deletion

A note update does not immediately mark all existing memories stale. The new
revision is distilled, then Phase 2 updates, merges, or forgets affected global
memory as one sourced decision. Until that commit, the last complete global
generation remains recallable.

A deleted note marks its current Stage 1 row `withdrawn` and queues Phase 2.
The consolidation proposal must disposition that withdrawal and archive or
update unsupported final memory. Other current supporting sources can keep a
memory active.

## 9. Prerelease pipeline cutover

ADR-0017 makes memory architecture changes destructive while MCP Vault remains
prerelease. Migration 0011 deletes every old `memory.*` job and all old memory
database state. `memory.reset_pipeline` is then admitted as a Vault-scoped,
persistent, idempotent filesystem cutover job.

It:

1. deletes every current file below `_mcp-vault/memory/` through Vault Core,
   retaining ordinary Core revision history and existing backups;
2. transactionally purges any residual final memory, Stage 1, proposal,
   candidate, diagnostic, idempotency, FTS, and memory-vector rows;
3. writes empty current-generation `MEMORY.md`, `memory_summary.md`, and
   `raw_memories.md` artifacts;
4. records `pipeline_generation` plus durable `regeneration_pending` state;
5. admits a brand-new full-Vault Phase 1 job with `fresh_start = true`, no
   progress cursor, and the current pipeline generation; and
6. clears the pending marker only after that exact durable job exists.

No old explicit or extracted memory is converted. Ordinary notes and
attachments are untouched and are the only regeneration input. If Phase 1 is
not configured, periodic reconciliation retains the pending marker and admits
the fresh job after configuration becomes ready.

## 10. Jobs and observability

Persistent job types are:

```text
memory.extract
memory.consolidate
memory.reset_pipeline
memory.revalidate
memory.rebuild
embedding.rebuild
```

Phase 1 progress reports note cursor/path, processed count, raw inputs staged,
no-output count, unchanged skips, bounded per-note failures, elapsed time, and
trusted schema diagnostics. Phase 2 reports raw inputs, created/updated/
retired/discarded counts, proposal reuse, and committed generation. Logs expose
the same redacted counts and error codes but never note bodies, raw/final memory
text, prompts, Provider response text, credentials, or authorization headers.

Running consolidation and pipeline reset cannot be cancelled through Admin after
their apply phase starts. Shutdown/interruption recovery uses the persisted
proposal and Vault Core revisions.

## 11. Security and isolation

- Every row, job, source, proposal, artifact, query, and embedding is scoped by
  `VaultContext`/`vault_id`.
- Note text, explicit inputs, Provider output, and stored proposal JSON are
  untrusted.
- Generated raw memory, summaries, reasons, metadata strings, and final content
  pass best-effort secret redaction before persistence.
- Evidence quotations are validated in memory and replaced with line/hash
  pointers before persistence.
- Protocol/Admin handlers do not call Providers, SQL, indexes, or the
  filesystem directly; they call Memory/Vault Core application services.
- Recall remains usable when Providers are offline.

## 12. Model roles

`memory_extraction` and `memory_consolidation` are separate first-class model
bindings. Admin considers the complete pipeline ready only when automatic
memory is enabled, Provider policy allows calls, and both effective models and
Providers are enabled. Their failures, costs, timeouts, progress, and retry
domains remain separate.
