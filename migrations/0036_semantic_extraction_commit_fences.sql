-- Fence concurrent extraction attempts for one source/revision.  The
-- sequence is advanced when a genuinely new extraction is started and is
-- captured by the extraction and every prepared publication snapshot.
ALTER TABLE semantic_sources
    ADD COLUMN extraction_commit_sequence INTEGER NOT NULL DEFAULT 0
    CHECK (extraction_commit_sequence >= 0);

ALTER TABLE semantic_extraction_sets
    ADD COLUMN extraction_commit_sequence INTEGER NOT NULL DEFAULT 0
    CHECK (extraction_commit_sequence >= 0);

ALTER TABLE semantic_prepared_snapshots
    ADD COLUMN extraction_commit_sequence INTEGER NOT NULL DEFAULT 0
    CHECK (extraction_commit_sequence >= 0);

CREATE INDEX semantic_extraction_commit_sequence_idx
    ON semantic_sources(vault_id, source_id, extraction_commit_sequence);
