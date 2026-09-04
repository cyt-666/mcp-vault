-- Derived multilingual retrieval metadata and crash-safe enrichment proposals.
--
-- Canonical memory Markdown remains authoritative. These rows are Vault-scoped
-- search projections and may be regenerated from canonical memory plus the
-- configured consolidation Provider.

CREATE TABLE memory_retrieval_metadata (
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    profile_hash TEXT NOT NULL,
    source_language TEXT,
    aliases_json TEXT NOT NULL DEFAULT '[]',
    aliases_text TEXT NOT NULL DEFAULT '',
    search_terms TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN ('pending', 'ready', 'failed')),
    last_error TEXT,
    generated_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, memory_id),
    FOREIGN KEY (vault_id, memory_id)
        REFERENCES memories(vault_id, id) ON DELETE CASCADE
);

CREATE INDEX memory_retrieval_metadata_status_idx
    ON memory_retrieval_metadata(vault_id, status, updated_at, memory_id);

CREATE INDEX memory_retrieval_metadata_profile_idx
    ON memory_retrieval_metadata(vault_id, profile_hash, content_hash, memory_id);

CREATE TABLE memory_retrieval_proposals (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    proposal_json TEXT NOT NULL,
    model_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('prepared', 'applied', 'rejected')),
    applied_count INTEGER NOT NULL DEFAULT 0 CHECK (applied_count >= 0),
    created_at INTEGER NOT NULL,
    applied_at INTEGER,
    UNIQUE (vault_id, id),
    UNIQUE (vault_id, input_hash),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE INDEX memory_retrieval_proposals_status_idx
    ON memory_retrieval_proposals(vault_id, status, created_at DESC, id);

CREATE UNIQUE INDEX jobs_one_active_memory_retrieval_per_vault_idx
    ON jobs(vault_id)
    WHERE job_type = 'memory.enrich_retrieval'
      AND status IN ('queued', 'running', 'retry_wait');

-- FTS is derived and can be rebuilt without changing canonical memory.
DROP TABLE memory_fts;

CREATE VIRTUAL TABLE memory_fts USING fts5(
    vault_id UNINDEXED,
    memory_id UNINDEXED,
    content,
    normalized_content,
    entities,
    tags,
    aliases,
    search_terms,
    tokenize = 'unicode61 remove_diacritics 2'
);

INSERT INTO memory_fts (
    vault_id, memory_id, content, normalized_content,
    entities, tags, aliases, search_terms
)
SELECT
    m.vault_id,
    m.id,
    m.content,
    m.normalized_content,
    COALESCE((
        SELECT group_concat(normalized_entity, ' ')
        FROM memory_entities e
        WHERE e.vault_id = m.vault_id AND e.memory_id = m.id
    ), ''),
    COALESCE((
        SELECT group_concat(normalized_tag, ' ')
        FROM memory_tags t
        WHERE t.vault_id = m.vault_id AND t.memory_id = m.id
    ), ''),
    '',
    m.normalized_content
FROM memories m;
