-- MCP Vault WP-02 operational state.
--
-- Canonical notes/attachments remain in Vault content roots. This migration
-- stores only authoritative operational metadata and rebuildable-work control
-- state. All timestamps are UTC Unix milliseconds.

CREATE TABLE vaults (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    content_root TEXT NOT NULL UNIQUE,
    reserved_root TEXT NOT NULL DEFAULT '_mcp-vault',
    status TEXT NOT NULL CHECK (status IN ('active', 'maintenance', 'disabled', 'error')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    settings_revision INTEGER NOT NULL DEFAULT 1 CHECK (settings_revision >= 0)
);

CREATE TABLE system_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    updated_at INTEGER NOT NULL,
    updated_by TEXT
);

CREATE TABLE vault_settings (
    vault_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    updated_at INTEGER NOT NULL,
    updated_by TEXT,
    PRIMARY KEY (vault_id, key),
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE TABLE encrypted_secrets (
    id TEXT PRIMARY KEY,
    purpose TEXT NOT NULL,
    owner_type TEXT NOT NULL,
    owner_id TEXT,
    key_version INTEGER NOT NULL CHECK (key_version > 0),
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE admin_users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
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
    FOREIGN KEY (user_id) REFERENCES admin_users(id)
);

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
    UNIQUE (vault_id, username),
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

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
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE TABLE oauth_issuers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    issuer_url TEXT NOT NULL UNIQUE,
    discovery_url TEXT,
    audience TEXT NOT NULL,
    jwks_cache_json TEXT,
    jwks_cached_at INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
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
    UNIQUE (issuer_id, subject, vault_id),
    FOREIGN KEY (issuer_id) REFERENCES oauth_issuers(id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE TABLE file_entries (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    path TEXT NOT NULL,
    entry_type TEXT NOT NULL CHECK (entry_type IN ('file', 'directory')),
    current_revision INTEGER NOT NULL CHECK (current_revision >= 0),
    content_hash TEXT,
    size INTEGER NOT NULL CHECK (size >= 0),
    modified_at INTEGER NOT NULL,
    filesystem_identity TEXT,
    deleted_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (vault_id, path),
    UNIQUE (vault_id, id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE TABLE file_revisions (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    operation TEXT NOT NULL CHECK (
        operation IN (
            'create',
            'replace',
            'patch',
            'append',
            'move',
            'copy',
            'delete',
            'restore',
            'external_change'
        )
    ),
    path_before TEXT,
    path_after TEXT,
    content_hash TEXT,
    history_blob_hash TEXT,
    size INTEGER CHECK (size IS NULL OR size >= 0),
    actor_type TEXT NOT NULL,
    actor_id TEXT,
    source_plane TEXT NOT NULL,
    idempotency_key TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE (vault_id, file_id, revision),
    UNIQUE (vault_id, idempotency_key),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)

);

CREATE TABLE operation_journal (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('prepared', 'file_committed', 'metadata_committed', 'rolled_back', 'needs_review')
    ),
    source_path TEXT,
    destination_path TEXT,
    prior_file_id TEXT,
    expected_revision INTEGER CHECK (expected_revision IS NULL OR expected_revision >= 0),
    prior_hash TEXT,
    proposed_hash TEXT,
    temp_path TEXT,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    error TEXT,
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

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
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    vault_id TEXT,
    job_type TEXT NOT NULL,
    dedup_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'running', 'retry_wait', 'completed', 'failed', 'cancelled')
    ),
    priority INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts > 0),
    available_at INTEGER NOT NULL,
    lease_owner TEXT,
    lease_until INTEGER,
    progress_json TEXT,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    UNIQUE (vault_id, dedup_key),
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE TABLE providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    base_url TEXT NOT NULL,
    secret_id TEXT,
    settings_json TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (secret_id) REFERENCES encrypted_secrets(id)
);

CREATE TABLE models (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    external_model_id TEXT NOT NULL,
    capability_json TEXT NOT NULL,
    settings_json TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (provider_id, external_model_id),
    FOREIGN KEY (provider_id) REFERENCES providers(id)
);

CREATE TABLE model_bindings (
    id TEXT PRIMARY KEY,
    vault_id TEXT,
    role TEXT NOT NULL,
    model_id TEXT NOT NULL,
    settings_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (vault_id, role),
    FOREIGN KEY (vault_id) REFERENCES vaults(id),
    FOREIGN KEY (model_id) REFERENCES models(id)
);

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
    metadata_json TEXT NOT NULL,
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE INDEX admin_sessions_user_idx ON admin_sessions(user_id);
CREATE INDEX oauth_grants_vault_idx ON oauth_subject_grants(vault_id);
CREATE INDEX file_entries_vault_revision_idx
    ON file_entries(vault_id, current_revision);
CREATE INDEX file_revisions_vault_file_idx
    ON file_revisions(vault_id, file_id, revision);
CREATE INDEX operation_journal_recovery_idx
    ON operation_journal(vault_id, state, updated_at);
CREATE INDEX outbox_available_idx
    ON outbox_events(available_at, claimed_until, delivered_at);
CREATE INDEX outbox_vault_idx
    ON outbox_events(vault_id, created_at);
CREATE INDEX jobs_available_idx
    ON jobs(status, available_at, lease_until);
CREATE INDEX jobs_vault_idx
    ON jobs(vault_id, status, available_at);
CREATE INDEX models_provider_idx
    ON models(provider_id, enabled);
CREATE INDEX audit_vault_time_idx
    ON audit_log(vault_id, occurred_at);

-- SQLite treats NULL values as distinct in ordinary UNIQUE indexes. These
-- partial indexes make one global job/model binding per key enforceable.
CREATE UNIQUE INDEX jobs_global_dedup_idx
    ON jobs(dedup_key)
    WHERE vault_id IS NULL;

CREATE UNIQUE INDEX model_bindings_global_role_idx
    ON model_bindings(role)
    WHERE vault_id IS NULL;
