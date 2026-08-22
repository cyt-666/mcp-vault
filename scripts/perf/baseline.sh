#!/usr/bin/env bash
set -Eeuo pipefail

# Small, deterministic release smoke for the public listener. This is not a
# claim about the full WP-14 10k-note target; it is a regression tripwire for
# the bounded disposable fixture. The full-scale benchmark remains an
# explicit release activity recorded with host metadata.

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/mcp-vault-perf.XXXXXX")
manifest="$work_dir/fixture-manifest.json"
fixture_pid=""
iterations=${MCP_VAULT_PERF_ITERATIONS:-20}
threshold=${MCP_VAULT_PERF_HEALTH_P95_SECONDS:-0.50}
report=${MCP_VAULT_PERF_REPORT:-$repo_root/target/mcp-vault-perf-report.json}

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
[[ -s "$manifest" ]]

health_url=$(jq -er '.health_url' "$manifest")
mkdir -p "$(dirname "$report")"
timings="$work_dir/health.timings"
: >"$timings"
for _ in $(seq 1 "$iterations"); do
  curl --fail --silent --show-error -o /dev/null -w '%{time_total}\n' "$health_url" >>"$timings"
done

python3 - "$timings" "$threshold" "$iterations" "$report" <<'PY'
import json
import math
import platform
import statistics
import sys
import time
from pathlib import Path

timings = [float(line) for line in Path(sys.argv[1]).read_text().splitlines() if line]
threshold = float(sys.argv[2])
iterations = int(sys.argv[3])
report = Path(sys.argv[4])
if len(timings) != iterations:
    raise SystemExit(f"expected {iterations} timings, got {len(timings)}")
ordered = sorted(timings)
p95 = ordered[min(len(ordered) - 1, math.ceil(len(ordered) * 0.95) - 1)]
payload = {
    "fixture": "disposable-seeded-vault",
    "operation": "GET /health/ready",
    "iterations": len(timings),
    "p50_seconds": statistics.median(timings),
    "p95_seconds": p95,
    "max_seconds": max(timings),
    "threshold_p95_seconds": threshold,
    "timestamp_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "platform": platform.platform(),
}
report.write_text(json.dumps(payload, indent=2) + "\n")
print(json.dumps(payload, indent=2))
if p95 > threshold:
    raise SystemExit(f"health p95 {p95:.6f}s exceeds {threshold:.6f}s")
PY

echo "Performance smoke passed; report: $report"
