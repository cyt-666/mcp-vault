# Align memory failure boundaries with Codex

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-27
- Updated: 2026-08-27

## Purpose and user-visible result

Phase 1 and Phase 2 must stop failing on bookkeeping that upstream Codex does
not delegate to the model or treat as semantic concurrency. Phase 1 will use the
Codex three-field extraction contract and derive note provenance locally. Phase
2 will tolerate operational projection-revision drift when semantic state is
unchanged, avoid rebuild feedback loops, reject obsolete prepared contracts,
and wait for an unrelated active extraction batch before admission.

After deployment, the current failed regeneration can resume without evidence
line errors or a permanently stale proposal. No user note, Provider secret, or
canonical non-memory Vault content is rewritten by the upgrade itself.

## Governing requirements

- `AGENTS.md`: transparent sourced memory, Vault-scoped state, untrusted LLM
  output, canonical Markdown, safe writes, and recoverable jobs.
- `docs/product-requirements.md` section 3.5: two-phase extraction and
  consolidation, provenance, autonomous operation, and LLM-free recall.
- `docs/architecture.md` section 11 and `docs/memory-system.md` sections 3-4:
  phase boundaries, prepared recovery, and derived projections.
- ADR-0016: Codex-style two-phase memory; this plan corrects prior amendments
  that added model-owned evidence coordinates and over-broad snapshot locks.
- OpenAI official Memories documentation: generation happens in the background
  after eligible inputs are idle; extraction and consolidation have separate
  model roles.
- Upstream Codex source:
  `codex-rs/memories/write/src/phase1.rs` uses only `raw_memory`,
  `rollout_summary`, and `rollout_slug`; `phase2.rs` materializes current inputs,
  uses workspace diff, and completes an exact selected watermark without
  invalidating the run for later inputs.

## Current repository state

- Phase 1 currently asks MiMo for `start_line`/`end_line` evidence. The live job
  recorded `memory_phase1_evidence_too_large` and
  `memory_phase1_evidence_anchor_invalid` before stopping at 147/178.
- Phase 1 calls `ensure_no_prepared_consolidation` before Provider work and
  before committing its row. A stale prepared proposal therefore blocks the
  extraction job before another Provider call.
- Phase 2 persists a typed proposal and compares every current-memory revision
  to the captured snapshot. It currently treats projection-only revision drift
  as a semantic conflict.
- Every reserved managed-file event enqueues `memory.rebuild`. `rebuild` scans
  every canonical record and calls `replace_bundle` even when the parsed
  projection is unchanged; `replace_bundle` intentionally increments revision.
- Live proposal `01a04163-8b08-7ec3-a3da-e97d433cc384` has one create action,
  one dirty input, 143 raw inputs, and 11 current memories. All raw snapshots
  still match. All 11 current content hashes/statuses match, but their revisions
  advanced by 136 due to rebuild feedback, producing five
  `memory_consolidation_snapshot_changed` failures.

## Scope

### Included

- Replace the Phase 1 model schema/prompt with three semantic fields and
  locally derive one whole-note revision/hash provenance anchor.
- Bump Phase 1 prompt/profile versions without requiring model-selected lines.
- Limit projection rebuild admission to canonical memory-record paths.
- Make projection rebuild idempotent when canonical content and derived state
  already match.
- Treat identical content/status with a newer operational revision as the same
  Phase 2 semantic snapshot; refresh expected revisions locally for targeted
  actions without overwriting changed content.
- Reject prepared proposals from an older prompt contract before generation.
- Avoid admitting reconciliation Phase 2 work while an unrelated Phase 1 job
  is active; the completing Phase 1 job still admits its follow-up.
- Regression tests, Admin labels, ADR/spec updates, isolated replay, and full
  Rust/frontend validation.

### Not included

- Embedding the complete Codex agent runtime or granting a Provider arbitrary
  filesystem/shell access. MCP Vault keeps its shared provider boundary and
  Vault Core writes.
- Weakening content-hash, Vault, source-revision, or action-target concurrency
  checks.
- Query-time LLM recall or a second memory authority.
- Mutating the live deployment during diagnosis or validation.

## Invariants and risks

- Note provenance remains local and revision-bound even though it is no longer
  a model-selected line range.
- A semantic Admin edit to a targeted current memory must still conflict; only
  same-content/status revision churn is normalized.
- Rebuild must remain able to import a genuine canonical Markdown edit and
  quarantine malformed/missing records.
- An old prepared proposal may be rejected only before applying its contract;
  current-version partial-apply markers remain recoverable.
- New Stage 1 inputs arriving after a selected Phase 2 batch remain dirty for a
  later generation rather than invalidating the committed selection.
- Provider output and memory text remain absent from logs and job errors.

## Proposed design

### Phase 1

The Provider schema becomes:

```json
{
  "raw_memory": "...",
  "rollout_summary": "...",
  "rollout_slug": "..."
}
```

The source note is untrusted Markdown without synthetic line labels. A ready
output receives one locally generated provenance record containing file ID,
path, exact revision, and normalized full-source hash. A no-op uses empty
semantic fields. MiMo's narrowly authorized missing-summary fallback remains,
but there are no model evidence coordinates to reject.

### Phase 2 and rebuild

Outbox admission recognizes only
`<reserved-root>/memory/records/**/*.md` as projection input. Rebuild compares
the parsed bundle with the existing projection and skips an identical record,
so aggregate artifact writes cannot recursively bump every memory revision.

Prepared snapshot comparison uses status/content hash as semantic identity.
For an action target whose semantics are unchanged but revision advanced,
preparation refreshes the expected revision to the current value. Changed
content/status still conflicts. A prepared proposal whose prompt version is not
the current consolidation contract is marked rejected and never reused.

Reconciliation does not create a consolidation job while another extraction
job is active. The extraction job may still enqueue its own follow-up at the end
of its handler, closing the admission race without running the full batch and
global consolidation together. A previously claimed Phase 2 job defers through
the job repository's attempt-neutral release path until Phase 1 is terminal.

## Work breakdown

1. Add Phase 1 three-field schema/prompt and local whole-source provenance in
   `crates/memory/src/service.rs`; update provider fakes and extraction tests.
2. Add semantic snapshot normalization and obsolete-proposal rejection across
   `crates/memory` and `crates/state` with partial-apply recovery tests.
3. Narrow outbox projection events and make `MemoryService::rebuild` idempotent;
   prove repeated rebuilds do not increment memory revisions.
4. Gate Phase 2 reconciliation admission against unrelated active extraction;
   add overlapping-job tests and preserve end-of-Phase-1 admission.
5. Update ADR-0016, memory/architecture/provider documentation and Admin error
   text; run isolated MiMo extraction/consolidation replays.
6. Run formatting, Clippy, workspace tests, frontend lint/tests/build, inspect
   source-state isolation, and move this plan to `completed/`.

## Progress

- [x] `2026-08-27` — Compared official documentation and upstream Codex Phase 1,
  Phase 2, job watermark, and workspace-diff source paths.
- [x] `2026-08-27` — Confirmed the live Phase 1 evidence failures and the exact
  revision-only stale prepared proposal without reading memory bodies.
- [x] `2026-08-27` — Implemented the exact Codex Phase 1 wire fields and local
  whole-source provenance; both live failure notes pass isolated real MiMo.
- [x] `2026-08-27` — Implemented Phase 2 semantic-revision normalization,
  old-contract rejection, idempotent/narrow rebuild, and attempt-neutral Phase
  1 admission barriers.
- [x] `2026-08-27` — Passed isolated real-provider Phase 1, five-input Phase 2,
  and one-input incremental Phase 2 replay.
- [x] `2026-08-27` — Passed complete workspace/frontend validation and closed
  the plan.

## Decisions

- Remove model-selected line ranges instead of relaxing their bounds. The
  upstream Codex contract never asks the extraction model for them.
- Preserve source trust with application-derived whole-note revision/hash
  provenance rather than model assertions.
- Keep the typed prepared proposal for Vault Core crash recovery, but stop
  treating internal projection revision churn as semantic change.
- Preserve the shared generic Provider boundary; copying Codex's unrestricted
  internal agent runtime is outside MCP Vault's provider/security contract.

## Surprises and discoveries

- The screenshot's final Phase 1 error was not another model failure: the 0ms
  current-note failure was a prepared-proposal conflict mapped to the generic
  retry label.
- The prepared Phase 2 proposal's raw inputs were all unchanged. Every current
  memory had the same content hash/status as captured; only revisions changed.
- Reserved aggregate/source-summary writes each admitted a full rebuild, and an
  unchanged rebuild increments every memory revision through `replace_bundle`.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
env CI=true pnpm --dir frontend/admin lint
env CI=true pnpm --dir frontend/admin test
env CI=true pnpm --dir frontend/admin build
bash -n scripts/debug/phase2-replay.sh
```

Focused acceptance must additionally prove:

- Phase 1 output contains no evidence field and a 178-note-style batch cannot
  fail on model line coordinates.
- two identical rebuilds leave every memory revision unchanged;
- aggregate artifact events do not enqueue memory rebuilds;
- revision-only drift can recover a prepared create/update proposal while a
  content/status edit still conflicts;
- active unrelated extraction prevents reconciliation consolidation admission,
  while the completing extraction's follow-up is admitted;
- a stale old-contract prepared proposal is rejected instead of retried five
  times.

Observed on 2026-08-27:

- both live Phase 1 failure notes passed isolated real MiMo extraction after
  removing evidence coordinates;
- the final exact Codex field names passed a second isolated MiMo extraction;
- a five-input initial Phase 2 created five memories at generation 1;
- a later one-input update Phase 2 updated one existing memory at generation 2;
- memory unit/integration suites passed 9/15 tests and Server passed 42 tests,
  including 81/257-input, idempotent rebuild, stale-proposal, and attempt-neutral
  deferral coverage;
- formatting, full-feature Clippy, workspace tests/doc-tests, Admin lint, 24
  frontend tests, TypeScript, and production build passed. The first sandboxed
  workspace run hit the known loopback-bind restriction in an existing Indexer
  test; the complete suite passed outside that restriction.

## Rollback and recovery

No migration is planned. Reverting restores the prior prompt and conflict
behavior. Existing applied proposals remain historical. Current-version partial
proposals remain recoverable; only obsolete prompt-version proposals are
rejected. Generated artifacts remain in Vault history.

## Outcomes

Phase 1 now uses `memory-stage1-v4` and the exact Codex
`raw_memory`/`rollout_summary`/`rollout_slug` wire object. Automatic note
provenance is application-derived from file/path/revision and normalized
whole-source hash; MiMo no longer returns evidence coordinates.

Phase 2 uses `memory-consolidation-v4`. Aggregate artifacts cannot admit
projection rebuilds, same canonical revisions rebuild as no-ops, revision-only
drift is normalized, old contracts and unapplied stale snapshots are rejected,
and partial markers retain crash recovery. Consolidation admission waits for
unrelated Phase 1 work and claimed jobs defer without consuming attempts.

No schema migration, live deployment write, or source-note mutation was needed.
The isolated real-Provider copies were deleted after validation.
