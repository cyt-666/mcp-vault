# WP-14 Worker Fairness and Job Visibility

Status: Complete
Owner: Codex
Created: 2026-08-26
Last updated: 2026-08-26

## Purpose and user-visible result

The Admin console will always show every currently running task, independently
from the bounded recent-history page. Waiting/retry work and terminal history
will be presented separately, with automatic refresh while any active work
exists. A long-running task will no longer disappear after fifty newer tasks
are created.

The durable Worker will continuously fill free concurrency slots. One long job
inside an earlier claim will no longer prevent later queued jobs from starting
while other slots are idle. Startup migration from legacy memory will cancel
and drain incompatible Phase 1/Phase 2 work before resetting derived memory,
and it will keep the durable re-extraction-pending marker until the dedicated
fresh full-Vault extraction job has actually been admitted.

## Governing requirements

- `AGENTS.md`: persistent operation handling, bounded/cancellation-aware
  workers, Vault scoping, and complete recovery tests.
- `docs/product-requirements.md` sections 3.9 and 4.4-4.5: durable jobs,
  visible state/failures/progress, bounded concurrency, and worker health.
- `docs/architecture.md` sections 9.2 and 11: renewable leases, persistent
  cancellation, resumable long work, and the Codex-style two-phase memory
  migration.
- `docs/implementation-plan.md` WP-06 and WP-12: bounded worker supervision,
  restart recovery, failed-job visibility/retry, and operable Admin jobs UI.
- ADR-0016 and `docs/memory-system.md`: legacy reset must regenerate Phase 1
  inputs from current note revisions before Phase 2 consolidation.
- `PLANS.md`: this plan is maintained while implementation and validation run.

## Current repository state

- `crates/server/src/workers.rs::run_job_loop` claims up to `batch_size` rows,
  calls `dispatch_job_batch`, and does not claim again until every task in that
  batch exits. `dispatch_job_batch` uses a per-batch semaphore, so completed
  short tasks cannot be replaced while one long task remains.
- Claimed rows become `running` before a semaphore permit is obtained, which
  can also overstate the number of handlers actually executing.
- `crates/state/src/background.rs::list` orders by `created_at DESC` and the
  Admin frontend requests only `/jobs?limit=50`. An older running job therefore
  disappears behind newer history.
- `ensure_legacy_memory_reextract_job` clears
  `legacy_reextract_pending` after `enqueue_singleton` even when that call
  returned an unrelated pre-reset `memory.extract` job.
- Startup admits `memory.reset_legacy` without first requesting cancellation
  of old `memory.extract` or `memory.consolidate` work. A reset and old
  extraction can consequently mutate the same Vault generation concurrently.

## Scope and non-scope

### Included

1. Capacity-driven durable job dispatch with correct lease renewal,
   cancellation, shutdown draining, maintenance guards, and health counters.
2. Vault-scoped active/history job queries and an Admin jobs view that pins
   running work above waiting/retry work and terminal history.
3. Legacy reset exclusion from old extraction/consolidation and exact
   recognition of the dedicated post-reset re-extraction job.
4. Backend repository/supervisor/API tests and frontend behavior tests.
5. Documentation updates for observable Admin and recovery behavior.

### Not included

- Raising Worker concurrency as a substitute for fair scheduling.
- Deleting job history or changing job-retention policy.
- Adding a distributed multi-process scheduler. Existing renewable SQLite
  leases remain the coordination mechanism and future workers remain safe.
- Changing the Phase 1 or Phase 2 memory model/prompt contracts.

## Invariants and risks

- Claims remain durable and Vault-scoped; no active work exists only in an
  in-memory queue.
- At most `WorkerConfig::concurrency` job handlers execute in this process.
- Shutdown waits for spawned handlers to observe cancellation before releasing
  this worker's leases. A handler result is never committed after its lease is
  released.
- Admin job data never includes note bodies, prompts, provider responses, or
  secrets.
- Legacy reset must not modify derived memory while an older extraction or
  consolidation job for the same Vault is still running.
- Cancellation is cooperative. Reset retries while an incompatible running job
  drains rather than racing it or force-releasing its lease.
- The dedicated legacy regeneration starts from a new job with no inherited
  progress cursor. The pending marker is not cleared for an unrelated active
  extraction.

## Proposed design

### Capacity-driven Worker loop

`run_job_loop` owns one long-lived `JoinSet`. It calculates free capacity from
the number of spawned handlers and claims at most that many rows (also bounded
by `batch_size`). Claimed rows are immediately validated and spawned; missing
handlers and pre-start cancellation are resolved without occupying a slot.
Whenever a task completes, the loop persists its outcome and immediately tries
to claim replacement work. When no capacity or no work exists it waits on
either the next task completion, the poll interval, or shutdown.

The existing lease monitor remains attached to each handler. Maintenance
operation guards remain alive until the corresponding terminal/retry transition
is committed. Shutdown cancels handlers, drains the join set, then releases any
remaining leases owned by this worker.

### Job projections for Admin

The State repository gains a status-group filter for active
(`queued`, `running`, `retry_wait`) and terminal (`completed`, `failed`,
`cancelled`) jobs while retaining the existing exact-status query. The Admin
API exposes a jobs overview containing:

- all realistically bounded running jobs;
- bounded/paged waiting and retry jobs plus their total count;
- bounded terminal history ordered by latest update/completion;
- non-sensitive worker-independent queue counts.

The existing `/jobs` contract remains compatible. The React page consumes the
overview, renders separate Chinese sections, auto-refreshes while active work
exists, and never derives “no running work” from the recent-history slice.

### Legacy reset exclusion

Before admitting a reset at startup, MCP Vault requests cancellation for old
`memory.extract` jobs and cancellable queued/retry consolidation jobs. The reset
handler independently checks for incompatible active jobs; if any remain it
requests cancellation where allowed and returns a short retry without touching
memory state. This makes recovery safe even if the startup admission path was
bypassed or the process restarted mid-transition.

After reset, `ensure_legacy_memory_reextract_job` checks the returned job's
deduplication key. It clears `legacy_reextract_pending` only when the active job
is exactly `vault:<id>:memory-legacy-reextract-v1`. If another extraction still
exists, admission remains pending and reconciliation retries later.

## Work breakdown

1. Add State job grouping/count helpers and tests in
   `crates/state/src/background.rs` and `crates/state/tests/background.rs`.
2. Refactor `WorkerSupervisor` dispatch in `crates/server/src/workers.rs` and
   add a regression where a blocked job remains active while later short jobs
   fill freed slots.
3. Harden startup/reset/re-extraction admission in `crates/server/src/lib.rs`
   and `crates/server/src/workers.rs`, with old-cursor/reset recovery tests.
4. Add the Admin overview DTO/route and integration tests in
   `crates/admin-api/src/lib.rs`.
5. Update `frontend/admin/src/App.tsx`, `pages.tsx`, CSS, and frontend tests for
   active/waiting/history sections and automatic refresh.
6. Update operator/architecture documentation and run the validation suite.

## Progress

- [x] 2026-08-26: Reproduced UI omission from `/jobs?limit=50` plus
  `ORDER BY created_at DESC`.
- [x] 2026-08-26: Reproduced scheduler head-of-line behavior from the
  batch-scoped `JoinSet` and semaphore.
- [x] 2026-08-26: Identified the reset/re-extraction singleton race from the
  uploaded worker log and current implementation.
- [x] 2026-08-26: Implemented capacity-driven dispatch; a blocked handler
  remains active while later short jobs fill freed slots, and configured
  concurrency remains bounded.
- [x] 2026-08-26: Implemented startup/reset quiescence and exact dedicated
  re-extraction admission; a prior job at cursor 57 is cancelled and its
  progress is not inherited by the fresh regeneration job.
- [x] 2026-08-26: Added Vault-scoped status counts, terminal history, the
  `/api/v1/jobs/overview` projection, and Chinese active/waiting/history UI.
- [x] 2026-08-26: Fixed Memory-page task loading and automatic polling to use
  the same active projection rather than a bounded recent-history slice.
- [x] 2026-08-26: Completed Rust, frontend, documentation, and diff checks.

## Decisions

- Do not increase `limit=50`, `batch_size`, or `concurrency` to mask the bugs.
  Those values only postpone recurrence.
- Keep terminal history bounded and paged, but treat running jobs as a separate
  operational projection.
- Preserve SQLite leases and cooperative cancellation; do not invent a second
  in-memory queue or forcibly steal leases during reset.

## Surprises and discoveries

- A claimed batch can contain fifteen short jobs and one multi-hour extraction.
  After the short jobs finish, three configured slots remain idle because the
  claim loop is awaiting the last task.
- The existing singleton helper intentionally returns any active job of the
  same type. That behavior is correct for ordinary coalescing but insufficient
  for migration admission, which must verify the exact generation/dedup key.
- Progress updates change `updated_at`, but Admin sorting by `created_at` means
  even frequently progressing jobs can disappear from the recent page.
- The first full workspace test run hit the existing WebDAV concurrent-PUT
  stress test once at 31/32 successful writes. Its focused rerun passed, and a
  second complete workspace run passed all tests, including that case.
- The host `pnpm` command attempted an unsolicited dependency reinstall and
  registry metadata fetch. To preserve the already installed frozen
  dependencies, frontend lint/test/build were executed through the repository's
  local `node_modules/.bin` tools; all completed successfully.

## Validation

Required commands:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
```

Results on 2026-08-26:

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  passed.
- `cargo test --workspace --all-features`: passed on the complete confirmation
  run; all 36 server tests, 9 State background tests, 15 Admin tests, and all
  other workspace/doc tests passed.
- `./node_modules/.bin/eslint .`: passed from `frontend/admin`.
- `./node_modules/.bin/vitest run`: 23 tests passed.
- `./node_modules/.bin/tsc --noEmit` and `./node_modules/.bin/vite build`:
  passed.
- `bash scripts/check-docs.sh`: passed.
- `git diff --check`: passed.

Focused evidence must include:

- one blocked handler plus later short jobs completing before it is released;
- in-flight handlers never exceeding configured concurrency;
- shutdown/cancellation lease recovery still passing;
- an old extraction at a non-zero cursor being cancelled before reset;
- no clearing of `legacy_reextract_pending` for an unrelated active job;
- a fresh dedicated re-extraction starting without inherited progress;
- more than fifty newer terminal jobs not hiding an older running task in the
  Admin response or rendered page.

## Rollback and recovery

No schema migration is planned. Reverting the code restores the previous query
and scheduling behavior without changing persisted jobs. Interrupted jobs keep
their SQLite leases and are reclaimed after expiry or explicit worker lease
release. Legacy reset keeps its persistent pending/version markers, so an
interrupted deployment can safely retry admission after configuration and
incompatible work settle.

## Outcomes

MCP Vault now fills Worker capacity continuously rather than waiting for a
claimed batch's slowest task. Legacy memory reset cannot race old extraction or
consolidation, and only the dedicated fresh full-Vault regeneration clears the
pending migration marker. Admin exposes exact Vault-scoped lifecycle counts,
pins running work independently from bounded history, separates waiting/retry
and terminal sections, and keeps both Jobs and Memory pages current. No schema
migration or public data-plane protocol changed.
