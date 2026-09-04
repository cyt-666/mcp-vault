# Implement source-language memory and offline multilingual recall

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-09-03
- Updated: 2026-09-03

## Purpose and user-visible result

Chinese and English questions must retrieve relevant durable memory regardless
of the canonical memory body's language. New memory stays in its source
language. Existing active, stale, and superseded memory can be explicitly
backfilled in place, with safe source-language rewrites and source-language,
Simplified-Chinese, and English retrieval aliases. Recall remains LLM-free and
reports incomplete alias coverage instead of claiming complete results.

## Governing requirements

- `docs/product-requirements.md` sections 3.3, 3.5, and 3.6.
- `docs/architecture.md` sections 3, 4, and 11.
- `docs/interfaces.md` sections 4 and 6.7.
- `docs/data-model.md` memory, vector, derived-state, and migration sections.
- `docs/security.md` provider-input, generated-output, logging, and Vault Core
  boundaries.
- ADR-0010, ADR-0011, ADR-0013, ADR-0016, ADR-0017, ADR-0022, and ADR-0023.

## Initial repository state

- At plan start, `MemoryService::recall` sent an escaped whitespace AND expression to
  `memory_fts`, optionally embeds the unchanged query, and ignores returned
  cosine magnitude after vector ranking.
- At plan start, SQLite vector search loaded a model/dimension partition before callers filtered
  `object_type`, so note chunks can occupy the memory Top-K pool.
- Before this work, Phase 1 v4 and consolidation v6 did not constrain output
  language.
- Canonical memory Markdown owns durable facts; FTS, embeddings, and the new
  aliases are derived and rebuildable.
- Migration 0013 was current at plan start. The worktree also contains a reviewed but
  uncommitted optional-slug normalization fix in five files; this plan must not
  overwrite or revert it.

## Scope and non-scope

Included: source-language prompt behavior, derived multilingual metadata,
prepared enrichment recovery, Admin-confirmed existing-memory backfill,
automatic new-memory enrichment, CJK-aware FTS, typed vector scope, recall
coverage, Admin/MCP/UI contracts, migrations, tests, and documentation.

Excluded: bilingual canonical bodies, query-time LLM translation, a new model
role, destructive Stage 1 reset, automatic paid backfill on upgrade, and
semantic merge/split/deduplication during language repair.

## Invariants and risks

- Every metadata/proposal/job/index query includes the Vault predicate.
- Provider output is untrusted, bounded, secret-redacted, and never logged.
- Enrichment failure cannot roll back or suppress an existing memory.
- A rewrite keeps identity, status, type, provenance, relations, validity, and
  canonical history; revision conflicts never overwrite newer content.
- Prepared output is persisted before canonical writes so recovery never
  repeats a paid request after partial apply.
- Alias text never participates in source identity, evidence health, duplicate
  identity, or final-memory content hashes.

## Proposed design

Migration 0014 adds `memory_retrieval_metadata` keyed by Vault/memory and exact
content/profile hashes plus `memory_retrieval_proposals` for prepared batches.
It recreates derived `memory_fts` with alias and deterministic-term columns.

`memory.enrich_retrieval` uses the current consolidation binding and processes
at most eight request-local memory indexes. It produces one validated BCP-47
source language, optional equivalent rewrite, and at most eight aliases per
target language. `zh-Hans`, `en`, and a distinct source language are retained.
Eligible existing statuses are active, stale, and superseded. Current exact
note sources provide bounded samples through Vault Core; missing/invalid source
health permits aliases but forbids body rewrite.

FTS input normalizes Unicode, emits Latin/digit tokens of length at least two,
and overlapping Han bigrams. Query terms are bounded, escaped, and ORed. Vector
search filters `memory` or `note` in SQL before exact-cosine Top-K (`note` is
the existing deterministic note-chunk object type) and uses non-negative
cosine-weighted reciprocal-rank fusion.

Admin GET/POST retrieval endpoints expose coverage and explicitly admit
backfill. Recall returns retrieval coverage and the stable
`multilingual_alias_coverage_incomplete` degradation code. No endpoint returns
alias contents by default.

## Work breakdown

1. Add migration/repository records for retrieval metadata, prepared batches,
   FTS terms, coverage, and Vault-scoped lifecycle cleanup.
2. Add deterministic tokenization/query construction and typed vector search
   scope, then update note and memory callers.
3. Implement enrichment request/schema/validation, source sampling, prepared
   proposal persistence, revision-safe apply, and automatic admission.
4. Add Admin routes/UI, job orchestration/progress, MCP recall coverage, and
   source-language prompt versions.
5. Add migration, isolation, recovery, cross-language, provider-contract,
   Admin/MCP/frontend tests and update all governing documentation.

## Progress

- [x] 2026-09-03 — Reproduced the whitespace/CJK FTS and mixed-object vector
  failure paths from current code.
- [x] 2026-09-03 — Accepted ADR-0023 and created this execution plan.
- [x] 2026-09-03 — Added migration 0014, Vault-scoped metadata/proposals,
  deterministic FTS terms, and 0013-to-0014 preservation coverage.
- [x] 2026-09-03 — Added bounded Han/Latin OR retrieval, object-scoped vector
  Top-K, and cosine-weighted semantic fusion.
- [x] 2026-09-03 — Added source-language enrichment, alias validation,
  proposal-first recovery, automatic new/body-changed admission, and explicit
  existing-memory backfill.
- [x] 2026-09-03 — Added Admin coverage/backfill UI and API, worker progress,
  MCP natural-language guidance, and recall coverage/degradation output.
- [x] 2026-09-03 — Added missing/out-of-range/language/alias/secret/literal
  failure guards; alias-only unavailable-source behavior; canonical revision,
  partial-proposal recovery, no-second-Provider-call, and concurrent-edit tests.
- [x] 2026-09-03 — Updated normative architecture, interface, data, security,
  Provider, Admin, deployment, traceability, ADR index, and migration-check
  documentation; all full validation gates passed.

## Decisions

- Preserve one source-language canonical body and store source/zh-Hans/en
  aliases only as derived retrieval metadata.
- Reuse `memory_consolidation`; do not add a Provider role.
- Existing backfill is explicit, handles active/stale/superseded records, and
  performs only equivalence-preserving rewrites.
- New-memory enrichment is a separate batch job so alias failures cannot make
  Phase 2 fail.
- Do not bump `MEMORY_PIPELINE_GENERATION`; migration and backfill are
  compatible with current canonical state.

## Surprises and discoveries

- Default FTS5 tokenization stores an unspaced Chinese phrase as one token.
- The vector backend computes cosine similarity but recall currently retains
  only result rank.
- Existing note embeddings use object type `note` for deterministic chunks;
  the typed filter preserves that stored compatibility name rather than
  renaming rows during this additive migration.
- Pending/failed alias rows must retain deterministic canonical CJK terms;
  clearing aliases must not regress same-language lexical recall.
- A first sandboxed run of the local-fake integration test could not bind a
  loopback port (`Operation not permitted`). The same test and final workspace
  suite passed outside that sandbox boundary; no real Provider was used.
- A prepared batch can encounter a new memory revision between generation and
  apply. The worker now checkpoints the conflict, preserves the new body,
  re-admits that row, and backs off before preparing another paid request.

## Validation

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
CI=true PNPM_CONFIG_MANAGE_PACKAGE_MANAGER_VERSIONS=false pnpm --dir frontend/admin lint
CI=true PNPM_CONFIG_MANAGE_PACKAGE_MANAGER_VERSIONS=false pnpm --dir frontend/admin test
CI=true PNPM_CONFIG_MANAGE_PACKAGE_MANAGER_VERSIONS=false pnpm --dir frontend/admin build
bash scripts/check-docs.sh
bash scripts/release/check-migrations.sh
```

Provider contract tests use local fakes only. Acceptance includes Chinese to
English and English to Chinese offline recall, same-language Chinese
paraphrase, incomplete-coverage degradation, object-scoped vector Top-K,
prepared-proposal recovery, revision conflict, and multi-Vault isolation.

Completed evidence on 2026-09-03:

```text
cargo fmt --all --check                                      PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                              PASS
cargo test --workspace --all-features                         PASS
pnpm --dir frontend/admin lint                                PASS
pnpm --dir frontend/admin test                                PASS (29 tests)
pnpm --dir frontend/admin build                               PASS
bash scripts/check-docs.sh                                    PASS
bash scripts/release/check-migrations.sh                      PASS (through 0014)
git diff --check                                              PASS
```

## Rollback and recovery

Migration 0014 preserves canonical files and all prior operational rows; its
new tables and FTS contents are derived. A code rollback ignores the new
tables. Backfill body rewrites are ordinary Vault Core revisions and can be
restored from history. Interrupted jobs retain prepared output and resume
without a second Provider call.

## Outcomes

MCP Vault now preserves the source language in Phase 1/Phase 2, asynchronously
persists validated source/`zh-Hans`/`en` aliases, and recalls covered memory
offline across Chinese and English. Canonical rewrites are source-verified,
revisioned, literal-preserving, and isolated from alias failure. Missing
coverage is explicit in MCP/Admin output, and historical backfill remains a
confirmed paid Admin action.

Migration 0014 is additive to canonical knowledge, rebuilds only memory FTS,
and preserves existing memories, file history, and jobs in its upgrade test.
Prepared enrichment survives partial application without a second Provider
call; startup/periodic recovery admits only previously pending rows. Vector
Top-K is object-scoped and cosine-weighted. The stored compatibility name for
deterministic note chunks remains `note` rather than introducing a data rewrite
to `note_chunk`.

The pre-existing optional `rollout_slug` fix remains in the worktree and was
not reverted or split out. No real paid Provider, deployment, commit, or push
was performed.
