# AGENTS.md

## Mission

You are implementing **MCP Vault**, a complete self-hosted Rust service that lets humans use an Obsidian Vault through WebDAV and lets AI Agents use the same knowledge through MCP, including discovery, hybrid retrieval, controlled writes, and long-term memory recall.

Do not reinterpret the project as:

- a demo;
- a thin filesystem MCP wrapper;
- a replacement note editor;
- a custom Obsidian synchronization plugin;
- a database-first proprietary knowledge store.

Read `docs/README.md`, `docs/product-requirements.md`, `docs/architecture.md`, and the documents relevant to the task before editing code.

For work expected to touch several modules or take more than a small focused change, follow `PLANS.md` and maintain an execution plan under `docs/exec-plans/active/`.

## Non-negotiable invariants

### User knowledge remains portable

The current note and attachment content is canonical in the Vault filesystem. Explicit durable memories promoted by the system are also materialized as Markdown.

SQLite is authoritative for operational state such as credentials, configuration, revisions, jobs, and audit records. Search indexes, embeddings, topic projections, and extraction candidates are derived and rebuildable.

Never make an index or vector database the only copy of user knowledge.

### Vault is the isolation boundary

All user-data operations require a `VaultContext`.

Do not create a global Vault singleton or helper shaped like `read(path)` when the operation should be `read(vault_context, path)`.

Every relevant table, job, event, cache key, vector row, audit entry, credential, and query must be Vault-scoped.

The first release may expose one configured Vault, but implementations must not erase the Vault boundary.

### Protocol layers do not own business logic

WebDAV, MCP, and Admin HTTP handlers authenticate, validate, translate, and call application services.

They must not:

- access the filesystem directly;
- execute SQL outside repositories;
- invoke LLM providers directly;
- update indexes directly.

All canonical file mutations go through Vault Core.

### One service, separate planes

Use a modular monolith and one deployable Rust server unless an accepted ADR changes this.

Run separate listeners:

- data plane: MCP and WebDAV;
- control plane: Admin UI and Admin API.

The control plane is LAN-only by deployment and allow-list policy, but still requires authentication.

### MCP is stateless at the protocol boundary

Target MCP specification revision `2026-07-28` with the official Rust SDK and negotiate compatible older revisions supported by the SDK.

Do not put business state in MCP protocol sessions. Bind authorization to a Vault and derive the Vault context from the endpoint and credential. Tools must not accept an arbitrary `vault_id`.

Return tools in deterministic order, use server discovery instructions, validate required HTTP headers and `Origin`, and preserve compatibility through the SDK rather than custom protocol code.

### Memory is transparent and sourced

`recall` is not a synonym for text search. It returns durable context relevant to the current task.

Every durable memory must carry provenance, confidence, temporal validity, lifecycle status, and Vault identity.

LLM output is an untrusted proposal. Validate structured output, deduplicate, detect contradictions, and apply promotion policy before materializing a canonical memory.

Normal recall must not require a live LLM request.

### File writes are safe

All writes must:

- normalize and validate relative paths;
- reject traversal, unsafe symlink traversal, and invalid platform names;
- use preconditions or expected revisions where applicable;
- be atomic;
- create revision history according to retention policy;
- emit durable outbox events;
- survive process restarts through reconciliation.

A WebDAV or Agent write must never silently overwrite a known concurrent change.

### Secrets are never plaintext at rest or in logs

Passwords use Argon2id. High-entropy API tokens are stored as keyed digests with a visible prefix for lookup. Provider secrets are encrypted with an installation master key.

Use redaction-aware types. Never log request authorization headers, passwords, full tokens, API keys, memory contents, or note bodies by default.

## Required architecture direction

Backend workspace modules should preserve these responsibilities even if crate boundaries are adjusted after measurement:

- `domain`: identifiers, value objects, domain errors, permissions;
- `vault-core`: file operations, revisions, history, application services;
- `storage-fs`: safe filesystem implementation;
- `state`: SQLx repositories, migrations, transactional outbox, jobs;
- `webdav`: DAV adapter and authentication;
- `mcp`: RMCP server, tools, resources, discovery instructions;
- `indexer`: Markdown analysis, FTS, links, topics;
- `memory`: extraction, lifecycle, consolidation, recall;
- `providers`: LLM, embedding, reranker adapters;
- `auth`: Admin, WebDAV, PAT, OAuth resource-server validation;
- `admin-api`: control-plane API;
- `server`: composition root, listeners, workers, health;
- `frontend/admin`: React administration console.

Lower-level crates must not depend on protocol or UI crates. Avoid dependency cycles.

## Rust implementation rules

- Rust stable, Tokio, Axum, SQLx, Serde, `tracing`, and `thiserror`.
- Use explicit domain errors in library crates. `anyhow` is acceptable only at binary/task boundaries.
- Keep HTTP DTOs separate from domain models.
- Keep SQL inside repository implementations.
- Do not hold async mutex guards across I/O unless the lock design explicitly requires it.
- Use cancellation-aware background workers and bounded concurrency.
- Use typed configuration and typed secret wrappers.
- Prefer trait abstractions only at real substitution or test boundaries; do not create traits for every struct.
- Pin important dependencies in lockfiles. Wrap pre-1.0 vector extensions behind an internal interface.
- Treat note content, LLM output, WebDAV paths, forwarded headers, and external provider responses as untrusted input.

## Required tests and checks

Before declaring a task complete, run the checks relevant to the changed area:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
```

Also run when relevant:

- WebDAV Litmus and the project’s Obsidian compatibility tests;
- official MCP conformance tests for supported protocol revisions;
- migration tests from a prior database snapshot;
- multi-Vault isolation tests;
- crash-recovery and reconciliation tests;
- provider contract tests using local fakes, never real paid APIs in CI;
- end-to-end tests through the public protocol, not only internal services.

If a command cannot run, state exactly why and preserve the failure output in the task summary.

## Documentation discipline

Update the relevant specification when public behavior, schema, security posture, or operational procedure changes.

Create or amend an ADR for a durable architectural decision. Do not casually override an accepted ADR in code.

Keep `AGENTS.md` concise enough to remain within Codex’s project-instruction budget. Put detail in `docs/`.

## Change workflow

1. Identify the governing requirements and ADRs.
2. Create or update an ExecPlan for substantial work.
3. Implement a complete vertical slice without bypassing boundaries.
4. Add tests before or with behavior.
5. Run formatting, linting, tests, and protocol conformance checks.
6. Update docs, migrations, and examples.
7. Summarize behavior, risks, commands run, and remaining work.

Do not optimize for the fastest visible demo. Optimize for a reliable service that can be operated and evolved for years.

## Code review rules

Flag as blocking:

- any cross-Vault query without an enforced Vault predicate;
- direct filesystem access outside the storage/Vault Core boundary;
- note writes without revision or HTTP precondition handling;
- protocol handlers containing indexing, memory, or provider business logic;
- plaintext secrets, secret logging, or API responses returning stored secrets;
- an LLM response applied without schema validation and policy checks;
- query-time recall that scans the Vault or requires a live LLM;
- admin routes exposed on the public listener;
- non-rebuildable derived indexes;
- deletion without audit/history behavior required by the specification;
- tests that prove only the happy path but not isolation and recovery.
