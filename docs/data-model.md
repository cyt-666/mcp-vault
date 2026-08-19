# Data and Storage Model

## 1. Storage roots

Recommended container layout:

```text
/data/
├── vaults/
│   └── <vault-id>/
│       └── content/              # Canonical Obsidian Vault root
├── state/
│   ├── mcp-vault.sqlite3         # Authoritative operational + derived state
│   └── instance-id
├── history/
│   └── <vault-id>/blobs/         # Content-addressed revision blobs
├── models/                       # Optional local embedding models
├── backups/
└── tmp/
```

The `content/` directory is the only directory a user needs to copy to obtain the current ordinary Obsidian Vault.

Inside a Vault, the service reserves:

```text
_mcp-vault/
├── index.yaml                    # Optional user taxonomy
└── memory/
    └── records/<yyyy>/<mm>/<id>.md
```

The reserved path is configurable before first use but cannot be changed casually after memories exist. It remains visible and portable in Obsidian.

Default index exclusions:

```text
.obsidian/**
.trash/**
_mcp-vault/memory/**
```

Memory files are indexed by the memory projector but are not recursively passed through automatic memory extraction.

## 2. SQLite policies

Use one SQLite database for transactional consistency of operational state and the durable outbox.

Required connection initialization:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
```

The implementation may make durability configurable, but the production default must favor correctness.

Use SQLx migrations. Never edit a migration that may have shipped. Migrations must be tested against a prior-release fixture.

Use UTC integer milliseconds or RFC3339 consistently. Recommended database representation: signed integer milliseconds.

Identifiers should use UUIDv7 or ULID strings.

## 3. Vault registry

```sql
CREATE TABLE vaults (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    content_root TEXT NOT NULL UNIQUE,
    reserved_root TEXT NOT NULL DEFAULT '_mcp-vault',
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    settings_revision INTEGER NOT NULL DEFAULT 1
);
```

Initial statuses:

```text
active
maintenance
disabled
error
```

Even when only one Vault may be configured, all dependent rows use `vault_id`.

## 4. Configuration and secrets

### 4.1 Settings

```sql
CREATE TABLE system_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by TEXT
);

CREATE TABLE vault_settings (
    vault_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by TEXT,
    PRIMARY KEY(vault_id, key),
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

Settings are typed in Rust. JSON storage does not permit unvalidated arbitrary values.

### 4.2 Encrypted secrets

```sql
CREATE TABLE encrypted_secrets (
    id TEXT PRIMARY KEY,
    purpose TEXT NOT NULL,
    owner_type TEXT NOT NULL,
    owner_id TEXT,
    key_version INTEGER NOT NULL,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

Encryption uses an installation master key and authenticated encryption. Provider secrets are never returned after save; the UI receives only presence and a masked hint stored separately.

## 5. Admin identity and sessions

```sql
CREATE TABLE admin_users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    disabled INTEGER NOT NULL DEFAULT 0,
    password_changed_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE admin_sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    token_digest BLOB NOT NULL UNIQUE,
    csrf_secret_digest BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    source_ip TEXT,
    user_agent_hash BLOB,
    revoked_at INTEGER,
    FOREIGN KEY(user_id) REFERENCES admin_users(id)
);
```

Session tokens are high entropy and only their digest is stored.

## 6. WebDAV credentials

Treat WebDAV credentials as app passwords rather than general human accounts.

```sql
CREATE TABLE webdav_credentials (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    name TEXT NOT NULL,
    username TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    permissions_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at INTEGER,
    revoked_at INTEGER,
    UNIQUE(vault_id, username),
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

Passwords use Argon2id. Default permissions include DAV read/write/delete for the bound Vault.

## 7. MCP credentials and OAuth configuration

### 7.1 Personal access tokens

```sql
CREATE TABLE mcp_tokens (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    name TEXT NOT NULL,
    token_prefix TEXT NOT NULL,
    token_digest BLOB NOT NULL UNIQUE,
    scopes_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at INTEGER,
    revoked_at INTEGER,
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

The token is shown once. Store a keyed HMAC digest or equivalent lookup-safe keyed digest, not plaintext.

### 7.2 OAuth issuers and subject grants

```sql
CREATE TABLE oauth_issuers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    issuer_url TEXT NOT NULL UNIQUE,
    discovery_url TEXT,
    audience TEXT NOT NULL,
    jwks_cache_json TEXT,
    jwks_cached_at INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE oauth_subject_grants (
    id TEXT PRIMARY KEY,
    issuer_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    vault_id TEXT NOT NULL,
    scopes_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revoked_at INTEGER,
    UNIQUE(issuer_id, subject, vault_id),
    FOREIGN KEY(issuer_id) REFERENCES oauth_issuers(id),
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

Access-token validation checks issuer, signature, time, audience/resource, subject grant, and scopes.

## 8. Stable file identity

### 8.1 Current entries

```sql
CREATE TABLE file_entries (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    path TEXT NOT NULL,
    entry_type TEXT NOT NULL,
    current_revision INTEGER NOT NULL,
    content_hash TEXT,
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    filesystem_identity TEXT,
    deleted_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(vault_id, path),
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

`entry_type` values:

```text
file
directory
```

Path comparison policy must be documented per platform. The canonical server representation uses normalized `/` separators and NFC Unicode. The service should reject collisions caused by case-insensitive filesystems during setup or scan.

### 8.2 Revisions

```sql
CREATE TABLE file_revisions (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    operation TEXT NOT NULL,
    path_before TEXT,
    path_after TEXT,
    content_hash TEXT,
    history_blob_hash TEXT,
    size INTEGER,
    actor_type TEXT NOT NULL,
    actor_id TEXT,
    source_plane TEXT NOT NULL,
    idempotency_key TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(vault_id, file_id, revision),
    UNIQUE(vault_id, idempotency_key),
    FOREIGN KEY(vault_id) REFERENCES vaults(id),
    FOREIGN KEY(file_id) REFERENCES file_entries(id)
);
```

`operation` values:

```text
create
replace
patch
append
move
copy
delete
restore
external_change
```

A move preserves `file_id` and increments revision.

## 9. Operation journal and outbox

### 9.1 Operation journal

```sql
CREATE TABLE operation_journal (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    state TEXT NOT NULL,
    source_path TEXT,
    destination_path TEXT,
    prior_file_id TEXT,
    expected_revision INTEGER,
    prior_hash TEXT,
    proposed_hash TEXT,
    temp_path TEXT,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    error TEXT,
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

States:

```text
prepared
file_committed
metadata_committed
rolled_back
needs_review
```

### 9.2 Transactional outbox

```sql
CREATE TABLE outbox_events (
    id TEXT PRIMARY KEY,
    vault_id TEXT,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    available_at INTEGER NOT NULL,
    claimed_by TEXT,
    claimed_until INTEGER,
    delivered_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
```

Consumers must be idempotent.

## 10. Durable background jobs

```sql
CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    vault_id TEXT,
    job_type TEXT NOT NULL,
    dedup_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    available_at INTEGER NOT NULL,
    lease_owner TEXT,
    lease_until INTEGER,
    progress_json TEXT,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    UNIQUE(vault_id, dedup_key)
);
```

Statuses:

```text
queued
running
retry_wait
completed
failed
cancelled
```

## 11. Note metadata projection

### 11.1 Notes

```sql
CREATE TABLE notes (
    file_id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    path TEXT NOT NULL,
    revision INTEGER NOT NULL,
    title TEXT,
    aliases_json TEXT NOT NULL,
    frontmatter_json TEXT NOT NULL,
    plain_text TEXT NOT NULL,
    first_paragraph TEXT,
    language TEXT,
    word_count INTEGER NOT NULL,
    analyzed_content_hash TEXT NOT NULL,
    analyzer_version INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(file_id) REFERENCES file_entries(id),
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

### 11.2 Headings, tags, and links

```sql
CREATE TABLE note_headings (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    level INTEGER NOT NULL,
    heading_path_json TEXT NOT NULL,
    title TEXT NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER,
    UNIQUE(file_id, ordinal)
);

CREATE TABLE note_tags (
    vault_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    normalized_tag TEXT NOT NULL,
    source TEXT NOT NULL,
    PRIMARY KEY(file_id, normalized_tag, source)
);

CREATE TABLE note_links (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    target_text TEXT NOT NULL,
    target_file_id TEXT,
    target_heading TEXT,
    link_type TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    UNIQUE(source_file_id, ordinal)
);
```

Links are resolved against Obsidian semantics where practical. Unresolved links remain indexed.

### 11.3 FTS

Use an FTS5 virtual table with content suitable for lexical retrieval:

```text
vault_id (unindexed/filter companion)
file_id (unindexed)
path
title
aliases
tags
headings
plain_text
```

Because FTS5 virtual tables do not enforce ordinary foreign keys, updates must occur through a tested repository transaction and reconciliation must detect drift.

## 12. Knowledge index

```sql
CREATE TABLE index_nodes (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    parent_id TEXT,
    node_type TEXT NOT NULL,
    stable_key TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT,
    source_type TEXT NOT NULL,
    source_ref TEXT,
    confidence REAL,
    sort_key TEXT NOT NULL,
    content_version TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(vault_id, stable_key),
    FOREIGN KEY(parent_id) REFERENCES index_nodes(id)
);

CREATE TABLE index_memberships (
    vault_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    relevance REAL NOT NULL,
    source_type TEXT NOT NULL,
    PRIMARY KEY(node_id, file_id, source_type)
);
```

Node types may include:

```text
root
folder
manual_topic
tag
semantic_topic
entity
```

LLM-generated summaries are projections and carry provider/model/prompt metadata in a companion table or `source_ref`.

## 13. Memory schema

The complete memory schema is defined in `memory-system.md`. At minimum it includes:

- canonical memories;
- multiple provenance sources;
- candidates and review decisions;
- entities, tags, and relations;
- FTS projection;
- embeddings;
- lifecycle and temporal validity;
- recall statistics.

Memory queries always include `vault_id`.

## 14. Provider and model configuration

```sql
CREATE TABLE providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    base_url TEXT NOT NULL,
    secret_id TEXT,
    settings_json TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(secret_id) REFERENCES encrypted_secrets(id)
);

CREATE TABLE models (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    external_model_id TEXT NOT NULL,
    capability_json TEXT NOT NULL,
    settings_json TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(provider_id, external_model_id),
    FOREIGN KEY(provider_id) REFERENCES providers(id)
);

CREATE TABLE model_bindings (
    id TEXT PRIMARY KEY,
    vault_id TEXT,
    role TEXT NOT NULL,
    model_id TEXT NOT NULL,
    settings_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(vault_id, role),
    FOREIGN KEY(vault_id) REFERENCES vaults(id),
    FOREIGN KEY(model_id) REFERENCES models(id)
);
```

Roles:

```text
memory_extraction
memory_consolidation
note_summary
topic_enrichment
embedding_note
embedding_memory
rerank
```

A `NULL vault_id` is the global default. Future Vault-specific bindings override it.

## 15. Embeddings

Wrap vector storage behind an internal `VectorIndex`.

Metadata must include:

```sql
CREATE TABLE embedding_records (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    object_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    chunk_key TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    dimension INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    vector_backend_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(vault_id, object_type, object_id, chunk_key, model_id)
);
```

A pinned `sqlite-vec` backend may be used, but it remains behind the project interface because the extension is pre-1.0. Provide a deterministic exact-cosine fallback for development, recovery, and small installations.

Never mix dimensions or models in one similarity query.

## 16. Audit

```sql
CREATE TABLE audit_log (
    id TEXT PRIMARY KEY,
    occurred_at INTEGER NOT NULL,
    request_id TEXT,
    vault_id TEXT,
    plane TEXT NOT NULL,
    actor_type TEXT NOT NULL,
    actor_id TEXT,
    action TEXT NOT NULL,
    target_type TEXT,
    target_id TEXT,
    target_path_hash TEXT,
    result TEXT NOT NULL,
    metadata_json TEXT NOT NULL
);
```

Do not store note body, full memory body, password, token, API key, or provider authorization header by default.

Audit retention is configurable and independent from application logs.

## 17. Backup catalog

```sql
CREATE TABLE backups (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    location TEXT NOT NULL,
    manifest_json TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    verified_at INTEGER,
    error TEXT
);
```

The manifest identifies Vault content snapshot, state database snapshot, history snapshot, schema version, service version, and checksums.

## 18. Rebuildability matrix

| State | Rebuild source | Rebuildable |
|---|---|---|
| Note metadata, headings, tags, links | Vault files | Yes |
| FTS | Note projection or files | Yes |
| Embeddings | Canonical text + model configuration | Yes |
| Topic projections | Files + taxonomy + provider config | Yes |
| Automatic candidates | Source notes + extraction version | Yes |
| Active memory Markdown | Canonical memory files | No; must be preserved |
| Credentials and settings | Operational DB | No |
| Revisions/history | Operational DB + blob store | No |
| Audit | Operational DB | No |
| Jobs/outbox | Operational DB | Operational, recoverable through reconciliation |

## 19. Migration rules

- Back up operational state before a schema upgrade.
- Run migrations before readiness becomes healthy.
- Keep migrations forward-only.
- A migration that changes canonical managed Markdown requires an idempotent filesystem migrator with journal and dry-run support.
- Provider/model changes schedule new derived work; they do not rewrite source notes.
- Every migration must preserve Vault IDs and credential bindings.
