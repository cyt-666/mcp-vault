-- M6 A80: persist validated, source/revision-bound observation fragments so
-- one prepared ExtractionSet can resume without exposing partial cards.
ALTER TABLE semantic_observations
    ADD COLUMN advisory_kind_unknown INTEGER NOT NULL DEFAULT 0
    CHECK (advisory_kind_unknown IN (0, 1));

CREATE TABLE semantic_extraction_batches (
    vault_id TEXT NOT NULL,
    extraction_set_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    batch_index INTEGER NOT NULL CHECK (batch_index >= 0),
    batch_count INTEGER NOT NULL CHECK (batch_count >= 1),
    batch_input_hash TEXT NOT NULL,
    batch_catalog_hash TEXT NOT NULL,
    prompt_id TEXT NOT NULL,
    schema_id TEXT NOT NULL,
    provider_fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('ready', 'dispatching', 'uncertain', 'validated', 'failed')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 2),
    normalized_proposal_json TEXT,
    normalized_proposal_hash TEXT,
    advisory_normalization_counts_json TEXT NOT NULL DEFAULT '{}',
    safe_error_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, extraction_set_id, batch_index),
    FOREIGN KEY (vault_id, extraction_set_id, source_id)
        REFERENCES semantic_extraction_sets(vault_id, extraction_set_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE,
    CHECK ((state = 'validated' AND normalized_proposal_json IS NOT NULL AND normalized_proposal_hash IS NOT NULL)
        OR (state <> 'validated' AND normalized_proposal_json IS NULL AND normalized_proposal_hash IS NULL))
);

CREATE INDEX semantic_extraction_batches_resume_idx
    ON semantic_extraction_batches(vault_id, extraction_set_id, state, batch_index);

-- One deterministic response repair may be consumed per source ExtractionSet,
-- regardless of how many 80-block batches that source has.
CREATE TABLE semantic_extraction_regen_tokens (
    vault_id TEXT NOT NULL,
    extraction_set_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    token_kind TEXT NOT NULL CHECK (token_kind = 'observation_regen'),
    batch_index INTEGER NOT NULL CHECK (batch_index >= 0),
    consumed_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, extraction_set_id, token_kind),
    FOREIGN KEY (vault_id, extraction_set_id, source_id)
        REFERENCES semantic_extraction_sets(vault_id, extraction_set_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, extraction_set_id, batch_index)
        REFERENCES semantic_extraction_batches(vault_id, extraction_set_id, batch_index) ON DELETE CASCADE
);
