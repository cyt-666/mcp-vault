# Repair bounded embedding rebuilds and model-change backfill

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-09-04
- Updated: 2026-09-04

## Purpose and user-visible result

Zhipu `embedding-3` must successfully build semantic vectors for long Chinese
notes without sending an over-limit input. Completed index jobs must not mask
failed vector work, failed embedding jobs must expose a stable redacted cause,
and selecting a new note or memory embedding model must schedule the missing
current-model vectors. An Admin must also be able to re-admit existing memory
vectors without re-running paid memory extraction.

## Governing requirements

- `docs/product-requirements.md` sections 3.3, 3.5, 3.6, 3.8, and 3.10.
- `docs/architecture.md` sections 9, 10, 11, 12, and 16.
- `docs/interfaces.md` Admin index, Provider, job, and memory contracts.
- `docs/admin-and-configuration.md` sections 11 through 13.
- `docs/data-model.md` Provider binding, embedding, vector, and job sections.
- ADR-0010, ADR-0013, ADR-0020, ADR-0023, and ADR-0024.

## Current repository state

- Note embeddings use deterministic `text-v1` chunks containing up to 6,000
  Unicode scalar values plus as many as 2,048 context characters.
- Zhipu `embedding-3` accepts at most 3,072 tokens per input. A live short query
  succeeds, while the first 6,000 characters of the current 29,865-character
  `MCPVault技术架构与实现.md` note reproduce
  `semantic_provider_not_ready`.
- `index.rebuild` schedules separate `embedding.rebuild` jobs and completes
  without waiting for them. The Admin warning reports incomplete coverage but
  not terminal embedding failures.
- Binding `embedding_note` schedules missing vectors. Binding
  `embedding_memory` does not backfill existing memory.
- Embedding job payloads retain their original model ID. Retrying an old MiMo
  job therefore cannot adopt a newly bound Zhipu model.
- The worktree contains the uncommitted source-language/multilingual-recall and
  optional-slug work. This plan must preserve and extend those changes.

## Scope and non-scope

Included: byte-bounded versioned note chunks, automatic current-model note and
memory scheduling, an explicit Admin memory-vector rebuild action, stable
embedding error codes and redacted job details, UI coverage/actions, tests, and
documentation.

Excluded: a new vector database, paid-provider CI, query-time translation,
rewriting canonical notes/memories, and changing generation model roles.

## Invariants and risks

- Canonical Vault Markdown and memory bodies are never modified by vector
  rebuilds.
- Jobs, source resolution, vector rows, coverage, and Admin actions remain
  Vault-scoped.
- Job payloads contain only source identities, hashes, model identity, and
  deterministic chunk keys, never note or memory bodies.
- A new embedding projection version must not reuse a terminal incompatible
  job's deduplication key.
- Provider response bodies and source contents remain absent from errors,
  logs, Admin responses, and progress.
- Chunking stays deterministic at scheduling, execution, status, and retrieval
  validation boundaries.

## Proposed design

Replace the 6,000-character `text-v1` profile with `text-v2` chunks whose full
provider input, including bounded note context, is at most 2,048 UTF-8 bytes.
Use byte overlap snapped to UTF-8 boundaries. This conservative input envelope
fits the observed Zhipu limit without needing a vendor tokenizer and remains
provider-independent.

Version embedding job deduplication so fixed source references produce fresh
jobs after this incompatible derived-projection repair. Preserve old jobs as
history. Map Provider failures to their existing redacted stable codes and add
safe job details containing only internal model ID, source type, and count.

Add a Memory application operation mirroring note scheduling: enumerate
active/stale/superseded current memories, prune stale current-model vector
metadata, and enqueue bounded current-model batches. Invoke it when
`embedding_memory` changes and expose an authenticated Admin action for an
explicit repeat. The Memory page reports coverage and lets the operator
schedule missing vectors without memory extraction.

## Work breakdown

1. Add the versioned byte-bounded note chunk profile and deterministic tests,
   including multi-byte input, context bounds, tail coverage, and changed job
   deduplication.
2. Preserve stable Provider failure codes in embedding workers and expose only
   redacted embedding job metadata in Admin job responses.
3. Add current-model memory-vector coverage/scheduling, binding-change
   admission, an Admin GET/POST contract, and UI action.
4. Add worker, Admin, memory, indexer, multi-Vault, and frontend regression
   tests; update Provider/index/memory/operations documentation.
5. Run formatting, workspace Clippy/tests, frontend lint/test/build, docs,
   migration checks, and diff validation.

## Progress

- [x] 2026-09-04 — Reproduced the live short-input success and 6,000-character
  Zhipu failure; identified a 29,865-character current note and current-model
  failed `embedding.rebuild` job.
- [x] 2026-09-04 — Created ADR-0024 and this ExecPlan without reverting the
  existing uncommitted multilingual/slug work.
- [x] 2026-09-04 — Implemented the `text-v2` note profile with a 2,048-byte
  complete-input bound, UTF-8-safe overlap, projection-versioned jobs, stable
  Provider error codes, and redacted job details.
- [x] 2026-09-04 — Implemented current-memory vector coverage/scheduling,
  binding-change admission, authenticated note/memory Admin actions, and Admin
  coverage/action panels.
- [x] 2026-09-04 — Added long-CJK, source-bound, existing-memory,
  model-selection, redaction, error-code, CSRF, and multi-Vault regressions;
  updated the Provider, interface, memory, architecture, and operations docs.
- [x] 2026-09-04 — Passed every validation gate and moved this plan to
  `completed/`.

## Decisions

- Bound the complete UTF-8 input rather than assuming one Unicode character is
  one Provider token.
- Keep chunking in the Index application boundary; the Provider adapter does
  not split one logical input into several billable calls or average vectors.
- Use the existing `embedding.rebuild` job and VectorIndex rather than adding a
  second vector pipeline.
- Rebuild memory vectors directly from current memory projections; never use
  Phase 1/Phase 2 as a vector backfill mechanism.

## Surprises and discoveries

- `index.rebuild` completion and `embedding.rebuild` completion are independent;
  the current UI makes the parent completion easy to misread as semantic
  readiness.
- A terminal job deduplication key prevents a later index rebuild from creating
  an equivalent fixed job, so the derived projection needs an explicit version
  in its job identity.
- A short semantic query against the newly bound `embedding-3` succeeds and
  returns `semantic_index_empty`, proving endpoint/auth/dimension readiness;
  the exact 6,000-character source chunk fails before any vector is stored.

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

Provider integration tests use local fakes only. Acceptance requires all
generated note inputs to respect the byte bound, model changes to schedule the
correct model ID, old failed jobs to remain history, existing memory to receive
new-model vectors without extraction, and two Vaults to remain isolated.

Completed on 2026-09-04:

- `cargo fmt --all --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  passed.
- `cargo test --workspace --all-features` — passed, including the new long-CJK
  and existing-memory multi-Vault integration coverage.
- Admin frontend lint, 30 tests, and production build — passed.
- `bash scripts/check-docs.sh` — passed.
- `bash scripts/release/check-migrations.sh` — passed through migration 0014.
- `git diff --check` — passed.

## Rollback and recovery

All affected vectors and note chunks are derived. Rolling back code leaves
canonical files untouched; obsolete vector rows may be deleted and rebuilt.
Interrupted embedding jobs retain reference-only payloads and can be retried.
Old projection-version jobs remain inspectable but are not reused by the new
scheduler.

## Outcomes

Long note inputs are now deterministically split into `text-v2` chunks whose
context plus body never exceeds 2,048 UTF-8 bytes. Embedding jobs carry
projection version 2 in their payload and deduplication key, so deploying this
repair admits new work while preserving old terminal failures for diagnosis.

Admins can independently schedule missing note vectors from the Index page and
existing memory vectors from the Memory page. Memory backfill reads only the
current Vault-scoped memory projection and does not re-run extraction,
consolidation, or canonical writes. Changing either embedding binding also
admits missing current-model work.

The Admin task view exposes safe model/source/projection metadata and preserves
the Provider's stable redacted error code. The Index UI now distinguishes a
completed parent rebuild from still-running or failed vector children. No
canonical note or memory body, migration, Provider role, or query-time LLM
behavior was changed by this repair.
