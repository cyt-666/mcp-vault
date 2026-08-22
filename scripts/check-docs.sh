#!/usr/bin/env bash
set -euo pipefail

required_docs=(
  AGENTS.md
  PLANS.md
  docs/README.md
  docs/product-requirements.md
  docs/architecture.md
  docs/implementation-plan.md
  docs/interfaces.md
  docs/data-model.md
  docs/security.md
  docs/development-and-testing.md
  docs/deployment-and-operations.md
  docs/compatibility-matrix.md
  docs/requirements-traceability.md
  docs/release-readiness.md
)

for path in "${required_docs[@]}"; do
  test -f "$path"
done

crates=(
  domain
  vault-core
  storage-fs
  state
  auth
  webdav
  mcp
  indexer
  memory
  providers
  admin-api
  backup
  server
)

for crate in "${crates[@]}"; do
  test -f "crates/$crate/Cargo.toml"
  test -f "crates/$crate/src/lib.rs" || test -f "crates/$crate/src/main.rs"
done

test -f frontend/admin/package.json
test -f frontend/admin/pnpm-lock.yaml
test -f frontend/admin/pnpm-workspace.yaml
test -f frontend/admin/tsconfig.json
test -f frontend/admin/vite.config.ts
test -d frontend/admin/dist
test -f deny.toml
test -f migrations/0001_operational_state.sql
test -f migrations/0002_operation_idempotency.sql
test -f migrations/0003_auth_security.sql
test -f migrations/0004_background_processing.sql
test -f migrations/0005_index_projections.sql
test -f migrations/0006_provider_vector_state.sql
test -f migrations/0007_memory_state.sql
test -f migrations/0008_backup_catalog.sql
test -f migrations/0009_auth_runtime_hardening.sql
test -f crates/state/tests/fixtures/pre_wp02.sql

grep -q '"build"' frontend/admin/package.json
grep -q '"lint"' frontend/admin/package.json
grep -q '"test"' frontend/admin/package.json

printf '%s\n' 'documentation and workspace checks passed'
