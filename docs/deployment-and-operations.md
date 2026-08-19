# Deployment and Operations

## 1. Deployment model

The supported production shape is Docker Compose with one MCP Vault application container and an optional reverse proxy already operated by the user.

The application container includes:

- Rust server binary;
- embedded Admin frontend assets;
- SQLite;
- background workers;
- optional local embedding runtime support.

Do not require a separate database, message broker, or vector service for the reference deployment.

## 2. Ports and network topology

Application listeners:

```text
8080/tcp  data plane: MCP, WebDAV, liveness/readiness, auth metadata
8081/tcp  control plane: Admin UI/API, setup, detailed diagnostics
```

Reference Compose publication:

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
      - ./secrets/master-key:/run/secrets/mcp-vault-master-key:ro
      - ./secrets/bootstrap-token:/run/secrets/mcp-vault-bootstrap-token:ro
    environment:
      MCP_VAULT_DATA_DIR: /data
      MCP_VAULT_MASTER_KEY_FILE: /run/secrets/mcp-vault-master-key
      MCP_VAULT_BOOTSTRAP_TOKEN_FILE: /run/secrets/mcp-vault-bootstrap-token
      MCP_VAULT_DATA_BIND: 0.0.0.0:8080
      MCP_VAULT_ADMIN_BIND: 0.0.0.0:8081
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

Replace the localhost publication with a specific LAN/VPN host address:

```yaml
- "192.168.1.20:8081:8081"
```

Do not publish `8081:8081` on all interfaces in the default example.

## 3. Reverse proxy

Only proxy the data plane.

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
- never route `/api`, `/setup`, or Admin assets from port 8081.

The repository should include Nginx and Caddy reference configurations and automated smoke tests.

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

Optional/rebuildable:

```text
/data/models
/data/tmp
```

Backups may include `/data/models` to avoid downloads but it is not canonical.

The master key file is outside `/data` by default and requires a separate protected backup procedure.

## 6. Startup sequence

1. Load bootstrap configuration.
2. Validate directory ownership and permissions.
3. Open SQLite and apply forward migrations.
4. Load/validate installation identity and master key.
5. Recover incomplete operation-journal entries.
6. Validate configured Vault root.
7. Start outbox and job workers.
8. Start filesystem watcher/reconciler.
9. Build routers.
10. Bind listeners.
11. Mark liveness healthy.
12. Mark readiness healthy only when operational database, Vault storage, migrations, and critical workers are ready.

Provider health is not required for core readiness because providers are optional/degradable.

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

Periodic full/incremental reconciliation:

- compares path, size, mtime, identity, and content hash as needed;
- imports direct edits as `external_change` revisions;
- finds missing files/deletes;
- repairs projection drift;
- never follows unsafe paths;
- is resumable.

## 11. Backup

### 11.1 Consistent snapshot

A backup job:

1. enters a short snapshot coordination phase;
2. obtains a SQLite online backup/snapshot;
3. records the current Vault revision/checkpoint;
4. snapshots or copies Vault files consistently;
5. copies required history blobs;
6. writes manifest with checksums, versions, paths, and counts;
7. verifies the produced backup;
8. records result in catalog.

The exact strategy may use filesystem snapshot features where available, with a portable copy fallback.

Canonical writes should be blocked only for the shortest necessary coordination period.

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
6. enter maintenance/read-only mode;
7. create pre-restore safety backup;
8. restore Vault, history, and state using atomic directory/database swap where possible;
9. run migrations if restoring an older supported version;
10. recover/reconcile;
11. run integrity and isolation checks;
12. leave maintenance mode.

Restore never writes paths outside configured roots.

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
- Full database/index rebuild does not alter canonical files.
- Restore to clean host passes end-to-end tests.
- Provider outage does not make readiness fail.
- Low disk space produces warnings and safely rejects writes before corruption.
