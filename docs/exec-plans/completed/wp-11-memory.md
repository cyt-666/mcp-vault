# WP-11 Complete Memory Subsystem

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-21
- Updated: 2026-08-21

## Purpose and user-visible result

Implement the complete transparent, sourced, Vault-isolated memory subsystem.
An authorized Agent can explicitly remember an atomic proposition, inspect its
provenance, recall relevant durable context without a query-time LLM, and
manage lifecycle state. Markdown under the reserved managed namespace is the
canonical representation for active and archived durable memories. Automatic
extraction produces reviewable SQLite candidates and only validated,
policy-approved candidates may be materialized.

## Governing requirements

- AGENTS.md: memory is transparent and sourced; LLM output is untrusted;
  canonical memory Markdown remains portable; all operations require
  VaultContext; protocol handlers call memory services rather than SQL/Core
  directly.
- Product requirements 3.2, 3.3, 3.5, 3.6, 3.9, 4.1-4.4, and completion
  criteria 5, 6, 8, 9, and 11.
- Architecture sections 3, 4.5, 6, 7, 9.1-9.2, 10-11, 13, 15-17.
- Memory system sections 3-20, especially canonical Markdown, candidates,
  promotion, contradiction, source invalidation, recall fusion, privacy, and
  acceptance tests.
- Interfaces sections 4.3-4.4, 5, 6.6-6.18, 7, and 8.
- Data model sections 13, 18-19 and the memory schema in memory-system.md.
- Security sections 7.4, 9, 12, 14-17, and 20.
- Accepted ADR-0001, ADR-0002, ADR-0006, ADR-0007, ADR-0008, and ADR-0010.
- PLANS.md: maintain this living plan, update progress/evidence, and archive
  only after all acceptance checks pass.

## Current repository state

- WP-09 provides Markdown analysis, note/headings/tags/links projections,
  Vault-scoped FTS, deterministic index nodes, and rebuild status.
- WP-10 provides ProviderService, structured generation, embeddings,
  Vault-scoped VectorIndex, model bindings, and reference-only embedding jobs.
- crates/memory/src/lib.rs is a documentation-only stub.
- migrations/0001-0006 contain no memory projection/candidate tables.
- crates/mcp exposes deterministic Vault/index/read/write/history tools and
  resources, but memory tools/resources are intentionally absent.
- server workers register only index.rebuild; outbox admission does not yet
  create extraction work and no memory job handler exists.
- Vault Core and storage-fs already have an explicit managed read boundary, but
  managed canonical writes need to be added without exposing reserved paths to
  ordinary DAV/MCP file operations or reconciliation deletes.

## Scope

### Included

- Add typed memory/candidate/source/relation identifiers.
- Add migration 0007 memory state, composite Vault foreign keys, memory FTS,
  candidates, provenance, entities, tags, relations, lifecycle metadata, and
  extraction metadata.
- Add State memory repositories with strict Vault predicates, deterministic
  filtering/pagination, candidate decisions, source invalidation, FTS rebuild,
  recall statistics, and isolation tests.
- Add explicit managed canonical Markdown write methods through Vault Core with
  atomicity, revision/history/audit/outbox behavior while keeping the reserved
  namespace hidden from ordinary user operations and reconciliation deletes.
- Implement MemoryService for explicit remember/reinforcement/merge/conflict,
  canonical Markdown rendering/parsing, validation, promotion/rejection,
  lifecycle transitions, source invalidation, and rebuild.
- Implement automatic extraction preparation from current note content,
  versioned structured schema/prompt metadata, candidate validation, and
  durable extraction job handling using ProviderService; no raw note bodies
  enter logs or durable job payloads.
- Implement recall candidate generation from memory FTS, entity/tag/context,
  recent active project/progress, optional vector search, reciprocal-rank
  fusion, bounded temporal/importance/confidence/continuity boosts, duplicate
  diversity, token/result budgets, provenance, and degradation reporting.
- Add MCP recall/get/list/remember/update/forget tools and memory/context and
  memory-record resources with endpoint/credential Vault binding and scope
  enforcement.
- Register durable outbox extraction admission and memory job handlers for
  extraction, promotion/materialization, projection rebuild, and source
  revalidation where the current worker composition can own them.
- Update memory/data-model/security/operations/testing documentation and
  checksums.

### Not included

- Admin HTTP routes, React memory browser, candidate review UI, and recall
  simulator (WP-12); the application service boundary is included.
- Cross-Vault/federated recall.
- Query-time LLM reranking; optional reranking remains a future provider role
  and recall must remain correct without it.
- Automatic promotion of all candidates; policy defaults remain conservative.
- Replacing canonical notes or treating SQLite/vector state as the source of
  memory truth.

## Invariants and risks

- Active/archived durable memory has one canonical Markdown record. Projection
  rows, FTS, entities, tags, relations, embeddings, and candidates are
  rebuildable.
- Every memory, source, relation, candidate, FTS query, vector query, job,
  audit fact, and resource response is Vault-scoped.
- Explicit remember is idempotent when an idempotency key is supplied and
  never silently overwrites an existing contradictory memory.
- Candidate content is validated before promotion; invalid LLM JSON, prompt
  injection, hypothetical/example text, unsupported source anchors, stale
  source revisions, and out-of-range scores cannot materialize Markdown.
- Canonical memory writes use Vault Core's managed boundary, atomic rename,
  history, expected revisions, durable journal/outbox, and recovery path.
- Normal recall never scans the filesystem or calls an LLM. It uses bounded
  projections and may degrade from hybrid/vector to lexical/context sources
  when embedding/provider work is unavailable.
- Memory files under the reserved root are not ordinary DAV/MCP file paths,
  are excluded from automatic extraction loops, and are not inferred deleted
  by ordinary reconciliation.
- Source invalidation marks extracted memories stale only when no current
  supporting source remains; explicit/unsourced memories survive note changes.
- Recall excludes stale, superseded, archived, rejected, expired, and
  quarantined records by default and returns provenance/status when requested.
- Memory bodies, source excerpts, prompts, provider responses, tokens, and
  credentials are not written to logs or redacted health/audit summaries.

## Proposed design

### Components and dependency direction

MCP/Admin caller -> MemoryService
  -> State memory repositories
  -> VaultCore managed Markdown mutation/read boundary
  -> IndexService lexical/entity/topic projections
  -> ProviderService optional extraction/embedding
  -> durable JobRepository and WorkerSupervisor

crates/memory owns domain-facing memory DTOs, lifecycle/promotion policy,
canonical Markdown serialization, extraction validation, recall ranking, and
application orchestration. It may depend on domain, state, vault-core,
indexer, providers, and auth, but not Axum/RMCP/WebDAV/frontend.

crates/mcp owns only authenticated protocol DTO translation, permission
checks, resource/tool registration, and MemoryService calls. crates/server
owns composition and worker registration.

### Data and transaction flow

Explicit remember:

1. Validate VaultContext, memory type, atomic body, scores, validity, tags,
   entities, sources, and idempotency key.
2. Normalize content and query active/archived/conflicting memory candidates.
3. Return reinforce/merge/conflict/rejected outcome as policy requires.
4. For new/promoted memory, render deterministic frontmatter/body and call
   VaultCore managed create/replace.
5. Persist memory metadata, sources, entities, tags, FTS row, audit/outbox,
   and canonical file identity/revision in one state transaction after the
   Core mutation; recovery/reconciliation leaves either an old or new
   canonical file and the projection is repairable.
6. Schedule optional embedding as a reference-only durable job.

Automatic extraction:

1. File outbox admission creates a Vault-scoped extraction job containing only
   file identity/path/revision and pipeline configuration references.
2. The handler reads current Markdown through Vault Core, skips reserved,
   excluded, non-Markdown, oversized, and stale revisions, and prepares
   bounded section inputs.
3. ProviderService generates strict structured JSON. Deterministic validation
   creates candidates with a fingerprint and no canonical write.
4. Promotion policy may materialize only allowed high-confidence candidates;
   reviewable candidates remain derived state.

Recall:

1. Validate and bound request.
2. Generate FTS, entity/tag/context, recent active, and optional vector pools
   from Vault-scoped projections.
3. Fuse ranks with stable RRF and bounded importance/confidence/temporal/
   continuity/relationship boosts.
4. Remove default-ineligible lifecycle/temporal records, deduplicate by
   proposition/related IDs, apply diversity and token budgets, and return
   compact sourced records plus degraded reasons.
5. Update recall counters asynchronously or via a durable bounded statistic
   update that does not touch canonical Markdown.

### Public interfaces and schema changes

Migration 0007 creates memories, memory_sources, memory_entities,
memory_tags, memory_relations, memory_candidates, and memory_fts. Composite
foreign keys include vault_id. A managed Core write keeps canonical memory
files in file_entries/revisions for history without making them visible to
ordinary user paths or index scanning.

MCP adds deterministic tools after note_context:
recall, get_memory, list_memories, create_note remains unchanged, then
remember, update_memory, forget_memory. Resource listing adds
vault://memory/context and vault://memory/{memory_id}.

### Failure, retry, and recovery

Provider timeout/rate-limit/5xx results in retryable extraction/embedding
jobs. Invalid schema/auth/policy/source revision results in a bounded
candidate/error state, not a canonical mutation. A Core managed-write crash
is recovered through the existing operation journal; projection rebuild
reconciles canonical Markdown and memory rows. Unknown jobs are released or
dead-lettered by the existing supervisor rather than dropped.

## Work breakdown

1. Create this plan and, if needed, ADR-0011 for the managed canonical memory
   write boundary.
2. Add domain IDs, migration 0007, state repositories, memory FTS, and
   Vault-isolation/migration tests.
3. Extend storage-fs/Vault Core for managed atomic writes and reserved-path
   reconciliation safety; add crash/history tests.
4. Implement MemoryService explicit commands, Markdown parser/renderer,
   lifecycle, candidates, provenance, source invalidation, and rebuild.
5. Implement recall fusion/budgets and optional ProviderService vector/
   extraction integration with deterministic lexical degradation.
6. Add MCP memory tools/resources and permission/response tests.
7. Register extraction/revalidation/materialization job handlers and outbox
   admission; add restart/retry/provider-outage tests.
8. Update docs/checksums, run complete acceptance checks, and archive this
   plan only after all evidence is captured.

## Progress

- [x] 2026-08-21 — Re-read root instructions, ordered specifications, memory
  system, security, interfaces, data model, operations, testing, ADRs, and
  current WP-09/WP-10 seams.
- [x] 2026-08-21 — Confirm first unfinished work package is WP-11 and create
  this ExecPlan before implementation.
- [x] 2026-08-21 — Add memory migration/state repositories and isolation tests,
  including composite memory/source/relation/candidate foreign keys and FTS.
- [x] 2026-08-21 — Add managed canonical Markdown Core boundary and recovery
  tests; ordinary user paths and reconciliation remain unable to expose or
  delete managed records.
- [x] 2026-08-21 — Implement MemoryService, candidates, lifecycle, sources,
  extraction validation, optional vector recall, lexical degradation, and
  canonical Markdown rebuild.
- [x] 2026-08-21 — Add MCP tools/resources and durable extraction,
  revalidation, projection-rebuild, and memory-embedding job handlers.
- [x] 2026-08-21 — Add source-revision revalidation, stale-memory recovery,
  multi-source retention, two-pass relation rebuild, missing-canonical-file
  quarantine, and per-Vault reserved-root job classification.
- [x] 2026-08-21 — Add supersession, historical recall, source update/delete,
  missing-file recovery, and provider-degradation integration coverage.
- [x] 2026-08-21 — Run final acceptance checks, refresh checksums, and archive
  this plan.

## Decisions

- Active durable memories are canonical managed Markdown plus authoritative
  operational projection metadata; FTS/vectors/entities/tags/relations are
  derived and rebuildable.
- Automatic extraction remains candidate-first. Explicit remember may create
  an unsourced assertion only when the caller supplies no source; such records
  are marked origin explicit_agent and retain that fact in metadata.
- Recall uses deterministic RRF with bounded type-specific temporal decay and
  never invokes an LLM. Query embedding is optional and provider outage
  reports degradation rather than failing lexical/context recall.
- Memory canonical paths are deterministic by UTC creation year/month and
  stable MemoryId. Reserved files use explicit Core methods and are excluded
  from ordinary user-plane path operations.
- Source revalidation compares note provenance revisions with current
  Vault-scoped file state. Unsupported extracted memories become stale, while
  a later current source can reactivate the same content hash without creating
  a duplicate canonical record.
- Relation projection rebuilds are deliberately two-pass because canonical
  supersession files can be enumerated in either order. A missing canonical
  file quarantines its operational row without deleting unrelated history.

## Surprises and discoveries

- The memory crate is still a stub, while WP-09 and WP-10 expose enough
  lexical/provider/vector seams to build the complete application boundary.
- Vault Core already has managed read and storage reserved-path validation but
  no managed write; this is a required architectural seam rather than a
  reason to write memory files directly from the memory crate.
- Existing outbox admission creates index jobs for file events, so memory
  extraction must be added without making indexing or provider availability
  part of the canonical write path.

## Validation

    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test -p mcp-vault-state --all-features
    cargo test -p mcp-vault-memory --all-features
    cargo test -p mcp-vault-mcp --all-features
    cargo test -p mcp-vault-server --all-features
    cargo test --workspace --all-features
    pnpm --dir frontend/admin lint
    pnpm --dir frontend/admin test
    pnpm --dir frontend/admin build
    bash scripts/check-docs.sh
    shasum -a 256 -c SHA256SUMS

Required evidence also includes two-Vault remember/recall isolation,
canonical Markdown round-trip, invalid direct Markdown quarantine, source
invalidation, contradiction/supersession, provider outage lexical fallback,
job lease/retry/restart, and bounded recall output tests. External MCP
conformance/Admin UI/Litmus remain later release/package gates.

## Rollback and recovery

Migration 0007 is forward-only and only adds operational/derived memory state.
Deleting/rebuilding candidates, FTS, entities, relations, or vectors never
deletes canonical Markdown. A managed Core journal crash is handled by the
existing startup recovery and reconciliation logic. If a canonical file is
present but projection state is missing, the memory rebuild parser recreates
the projection; if metadata is present but the file is absent, the record is
quarantined/stale except for an explicit operator repair path.

## Outcomes

WP-11 is complete. The service now has Vault-scoped memory state and
idempotency, Core-managed canonical Markdown, sourced lifecycle operations,
candidate-first extraction, source revision invalidation, deterministic hybrid
recall with safe provider degradation, MCP tools/resources, and durable
rebuild/revalidation/embedding handlers. Canonical content remains portable;
all operational projections are rebuildable and all protocol/application paths
retain the Vault boundary.

Validation evidence on 2026-08-21:

- `cargo fmt --all --check` passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  passed.
- `cargo test --workspace --all-features` passed, including 9 memory
  integration tests, 12 MCP tests, 18 server tests, and all existing
  WebDAV/Core/auth/provider/state suites.
- `pnpm --dir frontend/admin lint`, `test`, and `build` passed.
- `bash scripts/check-docs.sh` passed.
- `shasum -a 256 -c SHA256SUMS` passed.

WP-12 still owns Admin HTTP/UI memory browsing and candidate review. External
MCP conformance, WebDAV Litmus, and release-package gates remain WP-14 scope.
