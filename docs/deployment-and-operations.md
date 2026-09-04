# Deployment and Operations

## 1. Deployment model

MCP Vault is one self-contained application container. It does not require or
configure Nginx, Caddy, Traefik, a particular ingress controller, or a separate
database/message broker. Operators choose listener publication, TLS
termination, firewall/VPN policy, and any reverse proxy independently. The
repository's base Compose file starts MCP Vault directly; the Nginx bundle
under [`../deploy/nginx-https/`](../deploy/nginx-https/README.md) is only an
optional example. A single-service variant for operators who already run
Nginx is available under
[`../deploy/existing-nginx/`](../deploy/existing-nginx/README.md).

The application container includes:

- Rust server binary;
- embedded Admin frontend assets;
- SQLite;
- background workers;
- optional local embedding runtime support.

Do not require a separate database, message broker, vector service, or reverse
proxy for application initialization.

## 2. Ports and network topology

Application listeners:

```text
8080/tcp  data plane: MCP, WebDAV, liveness/readiness, auth metadata
8081/tcp  control plane: Admin UI/API, setup, detailed diagnostics
```

The application defaults Admin to loopback. Compose, systemd, Kubernetes,
firewall, VPN, or an operator-selected proxy decides whether either listener is
reachable elsewhere. MCP Vault itself does not enforce Admin source CIDRs.

Low-level direct-listener example:

```yaml
services:
  mcp-vault:
    image: ghcr.io/example/mcp-vault:VERSION
    restart: unless-stopped
    ports:
      - "8080:8080"
      - "127.0.0.1:8081:8081"
    volumes:
      - ./data:/data
    environment:
      MCP_VAULT_DATA_DIR: /data
      MCP_VAULT_SECRETS_DIR: /data/secrets
      MCP_VAULT_DATA_BIND: 0.0.0.0:8080
      MCP_VAULT_ADMIN_BIND: 0.0.0.0:8081
      # Exact Host authorities accepted by the MCP data endpoint.
      MCP_VAULT_DATA_HOSTS: vault.example.com
      # Set these to the exact browser origins used by the deployment.
      MCP_VAULT_DATA_ORIGINS: https://vault.example.com
      # Canonical external base URL shown in WebDAV/MCP connection cards.
      MCP_VAULT_DATA_PUBLIC_ORIGIN: https://vault.example.com
      MCP_VAULT_ADMIN_ORIGINS: https://admin.example.com
      MCP_VAULT_BACKUP_DIR: /data/backups
      MCP_VAULT_METRICS_ENABLED: "false"
    read_only: true
    tmpfs:
      - /tmp
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL
```

The final repository must provide a tested example rather than copying this fragment uncritically.

### LAN Admin access

For a direct-listener deployment, replace the localhost publication with a
specific LAN/VPN host address:

```yaml
- "192.168.1.20:8081:8081"
```

Do not publish `8081:8081` on all interfaces in the default example.

The base Compose file exposes this choice through
`MCP_VAULT_ADMIN_PUBLISH`, whose default is `127.0.0.1:8081`. For example,
`MCP_VAULT_ADMIN_PUBLISH=192.168.1.20:8081` publishes Admin on that LAN
address. The operator remains responsible for TLS and source-network policy.

An appliance that cannot terminate local TLS may expose Admin directly at an
exact LAN address and add that literal HTTP origin to
`MCP_VAULT_ADMIN_ORIGINS`. MCP Vault then emits host-only non-Secure cookies
only for that validated HTTP Origin. This sends the Admin password and session
in cleartext across the LAN, so restrict the published port to a trusted
network and prefer HTTPS or VPN whenever possible. Public IP HTTP origins and
cleartext DNS names are rejected.

## 3. Reverse proxy

Reverse proxying is optional and entirely deployment-owned. If used, the
public virtual server proxies only the data plane. Admin should use a separate
listener bound to an intended localhost/LAN/VPN address and any operator-chosen
source-network restriction.

Example external layout:

```text
https://vault.example.com/dav/v1/vaults/default/
https://vault.example.com/mcp/v1/vaults/default
```

Proxy requirements:

- HTTP/1.1 or HTTP/2 request support;
- preserve non-standard DAV methods;
- preserve MCP headers;
- allow request-scoped SSE streaming;
- disable buffering for MCP streams and large DAV uploads/downloads;
- preserve conditional headers;
- proxy `/.well-known/oauth-protected-resource`, the exact
  `/.well-known/oauth-authorization-server` path, and `/oauth/` to the data
  listener without requiring a bearer token; do not proxy unrelated well-known
  or Admin paths;
- bypass caching for those OAuth routes at every reverse-proxy and CDN layer;
  application no-store headers are mandatory but cannot override an operator
  rule that force-caches dynamic authorization pages;
- set appropriate body/time limits;
- preserve WebDAV `Authorization` and set `X-Forwarded-Proto: https` in the
  effective `/dav/` location;
- restrict the plaintext data listener by container publication and firewall
  rules so only the intended proxy can reach it. MCP Vault does not authenticate
  the source of the forwarded scheme;
- never route `/api`, `/setup`, or Admin assets from port 8081.

### ChatGPT plugin OAuth checklist

MCP Vault's built-in authorization server is the default and requires no
external identity service. Before adding it as a ChatGPT plugin:

1. Serve the data origin with publicly trusted HTTPS and set
   `MCP_VAULT_DATA_PUBLIC_ORIGIN` to that exact origin.
2. Proxy `/mcp/`, `/oauth/`, `/.well-known/oauth-protected-resource`, and the
   exact `/.well-known/oauth-authorization-server` path to the data listener.
   These OAuth routes are intentionally public; never proxy `/api/` or Admin
   assets from the control listener. Configure every CDN/edge rule to bypass
   cache for them. After upgrading from an image that served a login page from
   `/oauth/authorize`, purge that legacy URL before reconnecting ChatGPT.
3. Open the LAN-only Admin MCP page. Under “ChatGPT OAuth”, create a Vault
   OAuth username/password and choose the maximum scopes. This credential is
   independent from the Admin password. Saving again rotates it and revokes all
   prior local codes/tokens.
4. Fetch both copied metadata URLs without credentials. The protected-resource
   `resource` must exactly equal the copied MCP endpoint, and its first
   `authorization_servers` entry must be the configured public origin. The
   authorization metadata must advertise DCR, `code`, PKCE `S256`, token auth
   `none`, `offline_access`, and the same issuer. Protected-resource metadata
   continues to list only Vault/memory permission scopes.
5. In ChatGPT, add the copied MCP endpoint. Do not manually invent a callback,
   client ID, secret, or token: ChatGPT performs DCR and opens MCP Vault's own
   login page.
6. Sign in with the Vault OAuth credential, review the requested scopes and
   “保持长期连接” capability, and verify one bounded read tool call. After an
   upgrade that first adds `offline_access`, reconnect the existing ChatGPT
   connection once. Access tokens then renew automatically through rotating
   refresh tokens with a 180-day sliding idle lifetime. Use Admin to
   rotate/disable the OAuth login if the connection must be revoked.

`scripts/interop/http-smoke.sh` proves the complete real-HTTP DCR, login,
authorization-code + PKCE, `offline_access`, access-token MCP, refresh rotation,
duplicate-refresh grace, and delayed replay-revocation policy. A successful live ChatGPT result still depends on the
operator's DNS/TLS and ChatGPT account/UI state and must be recorded separately.

Operators that already run an IdP may instead expand “外部 OAuth/OIDC 兼容”,
save normalized RS256 public JWKS, and create an exact Subject-to-Vault grant.
That external issuer must support authorization code + PKCE `S256`, ChatGPT
CIMD/DCR or pre-registration, and MCP resource propagation. Never paste a
client secret, private key, access token, or refresh token into MCP Vault.

The WebDAV mount is versioned and Vault-scoped:

```text
https://vault.example.com/dav/v1/vaults/<vault-slug>/
```

Use a generated app credential for Basic Authentication. The Admin WebDAV
page will provide the final URL and one-time password display when its CRUD
surface is enabled; do not put an Admin password or MCP token in a DAV client.

Correctness does not depend on lowering a sync client's request concurrency.
Initial mirrors may issue many independent PUTs: canonical file work remains
concurrent, while the service queues only the short SQLite credential-touch and
revision/audit/outbox write phases. The release HTTP smoke exercises 50
concurrent nested PUTs followed by a read-back of every object.

Reference proxy examples and smoke tests are interoperability aids, not MCP
Vault runtime dependencies.

The Admin API and embedded console are served only from port 8081. A reverse
proxy for Admin must not be added to the public data-plane virtual host.

The base Compose profile publishes Admin only on loopback unless the operator
changes `MCP_VAULT_ADMIN_PUBLISH`. The application runs as non-root
`mcpvault`, uses a read-only root filesystem, drops Linux capabilities, applies
`no-new-privileges`, bounds PIDs/memory, and uses a noexec/nosuid/nodev tmpfs.
The application `/data` bind mount remains its only writable persistent volume.

## 4. TLS

Production data-plane access over an untrusted network requires TLS.

Preferred:

- reverse proxy with ACME certificates;
- direct TLS support may be added but is not required when the proxy path is documented and tested.

WebDAV Basic Authentication must not cross plaintext public transport.
Because MCP Vault accepts the proxy's `X-Forwarded-Proto: https` assertion
without an application-level peer allow-list, port 8080 must not be reachable
from untrusted clients.

Local HTTP to a loopback/private local-model provider is permitted only under explicit provider policy.

## 5. Persistent volumes

Back up these together:

```text
/data/vaults
/data/state
/data/history
```

Each new Admin-created Vault is a sibling directory at
`/data/vaults/<immutable-slug>`. Existing registered roots are retained during
upgrade; the managed-creation API does not accept an arbitrary host path.
Every Vault has its own WebDAV/MCP URL and credentials even though the roots
share one installation database and backup lifecycle.

MCP Vault creates its installation files by default under:

```text
/data/secrets/master-key
```

`/data/secrets` is persistent service state but is deliberately excluded from
ordinary downloadable MCP Vault backups. Back up the master key separately;
operators may point `MCP_VAULT_SECRETS_DIR` at a separate persistent mount.

Optional/rebuildable:

```text
/data/models
/data/tmp
```

FastEmbed model caches are optional derived artifacts. A missing or failed
local model download disables only that provider path; it must not fail core
readiness or prevent WebDAV, canonical writes, or lexical search.

Backups may include `/data/models` to avoid downloads but it is not canonical.

The filesystem containing each Vault content root must support atomic regular
file installation. MCP Vault prefers Linux `RENAME_NOREPLACE` for new files.
When that flag is unavailable, it uses a constrained same-directory hard-link
commit for already-synced temporary regular files. Therefore a deployment
whose mount rejects both exclusive rename and hard-link creation cannot safely
host a writable Vault; MCP Vault reports
`filesystem does not support safe atomic no-replace file creation` instead of
falling back to an overwrite race.

File and directory moves prefer `RENAME_NOREPLACE`. If the same-filesystem
mount rejects only that capability, MCP Vault serializes all absent-target
claims for that Vault, rechecks the destination, and uses ordinary atomic
`renameat`. This move fallback does not hard-link user entries and does not
copy/delete across filesystems. Run only one MCP Vault process against a given
content root and do not race protocol writes with direct host-directory writes.

For an appliance or NAS deployment, check the actual Vault root rather than
only `/data` because a nested bind/mount can use a different filesystem:

```bash
docker exec mcp-vault stat -f -c '%T' /data/vaults/<vault>/content
findmnt -T /host/path/to/vault -o TARGET,SOURCE,FSTYPE,OPTIONS
```

The release acceptance smoke must create a new file, reject a concurrent
same-name create without changing its bytes, replace an existing file under an
exact revision, and restart/reconcile on the production mount type.

The default master key is generated automatically under
`MCP_VAULT_SECRETS_DIR`. An explicit `MCP_VAULT_MASTER_KEY_FILE` may instead
point to an operator-managed regular raw-32-byte or 64-hex file. MCP Vault does
not inspect or mutate file permission bits. Startup persists/validates only a
one-way key verifier. A different key or a missing established key when
encrypted secrets/PATs exist is a hard startup failure and is never replaced.

## 6. Startup sequence

1. Load bootstrap configuration.
2. Open SQLite and apply forward migrations.
3. Load or atomically create the managed installation key, then validate its
   persisted identity.
4. Recover incomplete operation-journal entries per Vault. A local
   unrecoverable Vault is marked `error`; healthy Vaults and Admin continue.
5. Run the safe initial scan for each ready pre-existing Vault and persist its
   checkpoint. A newly managed Vault retains its durable `vault.initialize`
   job instead of blocking startup.
6. Build routers and bind both listeners.
7. Start the outbox/job supervisor and the periodic reconciliation loop.
8. Mark liveness healthy.
9. Mark process readiness healthy when the operational database, migrations,
   key, listeners, and critical workers are ready. Detailed Admin health and
   the data endpoints report each Vault's own availability.

Provider health is not required for core readiness because providers are optional/degradable.

Every Markdown note is available to lexical search and related-note recall
after the ordinary index rebuild; this requires no LLM and no memory review.
For semantic paraphrase matching, register an embedding-capable model and bind
it to `embedding_note`. Binding, startup, and later note index rebuilds enqueue
only missing/stale reference-only chunks. Until those jobs complete,
`search_notes`/`recall` explicitly report semantic degradation and continue
lexically.

To enable AI memory extraction after first setup, use Admin in this order:

1. choose `local_only` or `remote_allowed` data-send policy;
2. create the Provider and discover its model list, or manually register the
   exact provider model ID when discovery is absent/inaccurate;
3. bind that model to `memory_extraction`;
4. bind a suitable model to `memory_consolidation` (it may be the same
   registered model, but remains a separate role binding);
5. enable automatic memory; source admission is fixed to `automatic`;
6. optionally request “处理新增或有变化的笔记” once, then rely on future
   Markdown create/update/move/restore events.

No note frontmatter, tag, folder, or path convention is required. Enabling the
Vault-level feature allows eligible ordinary Markdown changes to reach the
extraction model. Legacy `explicit_only` and `all_notes` settings deserialize
as aliases for `automatic`; operators do not need to rewrite stored settings or
edit notes during upgrade.

Successful Phase 1 evaluation is persisted independently from whether raw
memory was generated. Later automatic events and default manual runs compare
the current file revision and effective extraction profile before sending
content, so an unchanged current note costs no generation call. The one-time
prerelease pipeline cutover removes all prior memory state and memory jobs;
current notes are then evaluated from note one under the current Phase 1
contract.

Enable “包含已处理且未变化的笔记” only when intentionally forcing a complete
re-evaluation despite unchanged recorded configuration, for example after an
upstream model alias changes behavior without a local revision. It may produce
a different semantic raw memory from the unchanged note and consume the same
per-note model budget again. A failed forced evaluation leaves the note stale
for the next default incremental run.

Phase 1 returns the Codex three-field `raw_memory`, `rollout_summary`, and
`rollout_slug` object. MCP Vault derives file/path/revision and normalized
whole-source hash locally; the Provider does not return canonical evidence
coordinates and Phase 1 does not write final memory. A separate durable
`memory.consolidate` job uses the `memory_consolidation` model to merge,
deduplicate, supersede, archive, or discard staged inputs. Only a fully
validated prepared proposal is materialized as canonical semantic memory.
Neither phase asks the Provider for confidence/importance scores, and neither
requires per-result human approval.

Select the first-class AI service type whenever possible. The Admin form fills
the current official global API root for DeepSeek, MiMo, Zhipu, Kimi, and
Gemini; Qwen/DashScope uses the exact region/workspace URL copied from its
console. A proxy/reseller domain keeps its custom URL but should select the
matching compatibility preset during manual model registration. The generic
type does not guess a vendor from model names such as `qwen3` because that name
may refer to a local Ollama/vLLM deployment.

The Base URL is an API root, not a full `chat/completions` URL. Paths already
containing `/api/paas/v4/`, `/v1beta/openai/`, or `/compatible-mode/v1/` are
preserved exactly; MCP Vault does not insert another `/v1`. Refer to
`provider-compatibility.md` for the current endpoints and wire matrix.

Reasoning-first presets default to a 32,768 generated-token bound per note.
The model form can change it and, where the official model permits, override
thinking. For a full-Vault backfill, multiply the per-note ceiling by the
eligible-note count to understand the theoretical worst case before starting;
actual usage is returned by the provider and is normally lower than the bound.

This is not a periodic LLM sweep. Disabled extraction admits no new
`memory.extract` event jobs, while the explicit backfill is a durable job that
survives restart. The Memory page reports readiness blockers and recent
extraction failures without exposing note bodies or provider responses.
The Phase 1 policy returns either `no_output` or one bounded semantic raw input
with application-derived source provenance. Ordinary
article/reference and technical content always remains in the note retrieval
index even when Phase 1 returns `no_output`. On upgrade, migration 0011 deletes
all prerelease memory rows and `memory.*` jobs. The automatically admitted
`memory.reset_pipeline` job removes the old managed memory namespace through
Vault Core and admits a brand-new current-generation full-Vault extraction with
no inherited cursor. Vault source notes, revision history, existing backups,
Provider settings, non-memory jobs, and audit history remain intact; old
explicit memories are intentionally not converted. If Phase 1 is not
configured, `regeneration_pending` remains visible and periodic reconciliation
admits the fresh pass after configuration becomes ready.

One structured memory-extraction call has a five-minute default response
deadline, configurable per Vault from 30 through 1800 seconds; the provider's
shorter default timeout continues to cover ordinary provider operations. The
Jobs and Memory pages show the current note and refresh every five seconds
while extraction is active. If a job reports `provider_response_timeout`,
`provider_response_incomplete`, or `provider_response_read_failed`, MCP Vault
had already received a successful HTTP status but could not finish reading the
body. It deliberately stops rather than automatically issuing another possibly
billable request. Check the provider/network path and then use the explicit
Admin retry only when repeating that note is acceptable.

If the job instead reports `provider_output_truncated`, the provider completed
an HTTP response but stopped at its token limit; inspect the registered model's
maximum-output limit before an explicit retry. `provider_final_content_missing`
means the successful response contained no final assistant text, commonly due
to a mismatched compatibility profile. `provider_structured_json_invalid`
means final text existed but was not one complete JSON value. Non-JSON HTTP
body errors usually indicate that the Base URL points to a web/proxy response
rather than the compatible API. None of these diagnostics retain or display
the provider response body.

`provider_schema_invalid` is narrower than malformed JSON: the model returned
one complete JSON value, but a required field, type, enum, array bound, or
additional-property rule did not match. Current jobs show the redacted mismatch
category and trusted schema path. Phase 1 requires exactly
`raw_memory`, `rollout_summary`, and nullable `rollout_slug`; its zero-result
form uses empty summary/raw strings and a null slug. Source provenance is local,
not a model field. Phase 2 requires a summary, bounded memory actions, and a
local disposition for every dirty raw input. These are multi-field contracts, so the
generic single-array-envelope repair does not apply; missing or renamed fields
remain visible contract failures rather than being guessed locally.

Phase 2 assigns dirty inputs request-local indexes before context-only inputs
and publishes the exact allowed discard indexes in the structured-output
schema. A generated bookkeeping violation does not partially commit the global
proposal; it enters `retry_wait` and consumes the job's bounded retry budget.

During a full-Vault run, one malformed generated output is recorded against
that note and later notes continue. The final job can read “完成但有失败” while
still reaching 100%. Three consecutive output-contract failures open a
cost-safety circuit; after fixing the model compatibility setting, explicit
retry resumes after the last checkpoint instead of resubmitting earlier paid
notes. Configuration/authentication/endpoint failures and retryable transport
outages remain job-level because continuing would only repeat a systemic
problem.

For a fresh installation, open the Admin listener and enter the desired Admin
username and password. No secret generation, container command, or token copy
step is required. Until that first account commits, any client admitted by the
Admin listener's deployment boundary can attempt to claim the installation;
keep the default loopback publication or an equivalently restricted LAN/VPN
policy until setup is complete.

## 7. Shutdown sequence

On SIGTERM/SIGINT:

1. stop accepting new mutations;
2. mark readiness unhealthy;
3. cancel/finish request-scoped streams;
4. stop claiming new jobs;
5. allow leased critical jobs to checkpoint within grace period;
6. flush outbox and SQLite;
7. finish or recover journaled file operations;
8. close listeners and database.

The worker supervisor stops new claims, releases its leases, and passes the
same cancellation signal to cooperative job handlers. A handler that does not
finish within the configured grace period is aborted; its durable lease remains
reclaimable on the next process start.

The container’s stop grace period must exceed the application shutdown timeout.

## 8. Health

### Liveness

`GET /health/live`

Indicates process/event-loop operation only. It must not fail because a remote LLM is unavailable.

### Readiness

`GET /health/ready`

Requires:

- database opened and migrations current;
- master key loaded if encrypted secrets exist;
- Vault content root accessible;
- no unrecoverable operation journal requiring maintenance;
- critical worker supervisor running.

### Detailed health

Authenticated Admin API includes:

- SQLite WAL and disk status;
- filesystem free space;
- history storage;
- outbox age;
- queue depth/failures;
- indexing coverage;
- watcher/reconciliation state;
- provider health;
- backup age/verification;
- version/migration.

## 9. Observability

### Logging

JSON structured logs in production.

Fields:

```text
timestamp
level
target
request_id
plane
vault_id
actor_id
operation
duration_ms
result
```

Respect redaction rules from `security.md`.

Durable background jobs emit additional structured events under the
`mcp_vault::jobs` target:

```text
job_started
job_progress
job_completed
job_retry_scheduled
job_failed
job_cancelled
job_progress_persist_failed
memory_extract_source_ingestion_failed
memory_extract_note_output_failed
```

For automatic memory, Phase 1 `job_progress` includes the current
ordinal/total, model-evaluated notes, unchanged-note skips, raw inputs staged,
`no_output` results, source-ingestion failures that prove no Provider call, and
generated-output failures after a Provider call, plus elapsed milliseconds and
a one-way hash of the current path. Phase 2 progress includes
dirty raw inputs plus created, updated, retired, discarded, generation, and
prepared-proposal reuse counts. Error events include only stable redacted error
codes. The events never include job payloads, note bodies, raw/generated memory,
prompts, provider responses, API keys, or raw upstream error text. With the
default JSON logging configuration, follow them with:

```bash
docker compose logs -f mcp-vault
```

The Admin Jobs/Memory pages remain the authoritative current-state view;
container logs provide the live execution timeline and post-restart diagnosis.

### Metrics

Recommended metrics:

- HTTP requests and latency by plane/route class;
- DAV method/status;
- MCP method/tool/status;
- conflicts and precondition failures;
- bytes uploaded/downloaded;
- SQLite busy/transaction time;
- outbox age and attempts;
- jobs by state/type;
- index and embedding coverage;
- recall latency/degradation/result count;
- provider request latency/error/token estimate;
- filesystem scan duration;
- backup age/duration/verification;
- disk free space.

### Tracing

Support W3C Trace Context and optional OpenTelemetry export. Export is disabled by default.

## 10. Initial scan and reconciliation

### Initial scan

At first startup or Vault registration:

- walk without following symlinks;
- establish stable file records;
- hash content with bounded concurrency;
- schedule Markdown analysis;
- identify managed memory files;
- collect attachment metadata;
- update scan checkpoint;
- expose progress.

### Watcher

A filesystem watcher accelerates out-of-band detection but is not trusted as complete.

### Reconciliation

The first implementation uses polling rather than a platform-specific native
watcher. This keeps the correctness path independent of watcher event loss.
The interval is configured with
`MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS` (default 300, maximum 86400).

The timer admits one deduplicated `vault.reconcile` job per ready Vault instead
of scanning every root serially in the scheduler. Equal-priority job claims are
interleaved by Vault so one large backlog cannot starve another Vault.

Periodic full/incremental reconciliation:

- compares path, size, mtime, identity, and content hash as needed;
- imports direct edits as `external_change` revisions;
- finds missing files/deletes;
- repairs projection drift;
- never follows unsafe paths;
- is resumable.

The scanner skips `_mcp-vault`, symlinks, special files, invalid names, and
unsafe entries without following them. If scan evidence is incomplete, Core
does not infer missing-file deletes. Direct creates/edits/deletes that are
safe to prove are imported as `external_change` revisions through Vault Core,
with history, audit, and outbox rows committed together. The revision keeps the
`external_change` operation for provenance, while the outbox row reports the
semantic lifecycle event (`FileCreated`, `FileUpdated`, `FileDeleted`, or
`FileRestored`). Workers also accept the former `external_change` event label
so already-persisted rows remain drainable after an upgrade.

## 11. Backup

### 11.1 Consistent snapshot

A backup job:

1. switches admission to read-only and drains all in-flight writes;
2. obtains a SQLite online backup/snapshot;
3. records the current Vault revision/checkpoint;
4. snapshots or copies Vault files consistently;
5. copies required history blobs;
6. writes manifest with checksums, versions, paths, and counts;
7. verifies the produced backup;
8. records result in catalog.

The exact strategy may use filesystem snapshot features where available, with a portable copy fallback.

Canonical writes are blocked only for the snapshot coordination period. The
mode transition happens before the drain, and protocol/worker writes hold
counted guards, so no mutation can slip between the drain check and snapshot.

The current portable artifact is an uncompressed tar owned by the service under
`MCP_VAULT_BACKUP_DIR`. The artifact contains `manifest.json`, the SQLite
snapshot at `state/mcp-vault.sqlite3`, Vault-scoped content under
`vaults/{vault_id}/content/`, and history under `history/{vault_id}/`. The
manifest records SHA-256 checksums, schema/service versions, Vault roots and
settings revisions, and retained encryption-key version identifiers. It never
contains master-key bytes. Creation validates the finished tar before the
catalog row becomes completed/verified; a later verify operation repeats the
full checksum and entry-policy validation.

### 11.2 Manifest

Include:

```text
backup format version
service version/build
database schema version
Vault ID/slug
snapshot revision/checkpoint
file count and checksums
state database checksum
history blob count/checksums
encryption-key version IDs, not keys
creation/completion/verification times
```

### 11.3 Retention

Configurable count/time policy. Never delete the last verified backup automatically.

### 11.4 Master key

Offer a separate, explicitly encrypted key export. Explain that:

- the Vault Markdown does not require the key;
- encrypted provider secrets do;
- loss of the key requires re-entering secrets;
- key export must not be stored beside unencrypted backups without warning.

## 12. Restore

Restore is a privileged maintenance operation.

Flow:

1. upload/select backup;
2. validate archive structure and manifest in staging;
3. verify checksums;
4. verify format/schema compatibility;
5. show proposed changes;
6. enter read-only mode and drain admitted writes;
7. create pre-restore safety backup;
8. enter offline mode and drain active protocol operations;
9. restore Vault, history, and state using atomic directory/database swap where possible;
10. replace the live SQLite schema, then run migrations for an older supported version;
11. recover/reconcile and run integrity/isolation checks;
12. leave maintenance mode.

Restore never writes paths outside configured roots.

The Admin restore flow requires a separately validated preview, the literal
`RESTORE` confirmation, and recent Admin-password reauthentication. The worker
creates a completed pre-restore safety artifact, keeps the process offline while
swapping staged content/history and restoring SQLite, then runs migrations,
integrity checks, and Vault journal recovery before returning readiness. An
unavailable encryption-key version, target-root mismatch, checksum failure,
traversal entry, link/special entry, or resource-limit violation is rejected
before configured roots are changed. A post-swap failure rolls back the roots
and SQLite snapshot when possible; the service reopens only when the rollback
integrity checks pass, otherwise it remains offline for operator recovery.

If a process restart or an unrecoverable swap leaves the gate offline, an
authenticated owner may POST `/api/v1/maintenance/recover` with
`{"confirmation":"RECOVER","password":"..."}`. The service re-runs SQLite
integrity, Vault-root, and journal-recovery checks and only then reopens normal
mode; it never clears the gate merely because an operator supplied the word.
After the active restore operation has exited, the control plane permits a new
Admin login and this recovery call while data-plane requests remain blocked.

## 13. Upgrade

### Before upgrade

- read release notes;
- create and verify backup;
- ensure no failed journal operations;
- record current image digest and schema version.

### Upgrade

- pull pinned version;
- stop old container cleanly;
- start new container;
- run migrations;
- monitor readiness and logs;
- run WebDAV/MCP smoke tests.

For the self-provisioning transition, remove the obsolete
`MCP_VAULT_ADMIN_ALLOWED_CIDRS` variable after moving its policy to the
deployment layer; MCP Vault rejects that obsolete setting rather than silently
ignoring it. Also remove `MCP_VAULT_BOOTSTRAP_TOKEN` and
`MCP_VAULT_BOOTSTRAP_TOKEN_FILE`; password-only first-Admin setup no longer
accepts either setting and startup rejects them with migration guidance.
Startup removes the former service-owned
`<MCP_VAULT_SECRETS_DIR>/bootstrap-token` path; explicitly managed token files
outside that path are left untouched and are no longer read.
Existing explicit master-key mounts remain supported. If moving an established master key to
`MCP_VAULT_SECRETS_DIR`, copy the exact existing bytes while the service is
stopped—never ask the service to generate a replacement for a database that
already has a key verifier or key-dependent records.

The multi-Vault transition adds no canonical-content migration. On first use,
the server records `vault.legacy_default_id` from the existing `default` slug
or sole Vault so historical unscoped Admin clients continue to target the same
Vault. Existing `/dav/v1/vaults/<slug>/` and `/mcp/v1/vaults/<slug>` URLs,
credentials, OAuth resources, IDs, histories, indexes, memories, and jobs are
unchanged. Create and verify a fresh global backup before adding a second
Vault; an older one-Vault backup remains readable but restore correctly rejects
it against a different live Vault topology.

The 0.1.17 upgrade adds migration 0013 without deleting memory or canonical
Markdown. Existing final note sources begin `unverified`; normal recall fails
closed for those note-dependent memories until the first generation-keyed
`memory.audit_sources` job proves current evidence. Source-less explicit
Agent/Admin memory remains available. Every completed full Vault reconciliation,
including post-restore reconciliation, admits a new paged audit generation.
Operators can also run it from Admin under **Memory → Source health**. Final-
source, affected-memory, Stage 1, and distinct-File-ID counts are intentionally
separate and must not be interpreted as interchangeable totals.

Migration 0014 creates only Vault-scoped multilingual retrieval projections and
rebuilds memory FTS; it does not rewrite existing canonical memory Markdown or
advance the prerelease memory pipeline generation. Take and verify a backup
before upgrade as usual. After upgrade, inspect **Memory → Cross-language
retrieval**. Existing active, stale, and superseded memory remains uncovered
until an Admin explicitly confirms backfill. The dialog reports estimated
eight-item Provider batches and may equivalently restore verifiable translated
bodies to their source language. Those body changes create ordinary revisions
and can be restored from history. Cancelled or failed work retains applied
metadata and resumes from durable pending/proposal state; do not delete the
retrieval proposal table to retry a paid response.

### Rollback

Database migrations are forward-only. Rollback to an older binary may require restoring the pre-upgrade backup. Release notes must state compatibility.

Do not promise binary rollback after an irreversible migration.

## 14. Maintenance mode

Maintenance modes:

```text
normal
read_only
offline
```

`read_only`:

- reads/search/recall remain available;
- WebDAV/MCP mutations return clear temporary errors;
- Admin operations such as backup diagnostics remain available.

`offline`:

- data-plane requests return maintenance status;
- control-plane recovery remains available.

## 15. History and trash retention

Configure independently:

- revision history days/count/size;
- deleted-file trash retention;
- memory archived retention;
- audit retention;
- job history retention;
- backup retention.

Cleanup is a durable job and respects active backup/restore operations.

## 16. Local embedding models

When local embeddings are enabled:

- store models under `/data/models`;
- verify model artifact hashes where available;
- download only through an explicit Admin action/job;
- show license/model identifier;
- isolate inference on bounded blocking workers;
- account for CPU/memory readiness;
- keep FTS operational during download/failure.

After upgrading from the original 6,000-character note-vector profile, open
the Knowledge Index page and run **Rebuild index** once. The deterministic
`text-v2` source keys and embedding projection version admit fresh
`embedding.rebuild` jobs instead of reusing terminal jobs from the old profile.
The completed index job only schedules vector work; verify the separate
“Rebuild semantic vectors” jobs and semantic coverage before declaring the
upgrade complete.

If `embedding_memory` was already bound before upgrade, open the Memory page
and use **Generate missing vectors**. This rebuilds vectors from current memory
records and does not re-run memory extraction, consolidation, or multilingual
alias generation.

## 17. Diagnostics

Generate a redacted diagnostic bundle containing:

- version and feature flags;
- effective non-secret configuration;
- schema version;
- health status;
- recent sanitized errors;
- job/outbox summary;
- index coverage;
- dependency/runtime information;
- optional selected protocol traces with bodies removed.

Do not include notes, memories, credentials, cookies, tokens, provider prompts, or secrets by default.

## 18. Disaster recovery tests

Automate:

- restore latest backup into a clean temporary deployment;
- verify SQLite integrity;
- compare manifest/checksums;
- authenticate using restored credentials or a controlled test credential;
- WebDAV read/write smoke;
- MCP discovery/search/recall smoke;
- history restore;
- derived index rebuild.

A backup is not considered healthy until verified.

## 19. Operational acceptance

- Admin port is loopback-only in default Compose.
- Public proxy cannot reach Admin.
- TLS WebDAV and MCP smoke tests pass.
- Restart during a file mutation recovers correctly.
- Restart during an extraction/embedding job retries idempotently.
- Restart during memory extraction, revalidation, or managed Markdown rebuild
  leaves leases reclaimable and preserves canonical memory files.
- Full database/index rebuild does not alter canonical files.
- Restore to clean host passes end-to-end tests.
- Provider outage does not make readiness fail.
- Low disk space produces warnings and safely rejects writes before corruption.

Backup/restore acceptance additionally requires the last verified artifact to
survive retention cleanup, a corrupt or traversal archive to be rejected
without root changes, and a clean temporary deployment to pass SQLite
integrity, restored credential, WebDAV, MCP discovery/search/recall, and
history-restore smoke checks.

## 22. First-release handoff

Use [`release-readiness.md`](release-readiness.md) as the release gate and
[`requirements-traceability.md`](requirements-traceability.md) as the evidence
index. The handoff must name the source revision and image digest, record the
migration version and last verified backup, and identify any unverified
Obsidian plugin/client or provider integration. A tag is not ready while a
required Litmus, full-scale performance, clean-host restore, proxy separation,
or security review item is merely assumed.
