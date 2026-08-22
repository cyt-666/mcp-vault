# MCP Vault

MCP Vault is a self-hosted, Markdown-native knowledge and long-term memory server.

It gives the same Vault two first-class interfaces:

- **Human interface:** Obsidian through standard WebDAV synchronization.
- **Agent interface:** Model Context Protocol (MCP) tools and resources for discovery, retrieval, memory recall, and controlled mutation.

The project is implemented as a Rust modular monolith with an embedded web administration console.

## Product intent

MCP Vault is not another note editor and is not merely a filesystem MCP server. It is intended to be durable personal knowledge infrastructure:

- the user owns ordinary Markdown files and attachments;
- Obsidian remains the primary human editing experience;
- Agents can understand what the Vault contains before knowing exact search terms;
- relevant preferences, decisions, facts, constraints, and project state can be recalled naturally;
- LLM and embedding providers are configurable and replaceable;
- one service is structurally ready to isolate multiple Vaults, even when the first release exposes only one configured Vault;
- administration is a separate, LAN-only control plane.

## Canonical data boundaries

The phrase “Markdown is the source of truth” applies specifically to **user knowledge content**.

| Data class | Canonical storage |
|---|---|
| Notes, attachments, explicit durable memories | Files under the Vault content root |
| Admin accounts, credentials, configuration, revisions, audit records, job state | SQLite operational state |
| FTS indexes, embeddings, topic projections, extracted candidates | Rebuildable derived state |

Operational state must be backed up. Derived state must be rebuildable.

## Repository documentation

Start with:

1. [`AGENTS.md`](AGENTS.md) — binding instructions for Codex and other coding agents.
2. [`docs/README.md`](docs/README.md) — document map and reading order.
3. [`docs/product-requirements.md`](docs/product-requirements.md) — required product behavior and completion criteria.
4. [`docs/architecture.md`](docs/architecture.md) — component boundaries and data flows.
5. [`docs/implementation-plan.md`](docs/implementation-plan.md) — complete-service implementation work packages.

The documents describe the intended complete service. They do not authorize a throwaway MVP architecture.

## Intended deployment

A production deployment exposes two network planes:

```text
Internet or trusted reverse proxy
    ├── WebDAV data endpoint
    └── MCP agent endpoint

Localhost / LAN / VPN only
    └── Admin UI and Admin API
```

The data plane must be protected by TLS when it leaves the host. The admin listener is separate and is not routed publicly.

## Technology direction

- Rust stable, Tokio, Axum
- Official Rust MCP SDK (`rmcp`)
- SQLite with SQLx and FTS5
- `dav-server` behind a custom Vault-backed filesystem adapter
- React, TypeScript, Vite, TanStack Query, and Ant Design for the admin console
- Pluggable LLM, embedding, and vector-index providers
- Docker-first deployment

Exact dependency versions belong in lockfiles and may be updated after compatibility and conformance testing.

## Development foundation

The repository uses Rust 1.94.0 and a Make-based task runner. The initial
server binds the data plane on `0.0.0.0:8080` and the control plane on
`127.0.0.1:8081`; protocol handlers are explicit boundaries until their work
packages are implemented.

Run the foundation checks with:

```bash
make check
```

The Admin frontend is under `frontend/admin/` and owns its `pnpm-lock.yaml`.
Cargo embeds the built `frontend/admin/dist/` bundle while keeping it outside
the canonical Vault content root. Dependency license and source policy is in
`deny.toml`.
