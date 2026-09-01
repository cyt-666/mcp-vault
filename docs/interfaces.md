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
progress, events, relationships, past work, or knowledge that may already
exist in ordinary Vault notes.

Treat related_notes returned by recall as retrieval cues and use read_note to
verify exact details. Treat recalled memories as sourced durable context,
observing their confidence, validity, lifecycle status, and provenance.

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
when semantic providers are unavailable. With an `embedding_note` binding it
uses deterministic note chunks for semantic/hybrid ranking. `recall` reports
semantic degradation while preserving projection-based lexical/context
results.

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

Output keeps two collections distinct:

- `memories`: atomic durable memories with status, confidence, importance,
  temporal validity, provenance, relations, and scores;
- `related_notes`: current revision-bound note cues with file ID, path, title,
  bounded matching snippet, tags/topics/headings, score, and resource URI.

`related_notes` is populated only when the credential also has `vault:read`.
The caller may bound it independently with `max_related_notes`. A cue is not an
accepted fact; the Agent reads the canonical note before relying on exact
details.

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

Purpose: stage an explicit sourced memory input for background consolidation.

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

Output is truthful about the asynchronous boundary:

```json
{
  "outcome": "staged",
  "memory": null,
  "raw_memory_id": "...",
  "consolidation_job_id": "..."
}
```

Recall changes only after the referenced Phase 2 job commits. Reusing an
idempotency key with the same input returns the existing raw identity; using it
with different input is rejected.

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

An internal_error remains redaction-safe but includes a bounded
details.component value (state, storage, vault_registry, or core). Storage
failures may also include the storage boundary's redacted
operation/error-kind diagnostic; absolute paths and raw operating-system error
strings are never returned.

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

### 9.2 Built-in OAuth authorization server

The default ChatGPT path is self-contained. When a Vault OAuth user is enabled,
the data listener implements:

- RFC 9728 protected resource metadata;
- `WWW-Authenticate` with `resource_metadata` on 401;
- RFC 8414 authorization-server metadata;
- bounded RFC 7591 Dynamic Client Registration for public clients;
- authorization code with mandatory PKCE `S256`;
- exact registered redirect URIs, RFC 8707 `resource`, and RFC 9207 `iss`;
- short-lived request handles, single-use authorization codes, and opaque access tokens;
- one-hour access tokens and rotating refresh tokens with a 180-day sliding
  idle lifetime, bounded retry grace, and replay-family revocation;
- `offline_access` for long-lived client connections without granting another
  Vault permission;
- Vault-bound MCP scopes and immediate local revocation.

For a path-based Vault resource, the canonical metadata endpoint is:

```text
GET /.well-known/oauth-protected-resource/mcp/v1/vaults/{vault_slug}
```

The origin-root `/.well-known/oauth-protected-resource` endpoint is an alias
only when exactly one active Vault has one unambiguous configured resource.
Metadata is public by design and returns only the exact resource identifier,
enabled authorization-server issuer URLs, supported scopes, header bearer
method, and protocol metadata. It never returns cached JWKS, subjects, grants,
tokens, or secrets.

Protected-resource `scopes_supported` contains only the eight Vault/memory
permission scopes. Authorization-server metadata additionally advertises
`offline_access`; this protocol scope is persisted with the grant but never
maps to an MCP permission or tool.

An unauthenticated or invalid request returns a same-origin challenge shaped
like:

```http
HTTP/1.1 401 Unauthorized
WWW-Authenticate: Bearer realm="mcp-vault", resource_metadata="https://vault.example.com/.well-known/oauth-protected-resource/mcp/v1/vaults/default", error="invalid_token", error_description="The bearer access token is invalid or expired"
```

Production OAuth requires `MCP_VAULT_DATA_PUBLIC_ORIGIN`, which makes the
challenge URL absolute without trusting the request `Host`. A direct local
development listener without that setting uses the equivalent same-origin
relative protected-resource path, but the built-in authorization server is not
advertised unless the configured origin is HTTPS or explicit loopback HTTP.

Public built-in endpoints are:

```text
GET  /.well-known/oauth-authorization-server
POST /oauth/register
GET  /oauth/v2/authorize
POST /oauth/v2/authorize
POST /oauth/token
```

`/oauth/v1/authorize` and `/oauth/authorize` remain compatibility aliases.
Their GET handlers issue a query-preserving, non-cacheable 307 redirect to the
current versioned endpoint instead of creating an authorization transaction at
an obsolete path. Their POST handlers remain available for already-rendered
legacy forms. Fresh metadata and the browser form use the current versioned
path. Reference proxies expose the `/oauth/` prefix rather than an exact leaf
path.

Registration accepts only public clients with token endpoint authentication
method `none`, response type `code`, and `authorization_code` plus optional
`refresh_token` grants. Redirect URIs are exact and must use HTTPS, except for
explicit loopback HTTP development callbacks. Authorization preserves the
client's opaque `state`, requires the exact MCP `resource`, and includes the
canonical issuer as `iss` in the redirect. Token requests are form encoded and
must repeat the same client, redirect, verifier, and resource.

The token endpoint is Host-validated but is not gated by the MCP data-plane
Origin allow-list. OpenAI hosts may exchange a code from a backend or may send
an application Origin or `Origin: null`; none of those values is OAuth client
authentication. The endpoint accepts no Admin/session cookie authority and
instead requires the exact public client, redirect URI, resource, single-use
code plus PKCE verifier, or a rotating refresh token.

Successful refresh gives the successor refresh token a new 180-day idle
lifetime. A duplicate use of the old token at or within 60 seconds returns
`invalid_grant` without invalidating the already-issued pair; reuse after that
grace revokes the complete family. Refresh `scope` may narrow Vault/memory
permissions but cannot add `offline_access`; an already granted offline
capability is inherited when the field is omitted or lists a business-scope
subset.

The browser-facing authorization form POST is Host-validated but is not gated
by the MCP data-plane Origin allow-list. System OAuth browsers and sandboxed
webviews may send `Origin: null` or the invoking application's origin. The form
instead requires the opaque, short-lived request handle created by the
validated authorization request; that handle remains bound to the exact client,
redirect URI, state, resource, scopes, and PKCE challenge. A correctly
authenticated retry of the same still-valid browser form receives a fresh
single-use authorization code. This makes duplicate browser/proxy POSTs safe
without making any authorization code replayable.

The login form accepts only the independent Vault OAuth username/password
configured on the Admin listener. It never accepts an Admin session or Admin
password. Passwords are Argon2id hashes; request handles, codes, access tokens,
and refresh tokens are stored only as versioned installation-keyed digests.
The form action is the absolute authorization endpoint derived from the
configured canonical public Origin. Interactive authorization HTML omits the
CSP `form-action` navigation directive for Chromium compatibility; error-only
HTML still uses `form-action 'none'`. This does not admit a request-derived
Host or wildcard action: the rendered action remains a fixed server-generated
URL, and the authorization POST must present the opaque transaction handle
bound to the exact client, redirect URI, state, resource, scopes, and PKCE
challenge. The rest of the policy remains deny-by-default.
The login form uses standard `username` and `current-password` autocomplete
semantics so browser password managers can fill it normally. OAuth responses
also send `Vary: *` in addition to explicit browser, CDN, and surrogate
no-store controls, preventing a shared cache from reusing a transaction page.
All OAuth HTML/JSON responses carry browser and intermediary no-store controls,
login pages deny framing and external content, and secrets are not logged.

The service does not pass MCP access tokens to LLM providers or any upstream API.

### 9.3 Optional external issuer compatibility

An operator that already runs an OAuth/OIDC provider may configure external
RS256 JWT validation and explicit Subject-to-Vault grants. The external server
must publish discovery metadata, support authorization code + PKCE `S256`,
support CIMD, DCR, or predefined client registration, and preserve `resource`.
MCP Vault accepts the exact resource indicator from `aud` or an explicit
`resource` claim while still checking issuer, signature, time, configured
audience, Subject grant, Vault, and scopes. External client secrets, access
tokens, refresh tokens, and private keys are never stored by MCP Vault.

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
Successful login sets an opaque HttpOnly/SameSite=Strict session cookie, sets
a separate SameSite=Strict CSRF cookie that the Admin frontend may read, and
returns the same session-bound CSRF value in the login response. Both cookies
carry `Secure` for an HTTPS Origin. For an explicitly configured localhost or
literal private/link-local IP HTTP Origin, they omit only `Secure`; public
cleartext origins fail startup validation. The CSRF value is not an
authentication bearer: every mutation must
still send it in `X-CSRF-Token`, where it is checked against the digest bound to
the authenticated session. `GET /api/v1/session` validates the HttpOnly session
after a page reload but does not return either stored bearer value. Logout
expires both cookies using the same transport-specific attribute mode.

Do not store the session bearer in JavaScript memory, local storage, or session
storage. The readable CSRF cookie exists only to reconstruct the mutation
header after reload and cannot authenticate a request by itself.

The setup-availability response is only a UI projection. The Auth service
atomically enforces that exactly one first Admin can be committed, so a stale
`true` response never authorizes a second account. No setup token is accepted
or returned. Before the first commit, any client that can reach the Admin
listener and satisfy its Origin policy can attempt the first claim; listener
publication is therefore the setup trust boundary.

### 10.3 API groups

Multi-Vault management uses an explicit Admin path scope:

```text
GET    /api/v1/vaults
POST   /api/v1/vaults
GET    /api/v1/vaults/{vault_slug}
PATCH  /api/v1/vaults/{vault_slug}
POST   /api/v1/vaults/{vault_slug}/rescan
POST   /api/v1/vaults/{vault_slug}/initialization/retry
```

`POST /vaults` accepts only `name` and `slug`. The service generates the ID and
content root and returns `202` with the Vault plus its durable
`vault.initialize` job. Vault summaries retain ID/slug/name/root/status/revision
and add effective `availability`: `initializing`, `ready`, `maintenance`,
`disabled`, or `error`.

Vault-owned groups below are also mounted at
`/api/v1/vaults/{vault_slug}/<group>`, including `dashboard`, `webdav`, `mcp`,
the Provider list/mode and model bindings, `index`, `memories`/`memory`, `jobs`,
and `audit`. The slug is resolved to `VaultContext`; no request body accepts a
Vault selector. System/health/diagnostics, Provider detail/model inventory, and
backup/restore remain global.

The historical unscoped forms remain compatibility aliases to the persisted
legacy-default Vault. They never select the first row after a second Vault is
created; when an upgraded database has several Vaults and no unique historical
default, they return `409 vault_selection_required`.

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
GET    /api/v1/mcp/oauth/local
PUT    /api/v1/mcp/oauth/local
DELETE /api/v1/mcp/oauth/local
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
GET    /api/v1/providers/{id}/models
POST   /api/v1/providers/{id}/models
POST   /api/v1/providers/{id}/models/refresh
GET    /api/v1/model-bindings
PUT    /api/v1/model-bindings/{role}

GET    /api/v1/index/status
POST   /api/v1/index/rebuild
GET    /api/v1/index/nodes

GET    /api/v1/memories
GET    /api/v1/memories/{id}
PATCH  /api/v1/memories/{id}
DELETE /api/v1/memories/{id}?expected_revision={revision}
POST   /api/v1/memories/{id}/archive
POST   /api/v1/memories/{id}/restore
POST   /api/v1/memories/merge
GET    /api/v1/memory/extraction
PUT    /api/v1/memory/extraction
POST   /api/v1/memory/extraction/run
POST   /api/v1/memory/extraction/restart

GET    /api/v1/jobs
GET    /api/v1/jobs/overview
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

`GET /api/v1/mcp/connection-info` returns `mcp_endpoint`, the exact
`oauth_protected_resource_metadata_url`, and the built-in
`oauth_authorization_server_metadata_url` derived from the configured public
data origin; no value is derived from an untrusted request `Host` header.

Memory archive and restore accept
`{"expected_revision": <current revision>}`. Permanent deletion uses the
required `expected_revision` query parameter so generic HTTP clients do not
need a DELETE request body. All three operations are authenticated,
Origin/CSRF checked, Vault-scoped, revision-aware, and audited. Permanent
deletion removes the current managed Markdown and memory projection; retained
file history and backup artifacts remain governed by their own retention
policies.

The extraction policy returned by `GET /api/v1/memory/extraction` and accepted
by `PUT` contains `enabled`, fixed `source_mode: "automatic"`,
and `request_timeout_seconds`. `max_evidence_per_note` and legacy
`max_candidates_per_note` remain accepted/returned as prerelease compatibility
fields but do not affect Phase 1 v4. `explicit_only` and `all_notes` are source-
mode migration aliases. Source admission does not depend on note frontmatter,
tags, paths, or folders, and there are no model-score threshold fields.

The status response separates `phase1_readiness` and `phase2_readiness`, adds a
combined `readiness`, Stage 1 `total|ready|no_output|withdrawn|pending` counts,
and redacted consolidation generation/success/cutover state. The complete manual
pipeline requires both `memory_extraction` and `memory_consolidation` bindings.
Cutover state reports `pipeline_generation` and `regeneration_pending`; startup
and periodic admission clear the latter only after a current-generation fresh
Phase 1 full-Vault job exists.

The Phase 1 Provider root is:

```json
{
  "raw_memory": "...",
  "rollout_summary": "...",
  "rollout_slug": "..."
}
```

An empty result uses empty semantic strings and `rollout_slug: null`. Local code
derives the note file/path/revision and normalized whole-source hash; the model
never returns evidence coordinates. Phase 1 does not write final memory. When a
Provider returns valid `raw_memory` but omits only `rollout_summary`, the adapter
may copy the already returned `raw_memory` string into that auxiliary field and
then rerun the complete three-field schema validation. It does not repair an
empty object or invent source provenance.

`POST /memory/extraction/run` accepts optional
`{"include_evaluated": false}`. The default skips current successful Stage 1
coverage before a Provider call; `true` includes unchanged notes. It returns
the active `memory.extract` job with `admission: "existing"` instead of creating
a duplicate, or a queued job with `admission: "queued"`. Phase 1 automatically
admits the singleton `memory.consolidate` follow-up. If a required fresh
regeneration is pending, saving the final ready extraction policy/model binding
immediately admits its singleton full-Vault job; the run endpoint may perform
the same admission and return that job rather than rejecting the operator until
the five-minute reconciliation loop runs. The compatibility restart
endpoint cancels non-running Phase 1 work and admits a full re-extraction; it no
longer clears or exposes candidate review state.

`GET /api/v1/index/status` returns ordinary Markdown/FTS coverage separately
from `note_semantic`: effective `embedding_note` model, current/expected chunk
counts, stale vectors, coverage ratio, and stable readiness blockers. A missing
semantic binding is not an index failure because lexical retrieval remains
operational.

Connection info uses the configured canonical data public origin. Without an
external origin, direct-listener URLs include the actual data bind port; the
default WebDAV endpoint is
`http://127.0.0.1:8080/dav/v1/vaults/default/`. Host and Origin allow-lists are
validation policy and do not silently remove or replace the advertised port.

Provider model discovery is optional. `GET/POST
/api/v1/providers/{id}/models` lists or manually registers provider-specific
model IDs and typed capability metadata; refresh uses the provider model-list
operation when available. Model-role binding remains a separate operation so a
Base URL or provider display name is never mistaken for a model selection.

Manual registration accepts typed model settings. For an OpenAI-compatible
generation model, the relevant shape is:

```json
{
  "external_model_id": "mimo-v2.5",
  "capabilities": {
    "structured_output": true,
    "context_window": null,
    "max_output_tokens": null
  },
  "settings": {
    "openai_compatibility_preset": "auto",
    "openai_structured_output_mode": "auto",
    "openai_token_limit_field": "auto",
    "openai_thinking_mode": "auto",
    "generation_token_limit": null
  },
  "enabled": true
}
```

Existing `{}` settings mean every axis is `auto`. Compatibility preset values
are `generic`, `deepseek`, `xiaomi_mimo`, `zhipu_glm`, `moonshot_kimi`,
`google_gemini`, and `alibaba_qwen`; output modes are
`strict_json_schema`, `json_object`, and `prompt_only`; token fields are
`max_tokens` and `max_completion_tokens`; thinking is `enabled` or `disabled`.
Each enum also accepts `auto`. Nullable `generation_token_limit` is a bounded
one-call generated-token ceiling. Responses return the stored typed settings
and revision. Arbitrary headers, secrets, and request bodies are not model
settings.

`POST /api/v1/providers` accepts first-class `provider_type` values
`deepseek`, `xiaomi_mimo`, `zhipu_glm`, `moonshot_kimi`, `google_gemini`, and
`alibaba_qwen` in addition to the existing OpenAI, Anthropic, generic, and
embedding types. Base URL is the exact API root; suffixes are appended without
inserting an extra `/v1` into an existing provider path.

`PATCH /api/v1/providers/{id}` accepts the same complete Provider fields plus
an optional `expected_revision`. `secret: null` preserves the current secret;
a non-empty secret rotates it. A stale revision returns HTTP 409 with
`revision_conflict`, and successful edits return the next revision with only a
masked secret hint.

`DELETE /api/v1/providers/{id}?expected_revision=<revision>` performs one
Provider lifecycle transaction. The revision query is optional for older
Admin clients but the bundled console always supplies it. The response is
redacted and has this shape:

```json
{
  "deleted": true,
  "provider_id": "provider-id",
  "models_deleted": 2,
  "bindings_deleted": 1,
  "embeddings_deleted": 42,
  "secrets_deleted": 1
}
```

Bindings are removed across global and all Vault override scopes. Embedding
metadata/vectors are derived and removed across their Vault partitions;
canonical notes, durable memories, job history, and audit history
are retained. The deletion audit fact stores only these counts.

`GET /api/v1/index/status` and the dashboard return `indexed_notes`,
`total_notes`, and a nullable numeric `coverage_ratio`; the structured
`coverage` object remains the detailed analyzer/degradation record. A zero-note
Vault reports an unknown ratio rather than a false `0%` failure.

The extraction endpoint returns the typed policy, optimistic revision, separate
Phase 1/Phase 2 readiness, raw-input counts, and consolidation generation.
Manual admission requires extraction enabled, Provider policy enabled, and both
model roles usable. `GET /api/v1/jobs` accepts optional `status` and exact
`job_type` filters. Completed jobs project progress ratio `1.0`; unknown
non-terminal progress remains null.

`GET /api/v1/jobs/overview?limit={history_limit}&offset={history_offset}` is the
Admin operational projection. It returns separate `running`, `queued`,
`retry_wait`, and terminal `history` arrays; exact per-status counts; truncation
flags for bounded waiting projections; and `next_history_offset`. Running jobs
are queried independently at the server maximum page size, so the bounded
terminal history cannot hide an older long-running task. Every row remains
Vault-scoped and uses the same redacted `job_summary` contract as
`GET /api/v1/jobs`.

Full-Vault Phase 1 reports `phase`, `completed`, `total`, `current_index`,
`current_path`, `last_completed_path`, `note_started_at`,
`last_note_elapsed_ms`, `notes_evaluated`, `raw_memories_staged`,
`phase1_no_output`, `source_policy_skipped`, `already_evaluated_skipped`,
`source_ingestion_failures`, bounded `source_ingestion_failure_notes`,
`generated_output_failures`, bounded `generated_output_failure_notes`, and
nullable `error_code`.
Phase 2 reports cumulative `completed|total|raw_inputs`,
`pending_raw_inputs`, `created`, `updated`, `retired`, `discarded`,
`generation`, and `reused_proposal` while one job drains bounded generations.
Pipeline reset reports removed managed memory files; cleared final/Stage 1/
candidate/proposal/diagnostic/vector counts; and fresh-regeneration follow-up
state.

Job `details` exposes the non-secret `include_evaluated` mode. Each failure-note
array retains at most 20 objects containing ordinal, source path, stable error
code, and elapsed time; generated-output diagnostics may additionally contain
trusted `schema_issue`/`schema_path`. A source-ingestion failure occurs before a
Provider call and includes missing, unreadable, over-512-KiB, and non-UTF-8
notes. A generated-output failure occurs after a Provider call and includes
wire-schema, Stage 1 bounds, and evidence-anchor/source mismatches. Responses
and logs never expose arbitrary payloads, note content, prompts, Provider
response text, or secrets.

Provider-backed jobs may report the stable redacted codes
`provider_response_content_type_invalid`, `provider_response_json_invalid`,
`provider_final_content_missing`, `provider_structured_json_invalid`,
`provider_output_truncated`, `provider_output_filtered`, or
`provider_output_repetition_truncated`. `provider_schema_invalid` additionally
reports one of `type_mismatch`, `enum_mismatch`,
`required_property_missing`, `unexpected_property`, `array_too_long`, or
`array_too_short` plus its trusted schema path. They expose response-contract
state, not response text.

One source-ingestion or generated-output failure is a completed note unit
inside a full-Vault backfill, and later notes continue. Only generated-output
failures participate in the consecutive cost-safety circuit. A mixed run
finishes with progress phase
`completed_with_errors` and normal terminal job status `completed`. Three
consecutive output-contract failures stop the job with progress phase
`stopped_output_failures` and error
`memory_extract_output_failure_limit`; an explicit job retry preserves the
full-Vault `last_completed_path` cursor. Systemic configuration,
authentication, endpoint, state, lease, and retryable transport errors retain
their job-level behavior.

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
