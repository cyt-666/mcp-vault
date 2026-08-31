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

for _ in $(seq 1 480); do
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
oauth_metadata_url=$(jq -er '.oauth_metadata_url' "$manifest")
oauth_authorization_metadata_url=$(jq -er '.oauth_authorization_server_metadata_url' "$manifest")
oauth_user=$(jq -er '.oauth_username' "$manifest")
oauth_password=$(jq -er '.oauth_password' "$manifest")
webdav_url=$(jq -er '.webdav_url' "$manifest")
health_url=$(jq -er '.health_url' "$manifest")
webdav_user=$(jq -er '.webdav_username' "$manifest")
webdav_password=$(jq -er '.webdav_password' "$manifest")
mcp_origin=$(printf '%s' "$mcp_url" | sed -E 's#(/mcp/.*)$##')

curl --fail --silent --show-error "$health_url" | jq -e '.status == "ready"' >/dev/null

# OAuth discovery is public and must bind the exact MCP resource without
# exposing issuer keys, grants, subjects, or credentials.
oauth_metadata=$(curl --fail --silent --show-error "$oauth_metadata_url")
printf '%s' "$oauth_metadata" | jq -e --arg resource "$mcp_url" --arg issuer "$mcp_origin" '
  .resource == $resource
  and (.authorization_servers == [$issuer, "https://issuer.example.test"])
  and (.bearer_methods_supported == ["header"])
  and (.scopes_supported | index("vault:read"))
  and ((.scopes_supported | index("offline_access")) == null)
  and (has("jwks_cache_json") | not)
  and (has("subjects") | not)
' >/dev/null

authorization_metadata=$(curl --fail --silent --show-error "$oauth_authorization_metadata_url")
oauth_authorization_endpoint=$(printf '%s' "$authorization_metadata" | jq -er '.authorization_endpoint')
printf '%s' "$authorization_metadata" | jq -e --arg issuer "$mcp_origin" '
  .issuer == $issuer
  and .authorization_endpoint == ($issuer + "/oauth/v2/authorize")
  and .token_endpoint == ($issuer + "/oauth/token")
  and .registration_endpoint == ($issuer + "/oauth/register")
  and (.code_challenge_methods_supported == ["S256"])
  and (.token_endpoint_auth_methods_supported == ["none"])
  and (.scopes_supported | index("offline_access"))
  and .authorization_response_iss_parameter_supported == true
' >/dev/null

# A client with cached pre-v2 metadata must be moved to the current endpoint
# without serving another cacheable authorization transaction from the alias.
legacy_authorize_headers="$work_dir/oauth-legacy-redirect.headers"
legacy_authorize_status=$(curl --silent --show-error \
  -D "$legacy_authorize_headers" -o /dev/null -w '%{http_code}' \
  "$mcp_origin/oauth/authorize?cache_probe=1")
[[ "$legacy_authorize_status" == "307" ]]
grep -Eiq '^location: /oauth/v2/authorize\?cache_probe=1\r?$' "$legacy_authorize_headers"
grep -Eiq '^cache-control: .*no-store' "$legacy_authorize_headers"
grep -Eiq '^vary: \*\r?$' "$legacy_authorize_headers"

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

# Exercise the complete standalone ChatGPT-style OAuth flow over the real data
# listener: DCR, login/consent, code + PKCE, MCP bearer use, offline access,
# refresh rotation, retry grace, and replay-family protection. Secrets remain inside this temporary 0600
# fixture directory and are never printed.
oauth_redirect_uri='https://chatgpt.com/connector_platform_oauth_redirect'
registration_payload=$(jq -nc --arg redirect "$oauth_redirect_uri" '{
  client_name: "HTTP smoke ChatGPT",
  redirect_uris: [$redirect],
  grant_types: ["authorization_code", "refresh_token"],
  response_types: ["code"],
  token_endpoint_auth_method: "none"
}')
registration=$(curl --fail --silent --show-error -X POST "$mcp_origin/oauth/register" \
  -H 'Content-Type: application/json' --data "$registration_payload")
oauth_client_id=$(printf '%s' "$registration" | jq -er '.client_id')
printf '%s' "$registration" | jq -e '
  .token_endpoint_auth_method == "none"
  and (has("client_secret") | not)
' >/dev/null

pkce_verifier='http-smoke-pkce-verifier-abcdefghijklmnopqrstuvwxyz0123456789'
pkce_challenge=$(printf '%s' "$pkce_verifier" \
  | openssl dgst -sha256 -binary \
  | openssl base64 -A \
  | tr '+/' '-_' \
  | tr -d '=')
oauth_login_html="$work_dir/oauth-login.html"
oauth_login_headers="$work_dir/oauth-login.headers"
curl --fail --silent --show-error -D "$oauth_login_headers" --get "$oauth_authorization_endpoint" \
  --data-urlencode 'response_type=code' \
  --data-urlencode "client_id=$oauth_client_id" \
  --data-urlencode "redirect_uri=$oauth_redirect_uri" \
  --data-urlencode 'scope=vault:discover vault:read vault:write vault:delete vault:history memory:read memory:write memory:manage offline_access' \
  --data-urlencode 'state=http-smoke-state' \
  --data-urlencode "code_challenge=$pkce_challenge" \
  --data-urlencode 'code_challenge_method=S256' \
  --data-urlencode "resource=$mcp_url" >"$oauth_login_html"
grep -F '授权 ChatGPT 访问 MCP Vault' "$oauth_login_html" >/dev/null
grep -F '<code>offline_access</code>（保持长期连接）' "$oauth_login_html" >/dev/null
grep -F "action=\"$oauth_authorization_endpoint\"" "$oauth_login_html" >/dev/null
grep -Eiq "^content-security-policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'" "$oauth_login_headers"
if grep -Eiq '^content-security-policy: .*form-action' "$oauth_login_headers"; then
  echo 'interactive OAuth page must omit form-action for Chromium compatibility' >&2
  exit 1
fi
request_handle=$(sed -n 's/.*name="request_handle" value="\([^"]*\)".*/\1/p' "$oauth_login_html")
[[ "$request_handle" == mcpv_oauth_req_* ]]

oauth_authorize_headers="$work_dir/oauth-authorize.headers"
authorize_status=$(curl --silent --show-error -D "$oauth_authorize_headers" \
  -o "$work_dir/oauth-authorize.body" -w '%{http_code}' \
  -X POST "$oauth_authorization_endpoint" \
  -H "Origin: $mcp_origin" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode "request_handle=$request_handle" \
  --data-urlencode "resource=$mcp_url" \
  --data-urlencode "username=$oauth_user" \
  --data-urlencode "password=$oauth_password")
[[ "$authorize_status" == "302" ]]
oauth_location=$(awk 'BEGIN{IGNORECASE=1} /^location:/{sub(/^[^:]*:[[:space:]]*/, ""); print}' \
  "$oauth_authorize_headers" | tr -d '\r')
first_oauth_code=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.parse_qs(urllib.parse.urlsplit(sys.argv[1]).query)["code"][0])' "$oauth_location")
oauth_state=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.parse_qs(urllib.parse.urlsplit(sys.argv[1]).query)["state"][0])' "$oauth_location")
oauth_iss=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.parse_qs(urllib.parse.urlsplit(sys.argv[1]).query)["iss"][0])' "$oauth_location")
[[ "$oauth_state" == "http-smoke-state" ]]
[[ "$oauth_iss" == "$mcp_origin" ]]

# A browser/edge retry of the same authenticated form must produce another
# valid code instead of replacing the successful navigation with an expiry
# page. Each code remains independently single-use.
oauth_retry_headers="$work_dir/oauth-authorize-retry.headers"
retry_status=$(curl --silent --show-error -D "$oauth_retry_headers" \
  -o "$work_dir/oauth-authorize-retry.body" -w '%{http_code}' \
  -X POST "$oauth_authorization_endpoint" \
  -H 'Origin: null' \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode "request_handle=$request_handle" \
  --data-urlencode "resource=$mcp_url" \
  --data-urlencode "username=$oauth_user" \
  --data-urlencode "password=$oauth_password")
[[ "$retry_status" == "302" ]]
oauth_retry_location=$(awk 'BEGIN{IGNORECASE=1} /^location:/{sub(/^[^:]*:[[:space:]]*/, ""); print}' \
  "$oauth_retry_headers" | tr -d '\r')
oauth_code=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.parse_qs(urllib.parse.urlsplit(sys.argv[1]).query)["code"][0])' "$oauth_retry_location")
[[ "$oauth_code" != "$first_oauth_code" ]]

token_response=$(curl --fail --silent --show-error -X POST "$mcp_origin/oauth/token" \
  -H 'Origin: https://chatgpt.com' \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=authorization_code' \
  --data-urlencode "code=$oauth_code" \
  --data-urlencode "client_id=$oauth_client_id" \
  --data-urlencode "redirect_uri=$oauth_redirect_uri" \
  --data-urlencode "code_verifier=$pkce_verifier" \
  --data-urlencode "resource=$mcp_url")
oauth_access_token=$(printf '%s' "$token_response" | jq -er '.access_token')
oauth_refresh_token=$(printf '%s' "$token_response" | jq -er '.refresh_token')
printf '%s' "$token_response" | jq -e --arg resource "$mcp_url" '
  .token_type == "Bearer"
  and .resource == $resource
  and (.scope | contains("vault:read"))
  and (.scope | contains("vault:write"))
  and (.scope | contains("offline_access"))
' >/dev/null

oauth_discovery=$(curl --fail --silent --show-error -X POST "$mcp_url" \
  -H "Authorization: Bearer $oauth_access_token" \
  -H "Origin: $mcp_origin" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'MCP-Method: server/discover' \
  --data "$discovery_body")
printf '%s' "$oauth_discovery" | jq -e '.result.supportedVersions | index("2026-07-28")' >/dev/null

oauth_tools_body='{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"http-smoke","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}'
oauth_tools=$(curl --fail --silent --show-error -X POST "$mcp_url" \
  -H "Authorization: Bearer $oauth_access_token" \
  -H "Origin: $mcp_origin" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'MCP-Method: tools/list' \
  --data "$oauth_tools_body")
printf '%s' "$oauth_tools" | jq -e '
  [.result.tools[].name] == [
    "vault_overview", "browse_index", "recent_changes", "search_notes",
    "read_note", "recall", "get_memory", "list_memories", "create_note",
    "edit_note", "move_note", "delete_note", "note_history",
    "restore_note_revision", "remember", "update_memory", "forget_memory"
  ]
' >/dev/null

oauth_create_body='{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"create_note","arguments":{"path":"oauth/http-smoke.md","content":"# OAuth HTTP smoke\n"},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"http-smoke","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}'
oauth_create=$(curl --fail --silent --show-error -X POST "$mcp_url" \
  -H "Authorization: Bearer $oauth_access_token" \
  -H "Origin: $mcp_origin" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'MCP-Method: tools/call' \
  -H 'MCP-Name: create_note' \
  --data "$oauth_create_body")
printf '%s' "$oauth_create" | jq -e '
  .result.isError == false
  and .result.structuredContent.ok == true
  and .result.structuredContent.data.revision.revision == 1
' >/dev/null

refresh_response=$(curl --fail --silent --show-error -X POST "$mcp_origin/oauth/token" \
  -H 'Origin: null' \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=refresh_token' \
  --data-urlencode "refresh_token=$oauth_refresh_token" \
  --data-urlencode "client_id=$oauth_client_id" \
  --data-urlencode 'scope=vault:discover vault:read' \
  --data-urlencode "resource=$mcp_url")
rotated_access_token=$(printf '%s' "$refresh_response" | jq -er '.access_token')
printf '%s' "$refresh_response" | jq -e '
  (.scope | contains("vault:read"))
  and (.scope | contains("offline_access"))
' >/dev/null
curl --fail --silent --show-error -o /dev/null -X POST "$mcp_url" \
  -H "Authorization: Bearer $rotated_access_token" \
  -H "Origin: $mcp_origin" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'MCP-Method: server/discover' \
  --data "$discovery_body"

replay_status=$(curl --silent --show-error -o "$work_dir/oauth-replay.json" -w '%{http_code}' \
  -X POST "$mcp_origin/oauth/token" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=refresh_token' \
  --data-urlencode "refresh_token=$oauth_refresh_token" \
  --data-urlencode "client_id=$oauth_client_id" \
  --data-urlencode "resource=$mcp_url")
[[ "$replay_status" == "400" ]]
jq -e '.error == "invalid_grant"' "$work_dir/oauth-replay.json" >/dev/null
retry_winner_status=$(curl --silent --show-error -o /dev/null -w '%{http_code}' \
  -X POST "$mcp_url" \
  -H "Authorization: Bearer $rotated_access_token" \
  -H "Origin: $mcp_origin" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'MCP-Method: server/discover' \
  --data "$discovery_body")
[[ "$retry_winner_status" == "200" ]]

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

echo "HTTP fixture smoke passed: built-in OAuth code/PKCE/offline refresh flow, duplicate-refresh grace, browser-compatible authorization CSP, token Origin compatibility, legacy authorization redirect, duplicate authorization retry, OAuth metadata, MCP discovery, Origin rejection, 50 concurrent WebDAV PUTs, revision precondition, and Admin plane separation"
