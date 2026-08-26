# System Architecture

## 1. Architectural style

MCP Vault is a **modular monolith** delivered as one Rust server binary plus embedded admin frontend assets.

This choice gives the project:

- one deployment unit;
- one configuration and migration boundary;
- simpler consistency between filesystem and SQLite;
- direct in-process application calls without coupling domain logic to protocols;
- the option to split workers or protocol frontends later because module boundaries and durable queues are explicit.

The system runs two independent HTTP listeners:

```text
Data-plane listener
    ├── WebDAV
    ├── MCP
    ├── public health/liveness
    └── authorization metadata

Control-plane listener
    ├── Admin UI
    ├── Admin API
    ├── setup flow
    └── detailed diagnostics
```

Only the data-plane listener is intended to sit behind a public TLS reverse proxy. The control-plane listener is bound or published only to localhost, LAN, or VPN networks.

## 2. Context diagram

```text
                    ┌─────────────────────┐
                    │      Obsidian       │
                    │  existing WebDAV    │
                    │     sync plugin     │
                    └──────────┬──────────┘
                               │ WebDAV
                               ▼
┌───────────────┐     ┌──────────────────────────────────────────────┐
│ Browser on    │     │                 MCP Vault                    │
│ trusted LAN   │────▶│                                              │
└───────────────┘     │  Admin UI/API     WebDAV        MCP Server   │
                      │       │               │              │        │
                      │       └───────────────┼──────────────┘        │
                      │                       ▼                       │
                      │               Application Services           │
                      │                       │                       │
                      │               ┌───────┴────────┐              │
                      │               │   Vault Core   │              │
                      │               └───────┬────────┘              │
                      │                       │                       │
                      │   ┌──────────────┬────┴──────┬────────────┐   │
                      │   │              │           │            │   │
                      │ Filesystem    SQLite      Outbox/Jobs   History│
                      │   │              │           │            │   │
                      │   └──────────────┴────┬──────┴────────────┘   │
                      │                       ▼                       │
                      │          Index / Memory / Provider Workers   │
                      └───────────────────────┬──────────────────────┘
                                              │ configured APIs/models
                                              ▼
                                      ┌────────────────┐
                                      │ LLM / Embedding│
                                      │    providers   │
                                      └────────────────┘
                               ▲
                               │ MCP
                    ┌──────────┴──────────┐
                    │ AI host and Agents  │
                    └─────────────────────┘
```

## 3. Canonical, operational, and derived data

The architecture distinguishes three durability classes.

### 3.1 Canonical knowledge

Stored beneath the Vault content root:

- Markdown notes;
- attachments;
- Obsidian configuration;
- user taxonomy files;
- committed semantic memory records and generated raw/summary memory artifacts.

A copied content root remains a valid ordinary Obsidian Vault.

### 3.2 Authoritative operational state

Stored in SQLite and operational directories:

- admin users and sessions;
- WebDAV credentials;
- MCP PAT digests and OAuth issuer configuration;
- provider configuration and encrypted secrets;
- Vault registry;
- stable file identity and revision counters;
- write intents and recovery journal;
- revision-history metadata;
- durable outbox;
- background job state;
- audit records;
- backup catalog.

This data is not generally rebuildable from Markdown and must be backed up.

### 3.3 Derived state

Rebuildable from canonical knowledge plus configuration:

- note metadata projection;
- headings, tags, links, backlinks;
- FTS indexes;
- topic and knowledge-index projections;
- embeddings;
- Stage 1 and final-memory query projections reconstructed from managed
  memory artifacts and source notes;
- semantic relationships and optional knowledge graph.

Deleting derived state must not delete current notes or durable memories.

## 4. Core domain boundaries

### 4.1 Domain

Owns identifiers, values, and invariants:

- `VaultId`, `VaultSlug`, `FileId`, `RevisionId`, `MemoryId`;
- normalized Vault-relative paths;
- permissions and scopes;
- file revisions and preconditions;
- lifecycle status types;
- domain errors.

It has no dependency on Axum, RMCP, WebDAV, SQLx, or provider clients.

### 4.2 Vault Registry

Resolves a configured Vault into `VaultContext`.

```rust
pub struct VaultContext {
    pub id: VaultId,
    pub slug: VaultSlug,
    pub content_root: PathBuf,
    pub settings_revision: i64,
}
```

The first release may allow only one active Vault in the Admin UI. The registry, routes, credentials, tables, events, and application methods remain Vault-scoped.

### 4.3 Vault Core

Vault Core owns canonical file behavior:

- safe path resolution;
- read and stat;
- create, replace, patch, append, move, copy, and delete;
- expected-revision and HTTP precondition evaluation;
- atomic commits;
- stable file identity;
- revision history;
- durable write intents and recovery;
- audit facts;
- outbox events.

Every mutation from WebDAV, MCP, Admin, reconciliation, and memory materialization uses Vault Core.

Every Core factory receives the same process-owned `VaultCoreRuntime`. Its
path-lock registry serializes conflicting paths across independently built
WebDAV, MCP, Admin, reconciliation, and worker Core instances; lock entries are
weakly retained so inactive paths do not accumulate. The runtime also carries
the counted maintenance gate. Protocol requests and worker mutations hold RAII
admissions for their full operation, while staged writes retain their write
admission until commit/drop.

### 4.4 Filesystem storage

The filesystem implementation owns low-level I/O:

- no-follow path traversal;
- temporary files;
- fsync policy;
- atomic rename;
- content hashing;
- streaming large files;
- metadata access;
- optional trash/history blob storage.

It does not know about MCP tools, DAV methods, memory extraction, or UI concepts.

### 4.5 State repositories

SQLx repositories own SQLite access for:

- Vault registry and settings;
- identities, sessions, credentials, and scopes;
- file records and revisions;
- journal, outbox, and jobs;
- indexes and memory;
- providers;
- audit and backups.

Business services depend on repository traits or concrete application repositories, not raw SQL in handlers.

## 5. Protocol adapters

### 5.1 WebDAV adapter

Use the Rust `dav-server` library behind a project-owned adapter implementing the library’s filesystem/guard interfaces.

The adapter:

1. authenticates a WebDAV app credential;
2. resolves the Vault from the URL and credential;
3. maps DAV paths to validated Vault-relative paths;
4. maps reads and metadata to Vault Core;
5. maps writes and DAV preconditions to Vault Core;
6. streams bodies rather than buffering attachments;
7. maps domain conflicts and errors to correct HTTP/DAV responses.

Do not use `LocalFs` directly in production because it bypasses revision, audit, outbox, and recovery behavior.

A project-owned wrapper isolates the codebase from the chosen DAV library and makes replacement possible.

### 5.2 MCP adapter

Use the official Rust MCP SDK (`rmcp`) rather than hand-written JSON-RPC.

The adapter:

- targets MCP revision `2026-07-28`;
- relies on SDK negotiation for supported older revisions;
- uses stateless Streamable HTTP;
- exposes discovery instructions;
- publishes deterministic tool/resource lists;
- derives `VaultContext` and permissions from endpoint and authorization;
- maps application results to structured output and resource links;
- never stores application state in protocol sessions.

### 5.3 Admin adapter

The Admin API is a conventional versioned JSON API served on the control-plane listener.

WP-13 adds a dedicated backup application boundary. It coordinates Vault Core/
storage reads, the StateStore SQLite snapshot/restore boundary, a service-owned
manifest/tar artifact, and durable global jobs. Archive validation and root
swaps remain below the Admin adapter; protocol handlers never receive archive
paths or execute backup SQL. A process `MaintenanceGate` coordinates normal,
read-only, and offline modes across data adapters and workers without replacing
the per-operation `VaultContext` isolation boundary. Read-only transition
rejects new writes and drains admitted mutations; offline transition
additionally rejects/drains active protocol operations before restore swaps
SQLite or Vault roots.

It performs:

- admin session authentication;
- CSRF validation;
- input validation;
- application-service invocation;
- sanitized response mapping.

The React application is built separately and embedded into the server image or binary for a single deployable artifact.

WP-12 implements this adapter as a stateful `AdminApiState` injected only into
the control listener. Authenticated routes use the existing AuthService
session/CSRF boundary and translate typed DTOs to provider, index, memory, job,
and state application services. Network publication and source filtering are
owned by the host, firewall, or operator-selected reverse proxy; the
application defaults the control listener to loopback and does not interpret
client CIDRs. The data listener continues to use the unconfigured Admin
boundary and has no Admin assets or routes.

## 6. Application services

Application services coordinate domain and infrastructure behavior.

Recommended services:

- `VaultQueryService`
- `VaultMutationService`
- `RevisionHistoryService`
- `KnowledgeIndexService`
- `SearchService`
- `MemoryCommandService`
- `MemoryRecallService`
- `ProviderConfigurationService`
- `CredentialService`
- `BackupService`
- `JobService`
- `AuditQueryService`

Handlers call these services rather than repositories directly.

## 7. Write consistency and crash recovery

Filesystem and SQLite cannot participate in one native atomic transaction. The system therefore uses a durable write-intent protocol and reconciliation.

### 7.1 Mutation sequence

For a canonical mutation:

1. authenticate and resolve `VaultContext`;
2. validate permissions, path, expected revision, and HTTP preconditions;
3. acquire a scoped lock for affected file identities/paths in deterministic order;
4. write a `prepared` operation-journal record in SQLite;
5. stream/write new content to a temporary file and compute its hash;
6. fsync the temporary file according to durability policy;
7. atomically rename into place and fsync the parent directory where supported;
8. in one SQLite transaction:
   - update file identity/path/revision;
   - add revision-history metadata;
   - add audit record;
   - insert outbox events;
   - mark the operation committed;
9. release locks;
10. workers consume outbox events asynchronously.

LLM, embedding, and indexing work never occurs inside the user write request.

### 7.2 Recovery

At startup and periodically, a reconciler examines incomplete operations.

Using journal state, current file hash, temporary files, and recorded prior revision, it either:

- finalizes the metadata transaction;
- safely rolls back an uncommitted temporary file;
- marks the item for operator review when intent cannot be inferred.

A full scanner also detects out-of-band edits made directly to the mounted volume and imports them as external changes.

### 7.3 Concurrency

Concurrency controls are layered:

- file/path locks prevent conflicting in-process operations;
- expected revision protects Agent edits;
- HTTP ETags and modification preconditions protect WebDAV operations;
- a process-local admission gate serializes only SQLite write phases across
  bursty protocol requests, matching SQLite's single-writer model while file
  streaming, fsync, history creation, and atomic replacement remain concurrent;
- SQLite constraints protect unique revisions and idempotency keys;
- jobs use leases and idempotent deduplication keys.

Worker leases are renewed while handlers run, and a persisted Admin
cancellation bit is polled into each cooperative handler token. Full index
jobs created by a file-event burst are generation-coalesced per Vault: older
active jobs terminate without rebuilding and only the newest generation runs.
If a newer dirty event arrives during that rebuild, its durable job remains for
one follow-up pass.

The Worker claim loop is capacity-driven rather than batch-drained. It owns one
process-level bounded task set, claims no more rows than the currently free
handler slots, and immediately fills a slot when any handler finishes. A
multi-hour extraction therefore cannot leave other configured slots idle or
prevent later eligible jobs from being claimed. Shutdown drains that task set
before releasing the worker's remaining leases.

Metadata commits reserve the SQLite writer before reading precondition state.
This avoids upgrading a stale WAL snapshot after another commit and leaving a
canonical file stranded ahead of its revision, audit, and outbox transaction.

Move/copy operations lock source and destination in canonical order to avoid deadlock.

Directory moves extend that lock set to every tracked descendant and prepare
one journaled path revision per entry before the physical directory rename.
If a DAV overwrite has already tombstoned the destination, the old tombstone
is moved into the reserved operational namespace in the same metadata
transaction; the source identity and descendant histories remain intact.

## 8. Revisions and history

Each current entry has a stable `FileId` separate from its path.

A mutation creates a monotonically increasing revision for that `FileId`. A move preserves the identity and records a new path revision.

History content is stored outside the Obsidian Vault as content-addressed blobs:

```text
/data/history/<vault-id>/blobs/<sha256-prefix>/<sha256>
```

This avoids syncing history to every Obsidian device. Metadata in SQLite maps revisions to blobs, actors, source planes, timestamps, and paths.

Retention is configurable. The current file remains canonical; history is a safety and recovery feature.

## 9. Durable events and jobs

### 9.1 Transactional outbox

Canonical changes insert outbox rows in the same SQLite transaction as revision metadata.

Outbox consumers emit application events such as:

```text
FileCreated
FileUpdated
FileMoved
FileDeleted
FileRestored
MemoryMaterialized
VaultSettingsChanged
ProviderSettingsChanged
EmbeddingModelChanged
```

The revision operation and event type answer different questions. Reconciled
filesystem edits remain `external_change` revisions for audit/history, while
their outbox event uses the semantic lifecycle type (`FileCreated`,
`FileUpdated`, `FileDeleted`, or `FileRestored`) so derived consumers do not
need to infer what changed from a reconciliation implementation detail.

An in-process channel may reduce latency but never replaces the durable outbox.

### 9.2 Persistent jobs

Long-running or retryable work is represented in SQLite:

- initial/reconciliation scan;
- note analysis;
- FTS update;
- topic-index update;
- Phase 1 memory extraction;
- Phase 2 memory consolidation;
- prerelease memory-pipeline reset and fresh regeneration;
- embedding;
- re-embedding after model change;
- index rebuild;
- backup and restore validation.

Workers use renewable leases, bounded concurrency, exponential backoff,
dead-letter state, deterministic deduplication keys, and cooperative persistent
cancellation. Generic `outbox.event` fan-out jobs have a terminal compatibility
handler; the retained source outbox row remains the durable event record.
Workers checkpoint only bounded, redaction-safe current-unit metadata. A
reclaimed multi-note extraction resumes from its last durable completed-path
cursor; it does not use a volatile in-memory loop position as the recovery
boundary.

## 10. Index architecture

### 10.1 Deterministic layer

Always available:

- folder tree;
- note title and aliases;
- headings;
- tags;
- Obsidian wikilinks and Markdown links;
- backlinks;
- recent changes;
- manually declared taxonomy.

### 10.2 Semantic layer

When configured:

- note and section embeddings;
- topic clustering;
- LLM-generated topic names and summaries;
- related-note similarity;
- memory extraction and semantic recall.

Semantic projections are versioned by provider, model, prompt, and content hash.
The current note embedding projection uses deterministic bounded overlapping
chunks derived from the current note projection. Durable jobs contain only
Vault-scoped source references and hashes; the Index service resolves text at
execution and excludes stale hashes. A missing `embedding_note` binding
degrades note retrieval to FTS without affecting canonical writes.

### 10.3 User taxonomy

A portable file may guide the virtual knowledge map:

```text
_mcp-vault/index.yaml
```

It can declare top-level topics, include/exclude globs, pinned notes, aliases, and descriptions. The service validates it and overlays derived subtopics.

The index never moves user notes automatically.

## 11. Memory architecture

Memory has separate command and query paths.

### Command path

```text
explicit remember or note change
    → Phase 1 semantic distillation and exact-source validation
    → sourced raw-memory staging
    → Phase 2 global deduplication/conflict/forgetting decisions
    → prepared-proposal and revision-snapshot validation
    → canonical semantic Markdown materialization
    → projection and embedding
```

### Query path

```text
recall request
    → durable-memory lexical/vector/entity/topic candidate generation
    → ordinary-note lexical/vector cue generation
    → Vault/status/temporal filtering
    → rank fusion and diversity
    → token-budgeted typed memories + related-note cues
```

Normal recall is an index query and does not call an LLM. See `memory-system.md`.

The Memory application service uses the Codex-style boundary adopted by
ADR-0016. `memory.extract` distills one ordinary note revision into semantic
`raw_memory`, a detailed source summary, and exact evidence anchors. The exact
evidence text is derived by MCP Vault from model-selected ranges in an
explicitly line-numbered view of the current revision; the model never supplies
the canonical quotation. Only file/revision/line/hash metadata is persisted. A valid zero-result is durable
`no_output` coverage. Phase 1 does not create final memory or FTS rows.

`memory.consolidate` is a separate Vault-scoped persistent job using the
`memory_consolidation` binding. It consumes dirty Stage 1 rows and current
global memory, then proposes create/update/keep/archive, supersession, raw-input
disposition, and compact-summary changes. Local code validates every reference,
captures base revisions, persists the prepared proposal, rechecks the complete
input snapshot, writes canonical artifacts through Vault Core, and atomically
marks exact raw hashes selected. One active consolidation job is permitted per
Vault. Before preparation, snapshot changes retry instead of overwriting
concurrent Admin work. Once prepared, memory mutations yield a retryable
conflict until the persisted generation finishes; a restarted worker reuses
that proposal, adopts byte-identical file-first writes, and drains successive
bounded batches. Startup and periodic reconciliation re-admit any durable dirty
input left while the singleton job was completing.

Explicit `remember` uses the same Stage 1/Phase 2 path and returns raw-input and
job IDs rather than an immediate final memory. Final semantic content and
supporting evidence are distinct. Normal recall consumes only the latest
committed Vault-scoped memory FTS/entity/tag/recent/vector projections and
defaults to omitting sources. It also delegates ordinary-note cues to the Index
application service; those cues require Vault read permission and never acquire
memory lifecycle or canonical memory Markdown.

Incremental Phase 1 compares note revision and an effective profile hash over
policy, prompt/pipeline, binding, model, and Provider configuration. Explicit
full re-extraction includes unchanged rows. Malformed output is a note-local
failure and later notes continue; bounded consecutive failures stop repeated
invalid paid calls while preserving the durable cursor. Progress and logs expose
only redacted counts, timings, stable error codes, and trusted schema paths.

Canonical artifacts are `_mcp-vault/memory/MEMORY.md`,
`memory_summary.md`, `raw_memories.md`, `source_summaries/`, and per-record
Markdown. Legacy quote-as-content extracted memories are removed and
all discarded during the ADR-0017 prerelease cutover. Migration 0011 deletes
every old memory row and `memory.*` job; no explicit or extracted memory is
converted. The idempotent `memory.reset_pipeline` job removes all current
managed memory files through Vault Core, writes empty current artifacts, and
admits a fresh full-Vault extraction at note one. Managed files remain excluded
from ordinary reconciliation and note-indexing loops.

Every durable memory job carries `pipeline_generation`. The Worker cancels a
missing or mismatched generation before handler or Provider invocation. Reset
is exclusive with extraction/consolidation/revalidation/rebuild work for the
same Vault, and `regeneration_pending` is cleared only after the dedicated
current-generation `fresh_start` job has been admitted.

## 12. Provider architecture

Provider adapters implement internal traits for:

- LLM structured generation;
- embeddings;
- optional reranking.

Providers are selected by model role. Configuration is global with future Vault override.

Provider requests use:

- bounded concurrency;
- explicit timeouts;
- retry policy for transient failures only;
- redaction-safe tracing;
- request-size checks;
- path privacy policy;
- schema validation.

OpenAI-compatible generation resolves a typed Provider preset and independent
model axes before serialization. First-class Provider kinds cover DeepSeek,
Xiaomi MiMo, Zhipu GLM, Moonshot/Kimi, Google Gemini, and Alibaba
Qwen/DashScope. Model settings may separately override structured-output mode,
token-limit field, thinking control, and one-call generation limit. This avoids
both a lowest-common-denominator request and a combinatorial adapter per field
combination.

The generic OpenAI-compatible kind does not guess a vendor from a model name:
the same `qwen` identifier can be served by DashScope, Ollama, vLLM, or a
proxy with different extensions. `auto` uses a first-class kind or recognizes
an exact official host only for legacy Provider rows. Detailed contracts and
primary references live in `provider-compatibility.md`. Every preset still
performs the same local schema validation; provider-side JSON mode is not
trusted as proof of correctness.

An adapter may use an SDK only when its network execution remains behind
`ProviderTransport`. It may not instantiate an independent HTTP client that
would evade URL resolution policy, redirect denial, response-size limits,
shared concurrency, redaction, or post-success no-replay behavior.

Operation-specific deadlines may override a Provider default without creating
a second transport boundary. A structured generation is not replay-safe after
the provider has returned a successful HTTP status: timeout, truncation, or
failure while reading that response body is terminal until explicit operator
retry because billable work may already have completed.

Provider adapters cannot write files or SQL projections directly.

The WP-10 provider boundary also owns endpoint policy, bounded transport,
structured-response validation, embedding dimension checks, and the
VectorIndex implementation. Provider definitions are global operational
configuration; model bindings resolve a Vault override before a global
default. Embeddings and vectors remain derived, Vault-partitioned state with
an exact SQLite fallback.

Provider configuration edits and deletions remain Provider application
operations. PATCH uses an optimistic Provider revision and keeps an existing
encrypted secret unless Admin supplies a replacement. Replacement removes
superseded Provider-owned ciphertext after the Provider row references the new
secret. DELETE is one immediate State transaction: remove bindings for the
Provider's models, remove Vault-partitioned derived embeddings/vectors, remove
models and health/configuration, then remove every encrypted secret owned by
that Provider. It deliberately retains canonical notes, memory lifecycle
records/materialized Markdown, durable job history, and append-only audit.

## 13. Multi-Vault evolution

The current product may expose one active Vault, but future multi-Vault support should require enabling management behavior rather than redesigning internals.

### Required now

- `vaults` table;
- `VaultContext` on every application method;
- Vault-scoped credentials and permissions;
- per-Vault data-plane routes;
- `vault_id` on all relevant rows, jobs, events, and caches;
- per-Vault index namespaces and vector partitions;
- per-Vault configuration overlay;
- isolation tests using at least two fixture Vaults.

### Future behavior

- Admin UI can create and manage several Vaults;
- one MCP connection is bound to one Vault;
- an Agent needing two Vaults configures two MCP server connections;
- cross-Vault search/recall requires a distinct federated capability and explicit grants;
- no ordinary tool accepts `vault_id`.

## 14. Recommended workspace

```text
mcp-vault/
├── Cargo.toml
├── crates/
│   ├── domain/
│   ├── vault-core/
│   ├── storage-fs/
│   ├── state/
│   ├── auth/
│   ├── webdav/
│   ├── mcp/
│   ├── indexer/
│   ├── memory/
│   ├── providers/
│   ├── admin-api/
│   └── server/
├── frontend/
│   └── admin/
├── migrations/
├── tests/
│   ├── fixtures/
│   ├── webdav/
│   ├── mcp/
│   └── e2e/
├── docs/
└── docker/
```

Crate boundaries may be consolidated if compile time or ergonomics justify it, but dependency direction and responsibility boundaries remain binding.

## 15. Dependency direction

```text
server
  ├── admin-api ─┐
  ├── mcp ───────┤
  ├── webdav ────┤
  ├── indexer ───┤
  ├── memory ────┤
  └── providers ─┤
                 ▼
             vault-core
               ├── domain
               ├── state abstractions
               └── storage abstractions

state and storage-fs implement lower-level abstractions.
Protocol crates never become dependencies of vault-core.
```

Avoid a generic “common” crate that accumulates unrelated code.

## 16. Observability

Use structured `tracing` spans with:

- request ID;
- plane;
- actor ID or credential ID, never full secret;
- Vault ID;
- operation;
- file ID/path hash where sensitive path logging is disabled;
- job ID;
- provider ID/model;
- duration and result.

Support OpenTelemetry trace propagation on MCP and HTTP boundaries. Metrics must cover request rates, latency, conflicts, queue depth, job failures, indexing coverage, provider latency, recall latency, and backup age.

## 17. Architectural acceptance

The architecture is preserved when:

- protocol handlers can be replaced without rewriting Vault Core;
- indexes can be rebuilt from files;
- remote providers can be disabled without breaking core service;
- a second fixture Vault passes isolation tests;
- a crash between file commit and metadata commit is recovered;
- WebDAV and MCP writes produce the same revision/audit/event behavior;
- the Admin listener can be absent from all public proxy routes;
- an Agent connection cannot change Vault by tool argument.
