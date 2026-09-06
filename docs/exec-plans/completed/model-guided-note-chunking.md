# Model-guided note chunking and diagnostic-only retrieval evaluation

Owner: Codex. Created/updated: 2026-09-06. Status: completed (implementation and offline acceptance; real effect comparison is a separate pending evaluation).

## Purpose and authority

User explicitly approved optional model grouping of original source units, complete
coverage validation, rule fallback and persisted reuse. User also rejected bundled
benchmark scores as a production semantic enable switch. This supersedes the
previous plan's automatic calibration requirement; preserve all unrelated fixes.
Governed by AGENTS.md, PLANS.md, product-requirements, architecture, ADR-0024/0026;
ADR-0028 records the user-approved amendment to ADR-0027.

## Current state and scope

HEAD 6500214 plus uncommitted regression fixes, schema 0018. Note chunks currently
recomputed deterministically by indexer for scheduling, resolution and retrieval.
Memory recall currently requires a passing bundled benchmark. Model bindings are
managed through actual Admin API/UI. Existing vectors and all current-set semantics
must survive. No paid requests, production Vault, push/deploy in this work.

## Design and invariants

Persist validated contiguous group boundaries as derived, Vault/file/source-scoped
state. Model sees numbered source units, returns groups only; body text comes from
the unchanged source projection. Query/status paths only read saved groups and never
call generation. New optional note_chunking generation binding opts in; unconfigured,
invalid or oversized proposals use rules. Preserve current valid vector inputs when
binding is added. Source changes permit new planning; unchanged sources reuse the
saved outcome, including fallback. Existing vector dimension validation stays strict.
Production semantic admission uses current valid vectors and bounded ranking; bundled
evaluations are diagnostic only and do not automatically dispatch on startup.

## Progress

- [x] Confirmed user change of direction; inspected existing dirty worktree and consumers.
- [x] Add ADR, derived plan storage, validation and production preparation/resolution.
- [x] Connect optional model binding in Admin and explain dimension semantics.
- [x] Remove automatic benchmark dependency from production query/startup behavior.
- [x] Add negative, persistence, stale-source, isolation and HTTP contract regressions.
- [x] Run relevant workspace/frontend/migration checks; update reports and operations.

## Decisions / discoveries

- Existing note section coordinates refer to the plain-text projection, not raw
  Markdown offsets. Model groups must preserve these coordinates and bytes.
- Benchmark failure is not model unavailability. Do not relabel quality_failed as
  passed or publish fabricated thresholds; retain historical reports as diagnostics.

## Validation and recovery

Record commands/results here during implementation. No irreversible migration or
canonical rewrite; additive schema only. Rollback requires coordinated prior-schema
backup and old binary as documented in memory-autocalibration-operations.md.

## Execution record

- Added schema 0019 with current-source guarded grouping and durable claim/fallback;
  preserved old vectors and retired automatic diagnostic jobs, not their cache/budget.
- Connected `note_chunking` in actual model binding API/UI and grouped input use in
  scheduling, embedding resolver, status and section-aware retrieval. Invalid model
  output uses saved rules; request timeout 60s, claim lease 120s.
- User clarified empty model settings must retain default bounds: groups enforce
  2048 total UTF-8 bytes including 512 context bytes; service defaults enforce
  8192 input bytes and 8192 vector dimensions if capabilities are absent. Dimensions
  remain model output validation, not automatic maximum selection or truncation.
- Added HTTP generation → persisted groups → embedding → section retrieval coverage,
  invalid output fallback/no repeated calls, cross-Vault and stale-source rejection,
  default-bound tests and schema 18→19 retirement preservation.
- Previous no-answer hard-negative assertions were updated for the explicit user
  policy change, not labeled quality improvements. A01 now checks zero startup
  diagnostic calls and immediate current-vector recall across restart.
- First full suite exposed an old test barrier blocking both memory and note calls;
  it now targets the note call explicitly and retains final deletion revalidation.
  Test compilation/fixture corrections and failed runs remain in target/memory-review.

## Outcomes

Implementation and offline/HTTP/browser regression complete: 325 default/all-feature
Rust tests, 35 frontend tests, Clippy, migrations and Chromium (including real role
binding) passed. Additional disk-reopen/source-edit regression passed. Real model
grouping quality remains unevaluated; no new paid call. See [acceptance report](../reports/model-guided-note-chunking-20260906.md) for actual commands, defaults, limits and environment findings.

- Local isolated service updated to schema 19 with consistent backup. Existing model
  bindings, completed reports/cache unchanged; requests 26→26, no grouping role bound
  and no new real generation calls.

## Authorized real paired follow-up — 2026-09-06

User bound `note_chunking` and explicitly requested real retrieval comparison.
New evaluation uses only isolated synthetic notes and read-only local configuration,
with a shared pre-dispatch maximum of 24 real requests (including retries), no
production Vault or user model-binding mutation. Frozen `chunking-paired.json`
contains 8 mixed-topic notes, 16 answerable queries and 4 no-answer queries; labels
were written before any model output. Compare rule/model chunks at identical model
and dimension using saved real vectors and production exact-cosine/object aggregation.
Report passage evidence as well as document rank; pure semantic experiment is not
full public hybrid recall or independent human evaluation. No post-result tuning.

Follow-up completed: 11 actual requests (8 generation + 3 embedding batches), 2048
dimensions, 47 unique saved inputs. Only 2/8 notes adopted model groups, 6 fell back.
Document Hit@1 stayed 16/16; top-chunk answer evidence fell 16/16→14/16. Both regression
cases inspected against source; no content lost, highest-ranked chunk lacked the
needed fact. Fallback subcauses were not preserved and remain undetermined. No
post-result tuning or paid rerun. See [real paired report](../reports/paired-chunking-real-20260906.md).

## Prompt clarification and authorized repeat — 2026-09-06

User requested clearer retrieval-purpose instructions and one real repeat. This
focused follow-up changes only the grouping prompt: independently embedded passages,
related complete facts with conditions/negations, avoidance of unnecessary fragments,
and the existing contiguous/byte constraints. No schema, ranking, dimension, timeout,
or source-cache invalidation change. Existing saved plans remain reusable.

Progress: prompt updated; 16 indexer tests, all-feature indexer Clippy, fmt/docs/diff checks passed. Real comparison completed with 11 requests; original sandbox listener failure preserved.
Decision: reuse the frozen original corpus for an explicit before/after comparison;
this is development-set evidence, not independent generalization acceptance. Keep
old output intact and use a new directory; cap actual requests at 24 including
embedding. No user notes or production configuration changed.

Repeat outcome: only 1/8 model plans adopted, 7/8 fallback; rules Hit@1 and answer
evidence 16/16 versus revised prompt 15/16, Recall@5 still 1. Correct irrigation
answer ranked second behind a water-quality note. This does not demonstrate an
improvement. Old-prompt evidence 14/16 is not a clean causal comparison because
adopted sources differ. No further real calls. See
[prompt repeat report](../reports/paired-chunking-prompt-v2-20260906.md).

## Superseded by rule-only policy — 2026-09-06

User ended model grouping. Its runtime, UI role and paired runner were removed;
these implementation and evaluation records are historical. ADR-0028 amendment
retains diagnostic-only evaluation and restores deterministic rule chunks everywhere.
See rule-only-note-chunking completion record for validation and compatibility.
