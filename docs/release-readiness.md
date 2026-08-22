# First-release readiness checklist

This checklist is the final review gate for the first complete MCP Vault
release. A checked code path is not enough: every required external or
operational assertion needs an evidence artifact tied to the source revision
and, for container checks, the image digest.

## Required gates

- [ ] No open critical/high security defect; threat-model review is signed off.
- [ ] `cargo fmt`, Clippy, workspace tests, frontend lint/tests/build, docs
      checks, and dependency/license policy pass.
- [ ] Official MCP conformance runs against every advertised revision and the
      fixed target `2026-07-28`; only reviewed, narrow expected failures remain.
- [ ] MCP project-specific tests prove Vault binding, Origin/header validation,
      deterministic tools, structured output, and stateless requests.
- [ ] WebDAV Litmus runs against a real HTTP deployment; its raw output is
      retained separately from sanitized release evidence.
- [ ] The exact supported Hēsperus Sync Engine and/or Remotely Save versions
      pass the manual matrix in `docs/compatibility-matrix.md`.
- [ ] Public HTTP E2E proves MCP discovery/search/read/mutation, WebDAV
      preconditions, provider degradation, and Admin unreachability through
      the reference public proxy.
- [ ] A prior prerelease database fixture upgrades through all migrations and
      passes integrity/foreign-key checks.
- [ ] A clean temporary deployment restores a verified backup, authenticates
      with restored/controlled credentials, reads/writes through DAV and MCP,
      and rebuilds derived projections.
- [ ] Full-scale performance report covers the fixture in
      `docs/development-and-testing.md` section 15; bounded smoke alone is not
      an SLA claim.
- [ ] Image digest, checksum manifest, SBOM, vulnerability result, and—when
      the release is declared signed—signature/attestation verification are
      attached to the same artifact.

Any unchecked item means the release is `not ready`, not “best effort”.

## Repeatable local gates

```bash
make fmt-check
make lint
make test
make frontend-lint
make frontend-test
make frontend-build
make docs-check
bash scripts/interop/http-smoke.sh
bash scripts/conformance/mcp.sh
bash scripts/perf/baseline.sh
```

Run Litmus explicitly with a disposable deployment and credentials:

```bash
MCP_VAULT_WEBDAV_URL='http://127.0.0.1:PORT/dav/v1/vaults/interop/' \
MCP_VAULT_WEBDAV_USERNAME='generated-fixture-user' \
MCP_VAULT_WEBDAV_PASSWORD='generated-fixture-password' \
bash scripts/interop/webdav-litmus.sh basic
```

The wrapper exits with code `2` when Litmus is unavailable. CI/release must
surface that as a blocked gate rather than converting it to a pass.

## Artifact procedure

1. Build the release image once.
2. Record the immutable image digest with `docker image inspect`.
3. Generate an SPDX SBOM for that exact digest and run the HIGH/CRITICAL image
   scan with unfixed findings reported separately.
4. Generate checksums for the release archive, SBOM, manifest, and operator
   documentation from the bytes that will be published.
5. Verify signatures/attestations when the release pipeline declares them;
   private signing material stays in CI/OIDC infrastructure.
6. Run the smoke and restore checks against the same image digest, not a
   later rebuild with an equivalent tag.

## Operator handoff

The handoff must include the image digest, migration version, backup ID and
verification time, managed/explicit master-key handling reminder, first-run
token retrieval check, deployment-owned TLS/listener/firewall/VPN policy (and
proxy review when one is used), health/readiness result, and known unverified
client/provider items. It must not include any secret or note content.
