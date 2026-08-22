-- MCP Vault WP-09 rebuildable Markdown and knowledge-map projections.
--
-- These tables are derived from canonical Vault files and may be deleted and
-- rebuilt. They remain Vault-scoped even though note_fts is an FTS5 virtual
-- table and therefore cannot enforce ordinary foreign keys.

CREATE TABLE notes (
    file_id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    path TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    title TEXT,
    aliases_json TEXT NOT NULL,
    frontmatter_json TEXT NOT NULL,
    plain_text TEXT NOT NULL,
    first_paragraph TEXT,
    language TEXT,
    word_count INTEGER NOT NULL CHECK (word_count >= 0),
    analyzed_content_hash TEXT NOT NULL,
    analyzer_version INTEGER NOT NULL CHECK (analyzer_version > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (vault_id, path),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX notes_vault_updated_idx
    ON notes(vault_id, updated_at DESC, file_id ASC);

CREATE TABLE note_headings (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    level INTEGER NOT NULL CHECK (level BETWEEN 1 AND 6),
    heading_path_json TEXT NOT NULL,
    title TEXT NOT NULL,
    start_byte INTEGER NOT NULL CHECK (start_byte >= 0),
    end_byte INTEGER CHECK (end_byte IS NULL OR end_byte >= start_byte),
    UNIQUE (file_id, ordinal),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX note_headings_vault_file_idx
    ON note_headings(vault_id, file_id, ordinal);

CREATE TABLE note_tags (
    vault_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    normalized_tag TEXT NOT NULL,
    source TEXT NOT NULL,
    PRIMARY KEY (file_id, normalized_tag, source),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX note_tags_vault_tag_idx
    ON note_tags(vault_id, normalized_tag, file_id);

CREATE TABLE note_links (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    target_text TEXT NOT NULL,
    target_file_id TEXT,
    target_heading TEXT,
    link_type TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    UNIQUE (source_file_id, ordinal),
    FOREIGN KEY (vault_id, source_file_id)
        REFERENCES file_entries(vault_id, id),
    FOREIGN KEY (vault_id, target_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX note_links_vault_target_idx
    ON note_links(vault_id, target_file_id, source_file_id);

CREATE VIRTUAL TABLE note_fts USING fts5(
    vault_id UNINDEXED,
    file_id UNINDEXED,
    path,
    title,
    aliases,
    tags,
    headings,
    plain_text,
    tokenize = 'unicode61 remove_diacritics 2'
);

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
    UNIQUE (vault_id, stable_key),
    UNIQUE (vault_id, id),
    FOREIGN KEY (vault_id, parent_id)
        REFERENCES index_nodes(vault_id, id) ON DELETE CASCADE
);

CREATE INDEX index_nodes_vault_parent_idx
    ON index_nodes(vault_id, parent_id, sort_key, id);

CREATE TABLE index_memberships (
    vault_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    relevance REAL NOT NULL,
    source_type TEXT NOT NULL,
    PRIMARY KEY (vault_id, node_id, file_id, source_type),
    FOREIGN KEY (vault_id, node_id)
        REFERENCES index_nodes(vault_id, id),
    FOREIGN KEY (vault_id, file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX index_memberships_vault_file_idx
    ON index_memberships(vault_id, file_id, node_id);

CREATE TABLE index_status (
    vault_id TEXT PRIMARY KEY,
    index_revision INTEGER NOT NULL CHECK (index_revision >= 0),
    indexed_entries INTEGER NOT NULL CHECK (indexed_entries >= 0),
    indexed_notes INTEGER NOT NULL CHECK (indexed_notes >= 0),
    indexed_bytes INTEGER NOT NULL CHECK (indexed_bytes >= 0),
    analyzer_version INTEGER NOT NULL CHECK (analyzer_version > 0),
    coverage_json TEXT NOT NULL,
    last_rebuilt_at INTEGER,
    last_error TEXT,
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);
