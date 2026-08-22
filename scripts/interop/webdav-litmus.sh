#!/usr/bin/env bash
set -Eeuo pipefail

# Wrapper for the upstream neon Litmus client. The command is intentionally
# strict: a missing Litmus binary is a blocked interoperability gate, never a
# silently passing test.

: "${MCP_VAULT_WEBDAV_URL:?set MCP_VAULT_WEBDAV_URL to the real HTTP WebDAV mount}"
: "${MCP_VAULT_WEBDAV_USERNAME:?set MCP_VAULT_WEBDAV_USERNAME for the test credential}"
: "${MCP_VAULT_WEBDAV_PASSWORD:?set MCP_VAULT_WEBDAV_PASSWORD for the test credential}"

if ! command -v litmus >/dev/null 2>&1; then
  echo "WebDAV Litmus is not installed; interoperability gate is blocked" >&2
  exit 2
fi

suite=${1:-basic}
exec litmus "$MCP_VAULT_WEBDAV_URL" \
  "$MCP_VAULT_WEBDAV_USERNAME" \
  "$MCP_VAULT_WEBDAV_PASSWORD" \
  "$suite"
