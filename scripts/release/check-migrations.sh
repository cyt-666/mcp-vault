#!/usr/bin/env bash
set -Eeuo pipefail

# Upgrade the committed pre-WP-02/prerelease fixture through the current
# forward-only migration set. The fixture is copied into a temporary database
# by the Rust test and is never modified in place.

cargo test -p mcp-vault-state empty_pre_wp02_fixture_upgrades_through_embedded_migration -- --nocapture
cargo test -p mcp-vault-state migration_creates_operational_tables_and_integrity_is_green -- --nocapture
cargo test -p mcp-vault-state migration_0009_clears_legacy_jwks_and_adds_key_verifier_state -- --nocapture
cargo test -p mcp-vault-state migration_0010_adds_codex_style_two_phase_memory_state -- --nocapture
cargo test -p mcp-vault-state migration_0011_discards_only_prerelease_memory_state_and_jobs -- --nocapture
cargo test -p mcp-vault-state migration_0012_adds_builtin_oauth_without_changing_existing_credentials -- --nocapture
cargo test -p mcp-vault-state migration_0013_fail_closes_legacy_note_sources_without_losing_memory -- --nocapture
echo "migration fixture checks passed"
