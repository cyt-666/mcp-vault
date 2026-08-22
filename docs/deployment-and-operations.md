# Deployment and Operations

## 1. Deployment model

MCP Vault is one self-contained application container. It does not require or
configure Nginx, Caddy, Traefik, a particular ingress controller, or a separate
database/message broker. Operators choose listener publication, TLS
termination, firewall/VPN policy, and any reverse proxy independently. The
repository's base Compose file starts MCP Vault directly; the Nginx bundle
under [`../deploy/nginx-https/`](../deploy/nginx-https/README.md) is only an
optional example.

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
      # Exact reverse-proxy socket peers, not a broad CIDR.
      MCP_VAULT_TRUSTED_PROXY_IPS: 172.20.0.10
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
- set appropriate body/time limits;
- forward original scheme/host only from a trusted proxy;
- configure `MCP_VAULT_TRUSTED_PROXY_IPS` with the exact proxy socket IPs so
  WebDAV can accept `X-Forwarded-Proto: https` without trusting arbitrary
  client headers;
- never route `/api`, `/setup`, or Admin assets from port 8081.

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

Local HTTP to a loopback/private local-model provider is permitted only under explicit provider policy.

## 5. Persistent volumes

Back up these together:

```text
/data/vaults
/data/state
/data/history
```

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
4. Recover incomplete operation-journal entries.
5. Run the safe initial scan for each active Vault and persist its checkpoint.
6. Build routers and bind both listeners.
7. Start the outbox/job supervisor and the periodic reconciliation loop.
8. Mark liveness healthy.
9. Mark readiness healthy only when operational database, Vault storage,
    initial scan, migrations, and critical workers are ready.

Provider health is not required for core readiness because providers are optional/degradable.

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
with history, audit, and outbox rows committed together.

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
