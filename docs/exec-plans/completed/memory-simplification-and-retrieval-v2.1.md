# Memory simplification and retrieval repair v2.1

Status: Complete; real-model quality remains explicitly unverified
Owner: Codex
Created: 2026-09-05
Last updated: 2026-09-05 17:20 CST
Baseline: `7b4913710ee9bdae7e8239eea80b78f9a6bef47e`

## Purpose and user-visible result

MCP Vault will expose only current memory. Forgetting a memory deletes its
current canonical copy and every online projection; it does not archive a
model-readable historical record. One source note owns one current generated
memory set, and a successful re-extraction replaces that set as a whole. A
source-content change invalidates the old set before regeneration, while a
same-File-ID rename updates navigation without invoking a model.

Explicit memories remain independently owned canonical Markdown and preserve
caller-supplied metadata. Automatic extraction makes one bounded structured
generation call returning `memories[]`; the model supplies useful semantic
content, not IDs, source coordinates, lifecycle actions, confidence, or
database bookkeeping. Recall remains LLM-free and combines current durable
memory with separately typed ordinary-note cues.

Retrieval will reject irrelevant lexical, semantic, alias, recency, and
importance-only candidates; keep fused score and raw cosine distinct; validate
embedding input/profile hashes; aggregate chunks per object; cover long
documents; and enforce one complete response budget.

## Governing requirements

- `AGENTS.md`: canonical Markdown, Vault isolation, Vault Core writes, protocol
  boundaries, secret handling, recovery, and required checks.
- `PLANS.md`: this living plan, evidence, rollback, and completion rules.
- `docs/product-requirements.md` sections 3.3, 3.5, 3.7, 3.9 and 4.
- `docs/architecture.md` sections 3, 4, 7, 9, 10, 11, 12, 13 and 17.
- `docs/interfaces.md` sections 4-8 and Admin memory routes in section 10.
- `docs/data-model.md` sections 1, 2, 9, 10, 13, 15, 18 and 19.
- `docs/memory-system.md`, `docs/security.md` sections 12, 14-16, and
  `docs/development-and-testing.md` sections 7, 8, 12-14, 16 and 18.
- ADR-0001, 0002, 0007, 0010, 0011, 0013, 0016, 0017, and 0022-0025.
- ADR-0026, added by this work, supersedes the historical lifecycle,
  two-phase global consolidation, destructive-generation reset, and source
  health graph while retaining portable canonical memory, exact provenance,
  persisted multilingual aliases, bounded embeddings, and note-chunk
  aggregation.

## Current repository state

The clean baseline exactly matches the design-review commit. `memories` stores
seven lifecycle states and required numeric importance/confidence. Explicit
`remember` writes `memory_stage1_outputs`, then queues `memory.consolidate`.
Automatic extraction expects `raw_memory`, `rollout_summary`, and
`rollout_slug`; Phase 2 proposes create/update/archive/supersede actions and
writes `MEMORY.md`, `memory_summary.md`, `raw_memories.md`, source summaries,
and per-record files. Source validity is represented by
`memory_source_health`. MCP accepts `include_historical`; Admin exposes
archive/restore/source-health/global retrieval-enrichment controls.

Current recall admits recency-only project/progress rows, permits any
non-negative semantic hit, labels reciprocal-rank contributions as lexical or
semantic without exposing raw cosine consistently, lets the first over-budget
item through, and stops rather than trying later shorter items. Note chunks are
byte windows over flattened text, reuse whole-note leading heading context,
and sample rather than completely cover content beyond 128 chunks.

Migrations end at `0014_multilingual_memory_retrieval.sql`. New schema changes
must use `0015` or later and must not edit published migrations.

## Scope and non-scope

Included: current-only memory reads, true deletion, source-level pause/CAS,
note-owned current sets, atomic full-set replacement, one-call extraction,
explicit-memory metadata fidelity, old-data preflight/authorization, removal
of normal Phase 2/source-health/lifecycle behavior, MCP/Admin/frontend changes,
relevance/vector/chunk/budget repair, deterministic evaluation, isolation and
recovery tests, ADRs and specifications.

Excluded: knowledge graphs, automatic cross-note fact fusion, semantic delete
blacklists, routine human approval, a new vector database, query-time
generative LLMs, production deployment, paid-provider calls, production Vault
mutation, and automatic execution of a destructive old-data migration.

## Invariants and risks

- Ordinary source notes are never rewritten or deleted by memory cleanup.
- Every row, job, cache key, vector, audit fact, and operation is Vault-scoped.
- Canonical writes and deletes use Vault Core; protocol handlers call services.
- Only a published set whose source File ID/hash still matches is readable.
- A stale job publishes only after checking source hash, source pause state,
  and expected set revision. Failure cannot be interpreted as an empty set.
- File and SQLite commits are not one transaction. A persisted one-source
  snapshot plus existing journal/outbox recovery must make retries idempotent.
- Removed content cannot be retrieved through known IDs, lifecycle filters,
  aggregate artifacts, resources, indexes, vectors, caches, or managed-path
  aliases. Vault Core history/backups remain separately retained and are never
  silently presented as current memory.
- Existing explicit metadata must not be inferred from defaults or collapsed
  during round trips. Automatic metadata remains optional.
- Existing mixed/cross-source records require a migration report and operator
  decision; code must not silently assign or delete them.

## Proposed design

### Current ownership model

`explicit` memories use one canonical record file per memory. `note_derived`
items belong to one `memory_note_sets` row keyed by `(vault_id,
source_file_id)`. The set records source content hash/revision, monotonically
increasing set revision, extraction pause state, published state, canonical
file identity/path/revision, and timestamps. Its items use server-generated
Memory IDs and one set Markdown file. Legacy lifecycle columns may remain only
as migration input; current queries require active current ownership and never
expose a historical state.

`memory_note_set_snapshots` persists at most one prepared snapshot for a source
operation. It contains the exact source hash, expected set revision, validated
items, provider/profile identity, and application-generated IDs. Retrying
adopts the same snapshot and IDs without another Provider call. Publish checks
the current File ID/hash, pause flag, and set revision immediately before the
managed file and projection handoff.

### Commands and source events

`remember` validates and writes explicit canonical memory immediately. Updates
use three-state optional fields and retain omitted values. `forget_memory`
always deletes; deleting a note-derived item rewrites its set and pauses that
source until an explicit resume/regenerate action. The result returns IDs and
effects, never deleted content.

Source content change immediately makes a published set ineligible, advances
its set revision, and coalesces extraction for the latest hash. Rename with the
same File ID/hash updates the displayed path only. Delete removes the set and
cancels stale publication. A delete-and-recreate path has no inherited set. If
Vault Core represents an explicit recreation as a tombstone restoration and
therefore retains its File ID for ordinary-note history, memory still allocates
a new MemorySet ID and new item IDs. Explicit memory is unaffected.

### Extraction and retrieval

Automatic extraction requests `{memories:[{content,kind?,tags?}]}`. Only the
array and non-empty content are required. Optional-field defects are normalized
without dropping content; missing/ambiguous/truncated roots fail the operation.
A valid empty array publishes an empty set. Prompts demand separately
retrievable supported facts, progress, environments, decisions, conditions,
negation, uncertainty, and non-adoption boundaries while treating source text
as untrusted data.

Ranking first establishes relevance from calibrated semantic evidence or
strong normalized lexical/entity evidence, then applies optional continuity
boosts. Diagnostics distinguish raw BM25/cosine, RRF contributions, candidate
count, relevant count, returned count, coverage, degradation, and budget
truncation. Object vectors match Vault, type, model/profile, dimension,
canonical/source hash, and exact embedding-input hash. Markdown-aware chunks
carry their local heading and cover the full supported input or report an
explicit incomplete range. One object contributes once. Budgeting includes
body/title/source/headings and skips oversized items before trying later ones.

### Upgrade boundary

Migration `0015` adds current ownership/set/proposal/pause and migration-report
state without deleting canonical knowledge. New/fresh Vaults activate the new
contract immediately. Existing rows are classified into reliably explicit,
single-note-derived, and ambiguous/mixed groups. All legacy memory is excluded
from model reads after upgrade. An authenticated, confirmed Admin migration
operation preserves reliable explicit records, regenerates note-derived sets,
and leaves ambiguous content in the preflight report/backup until the operator
chooses. Old aggregate/raw/summary artifacts and obsolete projections remain
outside every current/model read and may be removed through Vault Core only in
a separately authorized cleanup; this migration never performs an implicit
purge.

## Work breakdown and progress

- [x] 2026-09-05 10:10 CST — S0: added ADR-0026, updated the normative
  contract, created the 40-query/15-generation synthetic corpus and executable
  evaluator, and measured the immutable `7b4913710ee9...` baseline.
- [x] 2026-09-05 12:05 CST — S1: routed MCP/Admin/runtime reads through
  `memory_current_items`, implemented physical canonical deletion, source-set
  pause, vector cleanup, and reserved-path rejection for current and historical
  note reads.
- [x] 2026-09-05 13:20 CST — S2: added migration 0015 and
  `CurrentMemoryRepository`, deterministic current Markdown, whole-set CAS,
  prepared-snapshot adoption, File-ID/hash fail-closed reads, and injected
  post-canonical recovery coverage.
- [x] 2026-09-05 14:10 CST — S3: replaced the two-phase Provider contract with
  one bounded `memories[]` response, made explicit remember synchronous and
  idempotent, retired obsolete workers/routes/UI concepts, and removed replay
  helpers and aggregate-artifact generation.
- [x] 2026-09-05 16:45 CST — S4: implemented full-input migration confirmation,
  current-vector freshness, raw-cosine calibration, per-object aggregation,
  full Markdown chunk coverage, relevance gates, shared response-budget reuse,
  and count/truncation diagnostics.
- [x] 2026-09-05 17:20 CST — S5: updated specifications and Admin UI, completed
  Rust/frontend/MCP/HTTP validation, recorded the host-only ONNX linker issue,
  and explicitly left real-model semantic quality unverified because no
  Provider/data/cost authorization was supplied.

## Test migration matrix

The following are executable symbols, not proposed names. Several integration
tests intentionally cover more than one row because ownership, canonical
writes, and vector invalidation must be proven together.

| ID | v2.1 evidence |
|---|---|
| T01 | `semantic_note_score_uses_cosine_and_discards_negative_similarity`; `semantic_admission_requires_a_calibrated_profile_and_raw_cosine_floor` |
| T02 | `recall_gates_unrelated_queries_and_never_exposes_another_vault` verifies lexical operation plus explicit semantic degradation rather than fabricated cosine |
| T03 | `explicit_memory_is_direct_idempotent_revisioned_and_physically_deleted` covers Markdown/projection rebuild and all optional metadata |
| T04 | `update_memory_json_distinguishes_omitted_set_and_clear_fields`; `memory_patch_json_distinguishes_omitted_set_and_clear_fields` |
| T05 | `explicit_memory_is_direct_idempotent_revisioned_and_physically_deleted` preserves validity and does not synthesize confidence during update/rebuild |
| T06 | `note_source_owns_one_fail_closed_replaceable_set_and_move_needs_no_model`; deterministic generation cases g01-g15 |
| T07 | `duplicate_source_facts_and_explicit_memory_keep_independent_ownership` proves presentation dedup does not merge durable owners |
| T08 | `extraction_contract_is_one_current_set_without_model_owned_actions`; generation cases for third-party/tutorial, negation, and non-adoption |
| T09 | generation condition cases and report counters (`condition_errors = 0` for the deterministic v2.1 fixture output) |
| T10 | `lexical_admission_accepts_exact_questions_and_rejects_noise_or_negation`; hard-negative q31-q40 |
| T11 | `lexical_question_admission_accepts_strong_cross_language_metadata_only`; Chinese weak-overlap negatives in the same unit coverage |
| T12 | q02/q04/.../q30 plus `lexical_question_admission_accepts_strong_cross_language_metadata_only` |
| T13 | explicit update and whole-source replacement assertions delete old vectors and require exact current content/input hashes |
| T14 | `semantic_admission_requires_a_calibrated_profile_and_raw_cosine_floor`; `provider_service_uses_encrypted_secrets_and_vault_model_bindings` |
| T15 | `provider_service_uses_encrypted_secrets_and_vault_model_bindings` rejects zero, NaN, Inf, and wrong-dimension vectors/queries |
| T16 | `semantic_note_ranking_uses_one_best_current_chunk_per_note`; duplicate-owner integration test |
| T17 | `markdown_projection_preserves_heading_paragraph_list_code_and_table_adjacency`; multibyte/context chunk test |
| T18 | `note_embedding_chunks_are_bounded_deterministic_and_fully_cover_past_128`; paged vector-index assertion crosses the configured scan-page boundary |
| T19 | `long_memory_embeddings_cover_head_middle_and_tail_once_per_chunk_key` |
| T20 | budget section of `recall_gates_unrelated_queries_and_never_exposes_another_vault` skips the oversized first result and returns the later short result |
| T21 | the same integration test proves unused related-note reservation returns to the one shared budget and verifies counts/truncation |
| T22 | response token estimates include actual source/headings; MCP structured-output tests ensure source bodies and diagnostics are not leaked |
| T23 | `note_source_owns_one_fail_closed_replaceable_set_and_move_needs_no_model` covers change, move, delete, recreate-with-fresh-set, vectors, and pause |
| T24 | `extraction_distinguishes_empty_failure_and_inflight_source_change`; `prepared_snapshot_recovers_after_canonical_commit_without_another_model_call` |
| T25 | `obsolete_memory_job_with_old_cursor_is_discarded_before_handler_call`; retired-job assertions in full extraction/production handler tests |
| T26 | recall integration, Provider vector partitioning, `pat_cannot_use_the_other_vault_endpoint`, and state composite-FK tests |
| T27 | strict extraction schema/prompt tests, encrypted-secret Provider integration, redacted job progress, and MCP reserved-path rejection |
| T28 | `legacy_migration_is_preflighted_non_destructive_and_preserves_only_safe_explicit_rows`; Admin confirmation/CSRF test; migration-15 schema checks |

Additional v2.1 acceptance is explicit in
`remember_and_forget_are_current_only_across_all_model_read_paths`, including
get/list/recall/context/resource/known-ID and managed current/history bypasses.
The memory integration suite separately proves restart-style rebuild after
delete, an in-flight result losing to deletion/pause, valid empty-set versus
invalid response, source-hash conflict, independent explicit/source owners,
same-path recreation with fresh set/item identities, and canonical snapshot
recovery without a second model call.

## Validation

Final command record (all on 2026-09-05, exit 0 unless stated otherwise):

| Command | Result |
|---|---|
| `git status --short`; `git rev-parse HEAD` | Expected implementation worktree; baseline HEAD `7b4913710ee9bdae7e8239eea80b78f9a6bef47e` |
| `sha256sum /tmp/mcp-vault-baseline.59CESn/baseline.tar`; `git archive HEAD \| sha256sum` | Identical archive digest `18c1e18188b0dd669fe5f2beea892b3f707b590a8c01997d90131aafde4bbb74`, proving the baseline tree is the exact HEAD archive |
| `cargo run --locked --manifest-path /tmp/mcp-vault-baseline.59CESn/Cargo.toml -p mcp-vault-memory --example quality_baseline` | Passed; wrote `target/quality/baseline.json` against the shared corpus |
| `MCP_VAULT_GIT_HEAD=7b4913710ee9bdae7e8239eea80b78f9a6bef47e cargo run --locked -p mcp-vault-memory --example quality_eval -- --mode deterministic --fixtures tests/fixtures/memory-quality --output target/quality/after.json` | Deterministic acceptance passed |
| `cargo metadata --locked --no-deps --format-version 1` | Passed |
| `cargo test --locked -p mcp-vault-memory` | 7 unit + 7 integration tests passed |
| `cargo test --locked -p mcp-vault-indexer` | 14 tests passed |
| `cargo test --locked -p mcp-vault-providers` | 12 unit + 8 integration tests passed |
| `cargo test --locked -p mcp-vault-state` | 19 unit + 3 auth + 11 background + 11 repository tests passed |
| `cargo test --workspace` | All workspace and doc tests passed, including MCP 23, Admin 24, Server 43, Core 23, WebDAV 9, and the suites above |
| `cargo fmt --all --check` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --workspace --all-features` | Exit 101 before tests at native link: the downloaded `ort-sys` ONNX Runtime archive requires unavailable host glibc/libstdc++ symbols `__isoc23_strtol`, `__isoc23_strtoll`, `__isoc23_strtoull`, and `_M_replace_cold`. All-feature Rust checking/Clippy passed, so this is recorded as an external host ABI blocker, not a passing test. |
| `pnpm --dir frontend/admin install --frozen-lockfile`; `lint`; `test`; `build` | Passed; 30 Vitest tests and production Vite build |
| `MCP_VAULT_CONFORMANCE_PACKAGE=/tmp/mcp-vault-conformance-src.CKYcxp/conformance-74edef34d674f563537be8c6587cebaa58e830ca bash scripts/conformance/mcp.sh` | Official 2026-07-28 scenarios passed; only the declared expected baseline failure for unsupported `prompts/list` caching occurred. The package override avoided npm 10.9.2's local GitFetcher failure in the wrapper's default installer. |
| `bash scripts/interop/http-smoke.sh` | Passed OAuth/PKCE/refresh/CSP, MCP discovery/Origin, 50 concurrent WebDAV PUTs, revisions, and plane separation |
| `bash scripts/check-docs.sh`; `git diff --check` | Passed |

Deterministic report comparison:

| Metric | Immutable baseline | v2.1 worktree |
|---|---:|---:|
| Queries / answered / hard negatives | 40 / 30 / 10 | 40 / 30 / 10 |
| Recall@5 | 0.9333 | 1.0000 |
| MRR@5 | 0.8278 | 1.0000 |
| No-answer false-return rate | 1.0000 | 0.0000 |
| Labeled fake support precision | 0.4286 | 1.0000 |
| Labeled fake critical-fact coverage | 0.5417 | 1.0000 |
| Subject / condition / type errors | 2 / 9 / 1 | 0 / 0 / 0 |
| Explicit metadata exact round trips | not measured | 12 / 12 |

The v2.1 retrieval run made zero Provider requests and reported p50/p95/max of
2/4/4 ms on this local synthetic run. Fake generation validates schema,
accounting, and regression fixtures only (`semantic_quality_proven: false`). A
real model was **not run** because the user supplied no explicit
Provider/data/cost authorization. Consequently the 95% support-precision and
90% critical-fact real-model targets remain unverified and are not marked
passed.

## Rollback and recovery

Schema migrations are forward-only. Rollback restores a verified consistent
backup or deploys a corrected new binary; an old binary is not promised to
understand schema 15. Daily set publication is idempotently resumed from its
single prepared snapshot and rejected when source hash, pause state, or set
revision changed. Upgrade apply requires an explicit preflight confirmation;
restoring a backup warns that post-backup deletions may return and does not
silently expose restored legacy memory to model reads.

## Decisions

- Source ownership replaces cross-note global fusion.
- The model proposes content only; application code owns identity and writes.
- Source change fails closed even when regeneration fails.
- Derived-item deletion pauses only that source; explicit resume is required.
- Important general knowledge remains extractable, but instruction text cannot
  be promoted into claims about user adoption or mastery.
- Retrieval repair keeps the existing vector backend and query-time LLM-free
  contract.
- Migration confirmation hashes every legacy memory/source/tag/entity field
  consumed by apply, then recomputes under the per-Vault write lock; count-only
  confirmation was rejected because same-class content could change.
- Vault Core's explicit create-after-delete path preserves ordinary file
  history by restoring its tombstone/File ID. Memory ownership therefore uses
  deletion of the old set plus a fresh MemorySet ID and item IDs, which achieves
  non-inheritance without changing global note-history semantics.

## Surprises and discoveries

- Baseline code copies a missing rollout summary in the Provider adapter, but
  `normalize_stage1_generated_output` still couples empty semantic fields.
- Invalid Stage 1 slugs already degrade locally and are not a whole-batch
  failure.
- Current recall treats a roughly `0.02` RRF contribution as the public fused
  score; it is not a 2% cosine similarity.
- The pinned toolchain initially required rustup component installation outside
  the read-only sandbox; no repository file was changed by that setup.
- MCP `read_note(revision)` and `note_history` initially provided a hidden route
  to a known managed-memory path. A single `parse_user_tool_path` boundary now
  rejects the reserved namespace for all ordinary current/history operations,
  and the public MCP test covers it.
- The first shared-budget implementation permanently stranded the memory share
  reserved for related notes. Deferred memories are now retried after actual
  note consumption, so either result type can use genuinely unused capacity.
- Fault injection after Vault Core's `MetadataCommitted` phase showed that the
  prepared source snapshot is sufficient: retry adopted byte-identical
  canonical output, retained generated IDs, and made no second Provider call.
- The initial quality report embedded a mistyped baseline SHA. Comparing it to
  both `git rev-parse HEAD` and the archived baseline report caught the error;
  the evaluator now names the real immutable commit.
- The default conformance installer hit npm 10.9.2's `GitFetcher requires an
  Arborist constructor` bug. Building the exact pinned upstream commit from its
  tarball and using the supported package override produced a passing official
  run.
- `cargo test --workspace --all-features` cannot link this host's downloaded
  ONNX Runtime binary because its glibc/libstdc++ ABI is newer than the host.
  The same all-feature graph passes `cargo clippy --all-targets`, and the normal
  workspace test graph passes completely.

## Outcomes

All non-external v2.1 acceptance criteria are implemented and pass. MCP Vault
now has one model-readable current domain: explicit canonical records and
source-owned current note sets. Delete is physical at that domain, source
change fails closed, source deletion/pause defeats stale work, and retained
Vault history/backup data is not exposed through memory or ordinary managed
path reads. Direct remember no longer waits for consolidation, and automatic
extraction makes one bounded validated Provider call whose retryable snapshot
owns stable server-generated IDs.

Migration 0015 is additive. Legacy tables and canonical history remain for
backup/operator recovery but are excluded from current reads. Authenticated
Admin preflight/apply preserves only unambiguous explicit/import rows, binds
confirmation to the exact classified input, reports mixed/unsupported rows,
and schedules note regeneration without implicit cleanup. No production Vault,
credential, deployment, or paid Provider was touched.

Retrieval now requires lexical/entity evidence or calibrated current semantic
evidence, exposes raw cosine separately from weighted fusion, validates exact
profile/input/content identity, scans vector pages, aggregates one score per
object, fully covers supported long input, and shares one honest response
budget. The deterministic corpus improves Recall@5 from 0.9333 to 1.0 and
hard-negative false returns from 100% to 0% on the same 40 cases.

Removed runtime concepts include normal Stage 1/Phase 2 consolidation,
supersession/archive/restore memory actions, source-health/repair flows,
retrieval-enrichment jobs, aggregate raw/summary artifacts, old replay examples,
and their Admin/frontend controls. Legacy repository code remains narrowly as
the non-destructive migration/backup adapter.

Remaining external verification is explicit: real-model semantic quality was
not authorized, and this host cannot link the optional ONNX Runtime all-feature
test binaries. Neither is represented as passed.
