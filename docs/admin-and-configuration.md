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

The browser must never copy the session bearer into JavaScript-accessible
storage. On page reload, it first recovers the separately issued CSRF value and
then calls `GET /api/v1/session`; authenticated UI is rendered only when the
HttpOnly cookie is still accepted by the server.

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

Issue the session-bound CSRF value in a separate `Secure`,
`SameSite=Strict`, non-`HttpOnly` cookie so the Admin frontend can reconstruct
`X-CSRF-Token` after reload. This cookie is not an authentication
credential, and possession of it without the opaque HttpOnly session cookie
must grant no access. Store only its digest in SQLite and expire it together
with the session cookie on logout.

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

The page separates operational state instead of treating every row as one
"latest 50" list:

- every currently running task is pinned in the first section and refreshed
  every five seconds;
- queued and retry-wait tasks are shown separately with exact total counts and
  an explicit notice when the bounded projection is truncated;
- completed, failed, and cancelled rows are paged as terminal history and
  cannot displace running work.

Each task displays:

- service version and uptime;
- Vault status and current revision;
- note/attachment counts;
- FTS/index revision and coverage;
- memory counts and exceptional automatic-memory diagnostics;
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
- DeepSeek;
- Xiaomi MiMo;
- Zhipu GLM;
- Moonshot/Kimi;
- Google Gemini through its official OpenAI-compatibility endpoint;
- Alibaba Qwen/DashScope;
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

Structured-generation model settings are typed and composable:

- `openai_compatibility_preset`: `auto`, `generic`, `deepseek`,
  `xiaomi_mimo`, `zhipu_glm`, `moonshot_kimi`, `google_gemini`, or
  `alibaba_qwen`;
- `openai_structured_output_mode`: `auto`, `strict_json_schema`,
  `json_object`, or `prompt_only`;
- `openai_token_limit_field`: `auto`, `max_tokens`, or
  `max_completion_tokens`;
- `openai_thinking_mode`: `auto`, `enabled`, or `disabled`;
- `generation_token_limit`: optional one-call generated-token ceiling.

The Chinese console first asks for the AI service type and fills the official
API root where one stable global endpoint exists. Region/workspace-specific
DashScope URLs remain operator input. Manual model registration shows the
effective preset, structured-output mode, thinking policy, and token ceiling.
Legacy empty model settings decode as `auto`; a legacy generic Provider is
migrated only from an exact official API host, never from a model name served by
an unrelated local/proxy runtime.

Reasoning-first presets use a bounded 32,768-token default unless the operator
or a lower model capability clamps it. This is a per-call bound, not a money
balance or full-job quota; a full-Vault extraction normally performs one call
per eligible Markdown note. Detailed vendor behavior and primary references
are maintained in `provider-compatibility.md`.

Do not assume a provider’s model-list endpoint is universally available or accurate.

The implemented console therefore keeps two explicit paths: “发现/刷新模型”
uses the configured adapter's model-list operation, while “手动登记模型”
accepts the provider-specific model ID and its primary structured-generation,
embedding, or reranking capability. A discovered model and a manually entered
model use the same `ModelRecord` and can be selected by role; neither the
provider display name nor Base URL is used as an implicit model name.

Provider tests persist only redacted health state and model metadata. A test
failure never exposes response bodies or secrets and does not make the Vault
unready. Configuration updates use optimistic revisions; model bindings first
check the Vault-specific override and then the global default.

Each configured service also exposes “编辑 AI 服务” and “删除 AI 服务”. Editing
supports display name, first-class service type, Base URL, enabled state,
request/connect timeout, retry count, process-wide Provider concurrency,
organization identifier, and the explicit private-network switch. The API-key
replacement field is always empty on load: leaving it empty preserves the
stored ciphertext, while entering a value rotates the secret and removes the
superseded Provider-owned ciphertext. The form submits the displayed revision
and reports a conflict instead of silently overwriting a newer Admin change.

Deletion requires a confirmation naming the service and known model/binding
impact. One State transaction removes all global and Vault-specific bindings
to the service's models, rebuildable embedding/vector rows, model inventory,
health/configuration, and Provider-owned encrypted secrets. The result reports
redacted counts. Vault notes, committed memories, generated memory artifacts,
jobs, and audit history remain intact; no operator should edit SQLite to remove
a Provider.

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

`embedding_note` powers ordinary-note semantic/hybrid search and the
`related_notes` portion of recall. Binding or changing it schedules only
missing/stale current note chunks through durable `embedding.rebuild` jobs;
lexical note recall remains available when it is unbound.

The first-release console writes a current-Vault override and displays an
effective global binding when one exists. Future multi-Vault administration can
add global-default editing without changing binding resolution.

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

Ordinary Markdown is indexed automatically and does not enter memory review.
The page distinguishes FTS coverage from optional `embedding_note` coverage so
an operator can tell lexical availability from semantic readiness.

Rebuild actions must state which data is derived and which canonical data will not be touched.

## 13. Memory page

Defined in detail in `memory-system.md`.

The UI includes:

- active/stale/superseded/archived counts;
- full memory browser;
- provenance/source metadata on demand;
- Phase 1/Phase 2 readiness and durable job progress;
- Stage 1 ready/no-output/pending counts and committed generation;
- merge and supersession;
- edit/archive/delete;
- automatic-memory settings;
- recall simulator;
- prompt/provider/pipeline metadata;
- embedding coverage and failures.

The page explains the distinction between automatically recallable ordinary
notes and durable memory. Once automatic memory is enabled for the Vault,
eligible ordinary Markdown changes may be sent to the configured model without
requiring frontmatter, tags, special folders, or another authoring convention.
Every ordinary note also remains available in search and `related_notes`
recall. Legacy `explicit_only` and `all_notes` settings deserialize as aliases
for the fixed `automatic` mode; the UI exposes no per-note source switch.

Phase 1 asks the extraction model for semantic raw memory, a source summary,
and bounded line ranges from a server-numbered source view. Local code verifies
the current Vault/file/revision/range and derives excerpt hashes directly from
the authoritative note; the model does not echo evidence text. Phase 2 uses the
separately bound consolidation model to merge, deduplicate, resolve conflicts,
forget obsolete input, and write final semantic memory. Neither model supplies
a trust score, and there is no human review queue.

The two-stage card is explicit about both model roles. Automatic memory is off
by default. Once enabled, non-managed Markdown create, update, move, and
restore events admit `memory.extract` durable jobs; successful Phase 1 work
automatically admits the singleton `memory.consolidate` follow-up. This is
event-driven, not a periodic LLM scan. Required fresh regeneration is admitted
immediately after the final policy/model binding becomes ready; the periodic
reconciliation loop is only a crash-recovery fallback. While regeneration is
pending but both phases are ready, the manual start button remains enabled
instead of creating a no-task/no-button dead zone. The card never presents “generate
candidates” or per-result approval as a user goal. Actions are disabled while
another memory pipeline job is active; repeated admission returns the active
job instead of starting a duplicate full-Vault scan.

The ordinary manual action is “处理新增或有变化的笔记”. A successful evaluation
is remembered even when it produces zero memories, so unchanged notes under the
same extraction profile skip the model. An off-by-default checkbox changes the
action to “重新评估所有现有笔记”; its warning states that unchanged notes will call
the model again, may produce different semantic raw memory/evidence anchors,
and incur additional Token cost. This task option is separate from the
persisted automatic-memory policy.

The policy also owns a typed per-note Provider deadline from 30 through 1800
seconds, defaulting to 300 seconds, and an evidence-anchor cap from 1 through
10. These are advanced hard bounds; there are no model self-score threshold
controls. They do not lengthen model discovery, provider health, consolidation,
embedding, or unrelated requests.

The one-time `memory.reset_pipeline` cutover task is admitted automatically by
the service, not exposed as a routine destructive UI action. While the project
is prerelease it discards every old memory and `memory.*` task, removes the old
managed memory namespace through Vault Core, and starts a new current-generation
Phase 1 job at note one. The card shows cutover and regeneration-pending state.
Ordinary source notes, Vault history, existing backups, Provider settings,
audit facts, and non-memory jobs remain available; old explicit memories are
not converted.

Each long-term-memory card exposes lifecycle actions backed by the Admin API.
Active memories can be archived; archived, stale, or rejected memories can be
restored; every memory can be permanently deleted after a destructive-action
confirmation. Each request carries the displayed revision, refreshes on
conflict, and records an Admin audit event. Permanent delete removes the
current managed Markdown and projection; revision history and backups follow
their independent retention policies and the UI does not present the action as
an archive that can be restored.

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
portable Markdown, while Stage 1/final-memory SQLite rows, prepared proposals,
FTS, entities, relations, and embeddings are operational or rebuildable
projections.

Automatic extraction is admitted as a Vault-scoped durable job containing only
file identity, path, revision, and pipeline references. The worker rechecks
the source revision before Stage 1 replacement. Phase 2 persists an untrusted
prepared proposal, validates all source/base-revision references, and rechecks
its snapshot before canonical writes and the atomic selection commit.
`memory.rebuild` and `embedding.rebuild` remain separate jobs; Provider outages
degrade new extraction/consolidation and semantic search but do not make
existing lexical recall or canonical Vault writes fail.

The runtime stores the following Vault-scoped settings through the typed
configuration API rather than an unvalidated key/value editor:

- automatic-memory enablement, per-note cap/deadline, and schema/prompt version;
- recall weights, result/token budgets, and optional embedding role binding;
- memory retention, source invalidation, and diagnostic policies.

Changing hard bounds, bindings, or worker concurrency is hot-reloadable when
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

Progress is a projection of measured work, not a fabricated counter. Completed
jobs display 100% even when a short handler did not publish intermediate units;
queued/running/failed jobs without a bounded ratio display “未报告”. Structured
raw progress remains available for diagnostics. Full-Vault Phase 1 extraction
publishes a redacted phase, current note path/ordinal, processed/total note
units, last completed path, per-note elapsed time, model-evaluated count,
unchanged/pre-Provider skips, raw-input and `no_output` counts, isolated
generated-output failures, and source-ingestion failures. Each category keeps
up to 20 bounded redacted note diagnostics. The UI labels source failures as
“源文件无法处理（模型未调用）” and generated-output/evidence failures as
“模型输出校验失败（模型已调用）”; it never merges them into “格式或读取跳过”.
`already_evaluated_skipped` is displayed as “未变化且已处理，跳过模型”. Phase 2 publishes dirty raw-input count,
created/updated/retired/discarded counts, committed generation, and whether a
prepared proposal was reused. A mixed Phase 1 result is displayed as
“完成但有失败” at 100%, with
the latest failed path, stable Chinese error, and redacted schema category/path.
It is not displayed as an all-job failure.
The Jobs page, and the Memory page while extraction is active, refresh every
five seconds and explain the current unit of work instead of showing only
`0%`. A `last_error` shown while a
new attempt is running is labelled as the previous attempt, not the current
request. The Memory page consumes the same active-task projection as the Jobs
page, so an older `memory.extract` cannot disappear behind high-volume
index/outbox history or incorrectly re-enable the manual admission buttons.

A full-Vault extraction continues after one malformed generated output. Three
consecutive output-contract failures stop further calls with
`memory_extract_output_failure_limit` to bound invalid billing. Configuration,
authentication, endpoint, state, lease, and retryable transport failures remain
job-level outcomes. Retrying a stopped `memory.extract` job retains its durable
completed-note cursor instead of starting the paid backfill from note one.

Provider response-body diagnostics distinguish a total read timeout
(`provider_response_timeout`), an interrupted/truncated body
(`provider_response_incomplete`), and an otherwise unclassified body-read
failure (`provider_response_read_failed`). These errors are terminal until an
operator explicitly retries because the provider already returned a successful
HTTP status and may have charged for the request. Admin displays stable Chinese
explanations; raw bodies and provider secrets are never persisted in job
progress.

For live diagnosis, the server also emits redacted structured job events to
the configured process log. `job_started`, `job_progress`,
`memory_extract_note_output_failed`, `job_completed`, `job_retry_scheduled`,
`job_failed`, and `job_cancelled` show
the task ID/type, Vault, phase, measured counters, elapsed time, and stable
error code where applicable. A schema mismatch adds only its stable category
and trusted schema path. Memory extraction paths are logged only as a one-way
hash; note bodies, prompts, response values, provider responses, job payloads, and
credentials are never logged. Use `docker compose logs -f mcp-vault` for the
timeline and the Admin page for the latest durable progress snapshot.

Structured-response diagnostics additionally distinguish a non-JSON HTTP
body, missing final content, malformed structured JSON, provider token-limit
truncation, content filtering, and repetition truncation. These categories are
redacted and terminal: the UI explains the next operator action without
storing the response body, reasoning text, prompt, or note content.

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
- session and CSRF cookies pass browser tests, including authenticated page
  reload, mutation after reload, expired-session fallback, and logout cleanup;
- secret create/update never returns stored plaintext;
- changing embedding model schedules re-embedding rather than mixing vectors;
- provider models can be discovered or manually registered and bound to the
  current Vault's model roles;
- memory extraction defaults disabled, reports readiness blockers, admits
  future note events only after enablement, and supports an explicit existing-
  note backfill job;
- completed jobs never render as 0%, unknown progress remains visibly unknown,
  and index coverage uses current non-managed Markdown as its denominator;
- provider outage is visible without breaking core readiness;
- an invalid private/public provider URL is rejected or explicitly confirmed;
- Vault-scoped settings cannot affect another fixture Vault;
- Admin UI remains usable when LLM/embedding are disabled;
- public reverse-proxy fixture cannot reach Admin listener.
