-- Continuous, Vault-scoped memory source health and lifecycle reasons.
--
-- Source health is derived and rebuildable. Memory status reasons are part of
-- the durable lifecycle projection and are also materialized in canonical
-- Markdown by the memory service.

ALTER TABLE memories ADD COLUMN status_reason TEXT;
ALTER TABLE memories ADD COLUMN status_changed_at INTEGER;

-- Before 0.1.17, the only automatic transition to stale was source loss.
-- Preserve that semantics explicitly so exact proof during the first audit can
-- safely reactivate those rows; manually archived/superseded rows are untouched.
UPDATE memories
SET status_reason = 'source_unavailable', status_changed_at = updated_at
WHERE status = 'stale';

CREATE TABLE memory_source_health (
    vault_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN (
            'unverified', 'current', 'content_changed', 'deleted',
            'identity_missing', 'identity_ambiguous'
        )
    ),
    resolved_file_id TEXT,
    resolved_path TEXT,
    checked_revision INTEGER CHECK (
        checked_revision IS NULL OR checked_revision >= 0
    ),
    verified_content_hash TEXT,
    reason TEXT,
    last_event_id TEXT,
    checked_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, source_id),
    FOREIGN KEY (vault_id, source_id)
        REFERENCES memory_sources(vault_id, id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, resolved_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memory_source_health_state_idx
    ON memory_source_health(vault_id, state, updated_at, source_id);

CREATE INDEX memory_source_health_file_idx
    ON memory_source_health(vault_id, resolved_file_id, state, source_id);

CREATE INDEX memory_source_health_path_idx
    ON memory_source_health(vault_id, resolved_path, state, source_id);

CREATE INDEX memory_sources_path_idx
    ON memory_sources(vault_id, note_path, id);

CREATE INDEX memory_sources_excerpt_idx
    ON memory_sources(vault_id, excerpt_hash, id);

CREATE INDEX memory_stage1_outputs_path_idx
    ON memory_stage1_outputs(vault_id, source_path, id);

-- One durable cursor/result row per Vault. A new reconciliation or explicit
-- Admin request replaces the generation and can therefore repeat an audit
-- without colliding with an old completed job's deduplication key.
CREATE TABLE memory_source_audit_state (
    vault_id TEXT PRIMARY KEY,
    generation TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'running', 'completed', 'failed', 'cancelled')
    ),
    cursor_source_id TEXT,
    counters_json TEXT NOT NULL DEFAULT '{}',
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

-- Existing note-dependent sources begin unverified. This deliberately makes
-- upgraded normal recall fail closed until the first paged source audit proves
-- their evidence against the current Vault contents.
INSERT INTO memory_source_health (
    vault_id, source_id, state, resolved_file_id, resolved_path,
    checked_revision, verified_content_hash, reason, last_event_id,
    checked_at, updated_at
)
SELECT
    vault_id, id, 'unverified', note_file_id, note_path,
    NULL, NULL, 'upgrade_audit_required', NULL,
    NULL, created_at
FROM memory_sources
WHERE source_type = 'note';
