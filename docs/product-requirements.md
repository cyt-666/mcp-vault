# Product Requirements

## 1. Product definition

MCP Vault is a self-hosted service that exposes one canonical Markdown Vault to both people and AI Agents.

A person uses Obsidian and an existing WebDAV synchronization plugin. An Agent uses MCP. The service adds indexing, knowledge discovery, controlled mutation, revision history, and long-term memory without replacing ordinary files with a proprietary note database.

The first supported deployment is a single owner and a single configured Vault. The architecture, schema, credentials, and APIs must preserve a Vault isolation boundary so one service can support multiple Vaults later without redesigning core modules.

## 2. Primary users

### Vault owner

The owner deploys the service, connects Obsidian, configures MCP clients, chooses LLM and embedding providers, reviews what the system remembers, monitors jobs, and restores data.

### AI Agent

An Agent discovers the Vault’s broad contents, recalls relevant long-term context, retrieves exact source material, and performs authorized note or memory changes.

### Administrator

For the first release, the Vault owner and administrator are the same human. Administration remains a separate security plane.

## 3. Required functional capabilities

### 3.1 Obsidian and WebDAV

The service MUST:

- expose the full Vault content root through standard WebDAV;
- preserve arbitrary folder layouts rather than requiring `notes/` and `attachments/`;
- support Markdown and binary attachments;
- support the DAV methods and HTTP preconditions needed by tested Obsidian WebDAV clients;
- authenticate with separate, revocable, Vault-scoped WebDAV credentials;
- produce stable metadata such as ETags and modification times;
- preserve `.obsidian/` files while excluding them from semantic indexing by default;
- work with an existing Obsidian plugin rather than requiring a custom plugin;
- document that client-side WebDAV encryption must be disabled when server-side indexing and AI access are desired.

Primary compatibility target: the WebDAV backend of the current Hēsperus Sync Engine. Remotely Save remains a secondary compatibility target.

### 3.2 Vault discovery

An Agent MUST be able to understand what the Vault contains without guessing keywords or receiving a flat list of every file.

The service MUST provide:

- a compact Vault overview;
- a navigable knowledge index with stable node identifiers;
- high-level topic summaries and note counts;
- recent activity;
- note outlines and metadata;
- links, backlinks, tags, and related-note relationships.

The index may combine deterministic structure and optional LLM enrichment, but must remain a virtual view. It MUST NOT reorganize user files without an explicit mutation.

### 3.3 Retrieval

The service MUST provide:

- full-text search;
- semantic search when an embedding provider is enabled;
- hybrid ranking;
- scoped search by path, topic, tag, type, and time;
- full note reads;
- section/outline reads that avoid loading an entire long note unnecessarily;
- provenance in results;
- bounded output with pagination or cursors.

Retrieval MUST continue to work lexically when LLM or embedding providers are unavailable.

### 3.4 Controlled mutation and history

Authorized Agents MUST be able to:

- create a note;
- replace content with an expected revision;
- apply a patch;
- append content;
- move or rename a file;
- delete according to configured retention policy;
- inspect revision history;
- restore an earlier revision.

All writes MUST be atomic, auditable, revision-aware, and recoverable. A known concurrent modification MUST produce a conflict rather than silent overwrite.

### 3.5 Long-term memory

The service MUST make Vault content usable as durable Agent memory.

It MUST provide:

- proactive-recall instructions through MCP server discovery;
- `recall` with hybrid ranking and task context;
- `remember` for explicit durable memories;
- memory types including preferences, decisions, constraints, facts, projects, progress, events, relationships, and procedures;
- provenance, confidence, importance, temporal validity, and lifecycle status;
- automatic memory extraction from ordinary notes when configured;
- schema validation, deduplication, contradiction detection, and promotion policy;
- user inspection, editing, merging, rejection, archival, and deletion;
- canonical Markdown materialization for active durable memories;
- recall that does not require a live LLM call.

The service cannot force every MCP Host to invoke recall. It MUST make correct recall behavior likely through server instructions, clear tool descriptions, a compact memory-context resource, and low latency.

### 3.6 LLM and model providers

The owner MUST be able to configure and test:

- one or more LLM providers;
- one or more embedding providers;
- optional local embedding models;
- role-specific model bindings such as extraction, summarization, topic enrichment, and optional reranking;
- global defaults with future per-Vault overrides;
- provider timeouts, concurrency, retry, and privacy policy.

The service MUST support at least:

- OpenAI-compatible HTTP endpoints;
- an Anthropic-compatible adapter;
- local OpenAI-compatible endpoints such as Ollama or vLLM;
- remote embedding endpoints;
- a local embedding implementation behind an optional feature.

Provider failure MUST NOT block WebDAV, normal Vault writes, lexical search, or explicit memory access.

### 3.7 Administration console

The service MUST provide a browser-based administration console for:

- first-run setup and admin password creation;
- Vault status and storage location;
- WebDAV credentials;
- MCP access credentials and OAuth resource-server settings;
- permissions and revocation;
- LLM, embedding, and model-role configuration;
- index status and rebuild;
- memory browser and candidate review;
- background jobs and provider health;
- audit logs;
- backup, restore, and retention configuration;
- system version, migrations, and diagnostics.

The first-release console MUST use Simplified Chinese for operator-facing
copy, group navigation by common tasks, show page-specific summaries instead
of raw JSON by default, and progressively disclose advanced OAuth,
restore/recovery, and diagnostic controls.

Password-creation forms MUST state the effective minimum and rejected-default
policy beside the input. Validation errors MUST repeat actionable requirements
and MUST NOT require operators to infer hidden uppercase, number, or symbol
rules that the service does not enforce.

Before authentication, the console MUST show first-Admin creation only while
the service reports that no Admin exists. Once initialization is complete it
MUST show login without a registration control. This UI state does not replace
the server-side atomic one-time setup guard. First-run setup accepts only the
desired Admin username and password; it MUST NOT require an operator-generated
or manually copied bootstrap token.

The Admin UI and Admin API MUST run on a separate listener that is not publicly exposed by default. Network restriction does not replace authentication.

### 3.8 Authentication and authorization

The service MUST keep three independent security domains:

- Admin authentication;
- WebDAV authentication;
- MCP authorization.

The service MUST support high-entropy, Vault-bound personal access tokens for trusted clients.

It MUST also support standards-aligned MCP authorization as an OAuth 2.1 resource server using configured authorization-server metadata, protected-resource metadata, resource indicators, issuer/audience validation, and scopes.

MCP tools MUST derive the Vault from the authenticated endpoint and credential. Tools MUST NOT accept an arbitrary Vault identifier.

### 3.9 Indexing and background processing

The service MUST:

- perform an initial scan;
- observe server writes immediately;
- reconcile out-of-band filesystem changes;
- maintain FTS, metadata, link, topic, memory, and embedding projections;
- use a durable job queue and transactional outbox;
- retry transient failures safely;
- expose job state and failures;
- allow independent rebuild of derived projections.

### 3.10 Backup and recovery

The service MUST support consistent backup and restore of:

- Vault content;
- operational SQLite state;
- revision/history blobs;
- installation metadata needed to decrypt provider secrets.

The master encryption key MUST be backed up separately and must not be silently included in ordinary downloadable backups unless the owner explicitly requests an encrypted key export.

## 4. Non-functional requirements

### 4.1 Data integrity

- A successful canonical write must survive process restart.
- A crash between filesystem and database updates must be detected and reconciled.
- No protocol path may bypass Vault Core mutation rules.
- All destructive operations must be represented in audit and history according to policy.

### 4.2 Portability

- Copying the Vault content root must yield a usable ordinary Obsidian Vault.
- Removing the service must not require an export conversion.
- Internal managed Markdown must be documented and editable.

### 4.3 Privacy

- Remote AI processing is opt-in.
- Include/exclude path policy must be enforced before provider requests.
- Note text and memory content must not appear in logs by default.
- The owner must be able to operate without a remote LLM.

### 4.4 Performance targets

For a reference personal deployment with 10,000 notes, 50,000 attachments, and 100,000 memory/index records:

- metadata lookup p95: under 100 ms;
- lexical search p95: under 300 ms;
- hybrid recall p95 excluding remote reranking: under 750 ms;
- WebDAV metadata listing must be paged/streamed and memory-bounded;
- canonical writes must not wait for LLM or embedding work;
- background workers must use bounded concurrency.

Targets are engineering objectives and must be measured with Chinese and English fixtures.

### 4.5 Operability

- One Docker Compose command starts a fresh service, MCP Vault generates its
  installation key automatically, and the browser completes initialization
  with only the desired Admin username and password.
- Health and readiness endpoints reflect database, storage, migrations, and worker state.
- Migrations are forward-only, tested, and backed up before upgrade.
- Logs are structured and redact secrets.

### 4.6 Compatibility

- Target MCP revision: 2026-07-28.
- Negotiate earlier revisions supported by the official Rust SDK.
- Pass official MCP conformance suites for supported revisions.
- Pass WebDAV Litmus for implemented RFC 4918 features.
- Pass the project’s Obsidian Sync Engine and Remotely Save interoperability suite.

## 5. Explicit boundaries

The complete service does not require:

- a custom Obsidian plugin;
- an in-browser note editor;
- SaaS multi-tenancy;
- public Admin UI exposure;
- cross-Vault recall;
- real-time collaborative editing;
- mandatory knowledge-graph visualization;
- client-side encrypted WebDAV content that the server can still index.

Future work may add these only without breaking the accepted boundaries.

## 6. Completion criteria

The service is complete for the first release when:

1. Obsidian can synchronize notes and attachments bidirectionally through a tested existing WebDAV plugin.
2. Concurrent writes are detected, revision history is available, and recovery tests pass.
3. An MCP client can authenticate, discover server instructions, explore the Vault index, search, read, and perform authorized edits.
4. The MCP implementation passes conformance for supported revisions.
5. An Agent can `remember` a decision, see it materialized as Markdown, and `recall` it later from a semantically related task.
6. Automatic extraction can create reviewable candidates and safely promote configured high-confidence memories.
7. The owner can configure and test LLM and embedding providers from the LAN-only console.
8. Provider outages leave WebDAV, file writes, lexical search, and existing memory recall operational.
9. Indexes can be deleted and rebuilt without loss of canonical knowledge.
10. Backup and restore reproduce Vault content, credentials/configuration, revision history, and service operation.
11. Isolation tests prove every credential and query is bound to one Vault context.
12. Security, migration, crash-recovery, Litmus, MCP conformance, and end-to-end tests pass.
