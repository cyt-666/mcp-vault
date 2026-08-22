#!/usr/bin/env bash
set -Eeuo pipefail

# Run public-protocol checks against a disposable real HTTP deployment. The
# fixture owns all credentials and data; this script never writes to ./data.

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/mcp-vault-http-smoke.XXXXXX")
manifest="$work_dir/fixture-manifest.json"
fixture_pid=""

cleanup() {
  if [[ -n "$fixture_pid" ]] && kill -0 "$fixture_pid" 2>/dev/null; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

MCP_VAULT_FIXTURE_MANIFEST="$manifest" \
  cargo run --quiet -p mcp-vault-server --bin mcp-vault-fixture \
  >"$work_dir/fixture.log" 2>&1 &
fixture_pid=$!

for _ in $(seq 1 120); do
  if [[ -s "$manifest" ]]; then
    break
  fi
  if ! kill -0 "$fixture_pid" 2>/dev/null; then
    sed -n '1,160p' "$work_dir/fixture.log" >&2
    exit 1
  fi
  sleep 0.25
done

if [[ ! -s "$manifest" ]]; then
  sed -n '1,160p' "$work_dir/fixture.log" >&2
  echo "fixture did not publish a manifest" >&2
  exit 1
fi

mcp_url=$(jq -er '.mcp_url' "$manifest")
webdav_url=$(jq -er '.webdav_url' "$manifest")
health_url=$(jq -er '.health_url' "$manifest")
webdav_user=$(jq -er '.webdav_username' "$manifest")
webdav_password=$(jq -er '.webdav_password' "$manifest")
mcp_origin=$(printf '%s' "$mcp_url" | sed -E 's#(/mcp/.*)$##')

curl --fail --silent --show-error "$health_url" | jq -e '.status == "ready"' >/dev/null

# The public/data listener must not expose control-plane routes.
data_base=${mcp_url%%/mcp/*}
public_admin_status=$(curl --silent --show-error -o /dev/null -w '%{http_code}' \
  "$data_base/api/v1/system")
[[ "$public_admin_status" == "404" ]]

discovery_body='{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"http-smoke","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}'
discovery=$(curl --fail --silent --show-error -X POST "$mcp_url" \
  -H "Origin: $mcp_origin" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'MCP-Method: server/discover' \
  -H "MCP-Name: $mcp_url" \
  --data "$discovery_body")
printf '%s' "$discovery" | jq -e '
  .result.supportedVersions | index("2026-07-28")
' >/dev/null

# Origin policy is checked before the RMCP handler and must reject an
# untrusted browser origin even though the fixture injects its test PAT.
bad_origin_status=$(curl --silent --show-error -o /dev/null -w '%{http_code}' -X POST "$mcp_url" \
  -H 'Origin: https://untrusted.example.invalid' \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'MCP-Method: server/discover' \
  --data "$discovery_body")
[[ "$bad_origin_status" == "403" ]]

note_url="${webdav_url%/}/interop-note.md"
curl --fail --silent --show-error -u "$webdav_user:$webdav_password" \
  -X PUT -H 'Content-Type: text/markdown' \
  --data-binary $'# HTTP smoke\n\nCreated through the real WebDAV listener.\n' \
  "$note_url" >/dev/null

headers="$work_dir/note.headers"
curl --fail --silent --show-error -D "$headers" -o "$work_dir/note.body" \
  -u "$webdav_user:$webdav_password" "$note_url"
grep -F 'Created through the real WebDAV listener.' "$work_dir/note.body" >/dev/null

# A known-stale conditional write must not silently overwrite the note.
stale_status=$(curl --silent --show-error -o /dev/null -w '%{http_code}' \
  -u "$webdav_user:$webdav_password" -X PUT \
  -H 'If-Match: "known-stale-revision"' \
  --data-binary 'must not win' "$note_url")
[[ "$stale_status" =~ ^4[0-9][0-9]$ ]]

curl --fail --silent --show-error -u "$webdav_user:$webdav_password" \
  -X DELETE "$note_url" >/dev/null

# Sync clients issue many independent PUTs at once during an initial mirror.
# Exercise that public HTTP path against file-backed, multi-connection SQLite;
# route-only tests and `sqlite::memory:` cannot reproduce writer contention.
parallel_put_count=50
parallel_status_dir="$work_dir/parallel-status"
mkdir -p "$parallel_status_dir"
parallel_pids=()

for index in $(seq 0 $((parallel_put_count - 1))); do
  relative_path="sync-engine/concurrent/group-$((index % 5))/file-${index}.md"
  put_url="${webdav_url%/}/$relative_path"
  status_file="$parallel_status_dir/$index.status"
  response_file="$parallel_status_dir/$index.response"
  (
    if status=$(curl --silent --show-error -o "$response_file" -w '%{http_code}' \
      -u "$webdav_user:$webdav_password" -X PUT \
      -H 'Content-Type: text/markdown' \
      --data-binary "parallel HTTP payload $index" "$put_url"); then
      printf '%s' "$status" >"$status_file"
    else
      printf 'curl-error' >"$status_file"
    fi
  ) &
  parallel_pids+=("$!")
done

for pid in "${parallel_pids[@]}"; do
  wait "$pid"
done

for index in $(seq 0 $((parallel_put_count - 1))); do
  status=$(<"$parallel_status_dir/$index.status")
  if [[ "$status" != "201" && "$status" != "204" ]]; then
    echo "concurrent WebDAV PUT $index returned $status" >&2
    sed -n '1,20p' "$parallel_status_dir/$index.response" >&2
    exit 1
  fi

  relative_path="sync-engine/concurrent/group-$((index % 5))/file-${index}.md"
  get_url="${webdav_url%/}/$relative_path"
  curl --fail --silent --show-error -u "$webdav_user:$webdav_password" \
    "$get_url" >"$parallel_status_dir/$index.get"
  grep -Fx "parallel HTTP payload $index" "$parallel_status_dir/$index.get" >/dev/null
done

echo "HTTP fixture smoke passed: MCP discovery, Origin rejection, 50 concurrent WebDAV PUTs, revision precondition, and Admin plane separation"
