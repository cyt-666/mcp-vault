# Testing Patterns

## 1) Test Stack and Commands

- Primary backend test framework: Rust built-in unit/integration test harness with Tokio async tests; SQLx/SQLite tests use migrations (Cargo.toml:52,55; docs/development-and-testing.md:217-283).
- Assertion/mocking tools: standard assert macros, tempfile-backed filesystem/SQLite fixtures, injected Provider services and local fake HTTP endpoints (crates/memory/tests/memory_v3.rs:21-59; docs/development-and-testing.md:418-433).
- Frontend runner: Vitest with jsdom and globals enabled (frontend/admin/vite.config.ts:14-18).
- Commands:
  - Full gates: make check.
  - Rust unit/integration: cargo test --workspace --all-features.
  - Format/lint: cargo fmt --all --check; cargo clippy --workspace --all-targets --all-features -- -D warnings.
  - Frontend: pnpm --dir frontend/admin lint; pnpm --dir frontend/admin test; pnpm --dir frontend/admin build.
  - Protocol and smoke: bash scripts/conformance/mcp.sh; bash scripts/interop/http-smoke.sh; bash scripts/interop/webdav-litmus.sh.
  - Evidence: Makefile:382-433; .github/workflows/ci.yml:28-105.

## 2) Test Layout

- Rust unit tests appear in crate source modules; crate-level integration suites are in crates/*/tests/ (for example crates/memory/tests/memory_v3.rs and crates/state/tests/repositories.rs).
- Test fixtures live in crate tests/fixtures and root tests/fixtures directories (crates/memory/tests/fixtures/memory-quality/selection-v3.json; docs/development-and-testing.md:437-449).
- Admin frontend tests are under frontend/admin/src and run through Vitest (frontend/admin/package.json:7-13; frontend/admin/vite.config.ts:14-18).
- Setup commonly constructs temporary Vault roots, VaultContext values, StateStore, VaultCore and test Provider boundaries (crates/memory/tests/memory_v3.rs:21-59).

## 3) Test Scope Matrix

| Scope | Covered? | Typical target | Notes |
|---|---|---|---|
| Unit | Yes | Parsing, validation, ranking, permissions, errors | docs/development-and-testing.md:219-247 |
| Repository integration | Yes | Real SQLite migrations and Vault A/Vault B isolation | docs/development-and-testing.md:249-254 |
| Application integration | Yes | Temporary filesystem + SQLite, jobs, recovery and Provider fakes | docs/development-and-testing.md:255-283,418-433 |
| Protocol / E2E | Yes | MCP conformance, HTTP smoke, optional WebDAV Litmus | .github/workflows/ci.yml:73-118 |
| New semantic-memory acceptance | No, pending | A/B/C and real-model card quality | docs/semantic-memory-implementation-plan.md:630-739; current fixtures cover source selection only |

## 4) Mocking and Isolation Strategy

- Provider calls are tested with local fake HTTP servers and injected services; CI must not use billable APIs (docs/development-and-testing.md:418-433; docs/provider-compatibility.md:168-180).
- Repository tests use real SQLite migrations. The testing guide requires every Vault-data repository to include two-Vault isolation coverage (docs/development-and-testing.md:249-254).
- Existing memory tests exercise current-source invalidation, source-set publication and cross-Vault rejection (crates/memory/tests/memory_v3.rs:327-464,657-687,961-1058).
- Common risk for the new work: a semantic Card supported by several Sources must not be visible when any required private dependency is unreadable; the plan explicitly requires permission propagation before downstream cleanup (docs/semantic-memory-implementation-plan.md:503-507,630-679).

## 5) Coverage and Quality Signals

- Coverage tool + threshold: [TODO] no coverage command or enforced percentage found in Makefile or CI.
- Current reported coverage: [TODO] not collected during this mapping task.
- Existing source-selection fixture and live-eval examples are not semantic-card evidence: crates/memory/tests/fixtures/memory-quality/selection-v3.json; crates/server/examples/live_memory_v3_eval.rs; docs/semantic-memory-implementation-plan.md:683-708.
- Real Provider or production testing was not run for this document task.

## 6) Evidence

- Makefile:382-433
- .github/workflows/ci.yml:28-118
- docs/development-and-testing.md:217-283,385-489
- crates/memory/tests/memory_v3.rs
- frontend/admin/vite.config.ts and frontend/admin/package.json
- docs/semantic-memory-implementation-plan.md:630-739
