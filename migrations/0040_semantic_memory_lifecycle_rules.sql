-- M3-A: Vault-scoped target identity and durable lifecycle rules. Existing
-- M1/M2 rows remain readable with the zero rules revision until a new write
-- captures the current runtime revision.

CREATE TABLE semantic_targets (
    vault_id TEXT NOT NULL,
    target_key TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    fingerprint_version INTEGER NOT NULL CHECK (fingerprint_version >= 1),
    fingerprint TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, target_key),
    UNIQUE (vault_id, target_kind, scope_ref, fingerprint_version, fingerprint),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE TABLE semantic_rules_runtime (
    vault_id TEXT NOT NULL PRIMARY KEY,
    rules_revision INTEGER NOT NULL DEFAULT 0 CHECK (rules_revision >= 0),
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE TABLE semantic_rule_idempotency (
    vault_id TEXT NOT NULL,
    rule_kind TEXT NOT NULL CHECK (rule_kind IN ('correction', 'suppression')),
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    result_id TEXT NOT NULL,
    rules_revision INTEGER NOT NULL CHECK (rules_revision >= 0),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, rule_kind, idempotency_key)
);

CREATE TABLE semantic_task_states (
    vault_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    target_key TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),
    evidence_observation_id TEXT,
    evidence_source_id TEXT,
    evidence_source_revision_id TEXT,
    rules_revision INTEGER NOT NULL CHECK (rules_revision >= 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, task_id),
    FOREIGN KEY (vault_id, target_key)
        REFERENCES semantic_targets(vault_id, target_key) ON DELETE CASCADE
);

ALTER TABLE semantic_corrections ADD COLUMN action TEXT NOT NULL DEFAULT 'replace';
ALTER TABLE semantic_corrections ADD COLUMN target_kind TEXT NOT NULL DEFAULT 'semantic_target';
ALTER TABLE semantic_corrections ADD COLUMN source_id TEXT;
ALTER TABLE semantic_corrections ADD COLUMN source_revision_id TEXT;
ALTER TABLE semantic_corrections ADD COLUMN observation_id TEXT;
ALTER TABLE semantic_corrections ADD COLUMN card_id TEXT;
ALTER TABLE semantic_corrections ADD COLUMN composed_card_id TEXT;
ALTER TABLE semantic_corrections ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE semantic_corrections ADD COLUMN idempotency_key TEXT;

ALTER TABLE semantic_suppressions ADD COLUMN action TEXT NOT NULL DEFAULT 'suppress';
ALTER TABLE semantic_suppressions ADD COLUMN target_kind TEXT NOT NULL DEFAULT 'semantic_target';
ALTER TABLE semantic_suppressions ADD COLUMN source_id TEXT;
ALTER TABLE semantic_suppressions ADD COLUMN source_revision_id TEXT;
ALTER TABLE semantic_suppressions ADD COLUMN observation_id TEXT;
ALTER TABLE semantic_suppressions ADD COLUMN card_id TEXT;
ALTER TABLE semantic_suppressions ADD COLUMN composed_card_id TEXT;
ALTER TABLE semantic_suppressions ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE semantic_suppressions ADD COLUMN idempotency_key TEXT;

ALTER TABLE semantic_extraction_sets ADD COLUMN semantic_target_key TEXT NOT NULL DEFAULT '';
ALTER TABLE semantic_extraction_sets ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE semantic_prepared_snapshots ADD COLUMN semantic_target_key TEXT NOT NULL DEFAULT '';
ALTER TABLE semantic_prepared_snapshots ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE semantic_card_revisions ADD COLUMN semantic_target_key TEXT NOT NULL DEFAULT '';
ALTER TABLE semantic_card_revisions ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;

ALTER TABLE semantic_organization_jobs ADD COLUMN semantic_target_key TEXT NOT NULL DEFAULT '';
ALTER TABLE semantic_organization_jobs ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE semantic_organization_snapshots ADD COLUMN semantic_target_key TEXT NOT NULL DEFAULT '';
ALTER TABLE semantic_organization_snapshots ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE semantic_composed_card_revisions ADD COLUMN semantic_target_key TEXT NOT NULL DEFAULT '';
ALTER TABLE semantic_composed_card_revisions ADD COLUMN rules_revision INTEGER NOT NULL DEFAULT 0;

CREATE INDEX semantic_targets_lookup_idx
    ON semantic_targets(vault_id, target_kind, scope_ref, fingerprint_version, fingerprint);
CREATE INDEX semantic_corrections_target_idx
    ON semantic_corrections(vault_id, semantic_target_key, active, revision);
CREATE INDEX semantic_suppressions_target_idx
    ON semantic_suppressions(vault_id, semantic_target_key, active, revision);
CREATE INDEX semantic_task_states_target_idx
    ON semantic_task_states(vault_id, target_key, state);
