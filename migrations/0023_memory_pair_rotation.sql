-- Advance before external I/O so interrupted comparisons cannot block the queue.
ALTER TABLE memory_formal_pairs ADD COLUMN attempt_sequence INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memory_formal_pairs ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
DROP INDEX memory_formal_pending_pairs;
CREATE INDEX memory_formal_pending_pairs ON memory_formal_pairs(vault_id,done,attempt_sequence,priority DESC,pair_key);

-- The retired local daily cap must not keep an upgraded installation asleep.
-- Preserve other provider failures/backoff, all candidates and cached decisions.
UPDATE memory_formal_maintenance SET status='pending', retry_at=0
WHERE status='memory_equivalence_budget_exhausted';

-- Published contributions enter incremental candidate discovery transactionally.
CREATE TABLE memory_dedup_new_contributions (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    contribution_id TEXT NOT NULL,
    PRIMARY KEY(vault_id,contribution_id)
);
