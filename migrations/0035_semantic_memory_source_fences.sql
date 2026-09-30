-- Persist the source-generation fence independently from source content and
-- authorization revisions.  A new generation is required before an
-- invalidated in-flight extraction can become publishable again.
ALTER TABLE semantic_sources
    ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0
    CHECK (source_generation >= 0);

ALTER TABLE semantic_source_revisions
    ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0
    CHECK (source_generation >= 0);

ALTER TABLE semantic_extraction_sets
    ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0
    CHECK (source_generation >= 0);

ALTER TABLE semantic_prepared_snapshots
    ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0
    CHECK (source_generation >= 0);

-- Existing semantic rows were created without this fence.  Make every
-- already-ineligible source stale relative to its old source revision so an
-- old job cannot be revived merely by matching its old reason string.
UPDATE semantic_sources
SET source_generation = 1
WHERE eligible = 0 OR pending_rebuild = 1 OR invalid_reason IS NOT NULL;

CREATE INDEX semantic_source_generation_idx
    ON semantic_sources(vault_id, source_id, source_generation);
