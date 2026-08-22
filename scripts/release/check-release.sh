#!/usr/bin/env bash
set -Eeuo pipefail

# Verify a previously-built release image and generate a digest-bound local
# release manifest. This script does not publish or sign anything. A release
# pipeline must provide the image registry/signing credentials out of band.

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${MCP_VAULT_RELEASE_IMAGE:-mcp-vault:release}
artifact_dir=${MCP_VAULT_RELEASE_ARTIFACT_DIR:-$repo_root/target/release-artifacts}
mkdir -p "$artifact_dir"

command -v docker >/dev/null 2>&1 || {
  echo "docker is required for release-check" >&2
  exit 2
}
docker image inspect "$image" >/dev/null

runtime_user=$(docker image inspect "$image" --format '{{.Config.User}}')
stop_signal=$(docker image inspect "$image" --format '{{.Config.StopSignal}}')
image_id=$(docker image inspect "$image" --format '{{.Id}}')
[[ "$runtime_user" != "" && "$runtime_user" != "0" ]]
[[ "$stop_signal" == "SIGTERM" ]]

docker run --rm --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m \
  --entrypoint id "$image" -u >/dev/null

if ! command -v syft >/dev/null 2>&1; then
  echo "syft is required to generate the release SBOM" >&2
  exit 2
fi
if ! command -v trivy >/dev/null 2>&1; then
  echo "trivy is required for the release vulnerability gate" >&2
  exit 2
fi

sbom="$artifact_dir/sbom.spdx.json"
syft "$image" -o "spdx-json=$sbom"
trivy image --severity HIGH,CRITICAL --ignore-unfixed --exit-code 1 "$image"

git_revision=$(git -C "$repo_root" rev-parse HEAD)
manifest="$artifact_dir/release-manifest.json"
python3 - "$manifest" "$image" "$image_id" "$git_revision" "$sbom" <<'PY'
import json
import pathlib
import sys
from datetime import datetime, timezone

manifest = pathlib.Path(sys.argv[1])
manifest.write_text(json.dumps({
    "image": sys.argv[2],
    "image_id": sys.argv[3],
    "git_revision": sys.argv[4],
    "sbom": pathlib.Path(sys.argv[5]).name,
    "generated_at": datetime.now(timezone.utc).isoformat(),
}, indent=2) + "\n")
PY

(cd "$artifact_dir" && shasum -a 256 "$(basename "$manifest")" "$(basename "$sbom")" > SHA256SUMS)

if [[ -n "${MCP_VAULT_SIGNATURE_REF:-}" ]]; then
  command -v cosign >/dev/null 2>&1 || {
    echo "cosign is required when MCP_VAULT_SIGNATURE_REF is set" >&2
    exit 2
  }
  cosign verify "$MCP_VAULT_SIGNATURE_REF" >/dev/null
else
  echo "no signature reference supplied; this local artifact is not a signed release" >&2
  if [[ "${MCP_VAULT_ALLOW_UNSIGNED:-0}" != "1" ]]; then
    echo "set MCP_VAULT_ALLOW_UNSIGNED=1 only for local unsigned smoke; release gates require signature verification" >&2
    exit 2
  fi
fi

echo "release artifact checks passed for $image ($image_id)"
echo "manifest: $manifest"
