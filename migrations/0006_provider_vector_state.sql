-- MCP Vault WP-10 provider, embedding, and vector operational/derived state.
--
-- Provider/model configuration is operational state. Embedding metadata and
-- vector bytes are derived and may be deleted/rebuilt without touching
-- canonical Markdown, revisions, or durable memories.

ALTER TABLE providers
    ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1);

ALTER TABLE models
    ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1);

ALTER TABLE model_bindings
    ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1);

CREATE UNIQUE INDEX model_bindings_vault_role_idx
    ON model_bindings(vault_id, role)
    WHERE vault_id IS NOT NULL;

CREATE INDEX model_bindings_role_idx
    ON model_bindings(role, vault_id);

CREATE TABLE provider_health (
    provider_id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (
        status IN ('unknown', 'healthy', 'degraded', 'unavailable')
    ),
    checked_at INTEGER,
    latency_ms INTEGER,
    model_count INTEGER NOT NULL DEFAULT 0 CHECK (model_count >= 0),
    last_success_at INTEGER,
    last_error TEXT,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
);

CREATE TABLE embedding_records (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    object_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    chunk_key TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    content_hash TEXT NOT NULL,
    vector_backend_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (vault_id, id),
    UNIQUE (vault_id, object_type, object_id, chunk_key, model_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (provider_id) REFERENCES providers(id),
    FOREIGN KEY (model_id) REFERENCES models(id)
);

CREATE INDEX embedding_records_lookup_idx
    ON embedding_records(vault_id, object_type, object_id, model_id);

CREATE TABLE embedding_vectors (
    vault_id TEXT NOT NULL,
    embedding_id TEXT NOT NULL,
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    vector_blob BLOB NOT NULL,
    norm REAL NOT NULL CHECK (norm >= 0.0),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, embedding_id),
    FOREIGN KEY (vault_id, embedding_id)
        REFERENCES embedding_records(vault_id, id) ON DELETE CASCADE
);

CREATE INDEX embedding_vectors_dimension_idx
    ON embedding_vectors(vault_id, dimension, embedding_id);
