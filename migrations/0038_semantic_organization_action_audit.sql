-- Persist bounded organization actions, including successful no_change and
-- relation-only decisions.  This is an audit projection, not a fact graph.
CREATE TABLE semantic_organization_action_audits (
    vault_id TEXT NOT NULL,
    organization_job_id TEXT NOT NULL,
    action_ordinal INTEGER NOT NULL CHECK (action_ordinal >= 0),
    action_kind TEXT NOT NULL CHECK (action_kind IN (
        'create_composed_card', 'attach_equivalent_evidence',
        'add_supported_information', 'supersede_with_evidence',
        'record_conflict', 'link_related_only', 'keep_separate_scope',
        'no_change'
    )),
    candidate_ids_json TEXT NOT NULL,
    reason TEXT,
    outcome TEXT NOT NULL CHECK (outcome IN ('accepted', 'rejected', 'blocked')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, organization_job_id, action_ordinal),
    FOREIGN KEY (vault_id, organization_job_id)
        REFERENCES semantic_organization_jobs(vault_id, organization_job_id) ON DELETE CASCADE
);

CREATE INDEX semantic_organization_action_audit_kind_idx
    ON semantic_organization_action_audits(vault_id, action_kind, created_at);
