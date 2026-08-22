# WP-02 SQLite Operational State

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Give MCP Vault a durable, Vault-aware SQLite operational state layer that can
survive process restarts and become the transaction boundary for later Vault
Core, authentication, outbox, and job work. The state crate will open SQLite
with the required durability PRAGMAs, apply embedded forward-only SQLx
migrations, expose repository APIs rather than raw SQL to callers, and provide
integrity/migration diagnostics.

The first migration will create the operational foundation for Vault registry,
settings, identities/sessions, WebDAV/MCP/OAuth credentials, file identities
and revisions, operation journal, outbox, jobs, providers/models, and audit.
Derived index/memory tables and backup catalog remain owned by later packages.

## Governing requirements

- `AGENTS.md`: SQLite as authoritative operational state, Vault isolation,
  repository-only SQL, explicit domain errors, and required migration/isolation
  tests.
- `docs/implementation-plan.md` section 5 (WP-02): SQLx pool/PRAGMAs, initial
  migrations, repositories, transaction pattern, integrity diagnostics, and
  required tests.
- `docs/architecture.md` sections 3.2, 4.2, 4.5, 7, 9, 13, and 15:
  operational durability, Vault Registry, repository boundaries, journal/
  outbox shape, multi-Vault rows, and dependency direction.
- `docs/data-model.md` sections 2-10, 14, 16, and 19: required PRAGMAs,
  tables/columns/constraints, provider/audit state, and forward migration rules.
- `docs/security.md` sections 15-16: compound Vault isolation, query predicates,
  redacted audit/log data, and no secrets in operational responses.
- `docs/development-and-testing.md` sections 4, 5, 7, and 8: SQL confined to
  state, migration layout, foreign-key tests, and repository isolation tests.
- `docs/deployment-and-operations.md` sections 5-8: persistent state roots,
  startup migration ordering, readiness, and SQLite health diagnostics.
- Accepted ADR-0002 (`docs/adr/0002-vault-is-the-isolation-boundary.md`): every
  dependent row and repository operation preserves Vault scope.

## Current repository state

WP-00 and WP-01 remain completed in the working tree but are not committed.
WP-02 adds the first operational SQLite migration, the SQLx-backed state
boundary, Vault/settings repositories, and startup database validation. The
implementation preserves the domain crate's typed IDs, `VaultContext`, safe
`VaultPath`, revisions, and actor/scope types. Existing user changes remain
preserved.

## Scope

### Included

- Add SQLx SQLite dependencies and lockfile entries; use the Tokio runtime,
  bundled SQLite, migrations, JSON, and query/row support.
- Add `StateStore` pool initialization with `foreign_keys=ON`, WAL,
  `synchronous=FULL`, and `busy_timeout=5000`, with explicit in-memory test
  handling and connection diagnostics.
- Add root `migrations/0001_operational_state.sql` for Vaults, system/Vault
  settings, encrypted-secret metadata, Admin identities/sessions, WebDAV and
  MCP credentials, OAuth issuers/grants, files/revisions, journal, outbox,
  jobs, providers/models/bindings, and audit.
- Enforce composite Vault foreign keys and Vault-inclusive unique/index keys
  wherever SQLite can enforce them; use partial unique indexes for nullable
  global job/model scopes.
- Add a state error type, typed status/record DTOs separate from domain/HTTP
  models, a `VaultRepository`, a Vault-scoped `SettingsRepository`, and a
  `StateTransaction` unit-of-work entry point.
- Add integrity check, foreign-key violation, migration version, and table
  diagnostics without exposing a general-purpose SQL executor.
- Add fresh migration, empty prior-fixture upgrade, PRAGMA, constraints,
  optimistic setting revision, Vault A/B isolation, transaction rollback, and
  invalid operational data tests.

### Not included

- Index, FTS, knowledge-map, memory, embedding, or vector tables (WP-09/WP-11).
- Backup catalog/restore implementation (WP-13).
- Password hashing, PAT digests, OAuth token validation, or secret encryption
  (WP-05); this package stores only encrypted-secret metadata columns.
- Canonical filesystem behavior, revisions/history blob writes, journal
  recovery, outbox dispatch, or job workers (WP-03/WP-04/WP-06).
- Provider HTTP calls or Admin endpoints.

## Invariants and risks

- SQL remains private to `mcp-vault-state`; protocol/application crates receive
  repositories/records, never `SqlitePool` or SQL strings.
- `vault_id` is a required predicate/foreign key for every Vault-owned row.
  Composite foreign keys bind file revisions to the file entry's same Vault;
  tests deliberately attempt cross-Vault references.
- Global records such as providers and Admin users are not falsely assigned a
  Vault. Nullable `vault_id` is used only where the data model explicitly
  permits global defaults, with partial unique indexes closing SQLite NULL
  uniqueness gaps.
- Settings writes use `WritePrecondition` and a transaction so exact-revision
  callers receive a conflict instead of losing a concurrent update. An
  unconditional write is explicit.
- Migrations are embedded and validated by SQLx. No shipped migration is
  edited; future schema changes require a new numbered migration.
- SQLite errors are retained as typed state errors for internal diagnostics and
  never serialized directly to external protocol clients.
- This package must not load secret contents or log setting JSON values; the
  encrypted-secrets table is metadata-only until WP-05.

## Proposed design

### Components and dependency direction

`mcp-vault-state` will contain:

```text
state
├── error.rs         # StateError and diagnostic types
├── migrations.rs    # embedded SQLx Migrator
├── pool.rs          # StateStore, PRAGMAs, health, unit of work
├── vaults.rs        # VaultStatus, VaultRecord, VaultRepository
└── settings.rs      # JSON settings records and optimistic writes
```

The crate depends on `mcp-vault-domain`, SQLx SQLite, Serde/JSON, Tokio, and
`thiserror`. It does not depend on Axum, protocol crates, `storage-fs`, or the
server composition root. Later repository modules extend this state boundary.

### Data and transaction flow

Startup flow:

```text
database URL
  → SqliteConnectOptions (foreign keys, WAL, FULL, busy timeout)
  → bounded SqlitePool
  → embedded SQLx migrations
  → integrity/foreign-key diagnostics
  → StateStore ready for application composition
```

Repository flow:

```text
VaultContext / typed command
  → state repository validates/serializes values
  → SQL query includes vault_id or global-scope predicate
  → transaction/conditional update
  → typed state record or domain conflict
```

`StateTransaction` is the explicit future unit-of-work seam. WP-02 does not
make the transaction execute arbitrary caller SQL; repository methods will gain
transaction-aware variants when Vault Core needs atomic file metadata/outbox
updates.

### Public interfaces and schema changes

Initial public state APIs:

```rust
StateStore::connect(database_url) -> Result<StateStore, StateError>
StateStore::migrate() -> Result<(), StateError>
StateStore::integrity_check() -> Result<IntegrityReport, StateError>
StateStore::begin() -> Result<StateTransaction<'_>, StateError>
StateStore::vaults() -> VaultRepository
StateStore::settings() -> SettingsRepository

VaultRepository::insert(&VaultContext, name, VaultStatus) -> Result<...>
VaultRepository::find_by_id(VaultId) -> Result<Option<VaultRecord>, StateError>
VaultRepository::find_by_slug(&VaultSlug) -> Result<Option<VaultRecord>, StateError>

SettingsRepository::set_system(key, value, WritePrecondition, actor)
SettingsRepository::set_vault(&VaultContext, key, value, WritePrecondition, actor)
```

The migration is the first SQL schema and follows `docs/data-model.md`. It
deliberately excludes later derived tables and backup catalog.

### Failure, retry, and recovery

Connection/migration failures stop startup and keep readiness false. SQLite
busy handling is configured at the driver boundary and remains bounded. A
failed repository transaction rolls back on drop; no background retry is
introduced until WP-06's durable job layer. Forward migration validation
detects changed applied migration files through SQLx checksums.

## Work breakdown

1. Add the SQLx workspace dependency, state modules, root migration directory,
   and a prior-empty fixture. Validate dependency direction and compilation.
2. Implement `StateError`, `StateStore`, connection options, embedded migration,
   PRAGMA/integrity diagnostics, and `StateTransaction`.
3. Write the operational migration with composite Vault constraints, partial
   uniqueness indexes, indexes for scoped lookups, and checks for enumerated
   statuses.
4. Implement Vault registry/status records and settings repositories with
   typed domain conversion, JSON validation, and optimistic revision writes.
5. Add fresh/upgrade/constraint/isolation/transaction tests against temporary
   SQLite databases; include two Vault contexts and invalid cross-Vault rows.
6. Update database/development documentation if implementation details clarify
   the schema, run full checks, record evidence, and move this plan to
   `docs/exec-plans/completed/` only after acceptance.

## Progress

- [x] 2026-08-20 — Read root instructions and WP-02 database, architecture,
  security, deployment, and testing requirements.
- [x] 2026-08-20 — Inspected WP-00/01 state crate and domain APIs; no existing
  migrations or repositories are present.
- [x] 2026-08-20 — Added exact SQLx 0.8.6 dependency, state modules, root
  migration, and empty pre-WP-02 fixture.
- [x] 2026-08-20 — Implemented pool setup, directory preparation, PRAGMAs,
  embedded migration, integrity diagnostics, and StateTransaction.
- [x] 2026-08-20 — Implemented operational schema plus Vault/status and typed
  JSON settings repositories.
- [x] 2026-08-20 — Added migration, composite-FK, uniqueness, isolation,
  revision-conflict, rollback, and real SQLite integration tests.
- [x] 2026-08-20 — Ran final workspace/frontend/docs/Docker checks, verified a
  real server startup against a temporary SQLite database, and completed the
  ExecPlan.

## Decisions

- Use SQLx 0.8 with the Tokio runtime and bundled SQLite for this package. The
  workspace minimum Rust version remains 1.88 even though the local pinned
  toolchain is newer; the current SQLx 0.9 line raises its MSRV to 1.94, so it
  is not adopted without a workspace-wide MSRV decision.
- Keep one operational SQLite database and one `StateStore` pool; in-memory
  databases use one connection unless a shared-cache test URL is explicit.
- Store IDs as canonical UUID strings in SQLite and validate them back into
  typed domain IDs at repository boundaries.
- Use signed integer milliseconds for timestamps, `Revision::ZERO`/positive
  values for monotonic data, and JSON text for typed settings payloads.
- Enforce global nullable-scope uniqueness with partial indexes in addition to
  the documented compound unique constraints because SQLite permits multiple
  NULL values in a normal UNIQUE index.

## Surprises and discoveries

- The WP-00 Cargo workspace has no SQLx CLI and no existing database fixture;
  the upgrade test will therefore use a committed empty pre-WP-02 SQLite SQL
  fixture and document that no prior production schema exists yet.
- SQLx's current upstream main is 0.9.0 and requires Rust 1.94; the project
  deliberately stays on the 0.8 line to preserve the workspace's declared
  1.88 MSRV until a later explicit toolchain decision.
- Docker initially copied only crates and frontend assets; the builder now
  explicitly copies root migrations so the embedded SQLx migrator is available
  in container builds.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p mcp-vault-state --all-features
cargo test --workspace --all-features
make docs-check
make check
```

Expected results: migration and repository tests pass against real temporary
SQLite databases; PRAGMA and integrity diagnostics are green; two-Vault tests
prove isolation; all previous WP-00/01 tests remain green.

Observed results so far:

- `cargo test -p mcp-vault-state --all-features` passes 8 state unit tests and
  4 public repository integration tests.
- State Clippy passes with warnings denied; full workspace Clippy/tests pass.
- A real temporary server process created `state/mcp-vault.sqlite3` plus WAL
  files and returned healthy liveness/readiness after migration and integrity
  validation. The unprivileged sandbox could not bind ports, so this smoke was
  repeated with the required network permission.
- `make check && make build` passes formatting, workspace Clippy with warnings
  denied, workspace tests, frontend lint/test/build, documentation checks, and
  locked release compilation.
- `docker build --tag mcp-vault:wp02 .` passes, including the root migration
  directory in the builder context and compiling the release server.

## Rollback and recovery

WP-02 introduces the first database migration but no runtime database file in
the repository. Before deploying a future migration, operational backups are
required by the documented upgrade procedure. Locally, remove only temporary
test databases; do not delete a user `/data/state` database. If a migration
fails, startup remains unready and the forward migration file is corrected by
a new migration rather than editing an applied one.

## Outcomes

WP-02 is complete. MCP Vault now has a durable SQLx/SQLite operational-state
boundary with startup migrations and integrity checks, the initial Vault-aware
operational schema, typed Vault and settings repositories, optimistic setting
revisions, a transaction seam, and tests covering migration upgrades,
constraints, rollback, and two-Vault isolation. The server does not bind or
report readiness until database migration and integrity validation succeed.

The next unfinished work package is WP-03, Vault Core and safe filesystem
operations. This plan is moved to `docs/exec-plans/completed/`.
