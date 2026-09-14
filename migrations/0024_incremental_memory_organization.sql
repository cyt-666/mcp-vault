-- Retire the old algorithm, including completed/failed jobs. Canonical facts,
-- source contributions and in-flight file publications MUST survive SQL migration:
-- Vault Core performs the recoverable one-time regrouping after startup.
DELETE FROM jobs WHERE job_type IN ('memory.deduplicate', 'memory.deduplicate_source');
DELETE FROM memory_formal_pairs;
DELETE FROM memory_formal_examined;
DELETE FROM memory_formal_maintenance;
DELETE FROM memory_dedup_progress;
DELETE FROM memory_dedup_new_contributions;
DELETE FROM memory_equivalence_decisions;
DELETE FROM memory_equivalence_rewrites;
DELETE FROM memory_equivalence_dispatches;

CREATE TABLE memory_organization_state (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    initialized INTEGER NOT NULL DEFAULT 0 CHECK(initialized IN (0,1)),
    paused INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0,1)),
    phase TEXT NOT NULL DEFAULT 'resetting',
    status TEXT NOT NULL DEFAULT 'pending',
    retry_at INTEGER NOT NULL DEFAULT 0,
    cache_hits INTEGER NOT NULL DEFAULT 0,
    merged INTEGER NOT NULL DEFAULT 0,
    skipped INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE memory_organization_sources (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    source_file_id TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(vault_id,source_file_id)
);
CREATE TABLE memory_organization_items (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    contribution_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0 CHECK(done IN (0,1)),
    outcome TEXT NOT NULL DEFAULT 'pending',
    priority INTEGER NOT NULL DEFAULT 0,
    attempt_sequence INTEGER NOT NULL DEFAULT 0,
    generation INTEGER NOT NULL DEFAULT 1,
    vector_stamp TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(vault_id,contribution_id)
);
CREATE INDEX memory_organization_pending ON memory_organization_items(vault_id,done,attempt_sequence,priority DESC,contribution_id);
CREATE INDEX memory_organization_source ON memory_organization_items(vault_id,source_file_id);
CREATE TABLE memory_organization_decisions (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    input_hash TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(vault_id,input_hash)
);
CREATE TABLE memory_organization_dispatches (
    id INTEGER PRIMARY KEY,
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    input_bytes INTEGER NOT NULL,
    dispatched_at INTEGER NOT NULL
);
CREATE INDEX memory_organization_dispatch_vault ON memory_organization_dispatches(vault_id);
CREATE TABLE memory_organization_vectors (
    vault_id TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    formal_id TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY(vault_id,formal_id)
);
CREATE UNIQUE INDEX memory_organization_active_job ON jobs(vault_id,job_type)
WHERE job_type='memory.organize' AND status IN ('queued','running','retry_wait');

INSERT INTO memory_organization_state(vault_id) SELECT id FROM vaults;
INSERT INTO memory_organization_sources(vault_id,source_file_id)
SELECT vault_id,source_file_id FROM memory_note_sets;

-- The dirty source and canonical projection publish in the same transaction.
CREATE TRIGGER memory_organization_source_insert AFTER INSERT ON memory_note_sets BEGIN
    INSERT INTO memory_organization_sources(vault_id,source_file_id,priority)
    VALUES(NEW.vault_id,NEW.source_file_id,1)
    ON CONFLICT(vault_id,source_file_id) DO UPDATE SET generation=generation+1,priority=1;
END;
CREATE TRIGGER memory_organization_source_update AFTER UPDATE ON memory_note_sets BEGIN
    INSERT INTO memory_organization_sources(vault_id,source_file_id,priority)
    VALUES(NEW.vault_id,NEW.source_file_id,1)
    ON CONFLICT(vault_id,source_file_id) DO UPDATE SET generation=generation+1,priority=1;
END;
CREATE TRIGGER memory_organization_source_delete AFTER DELETE ON memory_note_sets BEGIN
    INSERT INTO memory_organization_sources(vault_id,source_file_id,priority)
    VALUES(OLD.vault_id,OLD.source_file_id,1)
    ON CONFLICT(vault_id,source_file_id) DO UPDATE SET generation=generation+1,priority=1;
END;
CREATE TRIGGER memory_organization_file_update AFTER UPDATE OF content_hash,path,deleted_at ON file_entries
WHEN OLD.content_hash IS NOT NEW.content_hash OR OLD.path IS NOT NEW.path OR OLD.deleted_at IS NOT NEW.deleted_at BEGIN
    INSERT INTO memory_organization_sources(vault_id,source_file_id,priority)
    SELECT vault_id,source_file_id,1 FROM memory_note_sets WHERE vault_id=NEW.vault_id AND source_file_id=NEW.id
    ON CONFLICT(vault_id,source_file_id) DO UPDATE SET generation=generation+1,priority=1;
END;
CREATE TRIGGER memory_organization_vector_insert AFTER INSERT ON embedding_records WHEN NEW.object_type='memory' BEGIN
    INSERT INTO memory_organization_vectors(vault_id,formal_id) VALUES(NEW.vault_id,NEW.object_id)
    ON CONFLICT(vault_id,formal_id) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER memory_organization_vector_update AFTER UPDATE ON embedding_records WHEN NEW.object_type='memory' BEGIN
    INSERT INTO memory_organization_vectors(vault_id,formal_id) VALUES(NEW.vault_id,NEW.object_id)
    ON CONFLICT(vault_id,formal_id) DO UPDATE SET generation=generation+1;
END;
