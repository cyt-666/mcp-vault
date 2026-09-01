# Multi-Vault management, isolation, and compatibility

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-09-01
- Updated: 2026-09-01
- Branch: `codex/multi-vault-support`

## Purpose and user-visible result

Enable one MCP Vault installation and one Admin owner to create and operate
several independently isolated Vaults. Each Vault has its own WebDAV and MCP
URLs, credentials, files/history, settings, index/vector projections, memory
pipeline, jobs, and audit context. Existing single-Vault installations retain
their IDs, roots, URLs, credentials, OAuth resources, state, and unscoped Admin
API behavior after upgrade.

## Governing requirements

- `AGENTS.md`: `VaultContext` is mandatory; protocol layers do not own
  business logic; credentials, jobs, projections, memories, and audit are
  Vault-scoped; multi-Vault isolation failures are blocking.
- `docs/product-requirements.md` sections 1, 3.1-3.9, 4.1-4.5, and 6.
- `docs/architecture.md` sections 4.2, 5-7, 9-13, 16, and 17.
- `docs/interfaces.md` sections 2, 3.2, 9, and 10.
- `docs/data-model.md` sections 3-4, 7, 10-14, 16, and 18.
- `docs/admin-and-configuration.md` sections 3, 6-13, and 20.
- `docs/security.md` sections 7.3, 15-16, and 21.
- `docs/deployment-and-operations.md` startup, reconciliation, backup/restore,
  diagnostics, and upgrade sections.
- ADR-0002 and ADR-0008.
- `PLANS.md`.

## Current repository state

- `vaults` already stores stable ID, unique slug/root, reserved root, status,
  and settings revision. `VaultRepository` supports insert/list/find and
  name/status changes but no managed creation workflow or stable legacy
  default selection.
- MCP and WebDAV routes already resolve `/.../vaults/{vault_slug}` and bind
  credentials to the resolved `VaultContext`. Existing protocol isolation
  tests pass.
- Core, storage/history, auth, index, vectors, memory, outbox/jobs, and most
  repositories already accept `VaultContext` or `vault_id`.
- The Admin API uses unscoped routes and `current_vault()`, which returns the
  first row in slug order. Nearly every Vault operation therefore targets one
  implicit Vault even when the registry contains more.
- The React console has no Vault registry/selector and builds every request
  against the unscoped Admin routes.
- Startup recovery aborts the process on any Vault failure and invokes normal
  active-only Core recovery for all statuses. Periodic reconciliation lists
  Vaults dynamically but performs work serially.
- Backup/restore already includes every registered Vault and requires exact
  target topology.

## Scope

### Included

- Managed Vault creation below `<data-dir>/vaults/<slug>`, immutable slugs,
  list/detail/update/rescan, initialization retry, disable, and re-enable.
- A stable typed `legacy_default_vault_id` system setting and compatibility
  aliases for every existing unscoped Vault Admin route.
- Explicit `/api/v1/vaults/{vault_slug}/...` Admin routes and an Admin resolver
  that constructs the context from the path rather than request payloads.
- React Vault list/create/selector behavior and per-Vault connection cards.
- Per-Vault initialization/readiness, startup recovery fault isolation,
  reconciliation scheduling, job admission/cancellation, and status semantics.
- Repository/query/cleanup audit plus regression tests for jobs, index/FTS/
  vectors, memory/generation/consolidation, credentials, files/history, and
  Admin routing.
- Forward-compatible documentation, ADR, migration/upgrade fixtures, and
  complete workspace validation.

### Not included

- Multiple Admin owners, tenant membership, or per-Admin Vault ACLs.
- Attaching arbitrary existing filesystem roots through the new API.
- Vault detach, registry deletion, or canonical-content deletion.
- Slug changes.
- Cross-Vault MCP tools, search, recall, or federation.
- Per-Vault backup/export; backup and restore remain installation-global.

## Invariants and risks

- Existing single-Vault IDs, roots, URLs, credentials, OAuth resources,
  histories, memories, and jobs are never rewritten merely to enable the
  feature.
- New Admin request bodies never accept `vault_id`; the authenticated path slug
  and registry row determine context.
- A Vault credential used at another Vault URL fails before business logic.
- Job row `vault_id`, not payload content, is authoritative. Vault work may not
  run as a global job.
- Every FTS/vector/memory query and destructive rebuild/reset has an enforced
  Vault predicate. Same relative paths and semantic content may coexist.
- A broken/disabled Vault cannot prevent the Admin plane or healthy Vaults from
  starting and serving.
- Managed root creation must reject symlinks, non-directory conflicts, and
  non-empty unregistered roots. Interrupted creation may leave only a reusable
  empty service-managed directory.
- Compatibility aliases must resolve one stable legacy default, never the
  current first row.
- UI switching must discard stale responses and volatile one-time secrets.
- Global backup/restore maintenance remains deliberately process-wide.

## Proposed design

### Components and dependency direction

- Add typed legacy-default and managed-Vault transaction methods in `state`.
- Add a Vault management application service below `admin-api` that validates
  the managed root, coordinates root creation with registry/job state, and
  returns typed lifecycle results. HTTP handlers only authenticate, resolve,
  validate DTOs, and map results.
- Extend `server` workers with `vault.initialize` and per-Vault status/admission
  behavior. Protocol adapters consume shared Vault availability resolution.
- Keep MCP/WebDAV tool schemas and business services unchanged.

### Data and transaction flow

1. Validate `name` and `slug`; calculate `<data-dir>/vaults/<slug>` server-side.
2. Safely create the directory, or reuse only an empty non-symlink orphan from
   an interrupted attempt.
3. In one State transaction insert the Vault, enqueue one `vault.initialize`
   job, and persist the legacy default only when none exists. The authenticated
   Admin boundary appends the same redacted best-effort audit fact used by
   existing control-plane operations.
4. Return `202` with Vault summary and initialization job. Data-plane access is
   unavailable until initial scan/index state is complete.
5. The worker resolves context from the job row, performs initial
   reconciliation/index/embedding admission, initializes the current memory
   generation state, and exposes ready/error state.
6. Disable cancels/parks Vault-derived work and closes its data-plane routes;
   re-enable admits reconciliation and any required index/memory recovery.

No user content is migrated. Existing Vault rows become ready through their
existing successful scan/index evidence; the upgrade does not enqueue a
destructive rebuild solely because multi-Vault management was enabled.

### Public interfaces and schema changes

- Add `GET/POST /api/v1/vaults`.
- Add `GET/PATCH /api/v1/vaults/{vault_slug}`.
- Add `POST /api/v1/vaults/{vault_slug}/rescan` and
  `/initialization/retry`.
- Nest Vault-owned Admin resources below `/api/v1/vaults/{vault_slug}/...`.
- Retain all old unscoped routes as compatibility aliases to the typed legacy
  default. If no unique legacy default can be established, return
  `409 vault_selection_required`.
- Vault responses expose `availability` (`initializing`, `ready`,
  `maintenance`, `disabled`, `error`) and an optional initialization job
  summary while retaining existing fields.
- Use existing `system_settings`, `vaults`, jobs, scan checkpoints, and memory
  state; add only forward migrations proven necessary by the implementation.

### Failure, retry, and recovery

- Managed-directory creation is idempotent for an empty orphan. A failed State
  commit never claims a non-empty directory.
- Initialization jobs use renewable leases and deterministic Vault-scoped
  deduplication. Retry never changes Vault identity or root.
- Startup recovers each registered Vault through an internal recovery permit;
  an ambiguous Vault becomes `error` while other Vaults continue.
- Disabled Vaults remain registered and backed up. Re-enable performs a fresh
  reconciliation before writes resume when readiness is stale.
- Worker dispatch is Vault-fair and skips/parks work for unavailable Vaults
  without spending attempts in a busy loop.

## Work breakdown

1. Add this ExecPlan and the multi-Vault lifecycle ADR/spec updates; capture
   baseline branch and focused tests.
2. Add State support for legacy default resolution and atomic managed Vault
   admission, including concurrent-create and upgrade tests.
3. Add Vault management service, lifecycle/readiness DTOs, scoped Admin router,
   compatibility aliases, and backend integration tests.
4. Add `vault.initialize`, startup recovery isolation, status gating, fair
   reconciliation/job behavior, and focused failure tests.
5. Audit index/vector/memory/job repositories and workers; add two-Vault tests
   for every read/rebuild/reset/cleanup boundary found.
6. Add Admin Vault registry, creation flow, selector, path-scoped requests,
   stale-response protection, and per-Vault connection cards/tests.
7. Update requirements, architecture, interfaces, data model, security,
   Admin, operations, compatibility, traceability, and release documentation.
8. Run formatting, linting, full workspace/frontend tests, public protocol
   smoke/conformance checks, migration/backup tests, and archive this plan only
   when all required evidence passes.

## Progress

- [x] 2026-09-01 — Confirm clean `main`, create and switch to
  `codex/multi-vault-support`, and re-read governing documents.
- [x] 2026-09-01 — Inspect the current registry, Admin implicit-first routing,
  protocol Vault resolution, startup loops, backup topology, and existing
  two-Vault tests.
- [x] 2026-09-01 — Implement atomic State admission, stable legacy default,
  safe managed-root checks, initialization availability, and concurrent/upgrade
  tests without a schema or content migration.
- [x] 2026-09-01 — Implement scoped Admin dispatch for every Vault-owned group,
  preserve unscoped compatibility aliases, expose per-Vault connection cards,
  and test credential/list/update/lifecycle isolation.
- [x] 2026-09-01 — Implement isolated startup recovery, durable
  `vault.initialize`, terminal error state, status-aware Worker admission,
  Vault-fair claims, queued periodic reconciliation, and MCP/WebDAV readiness
  gates. Existing index/memory reset/rebuild isolation tests remain green.
- [x] 2026-09-01 — Implement and validate the Admin Vault selector, managed
  creation form, URL-scoped API client, stale-response/secret remount boundary,
  availability/retry UI, and connection-specific pages.
- [x] 2026-09-01 — Add ADR-0020 and update requirements, architecture,
  interfaces, data model, security, Admin, operations, compatibility,
  traceability, release gates, and checksums.
- [x] 2026-09-01 — Complete final post-documentation validation and archive
  this ExecPlan.

## Decisions

- One Admin owner manages all Vaults; this is not a tenant authorization
  project.
- New Vaults are service-managed only. Existing registered roots remain valid.
- Slugs are immutable endpoint identities.
- Disable/re-enable is the only removal lifecycle in this work.
- Existing unscoped Admin routes remain compatibility aliases to a stable
  legacy default instead of becoming lexicographic or immediately failing when
  a second Vault is added.
- MCP/WebDAV continue to use distinct path-based endpoints and Vault-bound
  credentials; ordinary tool schemas remain unchanged.

## Surprises and discoveries

- Normal Core recovery requires an active Vault, while server startup currently
  calls it for every registry row. A disabled Vault can therefore prevent
  restart even before multi-Vault management is enabled.
- The data plane and most worker handlers already resolve Vaults dynamically;
  the largest product gap is the implicit Admin selection rather than router
  composition.
- Origin-root OAuth protected-resource metadata intentionally becomes
  ambiguous with several active Vaults; exact per-Vault metadata URLs remain
  the compatibility contract.
- Managed admission needed the registry row and initialization job in one
  transaction; separate repository calls would briefly expose a new active
  Vault as ready. `VaultAvailability` now gates that window structurally.
- The existing Admin handlers could be reused safely by an authenticated
  scoped dispatcher that reconstructs a fresh inner request and injects a
  per-request selected state clone. Resource ID path parameters therefore keep
  their old DTOs and never share mutable global selection.
- The local pnpm wrapper refuses dependency cleanup without a TTY; setting
  `CI=true` runs the project scripts non-interactively. Direct TypeScript,
  ESLint, Vitest, and Vite checks produced the same passing result.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
CI=true pnpm --dir frontend/admin lint
CI=true pnpm --dir frontend/admin test
CI=true pnpm --dir frontend/admin build
bash scripts/check-docs.sh
bash scripts/release/check-migrations.sh
bash scripts/interop/http-smoke.sh
bash scripts/conformance/mcp.sh
shasum -a 256 -c SHA256SUMS
```

Also run focused two-Vault Admin, credential, Core/storage/history, jobs,
index/vector, memory, startup recovery, and backup/restore tests. External
Litmus and manual Obsidian checks remain explicit release evidence when the
required client/tool is available.

Expected results: no cross-Vault read/write/query/rebuild/reset/cancel path;
old single-Vault URLs and credentials remain valid; one bad Vault does not
degrade another; all required commands pass or their exact external blocker is
recorded.

Final results on 2026-09-01:

- Rust formatting, Clippy with warnings denied, and all workspace/all-feature
  tests passed.
- Admin ESLint, 28 Vitest tests, TypeScript checking, and the production Vite
  build passed.
- Documentation/workspace checks, SHA-256 verification, migration fixtures,
  and `git diff --check` passed.
- The real public HTTP fixture passed built-in OAuth, MCP/WebDAV compatibility,
  control/data plane separation, and 50 concurrent WebDAV writes.
- Official MCP core conformance passed the reviewed compatibility baseline.
  Its one known cache scenario for the intentionally unsupported
  `prompts/list` method remains the documented baseline rather than a
  multi-Vault regression.
- External Litmus and manual Obsidian checks were not run because their
  clients are not available in this workspace; they remain release-time
  evidence. Existing WebDAV integration tests and the real HTTP fixture pass.

## Rollback and recovery

The feature is developed only on `codex/multi-vault-support`. Before merge,
rollback is branch deletion. Runtime changes are forward-compatible and do not
rewrite canonical content. If managed creation is interrupted before State
commit, only an empty directory may remain and the same slug retry can safely
reuse it. Registered Vaults are never deleted by this work. Any forward
migration is additive/idempotent and must pass the prior-release fixture before
merge. A failed initialization leaves the Vault registered in an observable
error state with explicit retry; it cannot affect healthy Vaults.

## Outcomes

Implementation and validation are complete on `codex/multi-vault-support`.
The Admin console now creates and selects managed Vaults, and every Vault has
distinct WebDAV/MCP links backed by Vault-bound credentials. The compatibility
aliases resolve a persisted legacy default, so adding another Vault does not
change old URLs or rewrite existing IDs, roots, credentials, OAuth resources,
history, memory, or jobs. Initialization, recovery, scheduling, cancellation,
indexing, and memory work are Vault-scoped; an unavailable Vault does not stop
healthy Vaults or the Admin plane. No SQL migration or canonical-content
rewrite was required. All automated validation, real HTTP smoke, and official
MCP core conformance checks passed, with the external-client release evidence
listed above remaining explicit.
