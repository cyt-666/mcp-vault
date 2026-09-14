-- Independent v3 memory storage. No legacy records or identities are imported.

CREATE TABLE memory_unit_sets (
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

CREATE INDEX v3_memory_unit_sets_freshness_idx
    ON memory_unit_sets(vault_id, source_file_id, source_content_hash, extraction_paused);

CREATE TABLE memory_units (
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
        REFERENCES memory_unit_sets(vault_id, id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, canonical_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX v3_memory_units_list_idx
    ON memory_units(vault_id, ownership, kind, updated_at DESC, id);

CREATE INDEX v3_memory_units_set_idx
    ON memory_units(vault_id, note_set_id, ordinal);

CREATE TABLE memory_unit_sources (
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
        REFERENCES memory_units(vault_id, id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, note_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX v3_memory_unit_sources_memory_idx
    ON memory_unit_sources(vault_id, memory_id, created_at, id);

CREATE INDEX v3_memory_unit_sources_note_idx
    ON memory_unit_sources(vault_id, note_file_id, source_content_hash);

CREATE TABLE memory_unit_idempotency (
    vault_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, idempotency_key),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memory_units(vault_id, id) ON DELETE CASCADE
);

CREATE TABLE memory_unit_reservations (
    vault_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, idempotency_key),
    UNIQUE (vault_id, memory_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE memory_units_fts USING fts5(
    vault_id UNINDEXED,
    memory_id UNINDEXED,
    content,
    normalized_content,
    entities,
    tags,
    search_terms,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE "memory_unit_snapshots" (
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
    provider_id TEXT,
    model_id TEXT,
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

CREATE UNIQUE INDEX v3_memory_note_set_one_prepared_source_idx
    ON memory_unit_snapshots(vault_id, source_file_id)
    WHERE status = 'prepared';

CREATE INDEX v3_memory_unit_snapshots_status_idx
    ON memory_unit_snapshots(vault_id, status, created_at, id);

-- Initialization is a one-time operator action, not a boot-time purge.
CREATE TABLE memory_unit_initialization (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    phase TEXT NOT NULL CHECK (phase IN ('required','clearing','ready')),
    manifest_json TEXT NOT NULL DEFAULT '{}',
    updated_at INTEGER NOT NULL
);

INSERT INTO memory_unit_initialization(vault_id,phase,updated_at)
SELECT v.id,'required',0 FROM vaults v;

CREATE TABLE memory_unit_runtime (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    paused INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0,1)),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0)
);

CREATE TABLE memory_unit_selection_batches (
    vault_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(vault_id,source_file_id,source_hash,input_hash),
    FOREIGN KEY(vault_id,source_file_id) REFERENCES file_entries(vault_id,id)
);

CREATE TABLE memory_unit_overviews (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    scope_key TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    generation INTEGER NOT NULL,
    content_json TEXT NOT NULL,
    dependency_count INTEGER NOT NULL CHECK (dependency_count > 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(vault_id,scope_key)
);

CREATE TABLE memory_unit_overview_dependencies (
    vault_id TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY(vault_id,scope_key,memory_id),
    FOREIGN KEY(vault_id,scope_key) REFERENCES memory_unit_overviews(vault_id,scope_key) ON DELETE CASCADE,
    FOREIGN KEY(vault_id,memory_id) REFERENCES memory_units(vault_id,id) ON DELETE CASCADE
);

CREATE TRIGGER memory_units_generation_insert AFTER INSERT ON memory_units BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(NEW.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER memory_units_generation_delete AFTER DELETE ON memory_units BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(OLD.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER memory_units_generation_update AFTER UPDATE OF
    revision,content,content_hash,kind,metadata_json,tags_json,entities_json,valid_from,valid_to
ON memory_units BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(NEW.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;

-- Source movement changes navigation even when its body hash is unchanged.
CREATE TRIGGER memory_units_generation_source_update AFTER UPDATE OF
    path,current_revision,content_hash,deleted_at ON file_entries
WHEN EXISTS(SELECT 1 FROM memory_unit_sources s WHERE s.vault_id=NEW.vault_id AND s.note_file_id=NEW.id)
  OR EXISTS(SELECT 1 FROM memory_units i WHERE i.vault_id=NEW.vault_id AND i.canonical_file_id=NEW.id)
BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(NEW.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;

ALTER TABLE memory_unit_runtime ADD COLUMN overview_generation INTEGER NOT NULL DEFAULT -1;
CREATE UNIQUE INDEX memory_unit_overview_active_job ON jobs(vault_id,job_type)
WHERE vault_id IS NOT NULL AND job_type='memory.overview' AND status IN ('queued','running','retry_wait');

CREATE TABLE memory_unit_selection_progress (
    vault_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    profile_hash TEXT NOT NULL,
    completed_batches INTEGER NOT NULL,
    total_batches INTEGER NOT NULL,
    skipped_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(vault_id,source_file_id),
    FOREIGN KEY(vault_id,source_file_id) REFERENCES file_entries(vault_id,id) ON DELETE CASCADE
);
