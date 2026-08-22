-- MCP Vault WP-06 durable background processing.
--
-- Existing lease columns remain compatible with rows created by WP-02/WP-04.
-- New state makes exhausted outbox delivery and cooperative job cancellation
-- explicit, and persists resumable scan progress per Vault.

ALTER TABLE outbox_events
    ADD COLUMN dead_lettered INTEGER NOT NULL DEFAULT 0
        CHECK (dead_lettered IN (0, 1));
ALTER TABLE outbox_events
    ADD COLUMN dead_letter_reason TEXT;

ALTER TABLE jobs
    ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0
        CHECK (cancel_requested IN (0, 1));

CREATE TABLE scan_checkpoints (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    scan_type TEXT NOT NULL,
    generation TEXT NOT NULL,
    cursor_path TEXT,
    status TEXT NOT NULL CHECK (
        status IN ('running', 'completed', 'failed', 'cancelled')
    ),
    entries_seen INTEGER NOT NULL DEFAULT 0 CHECK (entries_seen >= 0),
    files_seen INTEGER NOT NULL DEFAULT 0 CHECK (files_seen >= 0),
    directories_seen INTEGER NOT NULL DEFAULT 0 CHECK (directories_seen >= 0),
    changes_imported INTEGER NOT NULL DEFAULT 0 CHECK (changes_imported >= 0),
    unsafe_entries_skipped INTEGER NOT NULL DEFAULT 0
        CHECK (unsafe_entries_skipped >= 0),
    missing_deletes_skipped INTEGER NOT NULL DEFAULT 0
        CHECK (missing_deletes_skipped IN (0, 1)),
    last_error TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    UNIQUE (vault_id, scan_type),
    FOREIGN KEY (vault_id) REFERENCES vaults(id)
);

CREATE INDEX outbox_claim_idx
    ON outbox_events(dead_lettered, delivered_at, available_at, claimed_until);
CREATE INDEX jobs_claim_idx
    ON jobs(status, cancel_requested, available_at, lease_until);
CREATE INDEX scan_checkpoints_vault_idx
    ON scan_checkpoints(vault_id, scan_type, updated_at);
