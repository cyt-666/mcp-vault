# External Integrations

## 1) Integration Inventory

| System | Type | Purpose | Auth model | Criticality | Evidence |
|---|---|---|---|---|---|
| SQLite | Embedded database | Operational state, migrations, projections, jobs and audit | Local process/database access | High | crates/server/src/config.rs:15-19; docs/data-model.md:45-64,543-595 |
| Vault filesystem | Canonical local data store | Markdown notes, attachments, managed memory artifacts and revision data | Resolved VaultContext and filesystem policy | High | docs/architecture.md:81-111,158-215 |
| LLM / embedding / reranking Providers | Outbound HTTP APIs or configured local endpoints | Optional extraction, embeddings and ranking | Provider config and encrypted secret injection | High for semantic extraction; optional for base service | docs/provider-compatibility.md:5-16; docs/security.md:487-514 |
| MCP clients | Inbound protocol | Agent discovery, note operations and current memory reads/writes | Vault-bound PAT/OAuth authorization | High | docs/interfaces.md:143-173,269-319; docs/security.md:169-305 |
| Obsidian/WebDAV clients | Inbound protocol | Human sync to canonical Markdown Vault | WebDAV credentials and Vault binding | High | docs/product-requirements.md:34-49; docs/interfaces.md:60-142 |
| External OAuth issuer | Optional inbound token validation | Compatibility mode for externally issued OAuth tokens | Configured issuer metadata and grants | Optional | docs/security.md:275-293 |
| OTLP collector | Optional telemetry export | Export redacted tracing when configured | Endpoint configuration | Optional | crates/server/src/config.rs:71-74,223-238; docs/security.md:673-675 |

## 2) Data Stores

| Store | Role | Access layer | Key risk | Evidence |
|---|---|---|---|---|
| Vault roots | Canonical note and managed Markdown content | Vault Core and storage-fs | Path traversal, symlinks and concurrent writes | docs/architecture.md:81-91,158-215; docs/security.md:348-450 |
| SQLite | Authoritative operational records plus replaceable projections | crates/state repositories | Missing vault predicate or accidental mutation of derived/canonical boundaries | docs/architecture.md:93-126,216-229; docs/security.md:605-635 |
| Embedding/vector state | Derived retrieval data, rebuildable from canonical content/config | providers/indexer/state | Stale or cross-Vault results | docs/architecture.md:113-126,597-604; docs/data-model.md:869-935 |
| Secret files | Installation master key outside SQLite; Provider secrets are encrypted | server startup and Provider service | Key backup/ownership and leakage | docs/security.md:307-346 |

## 3) Secrets and Credentials Handling

- Credential sources: Admin-managed Provider configuration and credentials; installation master key defaults under the data directory or may use MCP_VAULT_MASTER_KEY_FILE (docs/security.md:319-327; crates/server/src/config.rs:47-50,142).
- Provider secrets use authenticated encryption; SQLite stores ciphertext and key verification metadata rather than plaintext secrets (docs/security.md:307-327; docs/architecture.md:97-109).
- Provider API authorization is injected by the shared transport; redirects to another host are denied and credentials are not forwarded cross-origin (docs/provider-compatibility.md:11-16; docs/security.md:499-514).
- Hardcoded secrets: [TODO] this mapping did not run a repository secret scanner; do not treat absence of examples as proof that no secret exists.
- Secret rotation/lifecycle is described in docs/security.md:335-346 and Provider delete behavior in docs/product-requirements.md:149-152.

## 4) Reliability and Failure Behavior

- Provider calls are subject to timeouts, body/request bounds, bounded concurrency, redirect/SSRF checks, redaction and cost-safe retry boundaries (docs/provider-compatibility.md:11-16; docs/architecture.md:554-589).
- Provider failure must not block normal Vault writes, lexical search or explicit memory access (docs/product-requirements.md:183).
- Durable jobs/outbox are stored in State and are Vault-scoped (docs/architecture.md:423-474; docs/data-model.md:511-629).
- [TODO] Live endpoint reachability and current account/model configuration were not tested for this mapping task.

## 5) Observability for Integrations

- Logging uses tracing; optional OTLP export is disabled unless configured (crates/server/src/config.rs:26-33,71-74; docs/security.md:673-675).
- Worker and Provider failures have stable redacted categories; logs must not contain Provider prompts/responses, secrets, or memory bodies (docs/security.md:637-650; docs/architecture.md:554-589).
- Missing visibility gap for this project: current semantic-memory evaluation has no completed A/B/C report; existing v3 quality evidence does not verify the new plan (docs/semantic-memory-implementation-plan.md:630-739; docs/codebase/.codebase-scan.txt:457-499).

## 6) Evidence

- crates/server/src/config.rs and crates/providers/src/transport.rs
- docs/provider-compatibility.md:1-31,168-180
- docs/security.md:69-107,307-346,487-514,605-675
- docs/data-model.md:45-64,543-629,869-935
- docs/architecture.md:77-126,423-474,544-617
