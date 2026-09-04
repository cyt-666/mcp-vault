# Aggregate note-vector chunks before note ranking

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-09-04
- Updated: 2026-09-04

## Purpose and user-visible result

Semantic and hybrid note retrieval must rank unique notes rather than raw
embedding chunks. One long note may match through several chunks, but it must
appear once, contribute once using its highest current non-negative cosine,
and never push other relevant notes out merely because it is longer.

## Governing requirements

- `docs/product-requirements.md` sections 3.3, 3.5, 3.10, and 4.4.
- `docs/architecture.md` note retrieval and object-scoped vector ranking.
- `docs/interfaces.md` section 6.4 and score-breakdown compatibility.
- `docs/data-model.md` embedding/vector partition and current-chunk rules.
- ADR-0010, ADR-0013, ADR-0023, ADR-0024, and ADR-0025.

## Current repository state

- Notes use deterministic `text-v2` chunks, each bounded to 2,048 UTF-8 bytes,
  with at most 128 chunks per note.
- SQLite exact cosine filters by Vault, model, dimension, and object type,
  sorts chunk rows, and truncates them to the requested limit.
- `IndexService::add_semantic_note_hits` requests the final note pool as the
  vector-row limit, validates each chunk, and deduplicates only afterward.
- Duplicate chunks advance semantic rank before the duplicate check. The raw
  cosine is reported in `semantic_cosine`, but `semantic_rrf` and the total use
  only reciprocal rank; negative hits therefore receive positive contribution.
- The worktree already contains the requested multilingual retrieval,
  bounded-embedding, and `0.1.20` changes. They must remain intact.

## Scope and non-scope

Included: bounded candidate over-fetch, current-chunk validation, best-chunk
aggregation by File ID, cosine-weighted unique-note rank, deterministic ties,
snippet precedence, tests, and contract documentation.

Excluded: vector regeneration, schema migration, a reranker, a native ANN
backend, changing memory-vector scoring, or increasing the 10,000-row exact
fallback cap.

## Invariants and risks

- Every candidate remains Vault/model/dimension/object-type scoped before
  similarity ranking.
- Stale chunk hashes never become note results.
- Query-time work remains bounded and does not scan the Vault filesystem.
- No note body, vector, Provider response, or secret enters logs or diagnostics.
- MCP request/response fields and score-breakdown keys remain compatible.
- Hybrid lexical retrieval and its snippet remain available if semantic work
  is missing, negative, stale, or unavailable.

## Proposed design

Request a raw vector candidate budget equal to the desired unique-note pool
times the maximum chunks per note, capped at 10,000. Keep the Provider/vector
interface chunk-oriented because current-source validation belongs to the
Indexer. Make equal-cosine ordering deterministic by object ID, chunk key, and
embedding ID.

Process the cosine-sorted candidates until the unique note pool is full. Stop
at the first negative score. Parse and validate each note/chunk reference
against the current derived note projection, cache reconstructed chunks per
File ID, and accept only the first valid, scope-matching hit for each note.
Duplicates and invalid hits do not advance unique semantic rank.

Add `cosine * reciprocal_rank(1.0, unique_rank)` to the note's fused score.
Keep `semantic_cosine` as the raw value and `semantic_rrf` as the actual added
component. A lexical candidate retains its existing snippet; semantic-only
candidates receive the winning chunk snippet.

## Work breakdown

1. Add ADR-0025 and this ExecPlan; record the current ranking discrepancy.
2. Update exact-cosine tie ordering and Index semantic candidate aggregation.
3. Add provider/indexer/MCP regressions for unique notes, max-only scoring,
   negative and stale hits, snippets, pagination, and isolation.
4. Update architecture, interfaces, data-model, testing, and traceability docs.
5. Run every required Rust/frontend/docs/migration/diff gate and archive this
   plan only when all pass.

## Progress

- [x] 2026-09-04 — Traced chunk ranking through SQLite vector search,
  `EmbeddingService`, `IndexService`, MCP `search_notes`, and
  `recall.related_notes`.
- [x] 2026-09-04 — Confirmed max-only per-note scoring and lexical-snippet
  precedence with the user; created ADR-0025 and this plan.
- [x] 2026-09-04 — Implemented bounded raw-candidate over-fetch, current chunk
  validation, max-only File-ID aggregation, negative-hit rejection,
  cosine-weighted unique-note rank, and deterministic vector ties.
- [x] 2026-09-04 — Added long-note crowding, stale-winner fallthrough,
  cosine-score, negative-hit, pagination, lexical/semantic snippet, Provider
  tie, and `recall.related_notes` regressions; updated governing docs.
- [x] 2026-09-04 — Passed every validation gate and moved this plan to
  `completed/`.

## Decisions

- Aggregate after current-chunk validation in the Indexer, not blindly in the
  generic vector backend.
- Use the maximum current chunk cosine only; do not sum, average, or reward a
  second matching chunk.
- Preserve lexical snippets in hybrid mode and existing score-breakdown keys.
- Reuse current vectors and version `0.1.20`; this ranking-only change does not
  bump the embedding projection version.

## Surprises and discoveries

- The architecture already requires non-negative cosine to scale reciprocal
  rank, but the note path currently implements this only for memory recall.
- The SQLite fallback already evaluates up to 10,000 vector rows per query, so
  bounded over-fetch does not add Provider calls or canonical file reads.
- The first related-note regression used a query term also present in every
  fixture path. The negative semantic note correctly remained eligible through
  the lexical half of hybrid retrieval; the test now uses a concept query with
  no lexical path/body match to isolate semantic behavior.
- Running three pnpm gates concurrently caused their dependency verification
  installs to replace the same ignored `node_modules` tree. Restoring once and
  running lint, test, and build sequentially passed; source and lockfiles were
  unchanged by that recovery.

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
git diff --check
```

All Provider behavior uses local fakes. Acceptance requires three distinct
notes to survive a pool dominated by multiple chunks from one long note,
negative hits to contribute nothing, stale winners to fall through to a
current chunk, and related-note recall to inherit the same result.

Completed on 2026-09-04:

- `cargo fmt --all --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  passed.
- `cargo test --workspace --all-features` — passed.
- Admin frontend lint, 30 tests, and production build — passed sequentially.
- `bash scripts/check-docs.sh` — passed.
- `bash scripts/release/check-migrations.sh` — passed through migration 0014.
- `git diff --check` — passed.

## Rollback and recovery

Rollback changes only query-time ranking code and documentation. Existing
vectors, jobs, note projections, canonical Markdown, and memory remain valid.
No recovery or reverse migration is required.

## Outcomes

Note semantic retrieval now over-fetches a bounded chunk pool, rejects negative
similarities, validates each hit against the current chunk key/hash, and lets
only the highest current chunk per File ID consume rank and score. The semantic
contribution is the raw cosine multiplied by unique-note reciprocal rank.

Long notes therefore appear once and cannot displace other notes simply by
having more chunks. A stale high-scoring chunk falls through to the same note's
next current chunk. Pure semantic results use the winning semantic snippet;
hybrid results keep the lexical snippet selected by exact text retrieval.

`search_notes` and `recall.related_notes` inherit the correction without a wire
change. Existing `text-v2` vectors remain valid, no migration or rebuild is
required, query-time Provider call count is unchanged, and version `0.1.20`
remains in place.
