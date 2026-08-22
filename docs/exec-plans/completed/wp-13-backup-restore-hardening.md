# WP-13 Backup, Restore, Observability, and Hardening

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-21
- Updated: 2026-08-21

## Purpose and user-visible result

Implement the operational recovery plane for MCP Vault. An owner can create
and verify a content-addressed, manifest-backed backup containing canonical
Vault files, SQLite operational state, and revision history; validate and
apply a restore through a staged, maintenance-gated swap; inspect non-secret
health/metrics/diagnostics; and operate the service with bounded resources,
retention, crash recovery, and a hardened non-root container.

## Governing requirements

- `AGENTS.md`: canonical knowledge remains portable; SQLite/history/credentials
  are authoritative and must be backed up; protocol handlers cannot own
  filesystem/SQL business logic; secrets and note bodies stay redacted.
- `docs/product-requirements.md` sections 3.7, 3.9, 3.10, 4.1, 4.5, and 6.
- `docs/architecture.md` sections 1-4, 8-10, 13-17, especially the separate
  control/data planes, durable jobs, recovery, and rebuildable projections.
- `docs/interfaces.md` sections 10-11 and backup/restore error contracts.
- `docs/data-model.md` sections 1-2, 9-10, 16-19, especially the backup
  catalog and authoritative/rebuildable state split.
- `docs/security.md` sections 8-10, 16-21, especially archive validation,
  resource limits, container hardening, and backup permissions.
- `docs/deployment-and-operations.md` sections 6-19 and 18-19.
- `docs/development-and-testing.md` sections 7-8, 13-18.
- Accepted ADR-0001, ADR-0002, ADR-0004, ADR-0005, ADR-0008, ADR-0009,
  ADR-0010, and ADR-0011.

## Current repository state

- `crates/admin-api/src/lib.rs` exposes `/backups`, `/restore/validate`, and
  `/restore` through the stateful backup application service.
- `crates/state` now owns the forward-only backup catalog migration/repository,
  SQLite snapshot/restore boundary, migrations, integrity checks, jobs/outbox,
  Vault registry, and redacted audit records.
- `crates/storage-fs` owns no-follow Vault/history storage, bounded walks,
  content hashes, atomic primitives, and free-space diagnostics.
- `crates/vault-core` remains the canonical mutation boundary; WebDAV, MCP,
  Admin, workers, and restore coordination now share the process maintenance
  gate.
- `crates/server` composes durable backup workers, readiness/metrics, optional
  OTLP tracing, two listeners, and the maintenance coordinator.
- `Dockerfile` and Compose declare the non-root, read-only-rootfs, resource,
  signal, backup, and metrics runtime policy.
- The WP-12 Admin UI now drives asynchronous backup creation/verification and
  the reauthenticated staged-restore flow.

## Scope

### Included

- Add a forward-only backup catalog migration and typed repository operations.
- Add a backup application boundary (a dedicated crate) that owns portable
  archive creation, manifest/checksum generation, verification, retention,
  staged restore validation, and safe path/entry/size limits.
- Include all registered Vault content, operational SQLite state, history
  blobs, backup metadata, schema/service versions, and encryption key version
  identifiers; never include plaintext master keys by default.
- Add durable backup/verify/restore job admission and server handlers with
  bounded, cancellation-aware execution.
- Add process maintenance modes (`normal`, `read_only`, `offline`) shared by
  data adapters, Admin, workers, and restore coordination; preserve reads in
  read-only mode and return clear maintenance responses for offline requests.
- Add non-sensitive health detail, Prometheus-style metrics, request/worker/
  backup counters, optional OTLP tracing configuration, disk/resource limits,
  retention cleanup, and redacted diagnostic bundle generation.
- Harden Compose/image defaults, add SBOM/vulnerability scan hooks where the
  repository can run them, and document upgrade/rollback/DR procedures.
- Add unit, repository, integration, crash/recovery, malicious-archive,
  cross-Vault, redaction, low-disk, and container/configuration tests.

### Not included

- External WebDAV Litmus, Obsidian compatibility, official MCP conformance,
  performance baselines, release signing, or final requirements traceability;
  those remain WP-14 release gates.
- Packaging the installation master key into ordinary backups. A separate
  explicitly encrypted key-export workflow may be represented and documented,
  but must not silently expose or copy the key.
- Cross-host/cloud backup transport or remote object-store credentials.

## Invariants and risks

- Backup output is a recovery artifact, never canonical knowledge. A copied
  Vault content root remains usable without this service.
- Archive entries are validated before extraction: relative paths only,
  normalized separators, no duplicate paths, no symlink/hardlink/device
  entries, bounded file/entry/total sizes, and allow-listed top-level roots.
- Restore stages under a service-owned temporary root, verifies the manifest
  and all checksums before maintenance, creates a pre-restore safety backup,
  swaps only configured roots, runs migrations/integrity/recovery, and removes
  staging data on success or failure.
- The restore path never accepts an arbitrary `vault_id` to select a target;
  it uses the registered Vault contexts and validates manifest identity.
- Read-only mode blocks canonical mutations while allowing bounded reads;
  offline mode gates data protocols and leaves authenticated Admin recovery
  routes available. Worker claims stop or remain reclaimable during swaps.
- Diagnostics, metrics, manifests, and job payloads contain no note bodies,
  memory bodies, Authorization/Cookie headers, passwords, tokens, API keys, or
  provider prompts/responses.
- Retention never automatically deletes the last verified backup and never
  removes active/in-progress artifacts.

## Proposed design

### Components and dependency direction

Add `crates/backup` as an application/infrastructure boundary depending on
`domain`, `state`, `storage-fs`, and `vault-core` only. It owns the portable
backup format and orchestration, while SQL remains in `state`, safe Vault I/O
remains in `storage-fs`/Core, and Admin only translates DTOs to service calls.
`server` composes the backup service, maintenance gate, worker handlers,
metrics, and listeners.

Use a small `MaintenanceGate` in `domain` (stateful process coordination, not
a Vault singleton) so WebDAV, MCP, Admin, workers, and backup share the same
normal/read-only/offline state without adding protocol dependencies downward.

### Data and transaction flow

1. Admin validates session, CSRF, confirmation, and bounded request fields.
2. Backup service creates a catalog row and durable global job with a bounded
   backup/restore operation ID; the HTTP response returns that operation.
3. A backup worker enters read-only coordination, records the Vault revision
   and scan/checkpoint snapshot, streams safe content/history entries into a
   staging archive, snapshots SQLite through the state repository boundary,
   writes a manifest, verifies checksums, atomically publishes the artifact,
   and marks the catalog verified/completed.
4. Restore validation never changes configured roots. It validates the archive
   into private staging and returns a redacted manifest/diff summary.
5. Restore apply creates a pre-restore backup, switches offline, validates
   again, atomically swaps staged content/history/database artifacts, runs
   migrations and Core recovery/reconciliation, then reopens normal mode only
   after integrity checks pass. Failure leaves the pre-restore state and marks
   maintenance/error for operator review.

### Public interfaces and schema changes

- Add `migrations/0008_backup_catalog.sql` with the catalog, operation status,
  manifest, verification/error timestamps, and bounded location fields.
- Add `StateStore::backups()` and typed `BackupRecord`/`BackupRepository` APIs;
  no Admin handler may query `backups` directly.
- Add `BackupService` methods for enqueue/list/verify/validate/apply/retention
  and safe diagnostic summaries.
- Replace WP-12 backup stubs with authenticated Admin operations returning
  stable envelopes and operation IDs; destructive restore requires explicit
  confirmation plus recent reauthentication.
- Add `/metrics` only when configured/enabled and keep its labels bounded and
  non-sensitive; detailed health remains on the authenticated control plane.

### Failure, retry, and recovery

Backup jobs retry transient I/O/SQLite failures with bounded backoff and
redacted error codes. Corrupt manifests, unsafe archive entries, checksum
mismatches, incompatible schemas, low disk, and key/version mismatches fail
permanently without touching configured roots. Leased jobs are reclaimable on
restart. A restore crash leaves a durable `restoring` catalog/job state; a
reclaimed job re-enters staged validation/apply, while an authenticated
`RECOVER` action reopens the process only after live integrity and Core journal
checks pass.

## Work breakdown

1. Add maintenance gate, configuration limits, backup catalog migration, and
   typed repository methods; prove migration and Vault identity constraints.
2. Implement safe archive/manifest creation and verification with content,
   history, SQLite snapshot, checksum, retention, and low-disk tests.
3. Implement staged restore validation/apply, pre-restore safety backup,
   maintenance transitions, integrity/recovery checks, and crash tests.
4. Wire durable backup/verify/restore workers and replace Admin API stubs/UI
   page with operation status, manifest, confirmation, and redacted errors.
5. Add health/metrics/diagnostic surfaces, resource limits, optional OTLP
   tracing, container hardening, SBOM hooks, and upgrade/rollback docs.
6. Run full checks, update docs/checksums, and archive this plan only after
   clean-host restore and security acceptance evidence is captured.

## Progress

- [x] 2026-08-21 — Read root instructions, ordered specifications, WP-13
  requirements, operations/security/testing docs, and accepted ADRs.
- [x] 2026-08-21 — Confirm WP-12 is complete and create this WP-13 ExecPlan
  before implementation.
- [x] 2026-08-21 — Implement maintenance/catalog/application backup boundary,
  including SQLite snapshot/restore, key-version compatibility, archive
  validation, retention, and low-free-space rejection.
- [x] 2026-08-21 — Implement staged restore and crash-safe recovery, including
  resumable `running/restoring` catalog states, rollback integrity checks, and
  authenticated `RECOVER` reopening.
- [x] 2026-08-21 — Implement worker/Admin/UI integration with separate control
  and data-plane maintenance behavior and redacted operation/audit responses.
- [x] 2026-08-21 — Implement observability, limits, hardening, docs, CI
  container checks, and final local validation.

## Decisions

- Use a portable uncompressed tar artifact with a JSON manifest and SHA-256
  checksums; the service never extracts unvalidated archive entries.
- Keep the installation master key outside ordinary backups; record only key
  version identifiers and document separate encrypted key export/re-entry.
- Treat backup/restore as global operational jobs while every content/history
  record in the manifest remains explicitly Vault-identified.

## Surprises and discoveries

- WP-12 already has stable backup route contracts and explicit not-configured
  responses, so the public adapter can be completed without changing API
  versioning.
- Existing Core status checks provide a per-Vault write barrier; WP-13 adds a
  shared read-only/offline gate so a multi-root restore also blocks protocol,
  Admin, and worker mutations during swaps.
- The new backup package uses `tar` only as a container. StateStore owns the
  catalog and SQLite snapshot boundary, while storage/Core retain ownership of
  safe Vault and history I/O.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
```

Focused evidence must include clean-host restore, corrupt/unsafe archive
rejection, checksum mismatch, low disk, Vault isolation, redaction, retention
last-verified protection, offline/read-only protocol behavior, worker lease
reclaim, diagnostic/metrics safety, and hardened image/configuration checks.

Completed evidence in this worktree includes the isolated temporary-root
create/validate/restore round trip, traversal rejection before extraction,
SQLite snapshot restore with the same pool, backup catalog transition/resume
coverage, source/root Vault isolation, low-free-space rejection before a
backup artifact is staged, fixed-label metric tests, WebDAV mutation method
classification, storage-fs directory-swap rollback coverage, Admin maintenance
gate coverage, schema-version compatibility rejection, full workspace/frontend
checks, and a release image build.
The local image reports `mcpvault`/UID 999 and `SIGTERM`; local `syft` and
`trivy` executables are not installed, so CI owns the SBOM and HIGH/CRITICAL
image-scan hooks.

## Rollback and recovery

Migration 0008 is forward-only. A failed backup does not alter canonical or
operational state beyond a failed catalog row and removable staging files. A
restore always creates a pre-restore backup before swapping. If staged apply
fails, the service rolls back roots/SQLite when possible and reopens only after
integrity and Core recovery checks pass; otherwise it remains offline until an
authenticated `RECOVER` request passes those same checks. Existing dirty
worktree changes are preserved; no commit or push is part of this work.

## Outcomes

WP-13 is complete. The repository now has a durable, Vault-isolated backup and
restore boundary, shared maintenance coordination, redacted observability,
container hardening hooks, and documented operational recovery procedures.
