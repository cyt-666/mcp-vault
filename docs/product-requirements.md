# Product Requirements

## 1. Product definition

MCP Vault is a self-hosted service that exposes one or more isolated canonical
Markdown Vaults to both people and AI Agents.

A person uses Obsidian and an existing WebDAV synchronization plugin. An Agent uses MCP. The service adds indexing, knowledge discovery, controlled mutation, revision history, and long-term memory without replacing ordinary files with a proprietary note database.

The supported ownership model is one owner/Admin with one or more configured
Vaults. Each Vault has distinct WebDAV/MCP endpoints and credentials. The
architecture, schema, jobs, indexes, memory, and APIs preserve the Vault as the
isolation boundary; multi-user tenancy and cross-Vault recall are separate
future capabilities.

## 2. Primary users

### Vault owner

The owner deploys the service, connects Obsidian, configures MCP clients,
chooses LLM and embedding providers, inspects what the system remembers,
monitors jobs, and restores data.

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
- automatic rebuildable note-level semantic projections when the
  `embedding_note` role is configured;
- deterministic versioned embedding inputs whose complete UTF-8 payload is
  bounded conservatively enough for configured remote Provider limits;
- note-level semantic Top-K after current-chunk validation: only the highest
  non-negative cosine chunk for one note may contribute, and chunk count must
  not increase that note's score or consume another note's result slot;
- scoped search by path, topic, tag, type, and time;
- full note reads;
- section/outline reads that avoid loading an entire long note unnecessarily;
- provenance in results;
- bounded output with pagination or cursors.

Retrieval MUST continue to work lexically when LLM or embedding providers are unavailable.
Outward note paths MUST resolve stable File IDs to current active paths so a
completed move cannot leave an Agent with a known-unreadable old path. A stale
content projection MUST retain its analyzed revision until rebuilt.

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
- separately typed related-note cues so an Agent can remember that ordinary
  Vault knowledge exists without promoting article contents to durable facts;
- `remember` as explicit durable input admission followed by background global
  consolidation;
- memory types including preferences, decisions, constraints, facts, projects, progress, events, relationships, and procedures;
- provenance, confidence, importance, temporal validity, and lifecycle status;
- Vault-level automatic memory generation from ordinary Markdown without
  requiring note authors to add service-specific markers, tags, or folders;
- a bounded Phase 1 policy that permits `no_output`, uses the Codex three-field
  semantic output, and derives source provenance locally;
- exact source identity/revision/hash checks; model-generated source
  coordinates or confidence/importance scores MUST NOT be used as trust
  evidence;
- rename-stable source identity and a current readable path when the source is
  active; deletion MUST NOT expose a last-known path as if it were readable;
- optional line/heading anchors for explicit or imported provenance, without
  requiring the automatic extraction model to generate them;
- separate extraction and consolidation model roles, persisted raw-memory
  staging, schema validation, global deduplication, contradiction resolution,
  supersession, and forgetting;
- application-owned final identifiers, evidence attachment, raw-input
  dispositions, and optimistic revisions; the consolidation model MUST propose
  semantic decisions rather than own this bookkeeping;
- autonomous consolidation so ordinary operation never depends on a human
  review queue or candidate inbox;
- user inspection, editing, merging, archival, restoration, and
  revision-aware permanent deletion;
- canonical Markdown materialization for committed semantic memories and
  inspectable generated raw/summary layers;
- source-language `raw_memory`, summaries, and canonical memory bodies rather
  than silently translating user knowledge to the prompt language;
- persisted, rebuildable retrieval aliases for the source language,
  Simplified Chinese, and English so covered memories support offline
  cross-language lexical recall;
- automatic alias enrichment for new or body-changed memory, plus an explicit
  authenticated Admin backfill for existing active, stale, and superseded
  memory;
- recall that does not require a live LLM call.
- direct current-model vector rebuilding for existing durable memory without
  re-running Phase 1 or Phase 2.

While the memory format remains explicitly prerelease, an incompatible
pipeline-generation upgrade MUST discard old memory jobs/state and regenerate
only from canonical Vault notes. It MUST NOT run an unversioned old job through
the new handler. Managed memory files are removed through Vault Core; ordinary
notes, revisions, Provider settings, audits, backups, and non-memory jobs remain
out of scope for that cutover.

Related-note cues are derived, rebuildable, revision-bound source pointers and
require Vault read permission. They are not durable memories and MUST remain
distinguishable in the response. The service cannot force every MCP Host to
invoke recall. It MUST make correct recall behavior likely through server
instructions, clear tool descriptions, a compact memory-context resource, and
low latency.

### 3.6 LLM and model providers

The owner MUST be able to configure and test:

- one or more LLM providers;
- one or more embedding providers;
- optional local embedding models;
- role-specific model bindings such as extraction, summarization, topic enrichment, and optional reranking;
- a bounded role-specific extraction deadline suitable for slower reasoning models;
- global defaults with future per-Vault overrides;
- provider timeouts, concurrency, retry, and privacy policy.
- model-change re-embedding and redacted per-job Provider failure categories.
- revision-safe editing, disabling, secret rotation, and deletion of Provider
  configurations from Admin without direct database access.

Deleting a Provider MUST atomically remove its model inventory, every global
and Vault-specific role binding to those models, and Provider-owned derived
vectors and encrypted secrets. It MUST NOT delete canonical Vault files,
durable memories or generated memory artifacts, job history, or audit history.

The service MUST support at least:

- OpenAI-compatible HTTP endpoints;
- an Anthropic-compatible adapter;
- first-class official-endpoint presets for DeepSeek, Xiaomi MiMo, Zhipu GLM,
  Moonshot/Kimi, Google Gemini, and Alibaba Qwen/DashScope;
- local OpenAI-compatible endpoints such as Ollama or vLLM;
- remote embedding endpoints;
- a local embedding implementation behind an optional feature.

Each OpenAI-compatible generation model MUST have typed compatibility settings
rather than relying on one request shape for every vendor. Provider preset,
structured-output mode, token-limit field, thinking control, and per-call
generation bound remain independent. The generic type MUST NOT infer an API
dialect from a locally served model name; first-class Provider type, exact
official host migration, or explicit model configuration selects vendor
extensions. Regardless of provider-side enforcement, MCP Vault MUST parse and
validate the returned JSON against its own phase-specific schema before it can
enter Stage 1 state or a prepared consolidation proposal.
For a schema with one required array envelope, a compatibility adapter MAY add
that envelope only when the returned direct object or array already satisfies
the complete item schema. It MUST run full root-schema validation afterward and
MUST NOT reinterpret an empty, renamed, or ambiguous object as a successful
zero-result response.

Provider libraries or SDKs MAY help serialize a protocol, but they MUST NOT
bypass the project-owned endpoint validation, SSRF policy, bounded body,
timeout, concurrency, redaction, or cost-safe retry boundary.

Provider failure MUST NOT block WebDAV, normal Vault writes, lexical search, or explicit memory access.

### 3.7 Administration console

The service MUST provide a browser-based administration console for:

- first-run setup and admin password creation;
- Vault status and storage location;
- WebDAV credentials;
- MCP access credentials, built-in OAuth login, and optional external
  resource-server settings;
- permissions and revocation;
- LLM, embedding, and model-role configuration;
- index status and rebuild;
- memory browser and exceptional processing diagnostics;
- background jobs and provider health;
- audit logs;
- backup, restore, and retention configuration;
- system version, migrations, and diagnostics.

The console MUST list, create, select, disable, and re-enable service-managed
Vaults. New content roots are derived from the immutable Vault slug under the
service data directory. Existing single-Vault URLs and unscoped Admin behavior
remain compatible through a stable legacy-default binding.

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

A valid Admin login MUST survive an ordinary page reload without copying or
persisting the session bearer in JavaScript-accessible browser storage. The UI
MUST confirm the server-side session before rendering authenticated content and
MUST retain the session-bound CSRF protection required for later mutations.
HTTPS Admin sessions MUST use Secure cookies. An explicitly configured
loopback or literal private-IP HTTP Admin Origin MAY use host-only non-Secure
cookies for trusted-LAN appliance compatibility, while retaining HttpOnly on
the session, SameSite=Strict, exact Origin/Referer, CSRF, expiry, revocation,
and a visible cleartext warning. Public cleartext origins MUST be rejected.

The Admin login shell, authenticated navigation, and browser metadata SHOULD
use the project Logo rather than a textual placeholder.

Provider configuration MUST include an operable model inventory and explicit
role bindings; a configured Base URL alone is not a selected model. The owner
MUST be able to refresh provider-advertised models, manually register a model
when discovery is unavailable, and bind the current Vault's extraction,
embedding, enrichment, and reranking roles.

Automatic memory MUST be visibly opt-in and event-driven, with durable
incremental/full note actions, separate Phase 1/Phase 2 readiness, pending raw
counts, committed generation, and recent job evidence. There is no normal
candidate-generation or per-result approval control. Admin progress MUST
distinguish unknown work from zero work and report model-evaluated notes,
pre-provider skips, raw inputs staged, `no_output`, isolated note failures, and
Phase 2 create/update/retire/discard outcomes.
Successful Stage 1 coverage MUST be persisted per Vault/source identity,
source revision, and effective extraction profile even when the model returns
`no_output`. Automatic events and manual backfill MUST check that coverage
before a Provider call. The default manual action processes only new, changed,
previously failed, or profile-stale notes; an explicit off-by-default option may
include already evaluated unchanged notes with a clear token-cost warning.
One malformed generated result MUST NOT abandon the remaining existing-note
backfill. The service MUST checkpoint that note without replaying its paid
request, continue later notes, and retain a bounded redacted failure reason;
repeated consecutive contract failures MAY open a cost-safety circuit.

Source safety MUST NOT depend on automatic-memory or Provider configuration.
Every file create, update, move, delete, restore, and reconciled external change
MUST update note-source health before optional extraction admission. A memory
with note sources MUST have at least one verified current note source to enter
normal recall, regardless of origin; a source-less explicit Agent/Admin memory
remains supported. Current health MUST fail closed when the live file hash
changes, including before background lifecycle work completes.

Cross-File-ID recovery MUST use one unique exact candidate in the same Vault:
normalized full-note evidence or the same anchored excerpt hash. Filename,
semantic/vector, LLM, ambiguous, truncated, and cross-Vault matches MUST NOT
bind. Source-unavailable stale memory MAY reactivate after exact recovery;
archived and superseded memory MUST NOT reactivate automatically. Unsupported
memory MUST be retained unless an authenticated explicit deletion occurs.

Admin MUST expose repeatable paged source audits and separate exact counts for
final sources, affected memories, Stage 1 sources, and distinct File IDs.

Existing-memory multilingual backfill MUST be an explicit Admin action with a
model-cost and backup warning. A source-language rewrite is permitted only
when bounded current source samples are verified through Vault Core. It MUST
preserve identity, lifecycle, provenance, relations, validity, technical
literals, and normal revision history. Unavailable or ambiguous sources permit
safe aliases only. A failed alias/rewrite batch MUST NOT roll back canonical
memory, and normal recall MUST report incomplete alias coverage while returning
the results it can produce.

The Admin UI and Admin API MUST run on a separate listener that is not publicly exposed by default. Network restriction does not replace authentication.

### 3.8 Authentication and authorization

The service MUST keep three independent security domains:

- Admin authentication;
- WebDAV authentication;
- MCP authorization.

The service MUST support high-entropy, Vault-bound personal access tokens for trusted clients.

It MUST also support standards-aligned MCP authorization without requiring an
external identity provider. The built-in authorization server MUST provide
authorization-server and protected-resource metadata, DCR for public clients,
authorization code + PKCE `S256`, resource indicators, exact redirects,
Vault-bound scopes, short-lived access tokens, rotating refresh tokens, and
local revocation. A separate Vault OAuth credential MUST be used instead of an
Admin credential. Optional external resource-server mode MUST retain issuer,
audience, signature, resource, Subject-grant, and scope validation.

MCP tools MUST derive the Vault from the authenticated endpoint and credential. Tools MUST NOT accept an arbitrary Vault identifier.

### 3.9 Indexing and background processing

The service MUST:

- perform an initial scan;
- observe server writes immediately;
- reconcile out-of-band filesystem changes;
- maintain FTS, metadata, link, topic, memory, and embedding projections;
- use a durable job queue and transactional outbox;
- retry transient failures safely;
- avoid automatic replay when a provider may already have completed billable
  work, including response-body failures after a successful HTTP status;
- expose job state and failures;
- expose bounded current-unit progress for long jobs without persisting note
  bodies, prompts, or provider responses in operational diagnostics;
- allow independent rebuild of derived projections.
- run a repeatable source audit after full reconciliation and restore.

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
5. An Agent can `remember` a decision, receive durable staging/job identities,
   and recall the semantic Markdown memory after Phase 2 commits.
6. Automatic Phase 1 can verify exact source evidence and Phase 2 can safely
   consolidate semantic memory without routine human review or model
   self-score trust.
7. The owner can configure and test LLM and embedding providers from the LAN-only console.
8. Provider outages leave WebDAV, file writes, lexical search, and existing memory recall operational.
9. Indexes can be deleted and rebuilt without loss of canonical knowledge.
10. Backup and restore reproduce Vault content, credentials/configuration, revision history, and service operation.
11. Two managed Vaults can use the same relative paths while credentials,
    files/history, jobs, FTS/vectors, memory, settings, and audit remain bound
    to exactly one Vault context.
12. One initializing, disabled, or failed Vault does not prevent Admin or a
    healthy Vault from starting and operating.
13. Security, migration, crash-recovery, Litmus, MCP conformance, and end-to-end tests pass.
14. Covered Chinese and English memories can be recalled with either language
    while the embedding role is unavailable, without a query-time LLM call.
