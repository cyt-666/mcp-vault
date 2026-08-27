# Harden Phase 2 model output and local consolidation ownership

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-27
- Updated: 2026-08-27

## Purpose and user-visible result

Phase 2 consolidation must accept semantically valid Xiaomi MiMo JSON without
letting the model own identifiers, evidence coordinates, or mechanically
derivable raw-input state. Invalid model decisions must fail with a precise,
redacted `memory_phase2_*` job error instead of the undifferentiated
`memory_consolidation_failed` code. Existing canonical memory and prepared-
proposal crash recovery remain revision-safe and Vault-scoped.

## Governing requirements

- `docs/product-requirements.md` section 3.5: sourced two-phase memory,
  autonomous consolidation, exact evidence, and LLM-free recall.
- `docs/architecture.md` section 11: local code owns references, revisions,
  canonical writes, and projection commits.
- `docs/memory-system.md` sections 4, 6, 10, and 11: Phase 2 validation,
  provenance, durable diagnostics, and isolation.
- ADR-0016: Codex-style two-phase memory and application-owned identifiers.
- ADR-0014: source evidence rather than model assertions is the trust boundary.
- `docs/provider-compatibility.md` section 4.2: MiMo JSON Object mode guarantees
  syntax, not the requested field hierarchy or cross-field semantics.

## Current repository state

The original `MemoryService::consolidate` deserialized Provider JSON directly
into typed `MemoryId`/`MemoryRawId` fields. The first hardening pass removed
model-owned create IDs, dispositions, and evidence indexes, but still required
MiMo to copy long Stage 1/current-memory UUIDs. Local preparation correctly
rejected invented references, while the contract remained unreliable at larger
input counts.

Live evidence from the 2026-08-27 deployment showed two applied MiMo proposals,
followed by three jobs that failed after 6, 4, and 0 committed inputs. An applied
proposal contained model-invented placeholder UUIDs, demonstrating that create
identifiers were not application-owned. Failed Provider output is intentionally
not retained, so remediation must add trusted structural diagnostics without
persisting generated text.

Failed jobs already support explicit Admin retry through
`JobRepository::request_retry`; automatic reconciliation deliberately does not
create an unbounded sequence of paid retries for one terminal generation.

## Scope

### Included

- A smaller Phase 2 wire DTO for semantic operations, request-local input and
  memory indexes, explicitly discarded ready inputs, and the compact summary.
- Application-generated IDs for every create action.
- Application-derived evidence references, `used` dispositions, and automatic
  `no_output`/`withdrawn` dispositions.
- Local mapping of bounded indexes to existing memory/raw IDs and precise
  redacted `memory_phase2_*` errors.
- An isolated one-shot local replay command that copies operational state and
  never starts background workers.
- Prepared proposal compatibility within the current prerelease generation,
  crash recovery, tests, Admin labels, and normative documentation.

### Not included

- Relaxing Vault isolation, source/evidence validation, optimistic revisions,
  or atomic Phase 2 application.
- Persisting Provider response bodies or memory contents in logs/diagnostics.
- Automatically retrying a non-retryable paid model-output failure forever.
- Changing the SQLite schema or canonical Markdown layout.

## Invariants and risks

- Every row, ID lookup, job, proposal, and write remains Vault-scoped.
- The model never chooses a create identifier or canonical evidence range.
- Every dirty ready input is either referenced by a write action or explicitly
  discarded; no-output and withdrawn state is derived locally.
- Prepared proposal snapshots and expected revisions remain stable across
  restart and partial file/projection application.
- Wire-contract changes require a prompt-version bump. Existing applied
  proposals remain historical; any current prepared proposal must remain
  readable until applied or explicitly rejected.
- Tolerant normalization may ignore irrelevant fields only where the operation
  itself is unambiguous; it must not infer a different lifecycle decision.

## Proposed design

### Components and dependency direction

`crates/memory` owns an untrusted Phase 2 wire DTO and converts it into the
existing typed prepared proposal. `crates/server` maps generated-output errors
to durable job codes. `crates/state` retains the existing explicit retry
mechanism. Admin only formats the new stable codes.

### Data and transaction flow

1. The Provider returns `memory_summary`, semantic actions, and
   `discarded_input_indexes`.
2. Local preparation maps `memory_index` and `input_indexes` back to the exact
   bounded request snapshot. Create actions receive new UUIDv7 values locally.
3. Each create/update input index inherits all already validated evidence
   anchors from the mapped Stage 1 row.
4. Local code derives `used` from action references, auto-discards `no_output`,
   auto-withdraws `withdrawn`, and requires every dirty ready input to be used or
   explicitly discarded.
5. The resulting typed proposal is validated against current snapshots,
   persisted, and applied through the existing Vault Core path.

### Public interfaces and schema changes

No MCP/Admin endpoint changes. The Provider-only schema changes from UUID
references to `input_indexes`, `memory_index`,
`supersedes_memory_indexes`, and `discarded_input_indexes`; prompt version v3
invalidates failed v2 input hashes. Job `last_error` gains precise
`memory_phase2_*` values rendered by the Admin UI.

### Failure, retry, and recovery

Invalid Provider structure/reference decisions are terminal generated-output
failures with safe codes. They do not persist response text or advance Stage 1
selection. Operators may explicitly retry the same failed job after deploying a
contract/model fix; prepared proposals continue to be reused without a second
Provider call.

## Work breakdown

1. Add the minimal wire DTO/schema/prompt and convert it to typed prepared state
   in `crates/memory/src/service.rs`.
2. Replace model-owned IDs/evidence/dispositions with deterministic local
   derivation while retaining snapshot/recovery validation.
3. Expose stable generated-output codes in `crates/server/src/workers.rs` and
   Chinese labels in `frontend/admin/src/view-model.ts`.
4. Update Provider fakes and add regression tests for UUID-free request state,
   81-input mapping, automatic evidence/dispositions, precise failures, and
   explicit retry.
5. Amend ADR-0016, `docs/memory-system.md`, architecture/provider compatibility,
   then run the required Rust/frontend checks.

## Progress

- [x] `2026-08-27` — Reproduced and bounded the live failure to Provider-output
  parsing/local preparation before proposal persistence.
- [x] `2026-08-27` — Confirmed model-invented create IDs and existing explicit
  Admin retry behavior.
- [x] `2026-08-27` — Implemented the minimal model contract and local
  identifier/evidence/disposition derivation.
- [x] `2026-08-27` — Added precise worker/Admin diagnostics, regression tests,
  normative documentation, and full validation.
- [x] `2026-08-27` — Replaced remaining model-copied UUID references with
  request-local integers after live `memory_phase2_stage1_unknown`.
- [x] `2026-08-27` — Added an isolated SQLite/Vault/history replay, corrected
  copied inode identity handling, and passed a fresh real MiMo v3 call without
  touching source state.
- [x] `2026-08-27` — Ran the complete Rust/frontend/documentation validation and
  closed the plan.

## Decisions

- Keep strict provenance/revision checks; reduce model-owned bookkeeping rather
  than weaken integrity constraints.
- Derive all evidence indexes from Stage 1 because those anchors are already
  bounded and server-validated.
- Preserve explicit retry instead of adding automatic paid retries after a
  terminal generated-output failure.

## Surprises and discoveries

- Applied MiMo output used syntactically valid placeholder UUIDs, so UUID
  parsing alone did not restore application ownership.
- The job repository already requeues failed jobs explicitly and preserves the
  intended cost-safety boundary; the apparent dedup dead end is recoverable from
  the Admin task page.
- Initial frontend validation attempted to reconcile `node_modules` and stopped
  because a non-interactive purge was not authorized and the sandbox could not
  reach the pnpm mirror. `CI=true` reused the available local store and all
  frontend checks then passed without downloading dependencies.
- A live retry with the v2 prompt processed 0 of 81 inputs and failed with
  `memory_phase2_stage1_unknown`. Removing model-owned create IDs was necessary
  but insufficient: long Stage 1 UUID copying is itself an unreliable model
  contract and needs locally mapped short/index references.
- A byte-identical Vault copy initially failed with `ExternalMismatch` because
  filesystem inode identity is intentionally part of Vault Core concurrency
  detection. The replay now clears only inode identities in the copied DB while
  retaining byte size and SHA-256 verification.
- A fresh v3 MiMo call against three real pending local inputs completed with
  three actions and three dispositions; 81-input mapping and the 257-input
  multi-generation worker pass deterministic local tests.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
env CI=true pnpm --dir frontend/admin lint
env CI=true pnpm --dir frontend/admin test
env CI=true pnpm --dir frontend/admin build
```

Expected results: all commands pass; new tests prove durable IDs are absent from
the Provider contract, 81 inputs map losslessly through indexes,
evidence/dispositions are local, precise failures reach job state, and
prepared-proposal recovery remains idempotent.

Observed results on 2026-08-27: every listed command passed. Memory tests ran 7
unit and 14 integration cases; the Server suite ran 39 cases including the
257-input multi-batch worker; Admin frontend tests ran 24 cases. A fresh isolated
real MiMo v3 replay prepared and applied 3 actions/3 dispositions without reusing
a proposal. The first sandboxed workspace test attempt hit a loopback-bind
permission error in an existing Indexer test; the isolated test and the complete
workspace suite both passed outside that restriction.

## Rollback and recovery

No migration is added. Reverting the code restores the previous Provider wire
contract. Existing canonical files and applied proposals remain readable.
Before rollback, allow or explicitly retry any prepared current-version
proposal so no in-flight typed proposal is abandoned.

## Outcomes

Phase 2 now uses prompt version `memory-consolidation-v3`. The model returns
only semantic create/update/archive actions, bounded request-local indexes,
explicit ready-input discards, and the compact summary. Local code maps indexes
to the captured snapshot, allocates UUIDv7 identifiers, expands all validated
Stage 1 evidence, and derives raw dispositions. Operation/reference failures
surface as stable `memory_phase2_*` job codes with Chinese Admin labels.

The persisted prepared-proposal shape did not change, so existing crash recovery
and applied proposal history remain compatible. No migration or pipeline reset
was added. Isolated real-Provider replay and all required repository checks pass.
