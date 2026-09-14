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

All dependent rows use `vault_id`. New managed Vaults retain the existing
registry schema and derive their root as `<data-dir>/vaults/<slug>`.

The typed global system setting `vault.legacy_default_id` stores a JSON Vault
ID for compatibility with historical unscoped Admin routes. On upgrade it is
initialized from the `default` slug, otherwise from the sole registered Vault;
it never changes merely because another Vault sorts earlier.

Managed admission inserts the registry row and one queued
`vault.initialize` job in the same SQLite transaction. Its dedup key is
`vault:<vault-id>:initialize`. Effective availability is derived as follows:

```text
non-active registry status   -> maintenance / disabled / error
active + queued/running job  -> initializing
active + failed job          -> error
active + completed/no job    -> ready
```

The no-job case preserves pre-feature single-Vault databases. Job
`vault_id`, not payload content, is authoritative.

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
    hint TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

Encryption uses an installation master key and authenticated encryption. Provider secrets are never returned after save; the UI receives only presence and a masked hint stored separately.

Provider secrets are owned by `owner_type = 'provider'` and the Provider ID.
A successful secret-bearing Provider edit retains only the newly referenced
ciphertext. Deleting the Provider removes all ciphertext owned by that ID in
the same State transaction as dependent model/binding/vector cleanup; audit
metadata records counts only, never hints or secret values.

Migration `0003_auth_security.sql` adds the non-secret `hint`, records the
digest key version used by Admin sessions and MCP PATs, and adds the protected
OAuth resource identifier. Existing migrations remain immutable.

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
    digest_key_version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    source_ip TEXT,
    user_agent_hash BLOB,
    revoked_at INTEGER,
    FOREIGN KEY(user_id) REFERENCES admin_users(id)
);
```

Session tokens are high entropy and only their keyed digest and digest-key
version are stored. The CSRF secret follows the same rule.

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
    digest_key_version INTEGER NOT NULL DEFAULT 1,
    scopes_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at INTEGER,
    revoked_at INTEGER,
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

The token is shown once. Store a keyed HMAC digest or equivalent lookup-safe
keyed digest, not plaintext. A master-key version allows retained old keys to
validate existing tokens during rotation.

### 7.2 Built-in OAuth authorization state

Migration `0012_builtin_oauth_authorization_server.sql` adds six operational
tables:

```text
oauth_local_users
    id, vault_id (unique), username, password_hash, scopes_json,
    enabled, password_changed_at, created_at, updated_at

oauth_clients
    id, client_name, redirect_uris_json, grant_types_json,
    response_types_json, token_endpoint_auth_method, timestamps/revocation

oauth_authorization_requests
    id, request_digest, digest_key_version, client_id, vault_id, resource,
    redirect_uri, scopes_json, state, code_challenge, expiry/consumption

oauth_authorization_codes
    id, code_digest, digest_key_version, client_id, user_id, vault_id,
    resource, redirect_uri, scopes_json, code_challenge, expiry/consumption

oauth_access_tokens
    id, family_id, token_prefix, token_digest, digest_key_version,
    client_id, user_id, vault_id, resource, scopes_json,
    creation/expiry/last-use/revocation

oauth_refresh_tokens
    id, family_id, token_prefix, token_digest, digest_key_version,
    client_id, user_id, vault_id, resource, scopes_json,
    creation/expiry/rotation/revocation
```

`password_hash` is an Argon2id PHC string. Request, code, access-token, and
refresh-token plaintext never enters SQLite; only 32-byte installation-keyed
digests and safe access/refresh lookup prefixes are stored. Every grant-bearing
row contains a Vault predicate and exact resource. Client rows are global
public registrations because DCR occurs before resource authorization, but
they cannot grant a Vault by themselves.

The existing `scopes_json` arrays on local authorization requests, codes, and
tokens may also contain `offline_access`. Auth parses it separately from domain
Vault/memory scopes, so it never contributes a permission. Older arrays without
that value remain valid and require no migration.

Authorization completion atomically records the first successful completion
time and inserts one code. A correctly authenticated retry of the same
still-valid request inserts another distinct code; password rotation/disable
deletes all outstanding request rows. Code exchange atomically consumes that
code and inserts one access/refresh
family. Refresh atomically rotates the presented row and inserts the next pair;
the inserted refresh row binds its newly calculated 180-day idle expiry rather
than copying the predecessor's deadline. A duplicate rotation at or within 60
seconds is rejected without revoking the committed successor; later replay
revokes both access and refresh rows in the family. `rotated_at` is the durable
decision timestamp, including when a concurrent compare-and-set loser re-reads
the row. Replacing/disabling
the one local user for a Vault consumes/revokes every outstanding local row in
one transaction. Expired/consumed rows are removed by bounded opportunistic
cleanup after a retention window.

### 7.3 Optional external OAuth issuers and subject grants

```sql
CREATE TABLE oauth_issuers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    issuer_url TEXT NOT NULL UNIQUE,
    discovery_url TEXT,
    audience TEXT NOT NULL,
    resource TEXT,
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

Access-token validation checks issuer, signature, time, audience/resource,
subject grant, and scopes. `jwks_cache_json` contains only normalized RSA public
key fields and is never returned in an Admin response or default log. Migration
0009 disables and clears every prerelease cached JWKS because older rows may
contain plaintext symmetric key material; the operator must re-save a public
RSA set through Admin.

`resource` is the exact canonical MCP endpoint identifier. A validated access
token may carry that resource indicator in `aud` (the normal RFC 8707 shape) or
in an explicit `resource` claim. The separately configured `audience` check
still applies, so setting audience and resource to the same MCP endpoint is the
recommended interoperable default.

### 7.4 Installation-key identity

```sql
CREATE TABLE installation_key_checks (
    key_version INTEGER PRIMARY KEY,
    verification_digest BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

`verification_digest` is a fixed-purpose keyed digest, not key material. It is
used only to reject a missing or different installation key at startup and is
safe to include in the operational-state backup.

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

The domain path value uses an empty logical string for the Vault root and
accepts a leading `/` only at the explicit URL-path decoding boundary. The
portable safety limits are 4,096 normalized UTF-8 bytes per path, 64 segments,
and 255 bytes per segment. The reserved `_mcp-vault` namespace is not an
ordinary user path and requires managed service access.

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

When WebDAV overwrites an existing destination, the DAV library first
materializes the old destination as a tombstone. Vault Core archives that
tombstone under the reserved operational namespace in the same metadata
transaction, then moves the source row into the destination while preserving
the source `file_id`. The archive is not exposed as ordinary Vault content;
the old delete revision and history remain available to recovery/audit code.

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
    idempotency_key TEXT,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    error TEXT,
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);

The optional `idempotency_key` is unique within a Vault when present. It
allows a retry after a process interruption to locate the original journal
intent without treating an opaque payload as an identity.
```

States:

```text
prepared
file_committed
metadata_committed
rolled_back
needs_review
superseded
discarded
```

`superseded` is reserved for the State witness verifier. It records that a
later create's active File ID, revision, metadata journal and physical hash
prove the same canonical result. It does not create a revision for the old
File ID or modify the later file and its history.

`discarded` is reserved for an explicitly confirmed predecessor-memory
initialization. It records a terminal old Core intent only after Vault Core
proves every operation path stayed inside the managed legacy-memory namespace;
its canonical files are still removed separately through normal Core
retirement and manifest hash checks. It is never used for ordinary Vault or
new v3 memory operations.

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
    last_error TEXT,
    dead_lettered INTEGER NOT NULL DEFAULT 0,
    dead_letter_reason TEXT
);

CREATE INDEX outbox_claim_idx
    ON outbox_events(dead_lettered, delivered_at, available_at, claimed_until);
```

Consumers must be idempotent. A worker claims an undelivered row with a
conditional lease, acknowledges it only after durable derived-work admission,
and clears the lease on success. Lease expiry makes the row reclaimable after
a process crash. Retryable failures use bounded exponential backoff;
non-retryable failures or exhausted attempts set `dead_lettered` and retain a
redacted reason for operator inspection.

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
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    UNIQUE(vault_id, dedup_key)
);

CREATE UNIQUE INDEX jobs_global_dedup_idx
    ON jobs(dedup_key)
    WHERE vault_id IS NULL;
CREATE INDEX jobs_claim_idx
    ON jobs(status, cancel_requested, available_at, lease_until);
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

`vault_id` is mandatory for Vault work and may be `NULL` only for explicitly
global jobs. Vault jobs are deduplicated by `(vault_id, dedup_key)`; global
jobs use the partial unique index above. Claims increment `attempts` and set a
conditional lease. Progress is bounded JSON, cancellation is durable, and
expired leases are reclaimable by another worker.

At equal priority, eligible claims are interleaved by `vault_id` using a
per-Vault row rank before age ordering. Global jobs form their own explicit
partition. Cancelling one Vault selects only rows with that `vault_id`.

## 10.1 Scan checkpoints

Filesystem scans keep their durable lifecycle separate from jobs so startup
and periodic reconciliation remain observable and restart-safe:

```sql
CREATE TABLE scan_checkpoints (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    scan_type TEXT NOT NULL,
    generation TEXT NOT NULL,
    cursor_path TEXT,
    status TEXT NOT NULL,
    entries_seen INTEGER NOT NULL DEFAULT 0,
    files_seen INTEGER NOT NULL DEFAULT 0,
    directories_seen INTEGER NOT NULL DEFAULT 0,
    changes_imported INTEGER NOT NULL DEFAULT 0,
    unsafe_entries_skipped INTEGER NOT NULL DEFAULT 0,
    missing_deletes_skipped INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    UNIQUE(vault_id, scan_type),
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

`scan_type` is currently `initial` or `reconciliation`. A generation owns
its progress updates; stale generations cannot overwrite a newer run. The
cursor is only a validated relative-path hint. Core still completes a full
safe comparison before marking a pass `completed`, so a missed watcher event
cannot become lost knowledge.

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
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
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
    UNIQUE(file_id, ordinal),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE TABLE note_tags (
    vault_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    normalized_tag TEXT NOT NULL,
    source TEXT NOT NULL,
    PRIMARY KEY(file_id, normalized_tag, source),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
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
    UNIQUE(source_file_id, ordinal),
    FOREIGN KEY (vault_id, source_file_id)
        REFERENCES file_entries(vault_id, id),
    FOREIGN KEY (vault_id, target_file_id)
        REFERENCES file_entries(vault_id, id)
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
    FOREIGN KEY (vault_id, parent_id)
        REFERENCES index_nodes(vault_id, id)
);

CREATE TABLE index_memberships (
    vault_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    relevance REAL NOT NULL,
    source_type TEXT NOT NULL,
    PRIMARY KEY(node_id, file_id, source_type),
    FOREIGN KEY (vault_id, node_id)
        REFERENCES index_nodes(vault_id, id),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
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

The deterministic rebuild also stores one Vault-scoped `index_status` row with
the projection revision, analyzed entry/note/byte counts, analyzer version,
coverage JSON, last successful rebuild time, and a redacted failure code.
The `note_fts` FTS5 companion table keeps `vault_id` and `file_id` as
unindexed filter columns; it is always replaceable from the canonical notes.

## 13. Memory schema

The normative v3 schema is migration `0028_source_preserving_memory_units.sql` and [Source-preserving memory units](memory-system.md):

| Table | Responsibility |
| --- | --- |
| `memory_units` | Explicit or source-owned current complete body, optional metadata, exact hash and canonical revision |
| `memory_unit_sources` | Vault-scoped current source identity, hash and coordinates |
| `memory_unit_sets` | One source-owned collection, expected source/set/canonical revisions, current hash and source pause |
| `memory_unit_snapshots` | Prepared complete-set handoff for Vault Core/SQLite crash recovery |
| `memory_unit_idempotency`, `memory_unit_reservations` | Exact completed/prepared explicit request identities |
| `memory_units_fts` | Rebuildable current-unit lexical index |
| `memory_unit_selection_batches` | Validated batch output keyed by exact source/input identity |
| `memory_unit_selection_progress` | Current-source batch counters and skipped-unit diagnostics |
| `memory_unit_overviews`, `memory_unit_overview_dependencies` | Derived navigation and exact current-unit dependencies |
| `memory_unit_runtime` | Pause and source/overview dependency generations |
| `memory_unit_initialization` | Offline required/clearing/ready phase and resumable cleanup manifest |

All keys, queries and child relationships enforce the Vault boundary. Automatic eligibility joins live source File ID/hash and canonical file/revision. Canonical bodies live under `memory-v3/explicit` and `memory-v3/sources` in the reserved root. Embedding rows use `object_type=memory_unit` with exact content/profile/prepared-input hashes.

Predecessor tables remain in historical DDL so applied migration checksums remain stable. Only the exclusive offline initializer reads their content-free counts and deletes allow-listed Vault-scoped rows. No runtime memory reader or format parser converts or adopts them. Migration 0028 copies no predecessor memory rows and marks every pre-existing Vault as requiring initialization. The initializer preserves non-memory operational records, ordinary files and revision history; repeat execution after ready cannot clear new units.

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
note_summary
topic_enrichment
embedding_note
embedding_memory
rerank
```

A `NULL vault_id` is the global default. Future Vault-specific bindings override it.

Migration 0006 adds optimistic configuration revisions to providers, models,
and bindings. It also adds provider_health for redacted test/availability
state. Existing provider/model rows from the operational foundation remain
compatible and are upgraded forward-only.

Migration 0018 adds `embedding_revision` to providers and models, initially copied
from each existing `revision`. Admin edits still compare and advance `revision`.
Vector fingerprints use `embedding_revision`, which remains unchanged for Provider
display-name/enabled-only edits and identical stored model configuration. Relevant
configuration changes atomically advance it to the new edit revision. Existing
fingerprints, vector bytes, input hashes and calibration signatures survive upgrade.
The migration also closes unfinished calibration records referenced only by terminal
jobs; an active job with that signature prevents retirement. Queries match Vault
identity, and stored caches, reports and consumed budgets are retained.

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
    profile_hash TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    vector_backend_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(vault_id, object_type, object_id, chunk_key, model_id)
);
```

A pinned `sqlite-vec` backend may be used, but it remains behind the project interface because the extension is pre-1.0. Provide a deterministic exact-cosine fallback for development, recovery, and small installations.

Never mix dimensions, models, profiles, or prepared-input hashes in one
similarity query. `content_hash` identifies the source object state;
`input_hash` separately identifies the exact prefixed/chunked text actually
sent to the embedding model. `profile_hash` covers the non-secret provider,
model, dimension, endpoint/settings/capabilities, task/prefix/chunk rules, and
projection version.

Similarity queries also require an exact object type before Top-K selection.
The current values are `memory` and `note`; each `note` row represents one
deterministic note chunk. Filtering after Top-K is forbidden because a large
note partition could otherwise crowd memory vectors out of recall.

For note retrieval, the object-type-filtered vector rows form a bounded raw
candidate pool rather than the final note Top-K. The Index service reconstructs
current chunk keys/hashes, rejects stale and negative hits, and retains only the
highest cosine row per `object_id` before unique-note rank fusion. This keeps
current-source validation above the generic vector backend and prevents a long
note's many chunk rows from occupying multiple note result slots.

The exact fallback stores normalized f32 values in a Vault-scoped
embedding_vectors BLOB row referenced by embedding_records. Both metadata and
vector bytes are derived and may be deleted and rebuilt independently of
canonical notes and memory Markdown.

For `object_type = 'note'`, `object_id` is the stable `FileId` and `chunk_key`
is a versioned deterministic plain-text chunk key such as `text-v2:0000`.
`content_hash` covers the exact title/path/heading context and chunk text sent
to the embedding model. A `text-v2` input is at most 2,048 UTF-8 bytes including
context. The Index service resolves the reference from the current `notes`
projection before a job sends content, skips a stale hash, and removes obsolete
current-model vectors before scheduling replacements. No separate canonical
chunk table is required; chunks are reproducible from the canonical note-derived
projection.

`embedding.rebuild` payloads also contain `projection_version`. This version is
part of the deduplication key so a corrected derived profile never mutates or
silently reuses an older terminal job. Admin job projections omit source IDs,
chunk hashes, paths, and bodies.

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
    status TEXT NOT NULL CHECK (status IN
        ('queued', 'running', 'completed', 'failed', 'validating', 'restoring')),
    location TEXT NOT NULL,
    manifest_json TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    verified_at INTEGER,
    error TEXT,
    created_by TEXT
);
```

Migration `0008_backup_catalog.sql` owns this table. Artifact bytes remain in
the service-owned backup directory; `manifest_json` is bounded redacted
metadata and never contains passwords, tokens, provider plaintext, or note
bodies. Backup jobs are global operational jobs, while every manifest content
and history path remains explicitly prefixed by its Vault ID.

The manifest identifies Vault content snapshot, state database snapshot,
history snapshot, schema version, service version, retained encryption-key
version identifiers, and checksums. Key material is never stored in the
artifact.

## 18. Rebuildability matrix

| State | Rebuild source | Rebuildable |
|---|---|---|
| Note metadata, headings, tags, links | Vault files | Yes |
| FTS | Note projection or files | Yes |
| Embeddings | Canonical text + model configuration | Yes |
| Topic projections | Files + taxonomy + provider config | Yes |
| Current memory SQLite/FTS projection | Current canonical memory/set Markdown + exact source metadata | Yes |
| Legacy Phase 1/Phase 2/source-health rows | Migration backup only | Not part of current runtime |
| Current explicit/set Markdown | Canonical memory files/source notes | Explicit must be preserved; note sets can be regenerated |
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

Migrations 0006-0014 below are immutable release history. Their prerelease
memory tables and jobs are not the current runtime contract; ADR-0026 and
migration 0015 define the active current-only model.

- Migration 0006 upgrades provider/model configuration and adds rebuildable
  provider health and Vault-scoped embedding/vector state.
- Migration 0007 adds the Vault-scoped final-memory projection, legacy
  candidate upgrade state, provenance, lifecycle/recall state, idempotency,
  diagnostics, and FTS.
- Migration 0008 adds the backup catalog and keeps artifact data outside
  SQLite so catalog cleanup cannot become an implicit knowledge deletion.
- Migration 0009 adds the installation-key verifier and removes legacy cached
  OAuth key JSON that cannot be proven public-only.
- Migration 0010 adds Phase 1 raw/no-output state, prepared Phase 2 proposals,
  committed generations, pipeline state, and the one-active-
  consolidation-per-Vault index.
- Migration 0011 performs the ADR-0017 prerelease cutover: it deletes every old
  memory job and database memory row, preserves ordinary Vault/provider/audit/
  backup/non-memory state, recreates generation state with explicit
  `pipeline_generation`/`regeneration_pending` fields, and admits filesystem
  cleanup through the durable `memory.reset_pipeline` job.
- Migration 0012 adds the built-in OAuth authorization server without changing
  existing Vault credentials.
- Migration 0013 adds continuously reconciled memory-source health and
  initializes legacy note sources as unverified.
- Migration 0014 adds multilingual retrieval metadata/recovery proposals and
  rebuilds only the derived memory FTS. It does not rewrite existing canonical
  memory; an authenticated Admin starts any historical body/alias backfill
  after taking a backup.
- Migration 0015 adds the current-only explicit/source-set schema, prepared
  whole-set snapshots, explicit idempotency reservations, non-destructive
  migration preflight state, and exact embedding profile/input hashes. Legacy
  rows are retained and never auto-promoted or deleted; an authenticated Admin
  explicitly preflights and executes migration.
- Every migration must preserve Vault IDs and credential bindings.


## Migrations 0016–0017: local deletion and calibration

Migration 0016 preserves prepared snapshots and allows null Provider/model identity for local deletion. Migration 0017 adds Vault/channel/signature calibration runs, frozen engineering budgets, cumulative dispatch accounting, bounded checkpoints and reports, plus a partial unique index for active retrieval.calibrate jobs. These changes do not rewrite canonical content or business vectors.

The complete behavior, API mapping, quality gates and upgrade/rollback procedure are
specified in [Automatic retrieval calibration and memory administration](memory-autocalibration-operations.md).

## Migration 0019: derived note grouping

`note_chunk_plans` is keyed by `(vault_id,file_id)` and stores current source hash
and JSON grouping/fallback/temporary claim metadata, never canonical prose. Reads
and publication require the current note and live canonical file hash. Rebuilds
retain matching plans; obsolete source plans cannot resolve vectors. Migration also
retires old automatic diagnostic jobs without refunding budgets or changing valid
vectors, completed reports, source pauses or canonical content.

## Migration 0020: bounded equivalence judgments

`memory_equivalence_decisions` stores `(vault_id,input_hash,relation,created_at)`
without canonical prose. The exact request, extraction profile and judgment rule
version determine its hash; the first committed decision wins for a key.
`memory_equivalence_dispatches` records actual transport dispatch byte counts and
timestamps under a Vault foreign key. An immediate transaction removes expired
accounting. Migration 0023 retires the former 256-request/4-MiB daily cap;
actual dispatches continue to be recorded without enforcing a daily total.
Failed or interrupted requests retain their reservation. No manual budget reset
is required when old reservations expire. This forward migration preserves all
source sets, explicit records, vectors, pauses and pending snapshots. It is not a
formal-memory schema migration and does not imply semantic upgrade completion.

### Formal memory projections (migration 0021)

`memory_formal_items` has independent IDs and canonical revisions.
`memory_formal_supports` links exact contribution/source/semantic hashes;
`memory_valid_formal_supports` qualifies them against current sources and canonical
sets. `memory_formal_mode` atomically switches each Vault's `memory_public_items`
view from old contributions to formal objects; explicit items remain independent.

`memory_formal_operations` persists before/after facts and optional full source
rewrites through canonical publication, atomic projection commit and cleanup.
`memory_formal_pairs` and `memory_formal_examined` preserve bounded candidate work;
`memory_formal_maintenance` exposes content-free progress and automatic retry time.
`memory_equivalence_decisions`, `memory_equivalence_rewrites` and
`memory_equivalence_dispatches` store rebuildable proposals and persistent billing
bounds. Every key/query is Vault-scoped. Formal knowledge is portable Markdown,
not confined to these projections. Formal facts use `mcp-vault-memory-fact/v2.2`;
source-set and explicit canonical formats remain compatible with v2.1.

`memory_formal_identity_reservations` retains only allocated formal IDs, preventing
absorbed IDs from being reused after a source is regenerated. It contains no
prose or lifecycle history and is not exposed by model-readable APIs.


Migration 0022 adds Vault-scoped `memory_dedup_progress`: current phase,
singleton scan cursor and cumulative successful body-check count. These are
operational checkpoints without memory bodies; migration preserves existing
formal objects, candidate decisions, retry deadlines and jobs. Cancellation may
leave the cursor past an interrupted item; that current item remains eligible
on the next sweep. Canonical memory formats remain v2.1 source sets/v2.2 facts.

Migration 0023 adds `attempt_sequence` and `priority` to `memory_formal_pairs`.
Pending work sorts by attempt sequence, descending priority, then stable key;
a Vault-scoped atomic update rotates an attempt before network I/O. Completion
is recorded separately. `memory_dedup_new_contributions` persists newly published
contribution IDs in the publication transaction. Discovery consumes these IDs
and promotes their candidate pairs transactionally. Existing projections and
successful decisions are retained; only the retired local-budget failure's retry
deadline is cleared automatically. Provider backoff remains unchanged.

`memory_formal_maintenance.fingerprint` now checkpoints the metadata/profile seen
at the start of a completed pass. Admission compares it with current metadata
and requires covered status plus no pending pairs, new contributions or recovery
operations before skipping a job. Fingerprinting pages metadata in batches of
128; no note bodies, secrets or model calls are involved.


## Migration 0024: incremental memory organization

ADR-0031 retires the scheduling and proposal records introduced in migrations
0020–0023. Migration deletes old automatic dedup jobs in every status, old pairs,
examined fingerprints, maintenance/sentence progress, new-contribution markers,
judgment/rewrite caches and dispatch accounting. It preserves canonical source
and formal projections, identity reservations and pending formal operations.
Vault Core must recover those operations before the one-time formal reset.

New Vault-scoped tables:

| Table | Authority |
|---|---|
| `memory_organization_state` | One-time initialization, pause, phase, backoff and counters |
| `memory_organization_sources` | Dirty source ID, generation and incremental priority |
| `memory_organization_items` | One checkpoint per contribution, exact input hash, generation, completion/outcome, attempt order and vector stamp |
| `memory_organization_vectors` | Coalesced per-formal-object vector arrival and generation |
| `memory_organization_decisions` | Rebuildable validated batch decisions keyed by exact input |
| `memory_organization_dispatches` | Content-free actual dispatch/input-byte accounting |

Source-set and source-file triggers persist dirty identities in the same
transaction as source changes. Embedding arrival triggers coalesce vector work.
Generation comparisons prevent acknowledgement from deleting newer work. Completed
unchanged contribution inputs survive source reconciliation; an explicit recheck
or relevant vector arrival increments their generation. The unique active-job
index permits at most one queued/running/retry-wait `memory.organize` per Vault.
The initialization marker is never reset on ordinary process startup. Historical
empty pair tables are not consulted or populated by the production organizer.


## Migration 0025: bounded initialization checkpoints

Adds `reset_cursor`, `reset_complete`, `adoption_cursor`, `reset_checked` and
`adoption_checked` to `memory_organization_state`. It preserves the existing
initialized flag, paused state, jobs and prepared formal operations. No previous
migration is rewritten. Reset and adoption each visit at most eight entries per
worker slice; checkpoints advance only after a canonical publication or verified
no-op. Interrupted publication recovers before cursor advancement. Already
initialized Vaults do not repeat cutover. Counters describe initialization visits,
not model calls or candidate pairs.

Organization diagnostics need no schema migration. The existing item `outcome`
text distinguishes retained-result reasons for new completions. Newly written
decision JSON contains an application-owned `validation` field alongside
`relations`; older readers ignore it and newer readers tolerate its absence.
Missing classifications remain unknown and are never backfilled by guessing.
On-demand Admin aggregation uses fixed category names and one read transaction,
with every query constrained by the authenticated Vault identity.

## Migration 0026: repair organization dimension discovery

No columns or canonical data change. Completed independent organization items
are requeued once when they have matching-content, positive-norm stored memory
vectors under the currently selected embedding model and that model omits an
expected dimension. A Vault-specific binding takes precedence over the global
binding. Each join between work, support, formal memory and vector records carries
the Vault predicate. Grouped contributions, unrelated models, missing vectors
and explicit-dimension overrides remain completed. Generation advances to fence
older completion attempts; pause state and decision caches remain intact.


## Migration 0027: per-fact support and lossless consolidation

Adds `memory_formal_facts` (Vault, formal ID, one-based ordinal, content) and
`memory_formal_fact_supports` (Vault, formal ID, ordinal, contribution ID).
Composite foreign keys enforce both the fact owner and contribution membership;
a fact cannot reference another group's or another Vault's membership. Stored
membership now means belonging to the group, not independently supporting its
entire body. Legacy documents without per-fact rows keep their whole-support
read semantics until the one-time local reset.

Formal Markdown schema v2.3 stores `facts` together with exact original memberships.
The numbered body is derived deterministically from those facts. Both tables are
rebuildable from canonical files; v2.2 remains readable. Every public current-read
predicate rejects a consolidated body if any fact lacks a current exact support.
On lost evidence, local reconciliation unfolds complete surviving original
contributions and atomically requeues them, instead of generating replacement text
or allowing a partial source to justify a missing detail.

The migration preserves current/canonical records, cached decisions, credentials,
pause state and prepared file operations. It resets the bounded initialization
checkpoint only in Vaults with existing formal objects and marks current source
sets dirty, so old judgments are revisited from original contributions exactly
once. Ordinary restarts never repeat the reset. Prepared operations recover before
initialization changes canonical objects. Rollback requires matching pre-upgrade
Vault and SQLite backups.

The pending-order index admits successful `continuing` stages before ordinary
work. Selecting one clears the preference before I/O; generation and input checks
prevent a stale request from changing newer work. Stage caches record bounded
proposal/check JSON plus application validation; physical request accounting
remains content-free and separate from successful publication counts.

## Migration 0028: source-preserving memory units

Creates independent v3 tables, FTS, batch checkpoints, source progress, overview dependencies, runtime pause/generation state and offline initialization manifests. Unit mutations and source-file changes invalidate overview generations. No prior migration file is rewritten and no predecessor memory is converted. Legacy rows are discarded only by the explicit offline maintenance command after paired backup and exclusive locking.

## Migration 0029: directory and topic navigation dependencies

Adds Vault-scoped invalidation triggers for current source index memberships and node labels. It requests a scoped overview pass while keeping matching global caches reusable. No user content, Provider settings or prior migration checksum changes.

## Migration 0033: terminal discarded legacy-memory journals

Extends `operation_journal.state` with `discarded`. Explicit legacy-memory
initialization uses this terminal state only after Vault Core proves every
canonical path in the selected intent is wholly inside the managed predecessor
memory namespace. The row and its content-free discard reason remain available
as operational history; the row is excluded from recovery and review queues.
Migration 0033 preserves all existing journal rows, indexes, operation payloads
and checksums of earlier migrations.
