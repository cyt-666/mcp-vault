-- MCP Vault WP-13 backup catalog.
--
-- Backup artifacts are service-owned recovery material. Their contents live
-- outside SQLite; this table stores only bounded lifecycle metadata and the
-- redacted manifest needed for Admin diagnostics.

CREATE TABLE backups (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'running', 'completed', 'failed', 'validating', 'restoring')
    ),
    location TEXT NOT NULL,
    manifest_json TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    verified_at INTEGER,
    error TEXT,
    created_by TEXT
);

CREATE INDEX backups_status_time_idx
    ON backups(status, started_at DESC, id DESC);
