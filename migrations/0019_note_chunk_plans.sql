-- Derived boundaries only; no canonical text or business vector rewrite.
CREATE TABLE note_chunk_plans (
    vault_id TEXT NOT NULL REFERENCES vaults(id),
    file_id TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    plan_json TEXT NOT NULL CHECK(json_valid(plan_json)),
    PRIMARY KEY (vault_id, file_id)
);

-- Retire old automatically admitted diagnostics; preserve their spent budgets/cache.
UPDATE retrieval_calibration_runs AS r SET status='cancelled',
    report_json=COALESCE(report_json,'{"error_code":"automatic_evaluation_retired"}')
WHERE r.status IN ('pending','running') AND EXISTS (
    SELECT 1 FROM jobs j, json_each(j.payload_json,'$.signatures') s
    WHERE j.vault_id=r.vault_id AND j.job_type='retrieval.calibrate'
      AND COALESCE(json_extract(j.payload_json,'$.explicit_diagnostic'),0)!=1
      AND s.value=r.signature
) AND NOT EXISTS (
    SELECT 1 FROM jobs j, json_each(j.payload_json,'$.signatures') s
    WHERE j.vault_id=r.vault_id AND j.job_type='retrieval.calibrate'
      AND json_extract(j.payload_json,'$.explicit_diagnostic')=1
      AND j.status IN ('queued','running','retry_wait') AND s.value=r.signature
);
UPDATE jobs SET status='cancelled', lease_owner=NULL, lease_until=NULL,
    completed_at=CAST(strftime('%s','now') AS INTEGER)*1000
WHERE job_type='retrieval.calibrate' AND status IN ('queued','running','retry_wait')
    AND COALESCE(json_extract(payload_json,'$.explicit_diagnostic'),0)!=1;
