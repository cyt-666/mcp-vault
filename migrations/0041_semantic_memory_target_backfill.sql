-- M3-B: deterministic legacy target aliases. Rows that predate target-aware
-- writes receive non-empty aliases and remain auditable rather than bypassing
-- lifecycle rules through the empty default.

ALTER TABLE semantic_task_states RENAME TO semantic_task_states_0040;
CREATE TABLE semantic_task_states (
    vault_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    target_key TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('not_started', 'in_progress', 'blocked', 'completed', 'unknown')),
    evidence_observation_id TEXT,
    evidence_source_id TEXT,
    evidence_source_revision_id TEXT,
    rules_revision INTEGER NOT NULL CHECK (rules_revision >= 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, task_id),
    FOREIGN KEY (vault_id, target_key)
        REFERENCES semantic_targets(vault_id, target_key) ON DELETE CASCADE
);
INSERT INTO semantic_task_states
    (vault_id,task_id,target_key,state,evidence_observation_id,evidence_source_id,
     evidence_source_revision_id,rules_revision,updated_at)
SELECT vault_id,task_id,target_key,
       CASE state WHEN 'pending' THEN 'not_started'
                  WHEN 'running' THEN 'in_progress'
                  WHEN 'failed' THEN 'blocked'
                  WHEN 'cancelled' THEN 'unknown'
                  ELSE state END,
       evidence_observation_id,evidence_source_id,evidence_source_revision_id,
       rules_revision,updated_at
FROM semantic_task_states_0040;
DROP TABLE semantic_task_states_0040;

UPDATE semantic_extraction_sets
SET semantic_target_key='legacy-m1-extraction:' || extraction_set_id
WHERE semantic_target_key='';

UPDATE semantic_prepared_snapshots
SET semantic_target_key='legacy-m1-card:' || card_revision_id
WHERE semantic_target_key='';

UPDATE semantic_card_revisions
SET semantic_target_key='legacy-m1-card:' || card_revision_id
WHERE semantic_target_key='';

UPDATE semantic_organization_jobs
SET semantic_target_key='legacy-m2-job:' || organization_job_id
WHERE semantic_target_key='';

UPDATE semantic_organization_snapshots
SET semantic_target_key='legacy-m2-card:' || composed_card_revision_id
WHERE semantic_target_key='';

UPDATE semantic_composed_card_revisions
SET semantic_target_key='legacy-m2-card:' || composed_card_revision_id
WHERE semantic_target_key='';

INSERT OR IGNORE INTO semantic_targets
    (vault_id,target_key,target_kind,scope_ref,fingerprint_version,fingerprint,created_at,updated_at)
SELECT vault_id,semantic_target_key,'legacy_m1',scope_ref,1,semantic_target_key,created_at,created_at
FROM semantic_card_revisions
WHERE semantic_target_key LIKE 'legacy-m1-card:%';

INSERT OR IGNORE INTO semantic_targets
    (vault_id,target_key,target_kind,scope_ref,fingerprint_version,fingerprint,created_at,updated_at)
SELECT vault_id,semantic_target_key,'legacy_m2',scope_ref,1,semantic_target_key,created_at,created_at
FROM semantic_composed_card_revisions
WHERE semantic_target_key LIKE 'legacy-m2-card:%';

CREATE INDEX semantic_card_revisions_target_idx
    ON semantic_card_revisions(vault_id,semantic_target_key,rules_revision);
CREATE INDEX semantic_composed_revisions_target_idx
    ON semantic_composed_card_revisions(vault_id,semantic_target_key,rules_revision);
