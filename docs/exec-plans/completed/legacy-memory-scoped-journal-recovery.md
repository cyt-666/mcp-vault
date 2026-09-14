# Explicitly discard legacy-memory Core journals during initialization

- Status: Complete
- Owner: Codex Luna
- Created: 2026-09-14
- Updated: 2026-09-14

## Purpose and user-visible result

After an Admin confirms the one-time deletion of predecessor memory, the
initializer must not replay old Core writes or attempt replayed-create witness
repair for `_mcp-vault/memory/`. It first persists the content-free file
manifest, then terminalizes only incomplete Core intents proven to stay wholly
inside that legacy namespace, and finally retires manifest files through Vault
Core using the original hash guards. Ordinary Vault content, new `memory-v3`
files and cross-boundary/unclassifiable journals remain untouched.

## Governing requirements

- [Product requirements](../../product-requirements.md), sections 3.4 and 3.5:
  writes remain atomic/revision-aware/recoverable; explicit cutover discards
  predecessor memory while preserving ordinary files.
- [Architecture](../../architecture.md), section 7: Vault Core owns journaled
  canonical file changes and recovery.
- [Memory system](../../memory-system.md), Cutover: predecessor `memory/`
  files are retired through Vault Core with exact hash guards; new v3 files
  remain canonical.
- [Data model](../../data-model.md), section 9: journal lifecycle is
  Vault-scoped and terminal states remain distinguishable.
- [ADR-0033](../../adr/0033-source-preserving-memory-units.md): initialization
  requires explicit confirmation, preserves ordinary Vault data and resumes
  from a durable manifest.
- [ADR-0034](../../adr/0034-replayed-create-witness-recovery.md): ordinary
  replayed-create recovery requires complete physical and metadata witnesses.

## Current repository state

`MemoryInitializationService` previously invoked scoped Core recovery before
building its manifest. That correctly excluded ordinary notes, but a pure
legacy `create` journal could fail with `AlreadyExists` before the explicit
discard reached the old file. `UnitRepository::finish_initialization` already
clears the allow-listed Vault-scoped predecessor tables, jobs, vectors,
outbox and settings. File cleanup already uses an exact-hash manifest and
`VaultCore::retire_managed_file`.

`operation_journal` currently has `prepared`, `file_committed`,
`metadata_committed`, `rolled_back`, `needs_review` and `superseded` states.
The new migration adds a durable `discarded` terminal state. Core's normal
startup recovery and the replayed-create witness API remain unchanged.

## Scope and non-scope

In scope:

1. Add a forward-only migration and State CAS operation that terminalizes
   selected old intents as `discarded` within one Vault transaction.
2. Add a Vault Core maintenance method that preflights all relevant journal
   paths, validates namespace purity and manifest coverage, removes only
   journal-owned temporary files, then commits the State CAS.
3. Persist/build the legacy file manifest before discarding journals, then
   retire files through existing Vault Core hash/history/outbox paths.
4. Add tests for the reported legacy replayed-create failure and a mixed-scope
   journal that must fail closed.
5. Update the Admin guide, ADRs, data model and execution plan.

Out of scope: ordinary Core recovery, ordinary/V3 file mutation, generic
operator journal resolution, production database access, deployment and
release publication.

## Invariants and risks

- Every State update remains scoped by `VaultContext` and uses expected journal
  state/update time as a compare-and-swap guard.
- A journal is discardable only when its typed payload, all recorded source
  and destination paths, before/after paths and optional tombstone path prove
  the entire operation stayed under the exact managed legacy-memory prefix.
- Unknown or cross-boundary journals fail before any journal terminalization or
  manifest file retirement. Core currently rejects user moves into/out of its
  reserved root; the fail-closed branch still protects historical/corrupt rows.
- Journal temporary files are validated and removed idempotently before the
  State batch CAS. A crash before CAS leaves active rows that can be retried;
  a crash after CAS resumes from the persisted manifest.
- Canonical file deletion remains solely in `retire_managed_file`; every
  present file must match its persisted manifest hash. A changed file fails
  without deletion.
- `discarded` preserves the old intent and reason as a terminal audit trail;
  it is no longer eligible for Core replay or review queues.

## Proposed design

On a `required` run, inventory only the predecessor path and persist the
manifest before touching journals. On a `clearing` run, reuse that manifest.
Vault Core then selects `prepared`, `file_committed` and `needs_review` rows
that touch `_mcp-vault/memory/`; it decodes each Core payload and rejects any
unclassifiable or mixed-boundary journal. For file intents, every currently
present or active path must already be represented by the manifest. It removes
validated journal-owned temporary names, then State atomically marks the
selected rows `discarded`. Initialization never calls `recover` or
replayed-create repair for that namespace. It continues with the existing
hash-guarded Core retirement loop, checkpointing each file and finally
clearing the predecessor tables and derived data.

## Work breakdown

1. Add migration `0033_discarded_legacy_memory_journals.sql` and State journal
   state/CAS repository support.
2. Add Core preflight/discard service with Vault, scope, temp, and manifest
   checks; leave global recovery APIs unchanged.
3. Reorder Memory initialization to persist/reuse the manifest before direct
   journal discard; retain Core retirement and State finish cleanup.
4. Add pure legacy create success and mixed-scope fail-closed regressions.
5. Update operator, ADR, data model and plan text; run formatting, tests and
   Clippy for changed crates.

## Progress

- [x] Re-read governing requirements, current initialization and journal
  schemas; confirm predecessor State cleanup already exists.
- [x] Agree on direct discard plus fail-closed mixed-boundary handling.
- [x] Add `discarded` State model, CAS method and migration.
- [x] Add Core scoped preflight and direct discard path.
- [x] Reorder initialization around the durable hash manifest.
- [x] Add pure legacy create and cross-boundary regression fixtures.
- [x] Finish migration/repository and Core/Memory test suites, formatting and
  Clippy; update documentation.

## Decisions

- Replayed-create witness repair remains valid for normal Core recovery, but
  explicit destructive legacy-memory initialization does not invoke it.
- `discarded` is a separate terminal state rather than reusing `rolled_back`
  or `superseded`, preserving truthful recovery history.
- Temporary names are removed first and State rows are changed in one CAS
  transaction. Canonical files are still retired afterward through Core.
- Mixed-scope and unclassifiable journals stop initialization; no endpoint or
  history outside the legacy namespace may be modified implicitly.

## Validation

- `cargo fmt --all --check`
- `cargo test -p mcp-vault-memory --test admin_initialization_service`
- `cargo test -p mcp-vault-state --all-features`
- `cargo test -p mcp-vault-core -p mcp-vault-memory --all-features`
- `cargo clippy -p mcp-vault-state -p mcp-vault-core -p mcp-vault-memory --all-targets --all-features -- -D warnings`
- `bash scripts/release/check-migrations.sh`
- `git diff --check`

## Rollback and recovery

The migration is forward-only and adds a journal terminal label; it does not
rewrite canonical files. Reverting code after migration leaves `discarded`
rows as terminal metadata, so prior binaries must not be used unless they
support this state. An interrupted initialization uses the persisted manifest;
files still require their original hash to match before retirement. Unrelated
and mixed-boundary journals remain unchanged for normal maintenance review.

## Outcomes

Validation passed:

- `cargo fmt --all --check`
- `cargo test -p mcp-vault-memory --test admin_initialization_service` (9
  passed)
- `cargo test -p mcp-vault-state --all-features` (60 tests passed across the
  library and integration suites)
- `cargo test -p mcp-vault-core -p mcp-vault-memory --all-features` (Core 24;
  Memory 43 across library and integration suites)
- `cargo clippy -p mcp-vault-state -p mcp-vault-core -p mcp-vault-memory --all-targets --all-features -- -D warnings`
- `bash scripts/release/check-migrations.sh` (migration library tests and the
  offline v3 cutover test passed)
- `git diff --check`

No deployment, image build, commit, or server access was performed.
