-- Rebuildable judgments contain no canonical prose. Existing current sets,
-- explicit memories, vectors, pending snapshots and source pauses are retained.
CREATE TABLE memory_equivalence_decisions (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    input_hash TEXT NOT NULL,
    relation TEXT NOT NULL CHECK (relation IN (
        'equivalent', 'left_covers_right', 'right_covers_left',
        'related', 'different', 'uncertain'
    )),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, input_hash)
);

-- Reserve at actual transport dispatch, including transport retries. A crash
-- can consume capacity but cannot refund a request that may have been billed.
CREATE TABLE memory_equivalence_dispatches (
    id INTEGER PRIMARY KEY,
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    dispatched_at INTEGER NOT NULL,
    input_bytes INTEGER NOT NULL CHECK (input_bytes >= 0)
);
CREATE INDEX memory_equivalence_dispatches_window
    ON memory_equivalence_dispatches(vault_id, dispatched_at);
