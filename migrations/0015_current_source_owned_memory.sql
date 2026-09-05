-- MCP Vault memory v2.1: current-only, source-owned memory sets.
--
-- This migration is deliberately additive.  Legacy lifecycle rows remain in
-- the old `memories` graph as migration input, but no v2.1 read path treats
-- them as current.  Operators must run an explicit preflight and migration;
-- startup never destroys or silently promotes legacy data.

CREATE TABLE memory_note_sets (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    source_content_hash TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),
    set_revision INTEGER NOT NULL CHECK (set_revision >= 1),
    extraction_paused INTEGER NOT NULL DEFAULT 0 CHECK (extraction_paused IN (0, 1)),
    canonical_file_id TEXT NOT NULL,
    canonical_path TEXT NOT NULL,
    canonical_revision INTEGER NOT NULL CHECK (canonical_revision >= 1),
    profile_hash TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    provider_id TEXT,
    model_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (vault_id, id),
    UNIQUE (vault_id, source_file_id),
    UNIQUE (vault_id, canonical_path),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_file_id)
        REFERENCES file_entries(vault_id, id),
    FOREIGN KEY (vault_id, canonical_file_id)
        REFERENCES file_entries(vault_id, id),
    FOREIGN KEY (provider_id) REFERENCES providers(id),
    FOREIGN KEY (model_id) REFERENCES models(id)
);

CREATE INDEX memory_note_sets_freshness_idx
    ON memory_note_sets(vault_id, source_file_id, source_content_hash, extraction_paused);

CREATE TABLE memory_current_items (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    ownership TEXT NOT NULL CHECK (ownership IN ('explicit', 'note_derived')),
    note_set_id TEXT,
    ordinal INTEGER,
    kind TEXT,
    content TEXT NOT NULL,
    normalized_content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    importance REAL CHECK (importance IS NULL OR (importance >= 0.0 AND importance <= 1.0)),
    confidence REAL CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    origin TEXT NOT NULL CHECK (
        origin IN ('explicit_agent', 'explicit_admin', 'import', 'note_extracted')
    ),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    canonical_file_id TEXT,
    canonical_path TEXT,
    canonical_revision INTEGER,
    valid_from INTEGER,
    valid_to INTEGER,
    tags_json TEXT NOT NULL DEFAULT '[]',
    entities_json TEXT NOT NULL DEFAULT '[]',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_recalled_at INTEGER,
    recall_count INTEGER NOT NULL DEFAULT 0 CHECK (recall_count >= 0),
    UNIQUE (vault_id, id),
    CHECK (
        (ownership = 'note_derived' AND note_set_id IS NOT NULL AND ordinal IS NOT NULL
            AND canonical_file_id IS NULL AND canonical_path IS NULL AND canonical_revision IS NULL)
        OR
        (ownership = 'explicit' AND note_set_id IS NULL AND ordinal IS NULL
            AND canonical_file_id IS NOT NULL AND canonical_path IS NOT NULL
            AND canonical_revision IS NOT NULL)
    ),
    UNIQUE (vault_id, note_set_id, ordinal),
    UNIQUE (vault_id, canonical_path),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, note_set_id)
        REFERENCES memory_note_sets(vault_id, id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, canonical_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memory_current_items_list_idx
    ON memory_current_items(vault_id, ownership, kind, updated_at DESC, id);

CREATE INDEX memory_current_items_set_idx
    ON memory_current_items(vault_id, note_set_id, ordinal);

CREATE TABLE memory_current_sources (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    source_type TEXT NOT NULL CHECK (
        source_type IN ('note', 'explicit_agent', 'explicit_admin', 'import')
    ),
    note_file_id TEXT,
    note_path TEXT,
    note_revision INTEGER,
    source_content_hash TEXT,
    heading_path_json TEXT NOT NULL DEFAULT '[]',
    start_line INTEGER,
    end_line INTEGER,
    excerpt_hash TEXT,
    actor_id TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE (vault_id, id),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memory_current_items(vault_id, id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, note_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memory_current_sources_memory_idx
    ON memory_current_sources(vault_id, memory_id, created_at, id);

CREATE INDEX memory_current_sources_note_idx
    ON memory_current_sources(vault_id, note_file_id, source_content_hash);

CREATE TABLE memory_current_idempotency (
    vault_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, idempotency_key),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memory_current_items(vault_id, id) ON DELETE CASCADE
);

-- Reserve an explicit-memory identity before touching the canonical file. A
-- retry can then adopt a file committed immediately before a process exit.
CREATE TABLE memory_current_explicit_reservations (
    vault_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, idempotency_key),
    UNIQUE (vault_id, memory_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE memory_current_fts USING fts5(
    vault_id UNINDEXED,
    memory_id UNINDEXED,
    content,
    normalized_content,
    entities,
    tags,
    search_terms,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE memory_note_set_snapshots (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    note_set_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    source_content_hash TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),
    expected_set_revision INTEGER,
    proposed_set_revision INTEGER NOT NULL CHECK (proposed_set_revision >= 1),
    extraction_paused INTEGER NOT NULL DEFAULT 0 CHECK (extraction_paused IN (0, 1)),
    items_json TEXT NOT NULL,
    canonical_bytes_hash TEXT NOT NULL,
    canonical_path TEXT NOT NULL,
    profile_hash TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('prepared', 'applied', 'rejected')),
    created_at INTEGER NOT NULL,
    applied_at INTEGER,
    UNIQUE (vault_id, id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_file_id)
        REFERENCES file_entries(vault_id, id),
    FOREIGN KEY (provider_id) REFERENCES providers(id),
    FOREIGN KEY (model_id) REFERENCES models(id)
);

CREATE UNIQUE INDEX memory_note_set_one_prepared_source_idx
    ON memory_note_set_snapshots(vault_id, source_file_id)
    WHERE status = 'prepared';

CREATE INDEX memory_note_set_snapshots_status_idx
    ON memory_note_set_snapshots(vault_id, status, created_at, id);

CREATE TABLE memory_v2_migration_state (
    vault_id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('not_started', 'preflighted', 'completed', 'blocked')),
    legacy_total INTEGER NOT NULL DEFAULT 0 CHECK (legacy_total >= 0),
    historical INTEGER NOT NULL DEFAULT 0 CHECK (historical >= 0),
    safe_explicit INTEGER NOT NULL DEFAULT 0 CHECK (safe_explicit >= 0),
    note_derived INTEGER NOT NULL DEFAULT 0 CHECK (note_derived >= 0),
    mixed_source INTEGER NOT NULL DEFAULT 0 CHECK (mixed_source >= 0),
    unsupported INTEGER NOT NULL DEFAULT 0 CHECK (unsupported >= 0),
    report_json TEXT NOT NULL DEFAULT '{}',
    preflighted_at INTEGER,
    completed_at INTEGER,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

-- A vector is reusable only for the exact preparation profile and exact input.
-- Existing rows remain legacy and are never accepted by v2.1 profile checks.
ALTER TABLE embedding_records
    ADD COLUMN profile_hash TEXT NOT NULL DEFAULT 'legacy';

ALTER TABLE embedding_records
    ADD COLUMN input_hash TEXT NOT NULL DEFAULT 'legacy';

CREATE INDEX embedding_records_v2_freshness_idx
    ON embedding_records(vault_id, object_type, object_id, model_id, profile_hash, input_hash);
