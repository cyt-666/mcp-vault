# WP-12 Admin API and Web Console

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-21
- Updated: 2026-08-21

## Purpose and user-visible result

Implement the authenticated control plane for MCP Vault. An owner can complete
first-run setup, log in with a secure Admin session, inspect operational health,
configure the single current Vault, manage WebDAV/MCP access and provider
bindings, operate index/memory/jobs/audit surfaces, and use the initial backup
and diagnostics boundaries without editing SQLite or a long-lived config file.
The React console is served only by the control-plane listener and communicates
with the versioned Admin API using session cookies and CSRF protection.

## Governing requirements

- `AGENTS.md`: separate authenticated control plane; protocol adapters call
  application services; no SQL/filesystem/provider access in handlers; secrets
  are redacted; all user-data operations retain `VaultContext`.
- `docs/product-requirements.md` sections 3.7, 3.8, 3.9, 4.1, 4.5, 5, and 6.
- `docs/architecture.md` sections 1, 4.5, 5.3, 6, 7, 9, 13, 14-17.
- `docs/interfaces.md` section 10 and Admin error/health contracts.
- `docs/admin-and-configuration.md` sections 1-20, especially setup,
  configuration hierarchy, page capabilities, and acceptance tests.
- `docs/security.md` sections 2, 4.1, 5, 8.4, 10, 16-21.
- `docs/deployment-and-operations.md` sections 2, 6-9, 11-18.
- `docs/development-and-testing.md` sections 3-8 and 17-18.
- Accepted ADR-0002, ADR-0004, ADR-0005, ADR-0008, ADR-0010, and ADR-0011.

## Current repository state

- `crates/admin-api/src/lib.rs` now exposes both the retained unconfigured
  boundary and the injected authenticated Admin router.
- `crates/server/src/router.rs::control_router_with_admin` mounts the stateful
  API and static assets only on the control listener; `server::run` injects
  shared state/auth/readiness and verified socket peer information.
- `crates/auth` already provides Admin setup/login/session/CSRF/password,
  WebDAV credential issuance/revocation, PAT issuance/revocation, OAuth issuer
  and grant services, and redacted secret types. It does not expose every Admin
  list/update DTO needed by the HTTP surface.
- `crates/state` owns typed repositories for Vaults/settings/auth/providers,
  index/memory projections, jobs/outbox, and files. New SQL remains confined to
  this crate; missing read/update operations must be added here rather than in
  handlers.
- `crates/indexer`, `crates/memory`, and `crates/providers` expose application
  services sufficient for status/rebuild/provider and memory operations.
- `frontend/admin` now has a browser-safe API client, setup/login shell,
  responsive navigation, operational pages, mutation forms, and one-time
  secret display kept only in volatile component state.
- `server/assets.rs` embeds `frontend/admin/dist`; the data router has no Admin
  routes, and existing tests prove listener separation for the unconfigured
  boundary.

## Scope

- Add a stateful, versioned Admin API with typed HTTP DTOs and a redacted,
  consistent error envelope.
- Add setup, login, logout, current-session, password-change, CSRF, strict
  Origin, and control-plane source/CIDR checks.
- Add the API groups defined in `interfaces.md`: dashboard/system/health,
  Vault/rescan, WebDAV credentials, MCP PAT/OAuth/connection info, providers
  and model bindings, index, memory/candidates, jobs, audit, and a safe
  backup/diagnostic application boundary.
- Wire the API into the separate server listener with shared state/auth,
  history/core factories, readiness/worker health, and Admin origins.
- Replace the frontend placeholder with an accessible responsive React console
  covering setup/login and the operational pages using the Admin API.
- Add backend HTTP integration tests, frontend unit tests, and Playwright-ready
  test seams for setup, auth, masking, listener separation, and destructive
  confirmation flows.
- Update operational/interface/development documentation and checksums where
  public behavior changes.

## Non-scope

- Backup/restore engine internals, maintenance-mode swaps, metrics/OpenTelemetry,
  container hardening, and release conformance remain WP-13/WP-14. WP-12 may
  expose explicit job/API seams and safe `not_configured` responses, but must
  not pretend an unimplemented restore is complete.
- Creating or deleting multiple Vaults remains future multi-Vault scope; all
  current Admin calls still resolve the registered Vault and preserve its ID.
- MCP/WebDAV protocol business logic, memory/provider business logic, and
  canonical file writes remain in their existing application services.

## Invariants and risks

- Admin routes/assets exist only on the control listener; the data listener and
  reference public proxy cannot reach them.
- Setup is allowed only before the first Admin user, with a valid one-time
  bootstrap token and allowed source network. The token never enters logs,
  audit payloads, or responses after consumption.
- Every state-changing request requires session, strict Origin/Referer policy,
  CSRF, and a validated content type. Cookies are Secure/HttpOnly/SameSite
  Strict; browser storage never holds bearer sessions.
- Stored secrets, password hashes, request bodies, note/memory bodies, and
  authorization/cookie headers never appear in API responses or logs.
- Vault-specific resources use the registered `VaultContext`; handlers do not
  accept arbitrary `vault_id` and repositories enforce the Vault predicate.
- Long operations enqueue durable jobs with bounded payloads and deterministic
  deduplication; Admin does not block on providers or filesystem scans.
- Destructive operations require explicit confirmation and revision/operation
  preconditions where applicable.

## Proposed design

`mcp-vault-admin-api` becomes a protocol adapter with an injected
`AdminApiState` containing `StateStore`, `AuthService`, `OriginPolicy`,
`AppConfig`/network policy, `Readiness`, and application-service handles. It
uses middleware to validate the socket peer/CIDR, session cookie, CSRF/Origin,
and request body policy, then handlers translate HTTP DTOs to existing
application services. SQL, direct filesystem I/O, provider calls, and memory
projection mutation remain below the adapter.

The first vertical slice uses explicit application helpers in `admin-api` for
the cross-module read/command composition that is not yet owned by a separate
service. New repository methods are added to `state`; secret issuance remains
in `AuthService`; provider changes remain in `ProviderService`; index/memory
changes remain in `IndexService`/`MemoryService`; Core is constructed per
Vault for rescan/rebuild operations.

Responses use:

```json
{"data": {}, "request_id": "..."}
{"error": {"code": "...", "message": "...", "fields": {}}, "request_id": "..."}
```

Secret issuance endpoints return a plaintext secret only in the one-time
command response. List/detail endpoints return configured state and masked
hints. A small frontend API client centralizes credentials, CSRF handling,
request IDs, error summaries, and redirects to login/setup.

## Work breakdown

1. Add missing typed state/auth/provider/job/audit read and update operations;
   prove Vault predicates and redaction with repository tests.
2. Implement Admin API DTOs, error mapping, source/CIDR middleware, setup/login
   sessions, CSRF/Origin, and session/password endpoints.
3. Implement operational API groups and wire stateful control routing into
   `server::run`; test that the data listener has no Admin surface.
4. Implement React setup/login shell, navigation, dashboard/Vault/access/
   provider/index/memory/jobs/audit/system pages, loading/error/confirmation
   states, and responsive/accessibility behavior.
5. Add API/frontend integration tests and Playwright-ready fixtures; update
   docs and checksums.
6. Run all repository checks and archive this plan only after evidence is
   captured.

## Progress

- [x] 2026-08-21 — Re-read root/ordered specifications, Admin/interface/
  security/operations/testing docs, and accepted ADRs.
- [x] 2026-08-21 — Confirm WP-12 is the first unfinished work package and
  create this ExecPlan before implementation.
- [x] 2026-08-21 — Add state/auth/provider/audit/job read and command seams,
  including Vault-scoped job retry and redacted audit queries.
- [x] 2026-08-21 — Implement authenticated Admin API and stateful control
  listener composition with source CIDR, session, CSRF, Origin, setup, and
  control/data separation tests.
- [x] 2026-08-21 — Implement React Admin console shell, browser-safe API client,
  setup/login flow, responsive navigation, operational pages, and durable
  rescan/index/job action seams.
- [x] 2026-08-21 — Add redacted Admin audit writes for successful security and
  operational mutations, plus endpoint tests for source/Origin/CSRF,
  one-time secret masking, audit correlation, and Vault isolation boundaries.
- [x] 2026-08-21 — Update interface/security/operations/testing documentation,
  run the complete Rust/frontend checks, and refresh documentation checksums.

## Decisions

- Admin remains a conventional control-plane HTTP API with a cookie session;
  PATs/OAuth tokens are never accepted as Admin credentials.
- HTTP DTOs remain separate from domain/state records. The API exposes IDs,
  status, timestamps, masked hints, and bounded summaries rather than raw rows.
- Backup/restore endpoints are represented only when an application operation
  exists; an unavailable engine returns an explicit stable error rather than a
  successful placeholder.
- Admin audit append failures are warning-level operational diagnostics in this
  adapter because the underlying mutation service has already committed; the
  request remains free of secret/body logging and the durable audit boundary is
  ready for a future transactionally coupled control-plane service.

## Surprises and discoveries

- The repository has mature lower-level authentication and projection services
  but the Admin crate is intentionally still a stub, so WP-12 requires a
  composition boundary rather than only route declarations.
- The existing server composition creates `MemoryService` for MCP/workers but
  does not retain an injected Admin application state; listener wiring is part
  of this package.

## Validation

    cargo fmt --all --check                         # passed
    cargo clippy --workspace --all-targets --all-features -- -D warnings  # passed
    cargo test --workspace --all-features            # passed
    cargo test -p mcp-vault-memory --all-features -- --test-threads=1  # passed
    pnpm --dir frontend/admin lint                  # passed
    pnpm --dir frontend/admin test                  # passed
    pnpm --dir frontend/admin build                 # passed
    bash scripts/check-docs.sh                      # passed
    shasum -a 256 -c SHA256SUMS                     # passed

Required focused evidence: setup one-time use, CSRF/Origin and verified source
rejection, Admin/data listener separation, secret masking/one-time issuance,
Vault-scoped audit listing, provider-disabled behavior, Vault isolation,
memory candidate review, job retry/cancel, and frontend setup/confirmation
flows. Playwright remains a harness seam because this repository does not yet
contain a browser runner; the external Litmus/MCP conformance runs remain
WP-14 release gates.

## Rollback and recovery

WP-12 adds no destructive schema migration unless a missing operational read
model is proven necessary. Route/UI changes are reversible. If a future Admin
operation enqueues a job, restarting the process leaves it leased/reclaimable;
failed setup/login does not create an Admin identity or expose secrets. Any
new migration must be forward-only, tested from the prior fixture, and backed
by the existing migration recovery procedure.

## Outcomes

- Stateful Admin API now covers setup/session/password, Vault/rescan,
  WebDAV credentials, MCP PAT/OAuth/connection info, Provider/model bindings,
  index, memory/candidates, jobs, audit, system/health, and explicit
  WP-13-gated backup/restore responses.
- The control listener is authenticated, Origin/CSRF protected, source-CIDR
  restricted, and composed separately from the data listener. Protocol
  handlers do not receive raw SQL or filesystem dependencies.
- Provider and access secrets are issued once or represented by masked hints;
  Admin mutations append redacted audit facts with request correlation.
- React console and API client pass lint, unit, TypeScript, and production
  build checks; the UI never stores session or issued secrets in browser
  persistence.
- WP-13 still owns the backup/restore engine, maintenance-mode recovery,
  observability/hardening, and WP-14 owns external MCP/WebDAV conformance and
  interoperability release gates.
