-- Operational checkpoints only; no memory content or lifecycle history.
CREATE TABLE memory_dedup_progress (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    phase TEXT NOT NULL DEFAULT 'pending',
    sentence_cursor TEXT,
    sentence_checked INTEGER NOT NULL DEFAULT 0
);
