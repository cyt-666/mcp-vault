-- Preserve every v2.1 snapshot, including prepared recovery work. Local deletes
-- do not require the extraction Provider to still be installed.
CREATE TABLE memory_note_set_snapshots_v16 (
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

INSERT INTO memory_note_set_snapshots_v16 SELECT * FROM memory_note_set_snapshots;
DROP TABLE memory_note_set_snapshots;
ALTER TABLE memory_note_set_snapshots_v16 RENAME TO memory_note_set_snapshots;

CREATE UNIQUE INDEX memory_note_set_one_prepared_source_idx
    ON memory_note_set_snapshots(vault_id, source_file_id)
    WHERE status = 'prepared';

CREATE INDEX memory_note_set_snapshots_status_idx
    ON memory_note_set_snapshots(vault_id, status, created_at, id);
