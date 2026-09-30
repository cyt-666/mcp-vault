# Codebase Structure

## 1) Top-Level Map

| Path | Purpose | Evidence |
|---|---|---|
| crates/ | Rust workspace modules for domain, storage, protocol adapters, indexing, memory, providers, APIs and server | Cargo.toml:1-18; docs/architecture.md:656-709 |
| migrations/ | Ordered SQLx database migrations | docs/data-model.md:1003-1042; crates/state/src/migrations.rs |
| frontend/admin/ | React/TypeScript administration interface | frontend/admin/package.json:1-30 |
| docs/ | Product, architecture, interface, data, security, operational and ADR specifications | docs/README.md:32-51 |
| scripts/ and tests/ | Protocol, interoperability, migration, performance, fixture and test support | Makefile:403-439; .github/workflows/ci.yml:73-118 |
| deploy/ and compose.yaml | Deployment configuration | compose.yaml; docs/deployment-and-operations.md |

## 2) Entry Points

- Main runtime entry: crates/server/src/main.rs:5-53.
- Server composition root: crates/server/src/lib.rs; run(config) initializes state, recovery, services, listeners and workers (crates/server/src/lib.rs:150-220).
- Secondary CLI paths: initialize-memory --inspect, initialize-memory --discard-legacy-memory and --check-config (crates/server/src/main.rs:10-36).
- Test fixture binary: crates/server/src/bin/mcp-vault-fixture.rs.
- Entry selection: default binary is mcp-vault; Cargo declares the bin in crates/server/Cargo.toml:52-57.

## 3) Module Boundaries

| Boundary | What belongs here | What must not be here | Evidence |
|---|---|---|---|
| domain | Vault identity/context, paths, revisions, permissions and errors | SQL, HTTP or filesystem implementation | docs/architecture.md:128-157; crates/domain/src/ |
| vault-core / storage-fs | Safe canonical file operations and filesystem primitives | MCP/Admin business logic | docs/architecture.md:158-215; docs/development-and-testing.md:91-100 |
| state | SQLx repositories, migrations, jobs and projections | Protocol DTOs or provider HTTP | docs/architecture.md:216-229; docs/development-and-testing.md:91-100 |
| memory / indexer / providers | Memory and retrieval application services; Markdown analysis; outbound model/embedding adapters | Direct protocol routing or independent Provider HTTP clients | docs/architecture.md:299-318,544-591; docs/provider-compatibility.md:11-16 |
| mcp / webdav / admin-api | Authenticated protocol adapters and translation to application services | Direct SQL or canonical file writes | docs/architecture.md:230-298; docs/development-and-testing.md:91-100 |
| server | Composition root, two listeners and persistent worker registration | Domain policy owned by protocol handlers | crates/server/src/lib.rs:150-220,330-370 |

## 4) Naming and Organization Rules

- Rust modules/files use snake_case examples such as memory_units, source_health and provider_compatibility; crates use kebab-case names (crates/memory/src/v3/service.rs; Cargo.toml:2-16).
- Frontend source uses TypeScript/TSX filenames and Vite configuration (frontend/admin/src/pages.tsx; frontend/admin/vite.config.ts:1-18).
- Architectural groups are crate-by-responsibility, with feature-specific submodules under state and memory (crates/state/src/units/; crates/memory/src/v3/).
- Rust formatting uses rustfmt style edition 2024; module visibility is explicit with pub use exports (rustfmt.toml:1-2; crates/memory/src/lib.rs:1-7).
- Import aliases: [TODO] no workspace-wide path alias system found; Rust crate paths use Cargo dependencies and relative module declarations.

## 5) Evidence

- Cargo.toml:1-18
- docs/architecture.md:128-318,656-709
- crates/server/src/main.rs and crates/server/src/lib.rs
- crates/memory/src/lib.rs and crates/state/src/lib.rs
- docs/README.md:32-51
