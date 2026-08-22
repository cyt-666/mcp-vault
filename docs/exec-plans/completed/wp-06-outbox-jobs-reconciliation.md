# WP-06 Durable Outbox, Jobs, Watcher, and Reconciliation

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Make background processing durable and restart-safe. Every Core outbox event
will be claimable through SQLite leases, acknowledged only after a handler
successfully admits the work, retried with bounded exponential backoff, and
eventually represented as dead-letter state instead of disappearing. Every
long-running task will use the persistent `jobs` table with Vault-scoped
deduplication, leases, progress, cancellation, retry, and terminal failure
state.

The server will have a cancellation-aware worker supervisor and a safe
filesystem reconciliation service. Startup will recover journals, establish
the initial scan/checkpoint, and start workers before readiness. A watcher may
accelerate detection, but a resumable periodic reconciliation remains the
authority. Direct file creation, modification, deletion, and move will be
imported as auditable `external_change` revisions through Vault Core without
following unsafe paths or silently overwriting a concurrent Core mutation.

## Governing requirements

- `AGENTS.md`: durable outbox, persistent operation handling, Vault-scoped
  jobs/events, cancellation-aware workers, bounded concurrency, safe writes,
  and restart reconciliation.
- `docs/implementation-plan.md` section 9 (WP-06): outbox dispatcher,
  persistent leases, worker supervisor, retries/dead letters/progress/
  cancellation, watcher, initial scan, periodic reconciliation, startup
  integration, and the listed tests.
- `docs/product-requirements.md` sections 3.9, 4.1, 4.4, and 4.5: initial
  scan, immediate server-write observation, out-of-band reconciliation,
  durable queue, bounded background work, and readiness/operability.
- `docs/architecture.md` sections 7, 9, 10, 13, 15, and 16: durable
  filesystem/SQLite recovery, transactional outbox, persistent jobs,
  deterministic/semantic projection boundaries, per-Vault evolution, and
  observability.
- `docs/data-model.md` sections 9-11 and 18-19: outbox/job schema,
  statuses/leases, projection rebuildability, and forward migrations.
- `docs/deployment-and-operations.md` sections 6-10: startup/shutdown order,
  readiness requirements, initial scan, watcher limitations, and resumable
  reconciliation.
- `docs/development-and-testing.md` sections 7-8 and 10: real SQLite
  repository tests, application integration tests, and recovery coverage.
- `docs/security.md` sections 9-10, 15-17, and 20: no-follow scanning,
  write integrity, Vault-scoped job payloads/dedup keys, redacted logs,
  bounded resource use, and security verification.
- Accepted ADR-0001, ADR-0002, and ADR-0005: Markdown remains canonical,
  Vault is the isolation boundary, and the service remains a modular monolith.

## Current repository state

WP-00 through WP-05 are present in the working tree. Vault Core already
performs atomic mutations, writes `outbox_events` in the same SQLite
transaction as revisions/audit, and recovers incomplete operation journals.
The state crate now owns typed outbox/job/checkpoint repositories. The
existing schema had outbox `claimed_by/claimed_until/delivered_at/attempts/
last_error` and job `lease_owner/lease_until/progress_json/status` fields;
WP-06 adds dead-letter/cancellation/checkpoint state. `storage-fs` now exposes
safe bounded enumeration, Core imports external changes, and `server::run`
starts the worker and polling reconciliation supervisor before readiness.

## Scope

### Included

- Add forward migration `0004_background_processing.sql` with outbox
  dead-letter state, job cancellation state, and Vault-scoped resumable scan
  checkpoints/indexes.
- Add typed `OutboxRepository` and `JobRepository` APIs with all SQL inside
  `state`: bounded claims, lease expiry/reclaim, ack/retry/dead-letter,
  deduplicated enqueue, job progress, cancellation, completion/failure, and
  Vault predicates.
- Add safe bounded filesystem enumeration in `storage-fs` that never follows
  symlinks/special files, skips the managed reserved namespace, validates
  relative paths, and applies backpressure through a bounded channel.
- Add `VaultCore` reconciliation/import services that compare path/size/mtime/
  identity/hash, create external-change revisions for direct edits/new files/
  deletes, preserve prior history where provable, and report ambiguous or
  unsafe entries without silently accepting them.
- Add a cancellation-aware server worker supervisor with bounded outbox
  dispatch and job execution seams, retry/backoff, dead-letter transition,
  progress/cancel checks, and graceful shutdown.
- Add a durable outbox-to-job admission handler so current file events are
  acknowledged only after their derived-work job is durably enqueued. Unknown
  job types remain queued until a later index/memory/provider worker registers
  a handler; they are never auto-acknowledged and discarded.
- Add initial scan and periodic reconciliation jobs with durable checkpoint/
  progress state, startup recovery before readiness, and non-sensitive worker
  health reporting.
- Add unit/repository/integration tests for leases, restart reclaim, duplicate
  delivery/deduplication, retries/dead letters, cancellation, bounded scan,
  two-Vault isolation, watcher loss followed by reconciliation, external
  edit/delete/move, and graceful supervisor shutdown.
- Update data-model/deployment/testing documentation, checksums, migration
  assertions, and examples.

### Not included

- Markdown/FTS/index projection logic (WP-09), provider calls (WP-10), memory
  extraction/materialization (WP-11), WebDAV/MCP protocol adapters (WP-07/08),
  or the Admin job API/UI (WP-12). This package only creates durable job/event
  admission and handler seams for them.
- A platform-specific native watcher dependency. The first implementation
  uses a cancellation-aware polling reconciliation loop; a later adapter may
  add OS watcher acceleration without changing the durable authority.
- Distributed leases, a separate message broker, or a second database.

## Invariants and risks

- Outbox acknowledgement happens only after the event's derived job is
  durably admitted or a registered handler completes. A process crash before
  ack leaves the event reclaimable after the lease expires.
- A claim is conditional on the current lease/terminal state. Two workers may
  observe a candidate, but only one can transition it to its lease owner.
- Lease timestamps and retry availability are UTC integer milliseconds. Worker
  clocks are not trusted for authorization; expired leases are reclaimed by
  conditional SQL and bounded clock-skew assumptions are documented.
- Deduplication keys include a Vault identity for Vault work. Global jobs are
  explicitly global; no caller can make a Vault job global by omitting a
  context accidentally.
- Outbox/job payloads contain event metadata and typed paths only; note bodies,
  authorization headers, secrets, and absolute roots are never logged. Payload
  size is bounded before admission.
- Scan results are advisory until Core imports them through expected-revision
  conditional commits. A direct edit racing with a Core write produces an
  external-mismatch/conflict outcome rather than a silent overwrite.
- The scanner opens only the configured Vault root, skips the service-managed
  namespace, rejects symlinks/special files, and never exposes an absolute
  filesystem path to a handler.
- Initial/reconciliation scans are resumable. A crash may repeat a bounded
  batch, but checkpoint and Core idempotency prevent duplicate revisions for
  unchanged content.
- Shutdown stops new claims, lets in-flight handlers finish/checkpoint within
  the configured grace period, and leaves leases recoverable if the process is
  killed.

## Proposed design

```text
VaultCore commit
  → SQLite transaction: revision + audit + outbox
  → OutboxRepository claim lease
  → handler admits JobRepository row (Vault + dedup key)
  → ack outbox
  → JobSupervisor claims job lease
  → bounded handler/checkpoint
  → complete / retry_wait / failed / cancelled

Vault root
  → safe bounded enumeration
  → scan checkpoint
  → Core external-change import
  → external_change revision + audit + outbox
```

### State repositories

`OutboxRepository` and `JobRepository` are concrete SQL-owning repositories
returned by `StateStore`. They return typed records, not `SqlitePool` or raw
rows. Claim methods use short transactions and conditional updates. Lease
owners are opaque worker IDs; they are never used as authorization identities.

`ScanCheckpointRepository` stores one checkpoint per `(vault_id, scan_type)`
with status, cursor path, generation, counts, error, and timestamps. The
cursor is a validated `VaultPath` string and is only a progress hint; a scan
generation/reconciliation pass still verifies current filesystem state.

### Worker supervisor

`server::workers` owns orchestration, cancellation, and bounded task
concurrency. State repositories own claim/ack/retry transitions. The default
outbox handler turns a file event into a durable derived-work job; it does not
pretend that indexing is complete. Job handler registration is explicit and
later crates can add handlers without changing lease semantics.

The supervisor reports `starting/running/draining/stopped` and current queue
counts through a small non-sensitive health snapshot. Readiness is marked only
after the supervisor has started its loops and startup scan/recovery has
completed.

### Reconciliation

`storage-fs` exposes a bounded `walk_entries` stream of safe relative metadata.
Core batches entries, hashes only candidates whose metadata/identity changed,
and compares them with Vault-scoped `file_entries`. For a new/changed file it
creates an `external_change` journal intent, records the already-present file
as committed, captures a history blob, and uses the same conditional metadata
transaction as normal Core writes. For a missing current file it creates a
tombstone external-change revision. A same-filesystem identity/hash match is
imported as an external move preserving `FileId`; ambiguous or unsafe results
are counted and surfaced for maintenance; no arbitrary path is repaired.

The initial scan uses the same path and import code with an explicit scan
generation. Periodic reconciliation records a completed checkpoint only after
the full pass and missing-entry comparison succeeds.

## Work breakdown

1. Add `0004_background_processing.sql`, typed outbox/job/checkpoint records,
   claim/lease/retry/cancel repositories, and migration/isolation tests.
2. Add safe `VaultStorage::walk_entries` enumeration with bounded backpressure
   and symlink/reserved-path/special-file tests.
3. Add Core external-change import, initial scan, reconciliation reports, and
   restart/idempotency/concurrency tests.
4. Add server worker supervisor, outbox-to-job admission, bounded retry/dead
   letter behavior, job handler seam, cancellation, and health state.
5. Integrate startup scan/recovery and periodic reconciliation before readiness
   and during graceful shutdown; update operations/data-model docs.
6. Run focused/security/workspace/frontend/docs/container validation, record
   evidence, and move this plan to `docs/exec-plans/completed/`.

## Progress

- [x] 2026-08-20 — Read root instructions, product/architecture/
  implementation/plan documents, PLANS, deployment/testing/security/data-model
  constraints, and inspected current outbox/jobs/Core/server/storage seams.
- [x] 2026-08-20 — Add migration and typed durable processing repositories.
- [x] 2026-08-20 — Add safe filesystem enumeration.
- [x] 2026-08-20 — Add Core external-change import and reconciliation.
- [x] 2026-08-20 — Add worker supervisor and retry/cancellation semantics.
- [x] 2026-08-20 — Integrate startup/periodic scan and readiness/shutdown.
- [x] 2026-08-20 — Run final checks and complete the plan.

## Decisions

- Use SQLite leases and conditional updates as the only durable claim
  authority. In-memory channels may wake workers but never replace the rows.
- Keep the first watcher implementation as polling reconciliation. Native OS
  watcher acceleration is an optimization and cannot be the correctness path.
- Admit outbox events to durable jobs before acknowledging them. This keeps
  the current package useful before index/memory handlers exist and prevents a
  no-op dispatcher from deleting work.
- Treat the filesystem scan as untrusted observation. Only Vault Core may
  materialize an external-change revision and it must use the same history,
  audit, outbox, and conditional-revision rules as protocol writes.

## Surprises and discoveries

- WP-04 already writes complete event metadata transactionally, so WP-06 can
  add dispatch/lease behavior without changing the canonical write sequence.
- The existing schema had most lease columns but no terminal outbox flag,
  cancellation bit, or scan checkpoint. A forward migration is required even
  though the tables already exist.
- `storage-fs` intentionally avoided directory enumeration; the scanner must
  add it without exposing the absolute content root or weakening no-follow
  behavior.

## Validation

```bash
cargo fmt --all --check
cargo test -p mcp-vault-state --all-features
cargo test -p mcp-vault-storage-fs --all-features
cargo test -p mcp-vault-core --all-features
cargo test -p mcp-vault-server --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
CI=true pnpm --dir frontend/admin lint
CI=true pnpm --dir frontend/admin test
CI=true pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
make build
make check
docker build --tag mcp-vault:wp06 .
```

Validation completed on 2026-08-20: `make check`, `make build`, all focused
Rust tests, and the full workspace test suite passed. Frontend lint/test/build,
docs checks, Rust documentation, `git diff --check`, and all `SHA256SUMS`
entries passed. Docker built `mcp-vault:wp06`; the container reported
`database_migration_version=4`, `/health/live` and `/health/ready` returned
HTTP 200, and `docker stop` exited successfully. Tests cover migration 4,
lease reclaim, deduplication, dead-letter visibility, cancellation, two-Vault
isolation, bounded scan behavior, direct edit/delete/move import,
startup-before-readiness ordering, and graceful shutdown.

## Rollback and recovery

`0004` is forward-only and must be backed up with SQLite before deployment.
Older binaries require a pre-`0004` database backup; no migration is edited or
reversed. Existing outbox/job rows remain valid because new columns have safe
defaults.

If a worker dies, its lease expires and another worker reclaims the row. If a
handler exhausts retry policy, the job/event becomes visible as failed or
dead-lettered and is not silently deleted. If a scan dies, the checkpoint
remains incomplete and the next generation resumes/revalidates from its safe
cursor. If external state is ambiguous, Core leaves the row/file untouched
and reports the issue for operator review.

## Outcomes

Implemented outcomes:

- `0004_background_processing.sql` adds explicit outbox dead-letter state,
  durable job cancellation, scan checkpoints, claim indexes, and forward
  migration coverage. Typed repositories enforce registered Vault contexts,
  conditional leases, deduplication, progress, cancellation, retry, and
  terminal transitions.
- `VaultStorage::walk_entries` enumerates with bounded-channel backpressure,
  deterministic ordering, no symlink/special-file following, reserved-root
  exclusion, and safe relative metadata only.
- `VaultCore::reconcile` imports safe external create/edit/delete/move
  observations through the journal/history/audit/outbox path and suppresses
  inferred deletes when scan evidence is incomplete.
- The server supervisor admits every outbox row to a durable derived-work job
  before acknowledgement, applies bounded dispatch/retry/dead-letter rules,
  shares shutdown cancellation with job handlers, releases leases, and
  reports lifecycle health. Startup performs an initial scan before readiness;
  polling reconciliation remains the correctness authority after watcher loss.
- Focused tests cover repository lease/retry/isolation behavior, bounded
  enumeration, Core external changes and unsafe symlink protection, outbox
  admission, startup checkpoint completion, and worker shutdown.

All planned validation and container smoke evidence is recorded above. This
plan is complete and is moved to `docs/exec-plans/completed/`.
