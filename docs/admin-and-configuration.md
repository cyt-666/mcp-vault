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

The control plane is intended for localhost, LAN, or VPN access only. Listener
publication and source-network restrictions are deployment responsibilities
implemented by host/container bindings, firewall/VPN policy, or an
operator-selected reverse proxy. MCP Vault does not enforce Admin source
CIDRs.

Network restriction does not replace an admin password.

## 2. Listener model

Recommended defaults inside the container:

```toml
[data_plane]
bind = "0.0.0.0:8080"

[admin_plane]
bind = "0.0.0.0:8081"
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

MCP Vault does not use `X-Forwarded-For` to decide who may access Admin. If an
operator wants source-IP policy, the deployment layer enforces it before
proxying to the Admin listener.

## 3. First-run bootstrap

### 3.1 Preconditions

Setup mode is active only when no Admin account exists.

The unauthenticated Admin shell reads `GET /api/v1/setup` before choosing an
authentication flow. A fresh installation displays only the first-Admin form;
an initialized installation displays only Admin login. If the status cannot be
confirmed, the shell fails closed to login. The status is presentation state,
not authorization: `POST /api/v1/setup` still atomically rejects every request
after the first Admin commit, including races from another browser.

The setup route is available only on the Admin listener. Whether a source can
reach that listener is controlled by the deployment.

### 3.2 First-claim boundary

Setup requires only the desired Admin username and password. MCP Vault does
not generate, store, display, or accept a bootstrap token, and it has no
predictable default account.

Before the first Admin commit, every client that can reach the Admin listener
and satisfy its exact Origin policy can attempt setup. The default listener is
loopback so a fresh direct installation is claimed locally. If an operator
publishes Admin to LAN/VPN before setup, that deployment boundary must admit
only clients trusted to become the owner. The state repository's atomic first
insert guarantees one winner but cannot identify which reachable person should
win.

### 3.3 Setup flow

1. The owner opens the Admin listener.
2. The UI confirms that setup is available.
3. The owner enters the initial Admin username and password.
4. The service initializes the default Vault registry entry or validates the configured content root.
5. The startup-provisioned installation-key version remains the active key for
   encrypted secrets and keyed credentials.
6. Setup mode is permanently disabled unless the database is deliberately reset through a local recovery command.

## 4. Admin authentication

### 4.1 Password

- Hash with Argon2id using current recommended parameters.
- Store algorithm parameters with the hash.
- Support transparent rehash after login when parameters change.
- The default minimum is 12 UTF-8 bytes: pure ASCII/English therefore needs at
  least 12 characters. The Chinese UI recommends at least eight Han characters
  rather than encouraging a short password that merely satisfies byte length.
- Reject control characters and the case-insensitive placeholders `password`,
  `password123`, `changeme`, `admin`, `admin123`, and `letmein`.
- Do not impose arbitrary composition rules that encourage weak patterns.
- Show these rules beside every Admin/WebDAV password-creation input and repeat
  the actionable rule when validation fails; never expose only a generic
  “password is unsafe” message.
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

Environment variables are for process-bootstrap values needed before runtime
configuration loads and optional installation-key overrides. They are not the
primary long-term UI configuration store; the default installation key is
provisioned by MCP Vault itself.

### 5.1 Bootstrap-only settings

Examples:

```text
MCP_VAULT_DATA_DIR
MCP_VAULT_SECRETS_DIR
MCP_VAULT_DATABASE_URL
MCP_VAULT_MASTER_KEY_FILE
MCP_VAULT_DATA_BIND
MCP_VAULT_ADMIN_BIND
MCP_VAULT_DATA_HOSTS
MCP_VAULT_DATA_ORIGINS
MCP_VAULT_DATA_PUBLIC_ORIGIN
MCP_VAULT_ADMIN_ORIGINS
MCP_VAULT_TRUSTED_PROXY_IPS
MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS
MCP_VAULT_BACKUP_DIR
MCP_VAULT_BACKUP_MAX_ENTRY_BYTES
MCP_VAULT_BACKUP_MAX_TOTAL_BYTES
MCP_VAULT_BACKUP_MAX_ARCHIVE_BYTES
MCP_VAULT_BACKUP_MAX_ENTRIES
MCP_VAULT_BACKUP_KEEP_COUNT
MCP_VAULT_METRICS_ENABLED
MCP_VAULT_OTEL_ENDPOINT
RUST_LOG
```

`MCP_VAULT_DATA_HOSTS` is a comma-separated allow-list of exact HTTP Host
authorities accepted by the MCP transport. Values are host names or IP
authorities, optionally including a port; they must not contain a scheme,
path, query, fragment, whitespace, or userinfo. It defaults to local
development authorities and must be set to the public hostname when a
reverse proxy or LAN hostname is used. This Host policy is independent from
the `MCP_VAULT_DATA_ORIGINS` browser-Origin policy.

`MCP_VAULT_DATA_PUBLIC_ORIGIN` is the single canonical `http(s)` origin shown
in generated WebDAV/MCP connection cards. Set it to the external reverse-proxy
origin, for example `https://vault.example.com` or
`https://vault.example.com:8443`. If absent, MCP Vault first reuses a configured
data Origin; when no data Origin exists, it generates a direct-listener URL
from `MCP_VAULT_DATA_HOSTS` and the actual `MCP_VAULT_DATA_BIND` port. Thus the
default local card is `http://127.0.0.1:8080`, not port 80. This advertised
origin does not grant Host/Origin access and remains separate from both
allow-lists.

`MCP_VAULT_TRUSTED_PROXY_IPS` is a comma-separated list of exact socket-peer
IP addresses. It is empty by default. WebDAV accepts
`X-Forwarded-Proto: https` only when the direct peer is in this list; the
header alone never makes plaintext Basic Authentication secure. Loopback
clients may use Basic Authentication over local HTTP. This setting does not
trust forwarded client addresses or grant any Vault permission.

`MCP_VAULT_SECRETS_DIR` defaults to `<MCP_VAULT_DATA_DIR>/secrets` and owns the
automatically generated `master-key` file. Ordinary MCP Vault backups exclude
this directory. An operator may mount it as a separate persistent volume or
override the key with `MCP_VAULT_MASTER_KEY_FILE`. MCP Vault does not inspect
or change filesystem permission bits on this file.

`MCP_VAULT_BOOTSTRAP_TOKEN` and `MCP_VAULT_BOOTSTRAP_TOKEN_FILE` are obsolete
and rejected at configuration load. Remove them when upgrading; setup no
longer accepts a token. Startup removes only the former service-owned
`<MCP_VAULT_SECRETS_DIR>/bootstrap-token` path. An old explicit token file at
another path is never read or deleted and may be removed by its operator.

`MCP_VAULT_ADMIN_ALLOWED_CIDRS` is obsolete and rejected at configuration
load so an old setting cannot create a false belief that the application still
enforces a network allow list. Move that policy to listener publication,
firewall/VPN, or reverse-proxy configuration, then remove the variable.

`MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS` controls the authoritative
polling pass used to recover from missed filesystem events. It defaults to
300 seconds and accepts values from 1 through 86400 seconds.

`MCP_VAULT_BACKUP_DIR` is a service-owned absolute directory for verified tar
artifacts. The backup size/count settings are numeric byte/entry limits and
are enforced before staging or extraction; `MCP_VAULT_BACKUP_KEEP_COUNT`
protects the last verified artifacts from automatic retention cleanup.
`MCP_VAULT_METRICS_ENABLED` opt-in exposes only bounded, non-sensitive
Prometheus text at the data listener's `/metrics`. `MCP_VAULT_OTEL_ENDPOINT`
opt-in exports redacted tracing spans through OTLP HTTP; it is unset by
default. Backup manifests expose only retained encryption-key version
identifiers; the master-key material remains outside ordinary backups.
The OTLP endpoint must be an absolute HTTP(S) URL without embedded userinfo.

When `MCP_VAULT_MASTER_KEY_FILE` is unset, MCP Vault atomically creates and
reuses `<MCP_VAULT_SECRETS_DIR>/master-key`. An explicit file must contain a
regular 32-byte raw or 64-character hex key and remains operator-managed. Once
provider ciphertext, an MCP PAT, or an installation-key check exists, startup
fails when the effective file is missing or its one-way verifier does not
match; the service never replaces a lost established key.

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

The first-release console uses Simplified Chinese and groups destinations by
the operator's task rather than presenting every subsystem as one flat list:

```text
首次访问
  ├── 未初始化：首次初始化
  └── 已初始化：管理员登录

常用
  ├── 总览
  └── Vault 设置
连接
  ├── Obsidian 同步（WebDAV）
  └── Agent 接入（MCP）
智能
  ├── AI 服务
  ├── 知识索引
  └── 长期记忆
运维
  ├── 后台任务
  ├── 备份与恢复
  ├── 审计日志
  └── 系统信息
```

Each page presents a concise status summary and common action first. OAuth
issuer/grant configuration, restore/recovery controls, and raw API responses
are progressive disclosures and remain collapsed until explicitly opened.
Credentials, jobs, memories, providers, backups, audit entries, index state,
and system state use page-specific lists or cards instead of a raw JSON dump.
Raw disclosures may contain the private paths or memory content already
authorized for that page, but stored secret plaintext is never returned.
Technical protocol names, scopes, IDs, paths, and URLs remain exact values even
when surrounding labels and guidance are Chinese.

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

The implemented control-plane surface lists, creates/updates, and revokes
subject grants for the current Vault. It accepts normalized `RS256` RSA public
JWKS only. Manually supplied keys stay valid until an audited update; automatic
discovery refresh is not claimed by this release.

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

The page also sets the current Vault's explicit `disabled`, `local_only`, or
`remote_allowed` privacy mode with optimistic revision handling. All Admin,
MCP, memory, and worker calls share one process `ProviderService`, so a
provider's maximum concurrency is enforced across planes rather than per HTTP
request.

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

Provider tests persist only redacted health state and model metadata. A test
failure never exposes response bodies or secrets and does not make the Vault
unready. Configuration updates use optimistic revisions; model bindings first
check the Vault-specific override and then the global default.

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

### 13.1 Memory service and runtime boundary

Memory commands are application-service operations. The MCP and future Admin
HTTP adapters authenticate the caller, resolve the endpoint-bound
`VaultContext`, validate DTOs, and call `MemoryService`; they do not read the
Vault filesystem, execute memory SQL, invoke providers, or update projections
directly. The service writes canonical records below the Vault reserved root
through Vault Core, so managed memory Markdown receives the same atomic-write,
revision, history, audit, and outbox guarantees as other Core-managed data.

The reserved memory namespace is not an ordinary WebDAV or MCP file path and
is excluded from note indexing and normal filesystem reconciliation. It remains
portable Markdown, while SQLite memory rows, FTS, entities, relations,
candidates, and embeddings are rebuildable operational projections.

Automatic extraction is admitted as a Vault-scoped durable job containing only
file identity, path, revision, and pipeline references. The worker rechecks
the source revision before promotion and treats provider output as an
untrusted candidate. `memory.revalidate`, `memory.rebuild`, and
`embedding.rebuild` are separate jobs; provider outages degrade recall to
lexical/context retrieval and do not make canonical memory writes fail.

The runtime stores the following Vault-scoped settings through the typed
configuration API rather than an unvalidated key/value editor:

- extraction policy, candidate thresholds, and schema/prompt version;
- recall weights, result/token budgets, and optional embedding role binding;
- memory retention, source invalidation, and diagnostic policies.

Changing thresholds, bindings, or worker concurrency is hot-reloadable when
the setting schema permits it. Changing an analyzer, embedding model, or
taxonomy schedules a derived projection rebuild and never rewrites canonical
memory Markdown implicitly. The WP-12 Admin API/UI exposes these operations;
the WP-11 service boundary is already present for MCP and worker callers.

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

The UI shows catalog/job state rather than blocking on archive creation. Restore
validation runs against private staging and reports manifest/target/checksum
results without changing configured roots. Applying restore requires typing
`RESTORE` and entering the current Admin password; the data plane remains
offline until post-restore integrity and Core recovery checks pass.

## 17. System page

Display:

- version/build commit;
- Rust/SQLite versions;
- schema migration version;
- enabled Cargo/runtime features;
- data paths;
- listener addresses and a reminder that publication/source-network policy is
  deployment-owned;
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

First-release usability requirements:

- all operator-facing copy and known error states are Simplified Chinese;
- navigation is grouped into common, connection, intelligence, and operations
  tasks;
- WebDAV permissions and common PAT scopes use labeled choices, while the API
  still validates the submitted stable wire values;
- advanced and destructive controls use progressive disclosure and explicit
  confirmations;
- one-time secrets remain visually prominent and offer a copy action without
  storing the value in browser persistence;
- desktop and narrow/mobile layouts preserve the same capabilities.

Accessibility requirements:

- keyboard navigation;
- semantic labels;
- readable error summaries;
- no color-only state;
- confirmation dialogs with explicit action text.

## 20. Configuration acceptance tests

- setup route is unavailable on the data listener;
- setup status changes from available to unavailable after the first Admin is
  committed, and the unauthenticated UI removes the registration form;
- setup is available only on the Admin listener, accepts only username/password,
  and requires exact Origin policy;
- setup cannot run after first admin creation;
- session cookie and CSRF behavior pass browser tests;
- secret create/update never returns stored plaintext;
- changing embedding model schedules re-embedding rather than mixing vectors;
- provider outage is visible without breaking core readiness;
- an invalid private/public provider URL is rejected or explicitly confirmed;
- Vault-scoped settings cannot affect another fixture Vault;
- Admin UI remains usable when LLM/embedding are disabled;
- public reverse-proxy fixture cannot reach Admin listener.
