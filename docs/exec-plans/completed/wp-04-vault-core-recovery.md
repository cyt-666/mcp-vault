# WP-04 Vault Core, Revisions, History, and Crash Recovery

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Create the application boundary that every later WebDAV, MCP, Admin,
reconciliation, and memory-materialization mutation will call. `VaultCore`
will bind operations to a `VaultContext`, validate canonical paths and
preconditions, serialize conflicting operations with deterministic per-path
locks, use `storage-fs` for safe physical I/O, and use state repositories for
durable file identities, revisions, history metadata, audit facts, outbox
events, and operation journal state.

The completed slice will support create, replace, exact append, exact unified
diff patch, move, copy, delete, read/stat, history inspection, and restore.
Writes will be idempotent when a client supplies an idempotency key. A
failure-injection boundary will exercise each documented commit phase, and a
startup/reconciliation service will finalize or roll back journal entries
without exposing absolute paths or applying an uncertain mutation silently.

## Governing requirements

- `AGENTS.md`: Vault isolation, protocol/application boundaries, safe atomic
  writes, expected revisions, revision history, durable outbox, restart
  recovery, and no silent concurrent overwrite.
- `docs/implementation-plan.md` section 7 (WP-04): query/mutation services,
  all canonical mutation types, expected revisions, stable File ID, locks,
  journal, history, audit/outbox, recovery, and idempotency.
- `docs/product-requirements.md` sections 3.4, 3.9, 4.1, and 6:
  controlled mutation/history, atomic/auditable/recoverable writes, outbox
  processing, and crash-recovery acceptance.
- `docs/interfaces.md` sections 3.3, 6.10-6.15, and 8: ETag/revision shape,
  create/edit/move/delete/history/restore semantics, exact patching, and
  stable application error categories.
- `docs/architecture.md` sections 4.3-4.5, 7, 8, 9, 15, and 16: Vault Core
  ownership, the filesystem/SQLite write-intent protocol, history blobs,
  transactional outbox, dependency direction, and redacted observability.
- `docs/data-model.md` sections 8-9 and 16-19: file identities, revision
  operations, journal states, outbox payloads, audit facts, and forward-only
  schema evolution.
- `docs/security.md` sections 9-10, 16, and 20: path/symlink policy,
  expected-revision protection, audit redaction, lock behavior, and crash
  recovery security tests.
- `docs/deployment-and-operations.md` sections 6-8 and 10: startup recovery
  before readiness, maintenance on uncertain journal state, shutdown flush,
  and out-of-band mismatch diagnosis.
- `docs/development-and-testing.md` sections 4, 5, 8, 13, and 14: dependency
  direction, non-blocking I/O, application integration tests, phase fault
  injection, and security coverage.
- Accepted ADR-0001, ADR-0002, and ADR-0005: Markdown remains canonical,
  Vault is the isolation boundary, and the service remains a modular monolith.

## Current repository state

WP-00 through WP-03 are present in the working tree. `mcp-vault-domain`
provides typed IDs, `VaultContext`, safe `VaultPath`, actor/source-plane
values, revisions, and preconditions. `mcp-vault-storage-fs` provides
Vault-bound no-follow storage, streaming atomic writes, hashes, metadata,
copy/move/delete, and a content-addressed `HistoryStore`.

`mcp-vault-state` currently has only Vault/settings repositories plus the WP-02
operational schema. The existing `file_entries`, `file_revisions`,
`operation_journal`, `outbox_events`, and `audit_log` tables are present, but
there are no typed file/revision/journal repositories or transaction-aware
metadata commit APIs. `mcp-vault-core` is still a documentation shell and no
protocol crate calls it yet. Existing changes are uncommitted and must remain
preserved.

## Scope

### Included

- Add forward migration `0002_operation_idempotency.sql` for a durable
  operation-journal idempotency key and Vault-scoped uniqueness.
- Add state-owned typed records/enums and repository methods for active/tomb-
  stoned file entries, revisions, journal lifecycle, audit rows, outbox rows,
  idempotency lookup, and atomic metadata commits.
- Add a typed commit hook seam in state so Core tests can inject failures after
  metadata transaction start and after outbox insertion without exposing SQL.
- Add `VaultCore` application services for read/stat, create, replace, append,
  exact unified diff patch, heading edits, move, copy, delete, history, and
  restore. All methods require a `VaultContext` and typed `VaultPath`.
- Add deterministic in-process lock management keyed by Vault and normalized
  path, locking move/copy source/destination paths in canonical order.
- Add journal phase transitions around storage-fs phased writes, redacted
  audit metadata, transactional outbox events, and idempotency replay.
- Add startup/reconciliation recovery for prepared/file-committed journal
  rows, including safe temp cleanup/finalization decisions and explicit
  `needs_review` for ambiguous state.
- Add unit/integration tests for every mutation, conflicts, exact patching,
  restore-as-new-revision, isolation, lock ordering, idempotency, outbox/audit
  atomicity, phase failures, restart recovery, and out-of-band mismatch.
- Update schema documentation and the root migration check for the new
  forward-only migration.

### Not included

- WebDAV/MCP/Admin adapters, authentication, HTTP ETag parsing, protocol error
  DTOs, or listener wiring. Later protocol packages call these Core services.
- Filesystem watcher/full initial scanner implementation; WP-07 will import
  broad out-of-band changes. WP-04 only diagnoses a mismatch encountered by a
  Core query or an incomplete journal operation.
- Durable distributed lock backend, job workers, index projections, memory
  extraction/materialization policy, backup/restore archives, or retention
  garbage collection.
- Fuzzy patches, arbitrary absolute paths, force-overwrite operations, or
  silent recovery of an intent whose target/hash cannot be proven.

## Invariants and risks

- Every public Core method receives a `VaultContext`; state queries include the
  Vault ID and storage/history instances are constructed from that context.
- A context must match the registered Vault ID, slug, and content root before
  any user-data operation. A context from another Vault cannot reuse a path,
  file ID, history blob, lock, or journal row.
- The Core layer never executes SQL and protocol crates never receive a raw
  pool. State owns all SQL and only exposes typed repositories/commit methods.
- A successful canonical mutation follows: journal `prepared` → storage temp
  stream/fsync → atomic rename → journal `file_committed` → one SQLite
  transaction for file identity, revision, history metadata, audit, outbox,
  and journal `metadata_committed`.
- Expected revisions are checked both before the physical write and again in
  the conditional state commit. A stale caller cannot silently replace a
  newer file even if an in-process lock was released or a second process is
  present.
- A failed stream never commits the destination. A failed metadata transaction
  leaves the canonical file old or new and leaves a journal row for recovery;
  no partial content is accepted as current.
- Restore never decrements a revision. It reads a validated history blob and
  records a new `restore` revision with fresh audit/outbox facts.
- Audit and outbox payloads contain paths only as hashes or bounded metadata;
  note bodies, absolute roots, tokens, and provider secrets are never stored.
- Idempotency replay returns the original committed mutation result. A key
  reused with a different operation/payload is rejected rather than applied.

## Proposed design

### Dependency direction

```text
protocol adapters (later)
          │
          ▼
       vault-core
       ├── domain
       ├── storage-fs
       └── state (typed repositories only)
```

The repository is currently a modular monolith, so `vault-core` will depend
on `mcp-vault-state` and `mcp-vault-storage-fs` while keeping SQL and protocol
concerns out of its source. A later measurement-driven refactor can extract
traits without changing the Core method contracts.

### State records and commit API

`crates/state/src/files.rs` will define `FileRecord`, `FileRevisionRecord`,
`JournalRecord`, `EntryType`, `FileOperation`, and `JournalState`, plus
`FileStateRepository`. The public commit input contains typed IDs/paths,
expected revision, hash/size/mtime/identity, actor/source plane, history hash,
idempotency key, audit metadata, and outbox event payloads. The repository
owns the SQL transaction and conditional updates; Core supplies no SQL.

The new migration adds `operation_journal.idempotency_key` and a partial
unique index on `(vault_id, idempotency_key)`. Existing `0001` remains
immutable. Deleted entries remain as tombstones so revision/history FKs and
auditability survive; creating the same path may explicitly reuse that
tombstone's stable File ID with a new revision.

### Core mutation flow

```text
VaultContext + typed command
  → registered-context/path/policy check
  → idempotency lookup
  → deterministic path lock(s)
  → current file/hash/precondition check
  → journal prepared
  → phased storage-fs write/copy/move/delete
  → journal file_committed
  → state conditional metadata transaction
       file entry + revision + history metadata
       audit + outbox + journal metadata_committed
  → typed MutationResult / revision conflict
```

For content replacement, the previous current content is first streamed into
`HistoryStore` when policy requires it; the new content is hashed by
`storage-fs`. Move preserves File ID; copy creates a new File ID; delete marks
a tombstone and retains its history; restore reads a previous history blob and
creates a fresh revision.

### Exact patch and query behavior

Unified diffs are parsed into explicit hunks and applied only when every
context/removal line matches the current content at the declared location.
No fuzzy search or offset guessing is allowed. Append and heading-section
operations are exact UTF-8 transformations; binary files accept replace,
append, copy, move, delete, and restore but reject text patch operations.

Queries return a `FileRecord` plus a streaming `ReadFile`/metadata handle.
Before serving a current read, Core compares stored hash/size/identity with
the filesystem and returns a typed external-mismatch error instead of silently
serving a state it cannot explain.

### Recovery behavior

`RecoveryService::recover` lists incomplete journal rows for one Vault. It
validates every stored path as a Vault-relative path and inspects only the
bound storage root. If the journal says `prepared` and the target still has
the prior hash (or is absent for create), it removes the known temp and marks
the operation rolled back. If the target has the proposed hash and the temp
is gone, it can finalize state metadata idempotently. If neither old nor new
state is provable, it marks `needs_review` and the caller can place the Vault
in maintenance before readiness.

WP-04 exposes a deterministic failure injector for tests. It does not pretend
that an in-process error is a power loss; tests explicitly invoke recovery
after each injected phase and assert old/new atomicity, journal resolution,
revision consistency, and idempotent outbox/audit results.

## Work breakdown

1. Add `0002_operation_idempotency.sql`, typed state records, repository
   queries, and the commit hook seam. Validate migrations and Vault predicates.
2. Extend `storage-fs` with a phased atomic-write handle and typed temporary
   path needed by Core journal/recovery without exposing absolute paths.
3. Implement `VaultCore`, lock manager, mutation commands/results, exact text
   patching, history capture, conditional state commits, audit/outbox events,
   and idempotency replay.
4. Implement `RecoveryService` and mismatch diagnosis; exercise all journal
   phases with deterministic failure injection and restart-style recovery.
5. Add state/Core integration tests for all operations, two-Vault isolation,
   conflicts, lock ordering, restore, idempotency, and crash outcomes.
6. Update schema/docs, run full Rust/frontend/docs/container checks, record
   evidence, and move this plan to `docs/exec-plans/completed/`.

## Progress

- [x] 2026-08-20 — Read the root instructions and WP-04 requirements plus
  product, architecture, interface, data-model, security, deployment, and
  testing constraints.
- [x] 2026-08-20 — Inspected WP-00-WP-03 code, migration 0001, state seam,
  storage-fs API, and the empty Vault Core boundary.
- [x] 2026-08-20 — Added migration 0002 and typed state repository/conditional
  commit APIs for files, revisions, journal, audit, outbox, and idempotency.
- [x] 2026-08-20 — Added phased storage-fs atomic-write support and typed
  relative temporary paths for journal/recovery use.
- [x] 2026-08-20 — Implemented Core mutation/query/history services, exact text
  patching, deterministic locks, history capture, and stable ETags.
- [x] 2026-08-20 — Implemented recovery/mismatch diagnosis, startup recovery
  before server readiness, and deterministic phase fault injection.
- [x] 2026-08-20 — Added 8 Core integration tests covering all mutation paths,
  concurrency, isolation, audit/outbox, idempotency, and recovery phases.
- [x] 2026-08-20 — Ran final Rust, frontend, documentation, startup, and
  container checks; updated the schema checksum and completed this plan.

## Decisions

- Keep `0001_operational_state.sql` immutable and add a forward `0002` only
  for journal-level idempotency. This preserves SQLx checksum safety and makes
  the durable operation key available before protocol adapters arrive.
- Use typed state repositories directly from Vault Core in this work package;
  no raw SQL or `SqlitePool` crosses the boundary. Extracting a separate trait
  crate is deferred until a second implementation/test double justifies it.
- Keep deleted `file_entries` as tombstones and allow explicit same-path
  recreation to reuse the stable File ID. This avoids deleting rows referenced
  by immutable revision history and keeps the current migration's path key
  intact; a later retention design can introduce a dedicated tombstone path.
- Use an in-process lock manager keyed by `(VaultId, normalized path)` for the
  first service. SQLite expected-revision checks remain authoritative across
  processes; a distributed lock backend is a later operational enhancement.
- Keep history capture and canonical writes synchronous in the mutation request
  but keep indexing/provider work out of the request. History is a safety copy,
  not a derived projection.

## Surprises and discoveries

- The WP-02 state transaction intentionally hid its raw SQL handle, so WP-04
  requires repository-owned conditional commit methods rather than widening
  that escape hatch.
- `storage-fs` currently exposes a single high-level write method. Core phase
  testing requires an opaque phased handle; the extension will preserve the
  existing convenience API and keep temporary paths relative/typed.
- The existing schema's `deleted_at` plus unique `(vault_id, path)` makes a
  tombstone path-reuse policy necessary; this plan chooses stable-ID reuse and
  records delete/recreate as separate revisions.

## Validation

```bash
cargo fmt --all --check
cargo test -p mcp-vault-state --all-features
cargo test -p mcp-vault-core --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
make check
make build
docker build --tag mcp-vault:wp04 .
shasum -a 256 -c SHA256SUMS
```

Acceptance evidence must include migration version 2, state/Core integration
tests for all mutation types, two-Vault isolation, stale revision conflicts,
exact patch rejection, deduplicated history, audit/outbox atomicity,
idempotent retry, every injected journal phase, recovery outcomes, and final
workspace/frontend/documentation/container checks.

## Rollback and recovery

`0002` is forward-only and must be backed up with the operational database
before deployment. A binary rollback to a pre-WP-04 build may require restoring
the pre-migration database backup. No code path edits or deletes `0001`.

Failed Core writes leave canonical content old or new and retain a journal row
until recovery resolves it. Recovery is conservative: it removes only a
known, validated temporary path or finalizes a state transaction whose content
hash is proven; ambiguous rows become `needs_review` and readiness remains
unhealthy until an operator resolves them.

## Outcomes

- Added migration `0002` with a Vault-scoped operation idempotency key and
  forward-only migration verification.
- Added typed state repositories and a conditional metadata transaction that
  commits file identity, revision, audit, outbox, and journal state together.
- Added phased safe filesystem writes, deterministic Vault/path locks, Core
  mutation/query/history/restore services, exact patching, streaming copy,
  mismatch diagnosis, and conservative journal recovery.
- Added eight Core integration tests covering mutation boundaries, stable IDs,
  history, isolation, concurrent stale revisions, idempotency, outbox/audit
  atomicity, external changes, exact patch rejection, and all injected phases.
- Startup recovery now runs for registered Vaults before readiness; an
  ambiguous journal blocks startup with maintenance-required status.
- `make check`, `make build`, and the loopback startup smoke passed. The smoke
  returned `{"status":"ok"}` from `/health/live` and `{"status":"ready"}`
  from `/health/ready`.
- `docker build --tag mcp-vault:wp04 .` passed and produced image digest
  `sha256:3e3e3d838c1bca4465cbb7e5e7fd6c6165b875cf2fa3d7c953d6f57f6c900b09`.

This plan is complete and is moved to `docs/exec-plans/completed/`.
