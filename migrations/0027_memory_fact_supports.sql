-- A consolidated body has per-fact evidence. Group membership alone must not
-- keep a detail readable after its last supporting contribution becomes stale.
CREATE INDEX memory_organization_pending_order ON memory_organization_items(
    vault_id, done, (CASE WHEN outcome='continuing' THEN 0 ELSE 1 END),
    attempt_sequence, priority DESC, contribution_id
);

CREATE UNIQUE INDEX memory_formal_supports_owner
ON memory_formal_supports(vault_id, formal_id, contribution_id);

CREATE TABLE memory_formal_facts (
    vault_id TEXT NOT NULL,
    formal_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 1 AND ordinal <= 32),
    content TEXT NOT NULL CHECK (length(content) > 0),
    PRIMARY KEY(vault_id, formal_id, ordinal),
    FOREIGN KEY(vault_id, formal_id)
        REFERENCES memory_formal_items(vault_id, id) ON DELETE CASCADE
);

CREATE TABLE memory_formal_fact_supports (
    vault_id TEXT NOT NULL,
    formal_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    contribution_id TEXT NOT NULL,
    PRIMARY KEY(vault_id, formal_id, ordinal, contribution_id),
    FOREIGN KEY(vault_id, formal_id, ordinal)
        REFERENCES memory_formal_facts(vault_id, formal_id, ordinal) ON DELETE CASCADE,
    FOREIGN KEY(vault_id, formal_id, contribution_id)
        REFERENCES memory_formal_supports(vault_id, formal_id, contribution_id) ON DELETE CASCADE
);

-- Revisit legacy judgments using original current contributions. The existing
-- paged initializer recovers prepared publications before unfolding old groups
-- through Vault Core. Never discard canonical data or recovery operations here.
UPDATE memory_organization_state
SET initialized=0, reset_complete=0, reset_cursor=NULL, adoption_cursor=NULL,
    reset_checked=0, adoption_checked=0, retry_at=0, phase='resetting',
    status=CASE WHEN paused=1 THEN 'paused' ELSE 'processing' END
WHERE EXISTS (
    SELECT 1 FROM memory_formal_items i
    WHERE i.vault_id=memory_organization_state.vault_id
);

INSERT INTO memory_organization_sources(vault_id, source_file_id, generation, priority)
SELECT vault_id, source_file_id, 1, 1 FROM memory_note_sets WHERE 1
ON CONFLICT(vault_id, source_file_id)
DO UPDATE SET generation=generation+1, priority=1;
