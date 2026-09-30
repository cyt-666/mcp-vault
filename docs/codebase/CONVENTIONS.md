# Coding Conventions

## 1) Naming Rules

| Item | Rule | Example | Evidence |
|---|---|---|---|
| Rust files/modules | snake_case | memory_v3.rs, source_health.rs | crates/memory/src/v3/service.rs; crates/state/src/units.rs |
| Functions and methods | snake_case | extract_note, search_fts | crates/memory/src/v3/service.rs:2086-2099; crates/state/src/units.rs:538-545 |
| Rust types | PascalCase | MemoryService, VaultContext | crates/memory/src/v3/service.rs:108-115; crates/domain/src/vault.rs |
| Constants/env vars | SCREAMING_SNAKE_CASE; env names use MCP_VAULT_ prefix | EXTRACTION_PIPELINE_VERSION; MCP_VAULT_DATA_BIND | crates/memory/src/v3/service.rs:53-57; crates/server/src/config.rs:15-24 |

## 2) Formatting and Linting

- Formatter: rustfmt with edition/style edition 2024 (rustfmt.toml:1-2).
- Rust linter: Clippy, workspace warnings denied in the documented gate (Makefile:385-386).
- Frontend linter/type checker: ESLint flat config with recommended TypeScript rules; TypeScript strict mode is enabled (frontend/admin/eslint.config.js:1-10; frontend/admin/tsconfig.json:1-18).
- Commands: cargo fmt --all --check; cargo clippy --workspace --all-targets --all-features -- -D warnings; pnpm --dir frontend/admin lint; pnpm --dir frontend/admin build (Makefile:382-401).

## 3) Import and Module Conventions

- Rust imports are explicit crate/module paths; public module surfaces use deliberate mod visibility and pub use exports (crates/memory/src/lib.rs:1-7; crates/memory/src/v3/mod.rs:1-23).
- Frontend imports use ES modules; no custom tsconfig path aliases are declared (frontend/admin/tsconfig.json:10-19).
- HTTP/MCP DTOs, validated commands/queries, domain models and output DTOs should be distinct (docs/development-and-testing.md:170-177).
- Avoid a trait for every struct; traits are expected at real storage/provider/repository/vector substitution boundaries (docs/development-and-testing.md:153-164).

## 4) Error and Logging Conventions

- Library/domain errors use typed thiserror enums; errors are translated at protocol boundaries and raw SQL/I/O/provider/crypto details must not be returned to clients (docs/development-and-testing.md:112-130; Cargo.toml:54).
- Structured application logging uses tracing; JSON is the default runtime log format and Pretty is available for local development (crates/server/src/config.rs:26-33,92-99; crates/server/src/lib.rs:130-147).
- Logs omit note/memory bodies, credentials, Provider payloads and sensitive headers by default (docs/security.md:637-650; docs/architecture.md:710-725).

## 5) Testing Conventions

- Rust unit tests are colocated or in crate tests/ directories; frontend tests use Vitest and jsdom (crates/memory/tests/memory_v3.rs; frontend/admin/vite.config.ts:14-18).
- Provider integration tests use local fake HTTP servers; CI does not use paid API keys (docs/development-and-testing.md:418-433; docs/provider-compatibility.md:168-180).
- Every repository with Vault data should test two-Vault isolation (docs/development-and-testing.md:249-254).
- Coverage threshold: [TODO] no enforced coverage threshold was found in workspace configuration or CI scan.

## 6) Evidence

- rustfmt.toml, Cargo.toml, Makefile
- docs/development-and-testing.md:91-177,217-283
- crates/memory/src/lib.rs, crates/memory/src/v3/mod.rs
- frontend/admin/eslint.config.js, frontend/admin/tsconfig.json, frontend/admin/vite.config.ts
- docs/security.md:637-675
