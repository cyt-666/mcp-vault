#!/usr/bin/env bash
set -euo pipefail

source_dir="${1:-data}"
source_dir="$(cd "$source_dir" && pwd -P)"
source_db="$source_dir/state/mcp-vault.sqlite3"
source_key="$source_dir/secrets/master-key"

if [[ ! -f "$source_db" || ! -f "$source_key" ]]; then
  echo "source data directory must contain state/mcp-vault.sqlite3 and secrets/master-key" >&2
  exit 1
fi
if [[ "$source_dir" == *"'"* ]]; then
  echo "source data path containing a single quote is not supported" >&2
  exit 1
fi

replay_dir="$(mktemp -d "${TMPDIR:-/tmp}/mcp-vault-phase2-replay.XXXXXX")"
replay_dir="$(cd "$replay_dir" && pwd -P)"
if [[ "$replay_dir" == *"'"* ]]; then
  echo "temporary replay path containing a single quote is not supported" >&2
  exit 1
fi
mkdir -p "$replay_dir/state"
touch "$replay_dir/.phase2-replay"

for directory in secrets vaults history; do
  if [[ -d "$source_dir/$directory" ]]; then
    cp -R "$source_dir/$directory" "$replay_dir/$directory"
  fi
done
sqlite3 "$source_db" ".timeout 10000" ".backup '$replay_dir/state/mcp-vault.sqlite3'"

external_roots="$(sqlite3 "$replay_dir/state/mcp-vault.sqlite3" \
  "SELECT COUNT(*) FROM vaults WHERE content_root NOT LIKE '$source_dir/%';")"
if [[ "$external_roots" != "0" ]]; then
  echo "refusing replay because a Vault root is outside the source data directory" >&2
  echo "MCP_VAULT_PHASE2_REPLAY_DATA_DIR=$replay_dir"
  exit 1
fi
sqlite3 "$replay_dir/state/mcp-vault.sqlite3" \
  "UPDATE vaults SET content_root = replace(content_root, '$source_dir', '$replay_dir');
   -- A copied file has a new inode. Keep byte/hash verification active, but
   -- do not compare the isolated copy with the source Vault's inode identity.
   UPDATE file_entries SET filesystem_identity = NULL;"

echo "MCP_VAULT_PHASE2_REPLAY_DATA_DIR=$replay_dir"
if [[ "${MCP_VAULT_PHASE2_REPLAY_PREPARE_ONLY:-0}" == "1" ]]; then
  exit 0
fi
if cargo run -p mcp-vault-server --example memory_phase2_replay -- "$replay_dir"; then
  if [[ "${MCP_VAULT_PHASE2_REPLAY_KEEP:-0}" == "1" ]]; then
    echo "MCP_VAULT_PHASE2_REPLAY_PRESERVED=$replay_dir"
    exit 0
  fi
  if [[ ! -f "$replay_dir/.phase2-replay" ]]; then
    echo "refusing to clean a replay directory without its sentinel" >&2
    exit 1
  fi
  rm -rf -- "$replay_dir"
  echo "MCP_VAULT_PHASE2_REPLAY_CLEANED=$replay_dir"
else
  replay_exit_code=$?
  echo "MCP_VAULT_PHASE2_REPLAY_PRESERVED=$replay_dir" >&2
  exit "$replay_exit_code"
fi
