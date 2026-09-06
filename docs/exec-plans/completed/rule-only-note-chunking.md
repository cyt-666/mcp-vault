# Rule-only note chunking

Owner: Codex. Date: 2026-09-06. Status: completed.

## Purpose / requirements
User explicitly ended model grouping after two real comparisons. Restore deterministic
chunks for preparation, resolution and retrieval. Governed by product retrieval
requirements, architecture, ADR-0026 and amended ADR-0028.

## Scope / current state
HEAD 6500214 with existing uncommitted fixes; preserve unrelated memory, diagnostic,
frontend and migration work. Remove generation and binding UI/API admission. Existing
schema 19 remains compatible; historical bindings/plans remain inert.

## Design / risks / rollback
Use the existing bounded rule chunker everywhere. Ignore legacy plan and pending
records. Valid rule vectors remain current; grouped vector keys become ineligible.
Normal embedding scheduling can fill missing rule vectors, without memory extraction
or full Vault re-extraction. No real model calls in this task. Old reports retained.
Rollback code requires the prior matching build; no database downgrade needed.

## Progress
- [x] Inspect source, Admin roles, existing plans and consumers.
- [x] Remove runtime grouping and configuration; update docs and regression.
- [x] Run relevant Rust/frontend checks and record results.

## Decisions
No destructive SQL migration, no automatic unbinding of unrelated models; legacy
note_chunking settings have no execution path. Diagnostic-only evaluation policy stays.

## Validation / outcomes
Rule-only runtime and Admin UI are active in the isolated local service. All relevant checks passed using documented local environment workarounds.

## Execution / actual checks

- Removed runtime chunk-plan generation/reading and the real paired runner (historical
  reports and saved outputs retained). All note chunk paths use bounded rules.
- Admin no longer admits note_chunking role; UI entry removed. Legacy status counters
  remain zero for response compatibility. Schema 19 repositories/migration retained.
- Extended actual embedding/retrieval integration with legacy binding and malformed
  pending grouping records: exact rule inputs preserved, no pending suppression.
- `cargo test --offline --locked -p mcp-vault-indexer`: initial sandbox run 12 passed,
  2 local-listener permission failures; failure log rule-only-tests.log preserved.
- Permitted local listener: `cargo test --offline --locked -p mcp-vault-indexer -p
  mcp-vault-admin-api`: 39 passed.
- `ORT_LIB_LOCATION=/home/cheng/code/mcp-vault/target/memory-review/followup-tools
  ORT_PREFER_DYNAMIC_LINK=1 LD_LIBRARY_PATH=/home/cheng/code/mcp-vault/target/memory-review/followup-tools
  cargo test --offline --locked --workspace --all-features`: 323 passed, zero failed.
- `cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings`: passed.
- pnpm lint/test/build attempted, wrapper failed opening its store SQLite database;
  all outputs retained in rule-only-ui-*.log. Ran identical installed ESLint, Vitest,
  tsc and Vite binaries directly in frontend/admin: passed, 35 tests.
- `cargo fmt --all --check`, `sh scripts/check-docs.sh`, `git diff --check`: passed.
- `cargo build --offline --locked -p mcp-vault-server --bins`: passed.
- Local isolated service restarted via `sh target/memory-review/local-model.09VRlm/start.sh`;
  readiness returns ready. Database had zero file_entries and zero note_chunk_plans;
  no model requests dispatched by this work. No production deployment or push.
- Browser first attempts failed on absent default Chromium path, then libnss3.so;
  preserved logs. Final run uses existing cache and anaconda library path.

Final browser command: `PLAYWRIGHT_BROWSERS_PATH=/home/cheng/code/mcp-vault/target/memory-review/playwright LD_LIBRARY_PATH=/home/cheng/anaconda3/lib MCP_VAULT_E2E_OUTPUT_DIR=/home/cheng/code/mcp-vault/target/memory-review/rule-only-browser-final python3 scripts/e2e/memory-admin.py`: exit 0, U1–U5, pagination and absence of model-grouping control passed.
