# Deployed migration checksum compatibility

Owner: Codex. Date: 2026-09-06. Status: completed.

## Purpose and requirements
Restore startup for the supplied schema-17 deployment without rewriting its migration
ledger or business records. Governed by AGENTS.md, PLANS.md, forward-only migration
and portable-data requirements. User provided DB/WAL/SHM for diagnosis; all work uses
private ignored copies. Never start a service with the supplied database or credentials.

## Findings / design / decisions
HEAD 6500214 plus existing uncommitted 0.2.3 work. Applied versions 16 and 17 have
known differing SHA384 values. Compare affected tables/indexes/triggers to a fresh
reference built from unchanged embedded SQL. Whitelist only the two observed hashes;
unknown checksums or non-equivalent schemas remain errors. Preserve original checksums
and use them in a per-run SQLx Migrator only after schema validation. All other normal
migration checks remain enabled. No migration SQL edited, no canonical operations.

## Progress
- [x] Copy DB/WAL/SHM with restrictive permissions; inspect migration and schema only.
- [x] Identify both mismatches; implement exact-hash plus schema compatibility.
- [x] Prove upgrade/restart and unknown/schema-mismatch rejection with synthetic tests.
- [x] Exercise migration only on an isolated supplied copy; check data preservation.
- [x] Document commands/results and upgrade/rollback.

## Risks and recovery
Compatibility must not admit arbitrary schema drift. Reference DDL comparison is
strict except trailing line whitespace. Retain backups including WAL; rollback to
schema 17 requires the pre-upgrade backup because 18/19 add columns/tables. Do not
edit _sqlx_migrations manually. No attached data enters source fixtures or logs.

## Evidence and execution

- Only versions 16 and 17 differ; all earlier applied checksums match current SQL.
  Reference comparison confirmed both resulting schemas, including uniqueness indexes,
  are equivalent under trailing-line-whitespace normalization. Original SQL text for
  the deployed variants is unavailable, so do not claim its exact textual difference.
- `cargo test --offline --locked -p mcp-vault-state`: 51 passed. Tests cover old hashes,
  upgrade to 19, repeat startup, unchanged ledger, unknown checksum rejection, and
  missing calibration-job index rejection before later migrations.
- Initial test harness used an unavailable pool accessor; corrected to an isolated
  SqlitePool. Failed compilation output retained in ignored inspection directory.
- Temporary `check_upgrade` example (source retained only in ignored artifacts) used
  StateStore::connect_and_migrate and a repeat migrate on a SQLite-backup copy of the
  supplied DB/WAL. Both passed, zero foreign-key violations. No server was started.
- SQL EXCEPT comparisons confirmed existing columns/rows unchanged in 12 selected
  tables: encrypted secrets, file metadata/revisions, current memory/sets/snapshots,
  embeddings/vectors, bindings and provider/model definitions. No row values printed.
  Original migration records 1–17 preserved; SQLite integrity_check returned ok.
- Targeted all-feature State Clippy passed. Full workspace checks pending below.
- All sensitive copies, diagnostic harness and logs live under ignored mode-0700
  target/memory-review/migration16-inspect; original attachments were not modified.

## Final validation and outcome

Full workspace all-feature tests: 325 passed, zero failed, using existing isolated
ORT dynamic library environment. `cargo clippy --offline --locked --workspace
--all-targets --all-features -- -D warnings`, `cargo fmt --all --check`,
`bash scripts/check-docs.sh`, and `git diff --check` passed. No frontend changes in
this fix; prior frontend validation remains unchanged. Actual deployment-copy upgrade
and repeated migration passed; operator must rebuild the 0.2.3 image from fixed
source. Docker build/deployment remains operator-owned due to local permissions.
