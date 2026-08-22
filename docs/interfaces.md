# External Interface Specification

## 1. Interface overview

MCP Vault exposes three independent interfaces.

| Interface | Client | Listener | Authentication |
|---|---|---|---|
| WebDAV | Obsidian sync plugins and DAV clients | Data plane | Vault-scoped app username/password |
| MCP | AI hosts and Agents | Data plane | Vault-scoped PAT or OAuth access token |
| Admin UI/API | Vault owner on trusted network | Control plane | Admin session cookie + CSRF |

All URLs are versioned. Examples use `v1`.

## 2. Vault binding

Data-plane endpoints identify one Vault:

```text
/dav/v1/vaults/{vault_slug}/
/mcp/v1/vaults/{vault_slug}
```

Authorization must grant access to the same Vault.

The application rejects:

- a credential for another Vault;
- an OAuth subject without a grant for this Vault;
- a token whose audience/resource does not match the MCP protected resource;
- a tool argument attempting to select another Vault.

When the first release has one Vault, the UI may generate these URLs automatically. Do not create unscoped internal APIs as a shortcut.

## 3. WebDAV contract

### 3.1 Protocol

Implement RFC 4918 behavior required by the selected DAV library and tested clients.

Required methods:

```text
OPTIONS
PROPFIND
GET
HEAD
PUT
DELETE
MKCOL
COPY
MOVE
LOCK
UNLOCK
```

Required HTTP behavior includes:

- byte ranges for large attachment downloads;
- conditional requests;
- `If-Match`, `If-None-Match`, `If-Modified-Since`, and `If-Unmodified-Since`;
- correct `207 Multi-Status`;
- depth handling with configured limits;
- streaming request and response bodies;
- stable ETags;
- correct content lengths and content types;
- DAV locks through the project lock abstraction.

The initial lock backend may be SQLite-backed or an adapter around a library lock manager, but the architecture must allow a durable distributed implementation later.

### 3.2 Authentication

Use HTTP Basic Authentication only over TLS outside localhost/trusted private transport.

A WebDAV credential is:

- bound to exactly one Vault;
- independently named and revocable;
- assigned read/write/delete permissions;
- optionally expiring;
- audited by credential ID.

Do not reuse the Admin password or MCP token.

The data-plane adapter mounts this contract at
`/dav/v1/vaults/{vault_slug}/`. The slug selects the Vault context; the
credential is looked up inside that same Vault, so neither a DAV request nor a
credential can switch Vaults by supplying an arbitrary `vault_id`. Admin
credential CRUD and generated connection cards remain control-plane work.

### 3.3 ETags and revisions

For files, generate a strong ETag from the current content hash and revision.

Conceptual form:

```text
"<revision>-<sha256>"
```

For directories, generate a weak ETag from the directory projection/version.

WebDAV preconditions map to Vault Core expected state. A failed precondition returns the appropriate HTTP status instead of overwriting.

### 3.4 Obsidian compatibility

Test at least:

- Hēsperus Sync Engine with its WebDAV backend;
- Remotely Save using WebDAV;
- desktop and one mobile platform;
- Markdown and binary attachments;
- create, update, delete, rename, conflict, and interrupted sync;
- `.obsidian` synchronization when enabled.

Do not rely on plugin-specific private APIs. Document any compatibility workaround in a dedicated test fixture and, if durable, an ADR.

## 4. MCP transport and protocol

### 4.1 Transport

Target MCP revision `2026-07-28`.

The endpoint accepts POST. Every JSON-RPC request is independent. There is no protocol-level session and no separate GET stream endpoint for the current revision.

A response is either:

- one `application/json` response; or
- a request-scoped `text/event-stream` response ending with the final result.

Use the official Rust SDK to negotiate older supported revisions rather than reproducing protocol compatibility manually.

### 4.2 Required transport validation

Validate:

- `Origin` when present;
- `MCP-Protocol-Version`;
- `Mcp-Method`;
- `Mcp-Name` when applicable;
- standard request metadata in `_meta`;
- body/header consistency required by the negotiated revision;
- request size and content type;
- authorization on every request.

Forward and consume W3C trace context according to the MCP metadata conventions.

### 4.3 Discovery instructions

Implement `server/discover`.

Recommended instructions:

```text
This server is the user's persistent Markdown knowledge and long-term
memory Vault.

Use vault_overview or browse_index when you need to understand what
knowledge exists and do not yet know exact search terms.

Use recall proactively before answering when the current request may
depend on prior preferences, decisions, constraints, project state,
progress, events, relationships, or past work.

Use search_notes to locate source material and read_note to verify exact
details. Treat recalled memories as sourced context, observing their
confidence, validity, lifecycle status, and provenance.

Use mutation tools only when the user requests or clearly authorizes a
persistent change. Preserve revisions and do not retry a conflict by
overwriting newer content.
```

Discovery, tool lists, and resources are authorization-dependent and therefore use private cache scope. Tool/resource list and read results include `ttlMs` and `cacheScope`; the current implementation uses `ttlMs: 1000` and `cacheScope: "private"` for rebuildable projections.

MCP Vault advertises only the capabilities it implements: tools and
resources. Unsupported optional surfaces such as `prompts/list` and
`completion/complete` return JSON-RPC method-not-found and are not silently
reported as empty capabilities.

### 4.4 Tool list

Return tools in this deterministic order, omitting tools the caller’s scopes do not permit:

1. `vault_overview`
2. `browse_index`
3. `recent_changes`
4. `search_notes`
5. `read_note`
6. `recall`
7. `get_memory`
8. `list_memories`
9. `create_note`
10. `edit_note`
11. `move_note`
12. `delete_note`
13. `note_history`
14. `restore_note_revision`
15. `remember`
16. `update_memory`
17. `forget_memory`

Names may receive a stable namespace if required by SDK conventions, but once released they must remain backward compatible.

The MCP foundation advertises deterministic discovery, browse, lexical
`search_notes`, read, mutation, and history tools. WP-11 adds `recall`,
`get_memory`, `list_memories`, `remember`, `update_memory`, and `forget_memory`
when the credential grants the corresponding memory scopes. Memory resources
are authorization-dependent and include `vault://memory/context` plus the
`vault://memory/{memory_id}` template. `search_notes` remains lexical-safe
when semantic providers are unavailable; `recall` reports semantic
degradation while preserving projection-based lexical/context results.

## 5. MCP scopes

Initial scopes:

```text
vault:discover
vault:read
vault:write
vault:delete
vault:history
memory:read
memory:write
memory:manage
```

Suggested grants:

### Read-only Agent

```text
vault:discover
vault:read
memory:read
```

### Trusted personal Agent

```text
vault:discover
vault:read
vault:write
memory:read
memory:write
```

### Maintenance Agent

```text
vault:discover
vault:read
vault:write
vault:delete
vault:history
memory:read
memory:write
memory:manage
```

Destructive tools must carry appropriate MCP annotations and be clearly described.

## 6. MCP tool contracts

All tools define JSON Schema 2020-12 input and output schemas.

All outputs return structured content. When a result refers to a note or memory, also return MCP resource links where supported.

Every result includes a stable `request_id` for audit correlation.

Tool structured content uses this envelope:

```json
{
  "request_id": "...",
  "ok": true,
  "data": {},
  "error": null
}
```

`data` is an object for all current MCP Vault tools so the output schema is
valid for both the 2026-07-28 and negotiated dated MCP schemas. Error results
set `ok` to `false` and provide a bounded structured `error` object instead of
leaking SQL, filesystem, provider, or secret details.

### 6.1 `vault_overview`

Purpose: provide a bounded map of the Vault before exact retrieval.

Scope: `vault:discover`.

Input:

```json
{
  "include_recent": true,
  "max_topics": 20,
  "max_tokens": 2000
}
```

Output:

```json
{
  "vault": {
    "name": "Personal",
    "description": "Personal technical and project knowledge",
    "index_revision": 42
  },
  "statistics": {
    "notes": 1200,
    "attachments": 340,
    "memories": 280,
    "topics": 18
  },
  "topics": [
    {
      "id": "topic:mcp-vault",
      "title": "MCP Vault",
      "summary": "Architecture and implementation decisions for the service",
      "note_count": 24,
      "child_count": 5,
      "last_activity_at": "2026-08-19T08:00:00Z"
    }
  ],
  "recent": [],
  "truncated": false,
  "request_id": "..."
}
```

The output must remain compact and must not dump every path.

### 6.2 `browse_index`

Purpose: navigate the virtual knowledge map.

Scope: `vault:discover`.

Input:

```json
{
  "node_id": "root",
  "depth": 1,
  "cursor": null,
  "limit": 50,
  "include_note_candidates": true
}
```

Output includes:

- node metadata and summary;
- ordered children;
- representative/pinned notes;
- next cursor;
- index revision;
- bounded note candidates with tags, outgoing links, and backlink counts;
- resource links.

A missing `node_id` means root.

### 6.3 `recent_changes`

Purpose: understand current activity and project continuity.

Scope: `vault:discover`.

Input filters:

- `since`;
- operation types;
- path prefix;
- limit/cursor.

Return compact metadata, not note bodies.

### 6.4 `search_notes`

Purpose: lexical, semantic, or hybrid source retrieval.

Scope: `vault:read`.

Input:

```json
{
  "query": "WebDAV conflict handling",
  "mode": "hybrid",
  "scope": {
    "path_prefix": null,
    "topic_ids": [],
    "tags": [],
    "modified_after": null,
    "modified_before": null
  },
  "result_granularity": "section",
  "limit": 12,
  "cursor": null,
  "include_score_breakdown": false
}
```

`mode`:

```text
lexical
semantic
hybrid
```

If semantic search is unavailable, hybrid falls back to lexical and reports degradation.

Output result fields:

- file ID, path, title;
- revision and modified time;
- heading/source anchor;
- bounded snippet;
- lexical/semantic/fused score when requested;
- tags/topic IDs, outgoing links, and backlink count;
- resource link.

### 6.5 `read_note`

Purpose: retrieve exact source content.

Scope: `vault:read`.

Input:

```json
{
  "path": "Projects/mcp-vault/design.md",
  "revision": null,
  "selection": {
    "kind": "full"
  },
  "max_bytes": 200000
}
```

Selection kinds:

```text
full
line_range
heading
byte_range
```

Output:

- path, file ID, revision, content hash;
- MIME type and encoding;
- requested content;
- selected source anchor;
- truncation flag;
- outline summary when truncated.

Binary files are returned as resource links/metadata rather than embedded base64 unless a negotiated capability explicitly requires content.

### 6.6 `note_context`

Purpose: decide whether and how to read a note without loading all content.

Scope: `vault:read`.

Input: path or file ID.

Output:

- title, aliases, frontmatter subset;
- outline/headings;
- tags;
- outgoing links and backlinks;
- related notes with relationship type;
- word count, revision, modified time;
- structural summary;
- memory references sourced from the note.

### 6.7 `recall`

Purpose: retrieve durable context useful for the current task, not documents that merely contain the query.

Scope: `memory:read`.

The complete schema and ranking behavior are in `memory-system.md`.

Input includes query, optional current project/entities/topics, type filters, time range, importance threshold, result and token budgets, and source/score options.

Output includes atomic memories with status, confidence, importance, temporal validity, provenance, relations, and scores.

### 6.8 `get_memory`

Purpose: inspect one durable memory and its provenance.

Scope: `memory:read`.

Input: memory ID.

Output includes canonical Markdown path/revision, all sources, lifecycle, relations, and resource links.

### 6.9 `list_memories`

Purpose: browse memory records deliberately; not a replacement for recall.

Scope: `memory:read`.

Filters:

- type;
- lifecycle status;
- tag/entity;
- source path;
- validity time;
- limit/cursor.

### 6.10 `create_note`

Purpose: create a new canonical file.

Scope: `vault:write`.

Input:

```json
{
  "path": "Projects/new-note.md",
  "content": "# New note\n",
  "if_absent": true,
  "idempotency_key": "client-generated-key"
}
```

Output: file ID, path, revision, hash, resource link.

`if_absent` defaults true. Existing files produce conflict.

### 6.11 `edit_note`

Purpose: perform a revision-aware mutation.

Scope: `vault:write`.

Input:

```json
{
  "path": "Projects/mcp-vault/design.md",
  "expected_revision": 18,
  "operation": {
    "type": "apply_unified_diff",
    "patch": "..."
  },
  "idempotency_key": "..."
}
```

Operation types:

```text
replace_all
apply_unified_diff
append
insert_after_heading
replace_heading_section
```

All operations require `expected_revision`. A conflict returns current revision/hash and no content change.

Patches must apply exactly; fuzzy patching is forbidden unless a future explicitly named tool makes the risk visible.

### 6.12 `move_note`

Scope: `vault:write`.

Input includes source path, destination path, source expected revision, destination absence precondition, and idempotency key.

### 6.13 `delete_note`

Scope: `vault:delete`.

Input includes path, expected revision, deletion mode, and idempotency key.

Modes:

```text
trash
permanent
```

Default is `trash` where configured. Permanent deletion still retains revision history according to policy.

### 6.14 `note_history`

Scope: `vault:history`.

Returns revision metadata and optional bounded diffs. It does not return every full historical blob by default.

### 6.15 `restore_note_revision`

Scopes: `vault:history` and `vault:write`.

Restoration creates a new current revision; it never rewinds revision numbers.

### 6.16 `remember`

Purpose: create or reinforce an explicit durable memory.

Scope: `memory:write`.

Input:

```json
{
  "type": "decision",
  "content": "The Admin Console must remain LAN-only.",
  "importance": 0.95,
  "valid_from": "2026-08-19T00:00:00Z",
  "tags": ["security", "admin"],
  "entities": ["Admin Console"],
  "source_note": null,
  "idempotency_key": "..."
}
```

Output states whether a memory was created, reinforced, merged, or flagged as a conflict.

### 6.17 `update_memory`

Scope: `memory:manage`.

Requires expected canonical revision. It can edit content/metadata, add sources, or explicitly mark supersession.

### 6.18 `forget_memory`

Scope: `memory:manage`.

Default action is archive. Permanent deletion must be explicit and audited.

## 7. MCP resources

Expose resources in addition to tools for hosts that use them.

Recommended URI scheme:

```text
vault://overview
vault://index/{node_id}
vault://note/{percent-encoded-path}
vault://memory/context
vault://memory/{memory_id}
vault://recent
```

Resource lists and reads:

- use private cache scope;
- include appropriate TTL;
- honor caller scopes;
- include revision/cache metadata;
- never enumerate another Vault.

`vault://memory/context` is compact and contains only high-importance active projects, current decisions, stable preferences, constraints, and recent progress within a token budget.

Tools remain available because not all MCP hosts automatically include resources.

## 8. MCP error model

Application errors are returned as structured tool errors without leaking internals.

Stable codes:

```text
not_found
invalid_path
permission_denied
revision_conflict
precondition_failed
already_exists
invalid_patch
unsupported_media_type
result_too_large
semantic_search_unavailable
provider_unavailable
memory_conflict
rate_limited
temporarily_unavailable
internal_error
```

Example:

```json
{
  "error": {
    "code": "revision_conflict",
    "message": "The note changed after the supplied revision.",
    "retryable": true,
    "details": {
      "expected_revision": 18,
      "current_revision": 19,
      "current_hash": "sha256:..."
    }
  },
  "request_id": "..."
}
```

Do not include local absolute paths, SQL, stack traces, or secrets.

## 9. MCP authorization

### 9.1 Personal access tokens

For trusted clients that accept static headers:

```http
Authorization: Bearer <high-entropy-token>
```

PATs are Vault-bound and scope-bound. They are a pragmatic direct-token mode, not a substitute for standards-based OAuth discovery where a client expects it.

### 9.2 OAuth resource-server mode

When enabled, the MCP endpoint implements:

- RFC 9728 protected resource metadata;
- `WWW-Authenticate` with `resource_metadata` on 401;
- configured authorization-server discovery;
- issuer, signature, expiry, not-before, audience, and resource validation;
- resource indicators;
- subject-to-Vault grants;
- MCP scopes.

The service does not pass MCP access tokens to LLM providers or any upstream API.

The first complete release may use a configured external OAuth/OIDC authorization server rather than implementing an authorization server itself.

## 10. Admin API

### 10.1 Listener and prefix

Control-plane listener only:

```text
/api/v1
```

The React UI is served from the same listener.

### 10.2 Session behavior

- `GET /api/v1/setup` — unauthenticated, non-secret first-Admin setup
  availability (`setup_available`);
- `POST /api/v1/setup` — one-time first-Admin claim on the Admin listener with
  `username`, `password`, and strict Origin validation;
- `POST /api/v1/session` — login;
- `DELETE /api/v1/session` — logout;
- `GET /api/v1/session` — current admin;
- state-changing requests require CSRF token and strict Origin validation.

Source-network admission is a deployment concern controlled by listener
publication, firewall/VPN rules, or an operator-selected reverse proxy. The
application rejects invalid sessions, disallowed Origin/Referer values, and
missing or mismatched `X-CSRF-Token` before invoking application services.
Successful login sets an opaque Secure/HttpOnly/SameSite=Strict cookie and
returns the session-bound CSRF value once; the cookie is never returned in
JSON.

Use secure, HttpOnly, SameSite=Strict cookies. Do not store session bearer tokens in browser local storage.

The setup-availability response is only a UI projection. The Auth service
atomically enforces that exactly one first Admin can be committed, so a stale
`true` response never authorizes a second account. No setup token is accepted
or returned. Before the first commit, any client that can reach the Admin
listener and satisfy its Origin policy can attempt the first claim; listener
publication is therefore the setup trust boundary.

### 10.3 API groups

```text
GET    /api/v1/dashboard
GET    /api/v1/system
GET    /api/v1/health/details
GET    /api/v1/diagnostics

GET    /api/v1/vault
PATCH  /api/v1/vault
POST   /api/v1/vault/rescan

GET    /api/v1/webdav/credentials
POST   /api/v1/webdav/credentials
PATCH  /api/v1/webdav/credentials/{id}
DELETE /api/v1/webdav/credentials/{id}

GET    /api/v1/mcp/tokens
POST   /api/v1/mcp/tokens
DELETE /api/v1/mcp/tokens/{id}
GET    /api/v1/mcp/oauth
PUT    /api/v1/mcp/oauth
GET    /api/v1/mcp/oauth/grants
POST   /api/v1/mcp/oauth/grants
DELETE /api/v1/mcp/oauth/grants/{id}
GET    /api/v1/mcp/connection-info

GET    /api/v1/providers/mode
PUT    /api/v1/providers/mode
GET    /api/v1/providers
POST   /api/v1/providers
GET    /api/v1/providers/{id}
PATCH  /api/v1/providers/{id}
DELETE /api/v1/providers/{id}
POST   /api/v1/providers/{id}/test
POST   /api/v1/providers/{id}/models/refresh
GET    /api/v1/model-bindings
PUT    /api/v1/model-bindings/{role}

GET    /api/v1/index/status
POST   /api/v1/index/rebuild
GET    /api/v1/index/nodes

GET    /api/v1/memories
GET    /api/v1/memories/{id}
PATCH  /api/v1/memories/{id}
POST   /api/v1/memories/{id}/archive
POST   /api/v1/memories/{id}/restore
POST   /api/v1/memories/merge
GET    /api/v1/memory-candidates
POST   /api/v1/memory-candidates/{id}/promote
POST   /api/v1/memory-candidates/{id}/reject

GET    /api/v1/jobs
GET    /api/v1/jobs/{id}
POST   /api/v1/jobs/{id}/retry
POST   /api/v1/jobs/{id}/cancel

GET    /api/v1/audit

GET    /api/v1/backups
POST   /api/v1/backups
POST   /api/v1/backups/{id}/verify
POST   /api/v1/restore/validate
POST   /api/v1/restore
POST   /api/v1/maintenance/recover
```

Connection info uses the configured canonical data public origin. Without an
external origin, direct-listener URLs include the actual data bind port; the
default WebDAV endpoint is
`http://127.0.0.1:8080/dav/v1/vaults/default/`. Host and Origin allow-lists are
validation policy and do not silently remove or replace the advertised port.

Deletion endpoints use explicit confirmation payloads and return operation/job IDs when asynchronous.

### 10.4 Admin error shape

```json
{
  "error": {
    "code": "validation_failed",
    "message": "One or more fields are invalid.",
    "fields": {
      "base_url": "HTTPS is required for a public provider endpoint."
    }
  },
  "request_id": "..."
}
```

### 10.5 Secret responses

Admin APIs never return stored secret plaintext.

Secret issuance responses return a generated password/token only once. List and
detail responses expose configured state, public prefixes, and masked hints;
the old secret is never returned after replacement. Backup creation, verify,
restore validation, and restore apply return bounded operation/job envelopes;
restore apply additionally requires `confirmation: "RESTORE"` and recent Admin
password reauthentication. Manifest summaries expose checksums/versions and
key version identifiers but never note bodies, provider secrets, or master-key
material.

Successful Admin mutations append a redacted audit fact with the request ID,
actor, plane, action, target identity, result, and bounded non-secret metadata.
The embedded console keeps an issued WebDAV password or MCP PAT only in
volatile component state until the operator hides it; it never persists the
secret in browser storage.

Provider mode is Vault-scoped and uses `disabled`, `local_only`, or
`remote_allowed`; responses include the optimistic setting revision. OAuth
grant list/create/revoke operations always derive the current `VaultContext`
from Admin state and never accept a caller-selected `vault_id`. Issuer responses
report cache presence/timestamp but never return the stored JWKS body.

Example:

```json
{
  "api_key": {
    "configured": true,
    "hint": "sk-proj-…9ab2"
  }
}
```

Replacing a secret requires sending a new value. An omitted field means unchanged; an explicit clear operation requires confirmation.

## 11. Health endpoints

Data-plane unauthenticated:

```text
GET /health/live
GET /health/ready
```

Responses contain no sensitive details.

Control-plane authenticated:

```text
GET /api/v1/health/details
```

Detailed health includes storage, database, migration, outbox, worker, index coverage, provider configuration, and backup age.

When `MCP_VAULT_METRICS_ENABLED=true`, the data listener also exposes
`GET /metrics` as bounded Prometheus text with fixed plane/status counters.
It never uses request paths, Vault slugs, credential IDs, note content, or
secret values as labels. During `read_only`, WebDAV/MCP mutation methods and
Admin state-changing routes return a temporary maintenance error while reads
and authenticated restore validation remain available. During `offline`, the
data plane is unavailable; the authenticated Admin restore/diagnostic surface
remains available for recovery.

## 12. Interface compatibility policy

- Released MCP tool names and required fields remain backward compatible within API v1.
- New optional fields may be added.
- Breaking tool schema changes require a new tool name or API/protocol version.
- Admin HTTP breaking changes require `/api/v2`.
- WebDAV behavior is protocol-defined; compatibility fixes require regression tests.
- Database schema versions are independent from public API versions.
