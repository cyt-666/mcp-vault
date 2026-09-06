#!/usr/bin/env bash
set -Eeuo pipefail

# Run the official MCP server conformance suite against the real fixture.
# The pinned source revision is used because published npm releases can lag
# the advertised specification. Build the checkout with its lockfile; npx git
# package installation is broken in some npm GitFetcher versions. Override
# MCP_VAULT_CONFORMANCE_PACKAGE with a reviewed immutable package/ref in CI.

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/mcp-vault-mcp-conformance.XXXXXX")
manifest="$work_dir/fixture-manifest.json"
fixture_pid=""
package_ref=${MCP_VAULT_CONFORMANCE_PACKAGE:-}
conformance_revision=74edef34d674f563537be8c6587cebaa58e830ca
spec_version=${MCP_VAULT_CONFORMANCE_SPEC_VERSION:-2026-07-28}
requirements=${MCP_VAULT_CONFORMANCE_REQUIREMENTS:-}
if [[ -n "${MCP_VAULT_CONFORMANCE_SCENARIOS:-}" ]]; then
  scenarios=$MCP_VAULT_CONFORMANCE_SCENARIOS
else
  case "$spec_version" in
    2026-07-28)
      scenarios='server-stateless tools-list resources-list http-header-validation dns-rebinding-protection caching'
      ;;
    2025-11-25)
      scenarios='tools-list resources-list dns-rebinding-protection'
      ;;
    2025-06-18)
      scenarios='tools-list resources-list'
      ;;
    2025-03-26|2024-11-05)
      echo "Pinned official suite has no applicable default server scenarios for $spec_version; this gate is blocked, not passed." >&2
      echo "This command cannot certify SDK compatibility for that revision." >&2
      exit 2
      ;;
    *)
      echo "unknown MCP_VAULT_CONFORMANCE_SPEC_VERSION: $spec_version" >&2
      echo "set MCP_VAULT_CONFORMANCE_SCENARIOS explicitly for a reviewed revision" >&2
      exit 2
      ;;
  esac
fi
expected_failures=${MCP_VAULT_CONFORMANCE_EXPECTED_FAILURES:-$repo_root/tests/conformance/expected-failures.yml}
output_dir=${MCP_VAULT_CONFORMANCE_OUTPUT_DIR:-$work_dir/results}

cleanup() {
  if [[ -n "$fixture_pid" ]] && kill -0 "$fixture_pid" 2>/dev/null; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  if [[ "$output_dir" == "$work_dir"/* ]]; then
    rm -rf "$work_dir"
  fi
}
trap cleanup EXIT INT TERM

if [[ -z "$package_ref" ]]; then
  package_ref="$work_dir/official-conformance"
  git init --quiet "$package_ref"
  git -C "$package_ref" fetch --quiet --depth=1 https://github.com/modelcontextprotocol/conformance.git "$conformance_revision"
  git -C "$package_ref" checkout --quiet --detach FETCH_HEAD
  npm --prefix "$package_ref" ci
fi

# Compilation is not part of the listener-startup timeout.
cargo build --quiet -p mcp-vault-server --bin mcp-vault-fixture

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
mkdir -p "$output_dir"

if [[ ! -f "$expected_failures" ]]; then
  echo "expected-failures file does not exist: $expected_failures" >&2
  exit 1
fi

echo "Running official MCP conformance package $package_ref"
echo "Target protocol version: $spec_version"
echo "Results directory: $output_dir"
if [[ -n "$requirements" ]]; then
  echo "Target requirements: $requirements"
  npx --yes "$package_ref" server \
    --url "$mcp_url" \
    --requirements "$requirements" \
    --expected-failures "$expected_failures" \
    --output-dir "$output_dir" \
    --verbose
else
  for scenario in $scenarios; do
    echo "Running official scenario: $scenario"
    scenario_log="$work_dir/scenario.log"
    npx --yes "$package_ref" server \
      --url "$mcp_url" \
      --scenario "$scenario" \
      --spec-version "$spec_version" \
      --expected-failures "$expected_failures" \
      --output-dir "$output_dir/$scenario" \
      --verbose 2>&1 | tee "$scenario_log"
    if grep -q '^SKIPPED:' "$scenario_log"; then
      echo "Official scenario $scenario was skipped; compatibility gate is blocked." >&2
      exit 2
    fi
  done
fi

echo "Official MCP conformance passed; sanitized results are in $output_dir"
