# WP-00 Repository and Build Foundation

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Establish the first buildable MCP Vault modular monolith without weakening the
target architecture. The repository will have a pinned Rust toolchain, an
explicit workspace with the required crate responsibilities, a minimal React
Admin asset pipeline, a task runner, CI checks, a multi-stage container build,
and a server binary that binds separate data and control listeners.

The initial process only exposes safe health responses and explicit
not-implemented protocol boundaries. It does not access Vault files, SQLite,
providers, or protocol business state. Later work packages can replace those
boundary responses with application services without changing listener
composition or dependency direction.

## Governing requirements

- `AGENTS.md`: modular-monolith responsibilities, separate listeners, protocol
  boundary rules, typed configuration, and required checks.
- `docs/product-requirements.md` sections 3.7, 3.8, 4.4, 4.5, and 4.6:
  separate administration/security planes, operational readiness, and pinned
  protocol baselines.
- `docs/architecture.md` sections 1, 4, 5, 14, 15, 16, and 17: two-listener
  composition, crate responsibilities, dependency direction, and observability.
- `docs/implementation-plan.md` section 3 (WP-00): required deliverables and
  acceptance commands.
- `docs/development-and-testing.md` sections 1-5, 7, 16, and 17: toolchain,
  workspace rules, dependency boundaries, CI, and developer commands.
- `docs/deployment-and-operations.md` sections 1-3, 6, 8, and 9: container
  shape, ports, startup/readiness, and redacted structured logging.
- `docs/interfaces.md` sections 1, 2, 10.1, and 11: versioned data/control
  endpoints and health behavior.
- `docs/security.md` sections 2, 4, 16, and 18: separate security planes,
  network exposure, redaction, and container hardening.
- Accepted ADRs 0002, 0004, and 0005: Vault isolation boundary, LAN-only
  control plane, and modular monolith.

## Current repository state

The repository contains the project specification documents and no Rust,
frontend, Docker, CI, migration, or task-runner implementation. `main` is
clean at commit `fca5e7f` and the local toolchain is Rust 1.94.0, Node.js
24.18.0, pnpm 11.19.0, and Docker 29.4.0. There are no existing crate APIs,
migrations, or tests to preserve.

## Scope

### Included

- Pin Rust 1.94.0 and establish a Cargo workspace with the planned crate
  responsibilities.
- Add minimal library boundaries for domain, Vault Core, filesystem storage,
  state, auth, WebDAV, MCP, indexer, memory, providers, and Admin API.
- Add typed bootstrap configuration for data/control binds, data directory,
  secret-file paths, logging, and shutdown settings.
- Add structured tracing initialization and safe liveness/readiness routes.
- Compose separate data and control routers in the server binary; mount the
  WebDAV, MCP, and Admin adapter boundaries without direct file or SQL access.
- Add a React/TypeScript/Vite Admin shell, deterministic frontend checks, and a
  static/embedded asset path used by the control listener.
- Add Make-based developer commands, CI skeleton, dependency/license policy,
  Docker multi-stage build, and documentation/schema checks.
- Add unit and integration tests for configuration validation, health state,
  listener route separation, and the frontend shell.

### Not included

- Domain IDs, `VaultContext`, path normalization, or other WP-01 behavior.
- SQLite schema, SQLx repositories, migrations, or state access (WP-02).
- Filesystem mutations, history, revisions, journal, or recovery (WP-03/WP-04).
- Authentication, OAuth, PATs, WebDAV credentials, or secret encryption
  implementation (WP-05).
- Real WebDAV/MCP protocol behavior, indexing, memory, providers, Admin API,
  or production backup/observability features owned by later packages.

## Invariants and risks

- The control listener defaults to loopback (`127.0.0.1:8081`); it must not be
  nested into the public data router.
- The data listener defaults to `0.0.0.0:8080` and exposes only data-plane
  paths plus public health routes.
- Placeholder handlers must not read the Vault root, execute SQL, call
  providers, or retain protocol session state.
- All future crates must keep dependency direction toward domain/Vault Core;
  protocol adapters cannot become dependencies of lower-level crates.
- Bootstrap configuration contains paths and policy only. It must never load,
  print, serialize, or log secret contents.
- The generated Admin bundle is derived UI output. It is not canonical Vault
  content and must not be placed under a Vault content root.
- The exact selected versions are compatibility anchors, not a claim that
  later protocol work is complete: `rmcp = 3.0.1`, `dav-server = 0.11.0`,
  `comrak = 0.54`, UUIDv7 via `uuid`, and XChaCha20-Poly1305 via
  `chacha20poly1305`.

## Proposed design

### Components and dependency direction

The Cargo workspace declares these crates:

```text
server
├── admin-api
├── mcp
├── webdav
├── indexer
├── memory
├── providers
├── auth
├── vault-core
├── state
├── storage-fs
└── domain
```

WP-00 creates compileable library shells and only the adapter crates expose a
safe fallback router. `server` owns configuration, tracing, listener
composition, health state, graceful shutdown, and Admin asset serving. The
lower-level shells do not depend on Axum or UI code. Later packages add real
traits and application services at the same edges.

Dependency decisions recorded for subsequent packages:

- Rust edition 2024 with minimum supported Rust 1.88 because RMCP 3.x
  requires it; CI/builds use the pinned project toolchain 1.94.0.
- UUIDv7 is the identifier strategy because it preserves time ordering while
  remaining a standard UUID representation; `uuid` will be enabled with its
  `v7` feature in WP-01.
- `chacha20poly1305`'s XChaCha20-Poly1305 is the authenticated-encryption
  strategy for the installation master-key subsystem in WP-05.
- Comrak 0.54 is the CommonMark/GFM AST baseline; Obsidian syntax remains
  project-owned parsing as required by `docs/development-and-testing.md`.
- `dav-server` 0.11.0 is wrapped by the project WebDAV adapter in WP-07 and
  must never receive the raw Vault root directly.
- `rmcp` 3.0.1 is the MCP SDK baseline for WP-08, including 2026-07-28 and
  compatible older revisions through SDK negotiation.

### Data and transaction flow

WP-00 has no canonical-data transaction. On startup, the server parses typed
environment configuration, initializes redacted tracing, binds both listeners,
and marks readiness only after both binds succeed. Health handlers read only an
in-memory readiness state. SIGINT/CTRL-C cancels both listeners through the
same shutdown signal. Later startup work will insert migrations, journal
recovery, Vault validation, and workers before the readiness transition.

### Public interfaces and schema changes

Initial routes are intentionally narrow:

- Data plane: `GET /health/live`, `GET /health/ready`, and versioned fallback
  mounts at `/dav/v1/vaults/{vault_slug}` and `/mcp/v1/vaults/{vault_slug}`.
- Control plane: `/api/v1/*` fallback boundary and a static Admin shell.

The fallback routes return an explicit 501 response and no user data. No
database migration or public tool schema is introduced in WP-00.

### Failure, retry, and recovery

Invalid environment values fail before binding. Binding either listener fails
the process rather than silently collapsing the two-plane model. A readiness
failure returns a non-sensitive 503 JSON response. No retryable durable work is
created in this package; the persistent job/outbox design remains WP-06.

## Work breakdown

1. Create the ExecPlan and record the current empty-repository baseline,
   dependency anchors, and non-scope. Validate the plan against `PLANS.md`.
2. Add Rust toolchain/workspace manifests and all planned crate shells. Validate
   `cargo metadata`, formatting, and workspace compilation.
3. Implement typed server bootstrap configuration, structured tracing, health
   state, separate routers, graceful shutdown, and static Admin asset serving.
   Validate route isolation and configuration tests without binding real ports.
4. Add the React/Vite Admin shell, frontend lockfile, and static asset fallback.
   Validate lint, unit test, and production build.
5. Add Makefile, CI workflow, Docker files, dependency/license policy, and
   docs/schema validation. Validate the local commands that are available.
6. Run the complete WP-00 acceptance checks, update this plan with evidence,
   discoveries, risks, and outcomes. Leave the plan active if any required
   check is blocked rather than claiming completion.

## Progress

- [x] 2026-08-20 — Read `AGENTS.md`, the required product/architecture/
  implementation/plan documents, and WP-00 supporting specifications.
- [x] 2026-08-20 — Inspected the clean repository and confirmed no existing
  implementation, lockfiles, migrations, or tests.
- [x] 2026-08-20 — Recorded dependency and architecture decisions above.
- [x] 2026-08-20 — Created the Cargo workspace, 12 crate shells, pinned
  toolchain, and lockfile. `cargo check --workspace --all-targets` passed.
- [x] 2026-08-20 — Implemented typed bootstrap configuration, structured
  tracing, health state, two listener routers, graceful shutdown, and static
  Admin asset boundary.
- [x] 2026-08-20 — Added React/Vite Admin shell, pnpm lockfile, and build policy
  for esbuild.
- [x] 2026-08-20 — Added Makefile, CI, Docker/Compose, dependency/license
  policy, and documentation/generated-doc checks.
- [x] 2026-08-20 — Completed Rust, frontend, Make, Docker build, and container
  listener smoke validation.

## Decisions

- Use a Makefile as the repository task runner because the environment has no
  `just`, `cargo-nextest`, or `cargo-deny` executable, while `make` is broadly
  available and can wrap the documented commands without hiding them.
- Keep WP-00 protocol crates as explicit, safe 501 boundaries instead of
  adding fake WebDAV/MCP behavior that would create later compatibility debt.
- Keep the Admin bundle outside Vault content and serve the compiled `dist`
  directory through a Rust embedding boundary with a checked-in fallback so
  Cargo builds remain possible before the frontend build runs.

## Surprises and discoveries

- The initial commit contains only documentation; every implementation
  boundary must be introduced in this package.
- The documented MCP target is newer than older RMCP examples. The official
  Rust SDK 3.0.1 uses the 2026-07-28 stateless/discovery model, so WP-08 must
  not copy session-based examples from earlier revisions.
- The local environment does not include optional quality tools such as
  `cargo-deny`, `cargo-nextest`, `cargo-audit`, `cargo-machete`, `sqlx`, or
  `just`; their absence is a tooling limitation, not permission to remove the
  repository policy or CI entry points.
- pnpm 11.19 requires a non-interactive build approval policy for esbuild;
  `frontend/admin/pnpm-workspace.yaml` records `allowBuilds: esbuild: true` so
  frozen installs in CI and Docker remain reproducible.
- The first Vite/Vitest setup resolved two Vite type graphs (7.1.3 and 7.3.6)
  and failed TypeScript config checking. Pinning the direct Vite dependency to
  7.3.6 and using the Vitest type reference removed the conflict.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
make docs-check
make build
```

Expected results: all available Rust and frontend checks pass; `make docs-check`
confirms required specification files, crate directories, and frontend build
metadata; `make build` produces the server binary using the same frontend
asset path as the container. Docker build is attempted separately and its
exact environment failure is recorded if the daemon or registry is
unavailable.

Observed results on 2026-08-20:

- `cargo fmt --all --check`, Clippy with `-D warnings`, and
  `cargo test --workspace --all-features` passed; the server suite contains 13
  boundary/configuration/health tests.
- `CI=true pnpm --dir frontend/admin lint`, `test`, and `build` passed. The
  frontend has one Vitest shell test and a successful Vite 7.3.6 production
  bundle.
- `make docs-check`, `make build`, `cargo run --locked -p mcp-vault-server --
  --check-config`, and `make check` passed.
- `docker build --tag mcp-vault:wp00 .` passed. A temporary container smoke
  returned `{"status":"ok"}` and `{"status":"ready"}` on data-plane health,
  served the Admin HTML on port 8081, and returned HTTP 404 for a data-plane
  MCP path sent to the control listener; the temporary container was removed.
- `cargo-deny`, `cargo-nextest`, `cargo-audit`, `cargo-machete`, `sqlx`, and
  `just` were not installed, so their optional standalone commands were not
  run. The CI/policy entry points remain present.

## Rollback and recovery

WP-00 introduces no database migration and does not mutate user Vault content.
To roll back, remove the new workspace/source/build files in the reviewed
change scope; no runtime data recovery is required. If a frontend build is
interrupted, delete only `frontend/admin/dist` generated outputs and rerun the
frontend build; the checked-in fallback `index.html` keeps Cargo asset
compilation deterministic. If listener startup fails, no readiness state is
published and the process exits without touching a Vault.

## Outcomes

WP-00 ships a buildable modular-monolith foundation with explicit lower-level
and protocol crate boundaries, typed process bootstrap, separate listener
composition, safe health behavior, a LAN-default control plane, embedded Admin
assets, reproducible Rust/frontend lockfiles, CI/task/container entry points,
and tests proving placeholders do not access user data. No later work package
was implemented. WP-01 should add the domain identifiers, `VaultContext`, and
validated Vault paths on the existing `mcp-vault-domain` boundary.
