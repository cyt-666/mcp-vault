-- MCP Vault WP-11 transparent, sourced, Vault-scoped memory state.
--
-- Canonical active/archived memory content is materialized as Markdown under
-- the reserved Vault namespace. These tables are the authoritative
-- operational projection and rebuildable candidate/search state.

CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    memory_type TEXT NOT NULL CHECK (
        memory_type IN (
            'identity', 'preference', 'decision', 'constraint', 'fact',
            'project', 'progress', 'event', 'relationship', 'procedure'
        )
    ),
    status TEXT NOT NULL CHECK (
        status IN ('candidate', 'active', 'superseded', 'stale', 'archived', 'rejected', 'quarantined')
    ),
    content TEXT NOT NULL,
    normalized_content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    origin TEXT NOT NULL CHECK (
        origin IN ('extracted', 'explicit_agent', 'explicit_admin', 'direct_markdown', 'import')
    ),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    canonical_file_id TEXT,
    canonical_path TEXT,
    canonical_revision INTEGER,
    valid_from INTEGER,
    valid_to INTEGER,
    extraction_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_recalled_at INTEGER,
    recall_count INTEGER NOT NULL DEFAULT 0 CHECK (recall_count >= 0),
    UNIQUE (vault_id, id),
    UNIQUE (vault_id, content_hash, status),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, canonical_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memories_recall_filter_idx
    ON memories(vault_id, status, memory_type, valid_to, importance, updated_at);

CREATE INDEX memories_canonical_path_idx
    ON memories(vault_id, canonical_path);

CREATE TABLE memory_sources (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    source_type TEXT NOT NULL CHECK (
        source_type IN ('note', 'explicit_agent', 'explicit_admin', 'direct_markdown', 'import')
    ),
    note_file_id TEXT,
    note_path TEXT,
    note_revision INTEGER,
    heading_path_json TEXT NOT NULL DEFAULT '[]',
    start_line INTEGER,
    end_line INTEGER,
    excerpt_hash TEXT,
    actor_id TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE (vault_id, id),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memories(vault_id, id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, note_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memory_sources_memory_idx
    ON memory_sources(vault_id, memory_id, created_at);

CREATE INDEX memory_sources_note_idx
    ON memory_sources(vault_id, note_file_id, note_revision);

CREATE TABLE memory_entities (
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    entity TEXT NOT NULL,
    normalized_entity TEXT NOT NULL,
    PRIMARY KEY (vault_id, memory_id, normalized_entity),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memories(vault_id, id) ON DELETE CASCADE
);

CREATE INDEX memory_entities_lookup_idx
    ON memory_entities(vault_id, normalized_entity, memory_id);

CREATE TABLE memory_tags (
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    normalized_tag TEXT NOT NULL,
    PRIMARY KEY (vault_id, memory_id, normalized_tag),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memories(vault_id, id) ON DELETE CASCADE
);

CREATE INDEX memory_tags_lookup_idx
    ON memory_tags(vault_id, normalized_tag, memory_id);

CREATE TABLE memory_relations (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_memory_id TEXT NOT NULL,
    target_memory_id TEXT NOT NULL,
    relation_type TEXT NOT NULL CHECK (
        relation_type IN (
            'supersedes', 'supports', 'contradicts', 'refines',
            'related_to', 'derived_from'
        )
    ),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    created_at INTEGER NOT NULL,
    UNIQUE (vault_id, source_memory_id, target_memory_id, relation_type),
    FOREIGN KEY (vault_id, source_memory_id)
        REFERENCES memories(vault_id, id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, target_memory_id)
        REFERENCES memories(vault_id, id) ON DELETE CASCADE
);

CREATE INDEX memory_relations_target_idx
    ON memory_relations(vault_id, target_memory_id, relation_type);

CREATE TABLE memory_candidates (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK (source_revision >= 0),
    candidate_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    extraction_fingerprint TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
    decision TEXT CHECK (decision IS NULL OR decision IN ('promoted', 'rejected', 'review')),
    decision_reason TEXT,
    created_at INTEGER NOT NULL,
    reviewed_at INTEGER,
    UNIQUE (vault_id, id),
    UNIQUE (vault_id, extraction_fingerprint),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memory_candidates_review_idx
    ON memory_candidates(vault_id, decision, confidence DESC, created_at DESC);

CREATE TABLE memory_idempotency (
    vault_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, idempotency_key),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memories(vault_id, id) ON DELETE CASCADE
);

CREATE TABLE memory_diagnostics (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    path TEXT NOT NULL,
    code TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (vault_id, path),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE INDEX memory_diagnostics_vault_idx
    ON memory_diagnostics(vault_id, updated_at DESC);

CREATE VIRTUAL TABLE memory_fts USING fts5(
    vault_id UNINDEXED,
    memory_id UNINDEXED,
    content,
    normalized_content,
    entities,
    tags
);
