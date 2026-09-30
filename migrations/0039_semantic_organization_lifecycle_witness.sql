-- M2 lifecycle/recovery fence. Keep the original state column for forward
-- compatibility, while lifecycle_state carries the complete state machine.
ALTER TABLE semantic_organization_jobs
    ADD COLUMN lifecycle_state TEXT NOT NULL DEFAULT 'running'
    CHECK (lifecycle_state IN ('running', 'prepared', 'written', 'applied', 'failed', 'blocked', 'cancelled'));

ALTER TABLE semantic_organization_snapshots
    ADD COLUMN expected_file_id TEXT;
ALTER TABLE semantic_organization_snapshots
    ADD COLUMN expected_file_revision INTEGER;
ALTER TABLE semantic_organization_snapshots
    ADD COLUMN actual_file_id TEXT;
ALTER TABLE semantic_organization_snapshots
    ADD COLUMN actual_file_revision INTEGER;
ALTER TABLE semantic_organization_snapshots
    ADD COLUMN actual_file_hash TEXT;
ALTER TABLE semantic_organization_snapshots
    ADD COLUMN source_fence_hash TEXT NOT NULL DEFAULT '';

CREATE INDEX semantic_organization_jobs_lifecycle_idx
    ON semantic_organization_jobs(vault_id, lifecycle_state, created_at);
CREATE INDEX semantic_organization_snapshots_pending_idx
    ON semantic_organization_snapshots(vault_id, organization_job_id, status);
