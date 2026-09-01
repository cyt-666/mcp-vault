# Requirements traceability and release evidence

This table is the review index for WP-14. It links externally visible
requirements to behavior and evidence. “Partial/manual” is intentional: it
prevents a route-level unit test from being mistaken for a deployment or GUI
compatibility result.

| Requirement area | Evidence | Status / release condition |
|---|---|---|
| Markdown and explicit memories remain canonical files | Vault Core/storage-fs tests; backup manifest/restore tests; `scripts/interop/http-smoke.sh` | Automated; clean-host restore remains a release gate |
| Every operation is Vault-scoped | Domain/state repository isolation tests; MCP/WebDAV credential endpoint tests; two-Vault fixtures | Automated; any cross-Vault failure blocks release |
| Managed multi-Vault lifecycle and old-client compatibility | Managed concurrent admission/root tests; stable legacy-default State/Admin tests; scoped Admin PAT/connection tests; initialization MCP/WebDAV gates; per-Vault startup/error/job fairness/cancellation tests; index and memory reset isolation; frontend selector/scoping tests; ADR-0020 | Automated code boundary; real-image two-link WebDAV/MCP smoke remains a release gate |
| MCP stateless `2026-07-28`, discovery, headers, caching | MCP tests; `scripts/conformance/mcp.sh`; `tests/conformance/expected-failures.yml` | Automated core gate; baseline is narrow and strict |
| WebDAV safe writes and RFC method behavior | WebDAV crate tests; `scripts/interop/http-smoke.sh`; Litmus wrapper | HTTP smoke automated; Litmus must run before first release |
| Obsidian interoperability | `docs/compatibility-matrix.md`; sanitized DAV request fixture | Manual client/version evidence required |
| Admin/data listener separation | Router tests; HTTP smoke `/api/v1/system` on data listener; deployment proxy checklist | Automated direct boundary; reference proxy check required |
| Simplified Chinese Admin usability | Frontend lint/test/build; authenticated desktop and 390px browser checks; conditional first-Admin setup, reload-safe session restoration, truthful job/index progress, and progressive-disclosure assertions | Automated UI evidence; setup and destructive operations still require backend auth/confirmation |
| Self-contained first-run provisioning | Auth/Admin concurrent password-only first claim, Server installation-key restart/lost-key tests, obsolete-token-variable rejection, and Compose config checks | Automated code/config evidence; clean-host release-image setup remains a release gate |
| Admin network publication versus authentication | Loopback default/config tests; Admin Origin/session/CSRF tests; ADR-0004 and deployment runbook | Source admission is deployment-owned; application Admin authentication remains mandatory |
| Auth separation, OAuth discovery, and secret redaction | Auth/MCP/WebDAV/Admin tests; real-HTTP built-in DCR/login/duplicate-form-retry/code-PKCE/`offline_access`/token/refresh/replay flow and OAuth `create_note`; deterministic full-scope OAuth `tools/list` plus every MCP tool handler; RFC 8414/9728 metadata and challenge tests; local token Vault/resource/scope/180-day sliding rotation, concurrent retry grace, delayed replay, and legacy-grant tests; optional RS256 `aud`/resource and Vault-grant tests; Admin dual-cookie/CSRF reload and logout tests; redacted diagnostics tests | Complete built-in wire boundary and OAuth tool surface automated; Admin and Vault OAuth credentials remain separate, protocol-only `offline_access` grants no MCP permission, session bearer remains HttpOnly, and no secret-log finding is allowed; live ChatGPT reconnect/login remains manual evidence |
| Durable writes, revisions, recovery, and outbox | Vault Core, state, worker, backup/restore tests; WP-13 recovery checks | Automated; crash/recovery failure blocks release |
| Provider outage/degradation | Provider local-fake contract tests; lexical/FTS fallback and readiness tests | Automated; no paid endpoint in CI |
| Ordinary-note semantic retrieval and recall cues | Indexer deterministic chunk/local-fake semantic tests; stale-vector scheduling tests; public MCP related-note and memory-only-scope negative tests; ADR-0013 | Automated; lexical behavior requires no provider and semantic behavior requires an `embedding_note` binding |
| Provider editing/deletion lifecycle | Provider/Admin revision-conflict tests; secret-preserving PATCH and stale-update test; two-Vault binding/vector cascade test; Admin confirmation/count assertions | Automated; canonical notes/memories and durable job/audit history are explicitly retained |
| Provider models and automatic memory | Admin model discovery/manual-registration/role-binding integration; first-class DeepSeek/MiMo/GLM/Kimi/Gemini/Qwen kinds; typed preset/output/token/thinking settings; transport-backed local contracts; Phase 1 semantic-raw/exact-evidence validation and durable no-output coverage; separate Phase 2 binding, prepared proposal, snapshot revalidation, dedup/conflict/forgetting, exact raw-hash commit, and one-active-job-per-Vault; semantic/evidence separation; ADR-0017 destructive prerelease cutover/fresh regeneration; mixed-success continuation and paid cursor; two-Vault isolation; truthful two-stage Admin progress; ADR-0016/0017 | Automated migration, State, Memory, Worker, MCP, Admin, Provider, and UI evidence; each paid official endpoint remains a release-environment check before live-compatibility claims |
| Migration from prior prerelease state | `pre_wp02.sql` upgrade test through migration 0012; focused 0009, 0010, destructive 0011 memory-cutover, additive 0012 OAuth-schema checks, and no-rewrite legacy-default initialization | Automated; preserve fixture and migration checksums |
| Performance regression | `scripts/perf/baseline.sh`; `tests/performance/baseline-policy.json` | Bounded smoke automated; full 10k-note report required for release |
| Threat model | `docs/security.md` verification matrix and security tests | Automated controls plus explicit human review sign-off |
| Image integrity and supply chain | Docker non-root/read-only smoke; SBOM/Trivy CI; release manifest/checksum/signing hooks | Artifact digest/SBOM required; signature required when advertised |
| Operator procedure | `docs/deployment-and-operations.md`; `docs/release-readiness.md` | Manual runbook review required |

The table must be updated when a public route, schema, security control,
migration, or release procedure changes. It is not a substitute for the
underlying tests or for evidence generated against the release image digest.
