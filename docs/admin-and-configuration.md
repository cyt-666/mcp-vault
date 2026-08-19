# Administration and Configuration

## 1. Control-plane principles

The administration console is the privileged control plane for MCP Vault.

It is separate from the WebDAV and MCP data/agent plane in all of the following ways:

- separate listener and port;
- separate authentication;
- separate routes;
- separate rate limits;
- separate network exposure;
- separate CSRF and browser security policy;
- distinct audit actor type.

The control plane is intended for localhost, LAN, or VPN access only. This restriction is enforced primarily at the host/container/reverse-proxy layer and secondarily through an application CIDR allow list.

Network restriction does not replace an admin password.

## 2. Listener model

Recommended defaults inside the container:

```toml
[data_plane]
bind = "0.0.0.0:8080"

[admin_plane]
bind = "0.0.0.0:8081"
allowed_cidrs = [
  "127.0.0.0/8",
  "::1/128",
  "10.0.0.0/8",
  "172.16.0.0/12",
  "192.168.0.0/16"
]
```

Recommended default Docker publication:

```yaml
ports:
  - "8080:8080"
  - "127.0.0.1:8081:8081"
```

To make Admin available to a LAN, the operator explicitly maps the host’s LAN address:

```yaml
ports:
  - "192.168.1.20:8081:8081"
```

VPN ranges such as Tailscale’s CGNAT space are configured explicitly.

The application must not blindly trust `X-Forwarded-For`. Forwarded addresses are honored only from configured trusted proxies.

## 3. First-run bootstrap

### 3.1 Preconditions

Setup mode is active only when no admin account exists.

The setup route is available only on the Admin listener and only from an allowed network.

### 3.2 Bootstrap secret

The deployment provides a one-time bootstrap secret through one of:

- `MCP_VAULT_BOOTSTRAP_TOKEN`;
- a root-readable mounted secret file;
- an interactive local CLI command that generates a short-lived token.

Do not derive setup authorization from a predictable default password.

### 3.3 Setup flow

1. The owner opens `/setup`.
2. The UI requests the one-time bootstrap token.
3. The owner creates the initial admin username/password.
4. The service initializes the default Vault registry entry or validates the configured content root.
5. The service creates an encrypted-secret key version.
6. Setup mode is permanently disabled unless the database is deliberately reset through a local recovery command.
7. The bootstrap token is invalidated and must not be written to audit/log plaintext.

## 4. Admin authentication

### 4.1 Password

- Hash with Argon2id using current recommended parameters.
- Store algorithm parameters with the hash.
- Support transparent rehash after login when parameters change.
- Enforce minimum length and reject known placeholder/default values.
- Do not impose arbitrary composition rules that encourage weak patterns.
- Rate-limit login attempts per source and account.

### 4.2 Session

Use an opaque high-entropy session token in a cookie:

```text
Secure
HttpOnly
SameSite=Strict
Path=/
```

Store only a digest in SQLite.

Session policy:

- idle timeout;
- absolute timeout;
- explicit logout/revocation;
- revoke all sessions after password change;
- rotate session after login;
- optional “remember this trusted browser” with a bounded duration.

### 4.3 CSRF and Origin

Every state-changing Admin request requires:

- valid session cookie;
- valid Origin/Referer policy;
- CSRF token bound to the session;
- content type validation.

Do not use CORS to make Admin reachable from arbitrary origins.

## 5. Configuration hierarchy

Configuration sources, from lowest to highest precedence:

1. compiled safe defaults;
2. bootstrap file/environment settings required before database access;
3. global settings stored in SQLite;
4. Vault-scoped settings stored in SQLite;
5. explicit request/task overrides permitted by schema.

Environment variables are for bootstrap and secrets that must exist before runtime configuration loads. They are not the primary long-term UI configuration store.

### 5.1 Bootstrap-only settings

Examples:

```text
MCP_VAULT_DATA_DIR
MCP_VAULT_DATABASE_URL
MCP_VAULT_MASTER_KEY_FILE
MCP_VAULT_BOOTSTRAP_TOKEN_FILE
MCP_VAULT_DATA_BIND
MCP_VAULT_ADMIN_BIND
RUST_LOG
```

Changing a database-stored setting through an environment variable after setup must not create ambiguous precedence. The UI shows the effective source and marks non-editable bootstrap values.

### 5.2 Typed settings

Settings must be represented by Rust types and versioned schemas.

Do not expose a generic key/value editor as the normal UI.

Each change:

- validates;
- increments a settings revision;
- records actor and old/new redacted metadata;
- emits a settings event;
- triggers only necessary worker reload/rebuild behavior.

## 6. Admin information architecture

Recommended navigation:

```text
Setup
Dashboard
Vault
WebDAV
MCP Access
AI Providers
Knowledge Index
Memory
Jobs
Audit
Backup & Restore
System
```

## 7. Dashboard

Display:

- service version and uptime;
- Vault status and current revision;
- note/attachment counts;
- FTS/index revision and coverage;
- memory counts and candidate queue;
- embedding coverage;
- pending/failed jobs;
- last successful backup;
- WebDAV/MCP request status;
- provider health;
- warnings requiring action.

Do not display note or memory contents on the dashboard.

## 8. Vault page

Functions:

- show Vault name, slug, content root, reserved root;
- validate filesystem permissions;
- show initial/reconciliation scan status;
- configure index include/exclude globs;
- configure attachment indexing metadata;
- configure `.obsidian` indexing/sync policy;
- trigger rescan and projection rebuild;
- show out-of-band change warnings;
- configure revision/history and trash retention.

The first release may not expose “Create another Vault,” but the page and backend use a Vault ID internally.

## 9. WebDAV page

Functions:

- display generated DAV URL;
- create app credentials;
- name credentials by device/client;
- choose read/write/delete permissions;
- set optional expiry;
- reveal generated password once;
- revoke or rotate;
- show last use and recent failures;
- display tested Obsidian plugin setup guidance;
- warn that client-side encryption prevents server indexing/AI access;
- provide “test credentials” using an internal DAV request or a safe protocol check.

Never display an existing password.

## 10. MCP Access page

### 10.1 Connection information

Display:

- per-Vault MCP endpoint;
- supported MCP revisions;
- available authorization modes;
- example client configuration;
- server instructions preview;
- current tool scopes.

### 10.2 Personal access tokens

Allow:

- create named token;
- select scopes;
- set expiry;
- reveal once;
- copy masked prefix;
- revoke;
- inspect last use.

The UI warns when granting delete/history/manage scopes.

### 10.3 OAuth resource server

Allow configuring:

- issuer URL;
- protected resource URL/audience;
- discovery/JWKS behavior;
- subject-to-Vault grants;
- scope mapping;
- token claim mapping when an issuer uses a non-default subject claim;
- test token validation through a local diagnostic that never logs the token.

Protected-resource metadata preview must be available.

## 11. AI Providers page

### 11.1 Provider record

Fields:

```text
name
provider type
base URL
authentication type
API key or secret
organization/project headers
default timeout
connect timeout
maximum retries
maximum concurrency
TLS policy
redirect policy
privacy classification
```

Provider types:

- OpenAI Responses API;
- OpenAI-compatible chat/structured generation;
- Anthropic Messages API;
- local OpenAI-compatible endpoint;
- remote embedding endpoint;
- local embedding runtime.

Provider adapters must remain project-owned abstractions even when several types share HTTP shapes.

### 11.2 URL policy

- Public providers require HTTPS.
- HTTP is permitted only for loopback or explicitly allowed private endpoints.
- Redirects are disabled by default or restricted to same-origin HTTPS.
- The UI warns before allowing link-local, Docker socket, metadata-service, or untrusted private destinations.
- Provider tests use the configured timeout and sanitized errors.

### 11.3 Model discovery and registration

The UI supports:

- refresh model list when provider supports it;
- manually enter model ID;
- assign capabilities;
- validate structured output;
- record context/output limits;
- record embedding dimension;
- test a minimal non-sensitive request.

Do not assume a provider’s model-list endpoint is universally available or accurate.

### 11.4 Model roles

Configure global role bindings:

```text
memory_extraction
memory_consolidation
note_summary
topic_enrichment
embedding_note
embedding_memory
rerank
```

Future Vault-specific overrides inherit from global defaults.

Each role has:

- provider/model;
- temperature or deterministic setting;
- max input/output;
- concurrency;
- timeout;
- prompt/schema version;
- enabled status.

## 12. Knowledge Index page

Display:

- analyzer version;
- note/index coverage;
- pending/failed analysis jobs;
- FTS status;
- topic nodes;
- taxonomy validation;
- semantic clustering status;
- summary/model version;
- last reconciliation.

Actions:

- rebuild metadata/FTS;
- rebuild topic projection;
- validate `_mcp-vault/index.yaml`;
- preview root overview;
- inspect notes assigned to a topic;
- schedule re-embedding separately.

Rebuild actions must state which data is derived and which canonical data will not be touched.

## 13. Memory page

Defined in detail in `memory-system.md`.

The UI includes:

- active/candidate/stale/superseded/archived counts;
- full memory browser;
- provenance/source links;
- candidate review;
- merge and supersession;
- edit/archive/delete;
- extraction policy;
- recall simulator;
- prompt/provider/pipeline metadata;
- embedding coverage and failures.

## 14. Jobs page

Display:

- queued/running/retry/failed/completed;
- job type and Vault;
- progress;
- attempts and next retry;
- sanitized last error;
- dependency/dedup key;
- start/completion times.

Actions:

- retry failed;
- cancel queued/running when supported;
- pause a worker class;
- resume;
- drain before shutdown/upgrade.

Do not allow arbitrary payload editing.

## 15. Audit page

Filters:

- time range;
- plane;
- actor/credential;
- Vault;
- action;
- result;
- target type.

Show redacted metadata and request ID.

Offer export in a structured format without secrets or note bodies.

## 16. Backup and restore page

Functions:

- configure local backup directory and retention;
- create consistent backup;
- verify checksum and manifest;
- list backups;
- download manifest;
- validate a restore archive without applying;
- perform restore in maintenance mode;
- show whether history and operational state are included;
- explain master-key handling.

A restore requires reauthentication and explicit confirmation.

## 17. System page

Display:

- version/build commit;
- Rust/SQLite versions;
- schema migration version;
- enabled Cargo/runtime features;
- data paths;
- listener addresses and effective allowed CIDRs;
- trusted proxies;
- storage/database health;
- log level;
- diagnostic bundle generation.

Diagnostic bundles must redact secrets and omit note/memory contents unless the owner explicitly includes selected samples.

## 18. Runtime reload behavior

Settings are classified:

### Hot reload

- provider concurrency/timeout;
- model bindings;
- recall weights;
- extraction thresholds;
- job concurrency;
- rate limits;
- allowed origins.

### Worker restart/rebuild

- analyzer version;
- embedding model;
- taxonomy;
- include/exclude policy.

### Process restart required

- listener bind;
- database path;
- data directory;
- master-key source;
- low-level SQLite settings.

The UI reports required action before save.

## 19. Frontend implementation

Recommended stack:

- React + TypeScript;
- Vite;
- React Router;
- TanStack Query;
- Ant Design;
- Zod for client-side response/form validation;
- Vitest and Testing Library;
- Playwright for end-to-end setup and critical flows.

The backend remains authoritative for validation.

Accessibility requirements:

- keyboard navigation;
- semantic labels;
- readable error summaries;
- no color-only state;
- confirmation dialogs with explicit action text.

## 20. Configuration acceptance tests

- setup route is unavailable on the data listener;
- setup requires valid bootstrap token and allowed source;
- setup cannot run after first admin creation;
- session cookie and CSRF behavior pass browser tests;
- secret create/update never returns stored plaintext;
- changing embedding model schedules re-embedding rather than mixing vectors;
- provider outage is visible without breaking core readiness;
- an invalid private/public provider URL is rejected or explicitly confirmed;
- Vault-scoped settings cannot affect another fixture Vault;
- Admin UI remains usable when LLM/embedding are disabled;
- public reverse-proxy fixture cannot reach Admin listener.
