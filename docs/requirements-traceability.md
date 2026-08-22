# Requirements traceability and release evidence

This table is the review index for WP-14. It links externally visible
requirements to behavior and evidence. “Partial/manual” is intentional: it
prevents a route-level unit test from being mistaken for a deployment or GUI
compatibility result.

| Requirement area | Evidence | Status / release condition |
|---|---|---|
| Markdown and explicit memories remain canonical files | Vault Core/storage-fs tests; backup manifest/restore tests; `scripts/interop/http-smoke.sh` | Automated; clean-host restore remains a release gate |
| Every operation is Vault-scoped | Domain/state repository isolation tests; MCP/WebDAV credential endpoint tests; two-Vault fixtures | Automated; any cross-Vault failure blocks release |
| MCP stateless `2026-07-28`, discovery, headers, caching | MCP tests; `scripts/conformance/mcp.sh`; `tests/conformance/expected-failures.yml` | Automated core gate; baseline is narrow and strict |
| WebDAV safe writes and RFC method behavior | WebDAV crate tests; `scripts/interop/http-smoke.sh`; Litmus wrapper | HTTP smoke automated; Litmus must run before first release |
| Obsidian interoperability | `docs/compatibility-matrix.md`; sanitized DAV request fixture | Manual client/version evidence required |
| Admin/data listener separation | Router tests; HTTP smoke `/api/v1/system` on data listener; deployment proxy checklist | Automated direct boundary; reference proxy check required |
| Self-contained first-run provisioning | Auth/Server concurrent create, restart reuse, lost-key, explicit-file, managed-token cleanup, and local-display tests; Compose config checks | Automated code/config evidence; clean-host release-image setup remains a release gate |
| Admin network publication versus authentication | Loopback default/config tests; Admin Origin/session/CSRF tests; ADR-0004 and deployment runbook | Source admission is deployment-owned; application Admin authentication remains mandatory |
| Auth separation and secret redaction | Auth/MCP/WebDAV/Admin tests; redacted diagnostics tests | Automated; no secret-log finding allowed |
| Durable writes, revisions, recovery, and outbox | Vault Core, state, worker, backup/restore tests; WP-13 recovery checks | Automated; crash/recovery failure blocks release |
| Provider outage/degradation | Provider local-fake contract tests; lexical/FTS fallback and readiness tests | Automated; no paid endpoint in CI |
| Migration from prior prerelease state | `pre_wp02.sql` upgrade test through migration 0009; migration check command | Automated; preserve fixture and checksum |
| Performance regression | `scripts/perf/baseline.sh`; `tests/performance/baseline-policy.json` | Bounded smoke automated; full 10k-note report required for release |
| Threat model | `docs/security.md` verification matrix and security tests | Automated controls plus explicit human review sign-off |
| Image integrity and supply chain | Docker non-root/read-only smoke; SBOM/Trivy CI; release manifest/checksum/signing hooks | Artifact digest/SBOM required; signature required when advertised |
| Operator procedure | `docs/deployment-and-operations.md`; `docs/release-readiness.md` | Manual runbook review required |

The table must be updated when a public route, schema, security control,
migration, or release procedure changes. It is not a substitute for the
underlying tests or for evidence generated against the release image digest.
