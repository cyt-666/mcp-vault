-- Preserve existing vector fingerprints exactly during upgrade.
ALTER TABLE providers ADD COLUMN embedding_revision INTEGER NOT NULL DEFAULT 1 CHECK (embedding_revision >= 1);
UPDATE providers SET embedding_revision = revision;
ALTER TABLE models ADD COLUMN embedding_revision INTEGER NOT NULL DEFAULT 1 CHECK (embedding_revision >= 1);
UPDATE models SET embedding_revision = revision;

-- Repair checkpoints stranded by the former worker's obsolete-signature skip.
-- An active job always wins, and neither vectors nor spent budgets are reset.
UPDATE retrieval_calibration_runs AS r
SET status = 'cancelled',
    report_json = COALESCE(report_json, '{"error_code":"calibration_job_finished_without_report"}'),
    updated_at = CAST(strftime('%s','now') AS INTEGER) * 1000
WHERE r.status IN ('pending','running')
  AND EXISTS (
      SELECT 1 FROM jobs j, json_each(j.payload_json, '$.signatures') s
      WHERE j.vault_id = r.vault_id AND j.job_type = 'retrieval.calibrate'
        AND j.status IN ('completed','failed','cancelled') AND s.value = r.signature
  )
  AND NOT EXISTS (
      SELECT 1 FROM jobs j, json_each(j.payload_json, '$.signatures') s
      WHERE j.vault_id = r.vault_id AND j.job_type = 'retrieval.calibrate'
        AND j.status IN ('queued','running','retry_wait') AND s.value = r.signature
  );
