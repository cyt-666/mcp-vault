-- Permit a recovery adjudication to terminate an old file_committed create
-- only after State has proved that a later create owns the same canonical
-- result. Historical migrations remain immutable.
CREATE TABLE operation_journal_v2 (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('prepared', 'file_committed', 'metadata_committed', 'rolled_back', 'needs_review', 'superseded')
    ),
    source_path TEXT,
    destination_path TEXT,
    prior_file_id TEXT,
    expected_revision INTEGER CHECK (expected_revision IS NULL OR expected_revision >= 0),
    prior_hash TEXT,
    proposed_hash TEXT,
    temp_path TEXT,
    payload_json TEXT NOT NULL,
    idempotency_key TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    error TEXT,
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

INSERT INTO operation_journal_v2 (
    id, vault_id, operation, state, source_path, destination_path,
    prior_file_id, expected_revision, prior_hash, proposed_hash, temp_path,
    payload_json, idempotency_key, created_at, updated_at, error
)
SELECT id, vault_id, operation, state, source_path, destination_path,
       prior_file_id, expected_revision, prior_hash, proposed_hash, temp_path,
       payload_json, idempotency_key, created_at, updated_at, error
FROM operation_journal;

DROP INDEX operation_journal_recovery_idx;
DROP INDEX operation_journal_vault_idempotency_idx;
DROP TABLE operation_journal;
ALTER TABLE operation_journal_v2 RENAME TO operation_journal;

CREATE INDEX operation_journal_recovery_idx
    ON operation_journal(vault_id, state, updated_at);
CREATE UNIQUE INDEX operation_journal_vault_idempotency_idx
    ON operation_journal(vault_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
