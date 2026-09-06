-- Operational/derived calibration only. No canonical memory or vector changes.
CREATE TABLE retrieval_calibration_runs (
    vault_id TEXT NOT NULL,
    channel TEXT NOT NULL CHECK(channel IN ('memory','note')),
    signature TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','passed','quality_failed','failed','cancelled')),
    checkpoint_json TEXT NOT NULL DEFAULT '{}',
    report_json TEXT,
    requests INTEGER NOT NULL DEFAULT 0 CHECK(requests >= 0),
    request_bytes INTEGER NOT NULL DEFAULT 0 CHECK(request_bytes >= 0),
    request_limit INTEGER NOT NULL DEFAULT 32,
    byte_limit INTEGER NOT NULL DEFAULT 2097152,
    previous_report_json TEXT,
    budget_json TEXT NOT NULL DEFAULT '{"max_requests":32,"max_bytes":2097152,"max_inputs":512,"batch_size":16,"timeout_seconds":900}',
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(vault_id,channel,signature),
    FOREIGN KEY(vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX retrieval_calibration_one_active_job
    ON jobs(vault_id,job_type)
    WHERE job_type='retrieval.calibrate' AND status IN ('queued','running','retry_wait');
