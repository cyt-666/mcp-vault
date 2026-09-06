-- Source contributions remain canonical source sets. Formal projection has an
-- independent identity and lifetime, and is rebuilt from managed facts Markdown.
ALTER TABLE memory_current_items ADD COLUMN semantic_hash TEXT NOT NULL DEFAULT '';
CREATE TABLE memory_formal_items (
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
    UNIQUE (vault_id, canonical_path),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, canonical_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memory_formal_items_list ON memory_formal_items(vault_id,updated_at,id);
CREATE TABLE memory_formal_mode (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1))
);
CREATE TABLE memory_formal_supports (
    vault_id TEXT NOT NULL,
    formal_id TEXT NOT NULL,
    contribution_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    semantic_hash TEXT NOT NULL,
    PRIMARY KEY(vault_id,contribution_id),
    FOREIGN KEY(vault_id,formal_id) REFERENCES memory_formal_items(vault_id,id) ON DELETE CASCADE
);
CREATE INDEX memory_formal_support_owner ON memory_formal_supports(vault_id,formal_id);
CREATE TABLE memory_formal_operations (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    committed INTEGER NOT NULL DEFAULT 0,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE memory_formal_maintenance (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    fingerprint TEXT NOT NULL DEFAULT '',
    cursor TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',
    checked INTEGER NOT NULL DEFAULT 0,
    merged INTEGER NOT NULL DEFAULT 0,
    retry_at INTEGER NOT NULL DEFAULT 0
);
CREATE VIEW memory_public_items AS
SELECT id,vault_id,ownership,note_set_id,ordinal,kind,content,normalized_content,content_hash,importance,confidence,origin,revision,canonical_file_id,canonical_path,canonical_revision,valid_from,valid_to,tags_json,entities_json,metadata_json,created_at,updated_at,last_recalled_at,recall_count FROM memory_current_items i
WHERE i.ownership='explicit' OR NOT EXISTS (SELECT 1 FROM memory_formal_mode m WHERE m.vault_id=i.vault_id AND m.enabled=1)
UNION ALL
SELECT id,vault_id,ownership,note_set_id,ordinal,kind,content,normalized_content,content_hash,importance,confidence,origin,revision,canonical_file_id,canonical_path,canonical_revision,valid_from,valid_to,tags_json,entities_json,metadata_json,created_at,updated_at,last_recalled_at,recall_count FROM memory_formal_items i
WHERE EXISTS (SELECT 1 FROM memory_formal_mode m WHERE m.vault_id=i.vault_id AND m.enabled=1);
CREATE VIEW memory_valid_formal_supports AS
SELECT p.* FROM memory_formal_supports p
JOIN memory_current_items c ON c.vault_id=p.vault_id AND c.id=p.contribution_id AND c.semantic_hash=p.semantic_hash
JOIN memory_note_sets s ON s.vault_id=c.vault_id AND s.id=c.note_set_id AND s.source_file_id=p.source_file_id AND s.source_content_hash=p.source_hash
JOIN file_entries f ON f.vault_id=s.vault_id AND f.id=s.source_file_id AND f.content_hash=s.source_content_hash AND f.deleted_at IS NULL AND lower(f.path) LIKE '%.md'
JOIN file_entries canonical ON canonical.vault_id=s.vault_id AND canonical.id=s.canonical_file_id AND canonical.deleted_at IS NULL AND (canonical.current_revision=s.canonical_revision OR EXISTS(SELECT 1 FROM memory_formal_operations op,json_each(op.payload_json,'$.source_rewrites') r WHERE op.vault_id=s.vault_id AND json_extract(r.value,'$.set.source_file_id')=s.source_file_id));

CREATE TABLE memory_formal_examined (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    contribution_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    PRIMARY KEY(vault_id,contribution_id)
);
CREATE TABLE memory_formal_pairs (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    pair_key TEXT NOT NULL,
    left_id TEXT NOT NULL,
    right_id TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(vault_id,pair_key)
);
CREATE INDEX memory_formal_pending_pairs ON memory_formal_pairs(vault_id,done,pair_key);
CREATE UNIQUE INDEX memory_dedup_one_active_job ON jobs(vault_id,job_type)
WHERE job_type='memory.deduplicate' AND status IN ('queued','running','retry_wait');
CREATE TABLE memory_equivalence_rewrites (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    input_hash TEXT NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY(vault_id,input_hash)
);
-- Identity-only operational reservations prevent absorbed IDs from being reused.
-- They contain no memory content or model-readable lifecycle history.
CREATE TABLE memory_formal_identity_reservations (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    memory_id TEXT NOT NULL,
    PRIMARY KEY(vault_id,memory_id)
);
