#!/usr/bin/env bash
set -Eeuo pipefail

# Exercise real predecessor migrations and exclusive v27-to-v3 discard/resume.
# Whole test targets avoid silently passing a removed name with zero tests.
cargo test --locked -p mcp-vault-state --lib
cargo test --locked -p mcp-vault-memory --test memory_initialization
echo "migration and offline cutover checks passed"
