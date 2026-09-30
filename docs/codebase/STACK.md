# Technology Stack

## 1) Runtime Summary

| Area | Value | Evidence |
|---|---|---|
| Primary language | Rust 2024 workspace; package MSRV is 1.88 | Cargo.toml:20-25 |
| Runtime + version | Tokio async runtime; repository toolchain pins Rust 1.94.0 | rust-toolchain.toml:1-4; Cargo.toml:55 |
| Package manager | Cargo for Rust; pnpm 11.19.0 for Admin frontend | Cargo.lock; frontend/admin/package.json:1-7 |
| Module/build system | Cargo workspace with 13 crates; React/TypeScript Admin built by Vite and embedded in the server | Cargo.toml:1-18; frontend/admin/package.json:7-13; crates/server/src/lib.rs:1-40 |

## 2) Production Frameworks and Dependencies

| Dependency | Version | Role in system | Evidence |
|---|---|---|---|
| Tokio | 1.x | Async runtime and worker coordination | Cargo.toml:55 |
| Axum | 0.8 | HTTP listeners and Admin/API routing | Cargo.toml:28; crates/server/Cargo.toml:10-25 |
| SQLx + SQLite | SQLx 0.8.6 | Migrations and operational/rebuildable projections | Cargo.toml:52; docs/data-model.md:45-64 |
| RMCP | 3.0.1 | MCP server protocol | Cargo.toml:46; crates/mcp/Cargo.toml |
| Comrak | 0.54.0 | Markdown parsing for source units | Cargo.toml:33; crates/memory/Cargo.toml:9-18 |
| Reqwest | 0.12.28 | Shared outbound HTTP client used by Provider integration | Cargo.toml:62; crates/providers/Cargo.toml:9-22 |
| React / TypeScript | React 19.1.1 / TypeScript 5.9.2 | Admin user interface | frontend/admin/package.json:15-29 |

## 3) Development Toolchain

| Tool | Purpose | Evidence |
|---|---|---|
| rustfmt, Clippy | Rust formatting and lint gates | rust-toolchain.toml:1-4; Makefile:379-386 |
| pnpm, ESLint, Vitest, TypeScript, Vite | Frontend install, lint, test, type-check and build | frontend/admin/package.json:6-13,19-29; frontend/admin/tsconfig.json:1-19 |
| Docker | Multi-stage frontend/Rust build and non-root runtime | Dockerfile:1-37 |
| GitHub Actions | Rust, frontend, repository, protocol, migration and container checks | .github/workflows/ci.yml:11-160 |

## 4) Key Commands

- Full repository gates: make check (Makefile:403-407).
- Rust format: cargo fmt --all --check (Makefile:382-383).
- Rust lint: cargo clippy --workspace --all-targets --all-features -- -D warnings (Makefile:385-386).
- Rust tests: cargo test --workspace --all-features (Makefile:388-390).
- Admin frontend: pnpm --dir frontend/admin lint, test, and build (Makefile:394-401).
- MCP conformance and public HTTP smoke: make conformance and make e2e (Makefile:426-433).

## 5) Environment and Config

- Config sources: process environment parsed by crates/server/src/config.rs; Docker defaults in Dockerfile:24-30; deployment examples in compose.yaml.
- Required environment variables: [TODO] enumerate all optional/required MCP_VAULT_* values from AppConfig::from_lookup without copying secret values (crates/server/src/config.rs:104-176).
- Deployment constraints: the binary serves distinct data and Admin listeners; the Admin bind defaults to loopback, the data listener defaults to port 8080, and SQLite is the configured state store (crates/server/src/config.rs:15-24,35-74).

## 6) Evidence

- Cargo.toml and Cargo.lock
- crates/server/Cargo.toml, crates/memory/Cargo.toml, crates/state/Cargo.toml, crates/providers/Cargo.toml
- frontend/admin/package.json and frontend/admin/tsconfig.json
- rust-toolchain.toml, Makefile, Dockerfile, .github/workflows/ci.yml
- crates/server/src/config.rs
