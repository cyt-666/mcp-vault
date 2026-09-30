-- Independent, Vault-scoped semantic memory namespace. Canonical source notes
-- stay in the Vault filesystem; published cards are materialized through Core.

CREATE TABLE semantic_sources (
    vault_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    current_revision_id TEXT,
    source_path TEXT NOT NULL,
    current_content_hash TEXT NOT NULL,
    authorization_revision INTEGER NOT NULL DEFAULT 0 CHECK (authorization_revision >= 0),
    eligible INTEGER NOT NULL CHECK (eligible IN (0, 1)),
    invalid_reason TEXT,
    pending_rebuild INTEGER NOT NULL DEFAULT 0 CHECK (pending_rebuild IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, source_id),
    UNIQUE (vault_id, file_id),
    UNIQUE (vault_id, source_id, current_revision_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, file_id) REFERENCES file_entries(vault_id, id),
    FOREIGN KEY (vault_id, current_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE semantic_source_revisions (
    vault_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    file_revision INTEGER NOT NULL CHECK (file_revision >= 1),
    content_hash TEXT NOT NULL,
    source_path TEXT NOT NULL,
    source_time_scope_json TEXT NOT NULL DEFAULT '{}',
    availability TEXT NOT NULL CHECK (availability IN ('current', 'historical', 'unavailable')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, source_revision_id),
    UNIQUE (vault_id, source_id, file_revision, content_hash),
    UNIQUE (vault_id, source_revision_id, source_id),
    FOREIGN KEY (vault_id, source_id) REFERENCES semantic_sources(vault_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, file_id) REFERENCES file_entries(vault_id, id)
);

CREATE INDEX semantic_source_revisions_file_idx
    ON semantic_source_revisions(vault_id, file_id, file_revision DESC);
CREATE INDEX semantic_sources_eligibility_idx
    ON semantic_sources(vault_id, eligible, pending_rebuild, source_id);

CREATE TABLE semantic_evidence_refs (
    vault_id TEXT NOT NULL,
    evidence_ref_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    validation_status TEXT NOT NULL CHECK (validation_status IN ('validated', 'invalidated')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, evidence_ref_id),
    UNIQUE (vault_id, evidence_ref_id, source_id),
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_evidence_spans (
    vault_id TEXT NOT NULL,
    evidence_ref_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    span_role TEXT NOT NULL CHECK (span_role IN ('body', 'context')),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    start_byte INTEGER NOT NULL CHECK (start_byte >= 0),
    end_byte INTEGER NOT NULL CHECK (end_byte > start_byte),
    content_hash TEXT NOT NULL,
    PRIMARY KEY (vault_id, evidence_ref_id, span_role, ordinal),
    FOREIGN KEY (vault_id, evidence_ref_id) REFERENCES semantic_evidence_refs(vault_id, evidence_ref_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id) REFERENCES semantic_source_revisions(vault_id, source_revision_id) ON DELETE CASCADE
);

CREATE TABLE semantic_extraction_sets (
    vault_id TEXT NOT NULL,
    extraction_set_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('running', 'prepared', 'success_nonempty', 'success_empty', 'partial', 'failed', 'cancelled')),
    input_hash TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    authorization_revision INTEGER NOT NULL CHECK (authorization_revision >= 0),
    request_hash TEXT,
    safe_error_code TEXT,
    observation_count INTEGER NOT NULL DEFAULT 0 CHECK (observation_count >= 0),
    card_count INTEGER NOT NULL DEFAULT 0 CHECK (card_count >= 0),
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    PRIMARY KEY (vault_id, extraction_set_id),
    UNIQUE (vault_id, extraction_set_id, source_id),
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_extraction_idempotency (
    vault_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    extraction_set_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, idempotency_key),
    FOREIGN KEY (vault_id, extraction_set_id)
        REFERENCES semantic_extraction_sets(vault_id, extraction_set_id) ON DELETE CASCADE
);

CREATE TABLE semantic_observations (
    vault_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    extraction_set_id TEXT NOT NULL,
    local_observation_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('preference', 'constraint', 'decision', 'experience', 'procedure', 'state')),
    statement TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('user', 'project', 'task', 'unspecified')),
    assertion_status TEXT NOT NULL CHECK (assertion_status IN ('source_asserted', 'proposed', 'adopted', 'committed', 'observed', 'rejected', 'unknown')),
    source_time_scope_json TEXT NOT NULL DEFAULT '{}',
    conditions_json TEXT NOT NULL DEFAULT '[]',
    exceptions_json TEXT NOT NULL DEFAULT '[]',
    ordered_steps_json TEXT NOT NULL DEFAULT '[]',
    result_json TEXT,
    uncertainty_json TEXT,
    admission_reason TEXT NOT NULL,
    value_for_future_work TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, observation_id),
    UNIQUE (vault_id, observation_id, source_id),
    UNIQUE (vault_id, extraction_set_id, local_observation_key),
    FOREIGN KEY (vault_id, source_id) REFERENCES semantic_sources(vault_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, extraction_set_id, source_id)
        REFERENCES semantic_extraction_sets(vault_id, extraction_set_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_observation_evidence (
    vault_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    evidence_ref_id TEXT NOT NULL,
    PRIMARY KEY (vault_id, observation_id, evidence_ref_id),
    FOREIGN KEY (vault_id, observation_id, source_id)
        REFERENCES semantic_observations(vault_id, observation_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, evidence_ref_id, source_id)
        REFERENCES semantic_evidence_refs(vault_id, evidence_ref_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_memory_cards (
    vault_id TEXT NOT NULL,
    card_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    topic_key TEXT NOT NULL,
    current_revision_id TEXT,
    eligibility TEXT NOT NULL CHECK (eligibility IN ('pending', 'readable', 'invalidated')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, card_id),
    UNIQUE (vault_id, card_id, source_id),
    UNIQUE (vault_id, card_id, current_revision_id),
    UNIQUE (vault_id, source_id, topic_key),
    FOREIGN KEY (vault_id, source_id) REFERENCES semantic_sources(vault_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, card_id, current_revision_id)
        REFERENCES semantic_card_revisions(vault_id, card_id, card_revision_id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE semantic_card_revisions (
    vault_id TEXT NOT NULL,
    card_revision_id TEXT NOT NULL,
    card_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    extraction_set_id TEXT NOT NULL,
    revision_number INTEGER NOT NULL CHECK (revision_number >= 1),
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    assertion_status TEXT NOT NULL,
    temporal_scope_json TEXT NOT NULL DEFAULT '{}',
    composition_profile_id TEXT NOT NULL,
    canonical_markdown_hash TEXT NOT NULL,
    canonical_file_id TEXT,
    canonical_path TEXT NOT NULL,
    canonical_revision INTEGER,
    publication_state TEXT NOT NULL CHECK (publication_state IN ('prepared', 'published', 'blocked')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, card_revision_id),
    UNIQUE (vault_id, card_id, revision_number),
    UNIQUE (vault_id, card_revision_id, card_id, source_id),
    UNIQUE (vault_id, card_revision_id, card_id),
    FOREIGN KEY (vault_id, card_id, source_id)
        REFERENCES semantic_memory_cards(vault_id, card_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, extraction_set_id, source_id)
        REFERENCES semantic_extraction_sets(vault_id, extraction_set_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, canonical_file_id) REFERENCES file_entries(vault_id, id)
);

CREATE TABLE semantic_card_dependencies (
    vault_id TEXT NOT NULL,
    card_revision_id TEXT NOT NULL,
    card_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    PRIMARY KEY (vault_id, card_revision_id, source_id),
    FOREIGN KEY (vault_id, card_revision_id, card_id, source_id)
        REFERENCES semantic_card_revisions(vault_id, card_revision_id, card_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_card_items (
    vault_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    card_revision_id TEXT NOT NULL,
    card_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    item_kind TEXT NOT NULL CHECK (item_kind IN ('core_assertion', 'required_qualifier', 'optional_detail', 'unresolved_item', 'condition', 'exception', 'ordered_step')),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    content TEXT NOT NULL,
    PRIMARY KEY (vault_id, item_id),
    UNIQUE (vault_id, card_revision_id, item_kind, ordinal),
    UNIQUE (vault_id, item_id, card_revision_id, source_id, observation_id),
    FOREIGN KEY (vault_id, card_revision_id, card_id, source_id)
        REFERENCES semantic_card_revisions(vault_id, card_revision_id, card_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, observation_id, source_id)
        REFERENCES semantic_observations(vault_id, observation_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_card_assertion_supports (
    vault_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    card_revision_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    evidence_ref_id TEXT NOT NULL,
    support_kind TEXT NOT NULL CHECK (support_kind IN ('single', 'and', 'or')),
    PRIMARY KEY (vault_id, item_id, evidence_ref_id),
    FOREIGN KEY (vault_id,item_id,card_revision_id,source_id,observation_id)
        REFERENCES semantic_card_items(vault_id,item_id,card_revision_id,source_id,observation_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, observation_id, source_id)
        REFERENCES semantic_observations(vault_id, observation_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, evidence_ref_id, source_id)
        REFERENCES semantic_evidence_refs(vault_id, evidence_ref_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_prepared_snapshots (
    vault_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    extraction_set_id TEXT NOT NULL,
    card_id TEXT NOT NULL,
    card_revision_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    source_content_hash TEXT NOT NULL,
    authorization_revision INTEGER NOT NULL CHECK (authorization_revision >= 0),
    expected_card_revision_id TEXT,
    proposed_card_revision_number INTEGER NOT NULL CHECK (proposed_card_revision_number >= 1),
    expected_file_id TEXT,
    expected_file_revision INTEGER,
    proposed_file_revision INTEGER NOT NULL CHECK (proposed_file_revision >= 1),
    published_file_id TEXT,
    published_file_revision INTEGER,
    target_path TEXT NOT NULL,
    proposed_file_hash TEXT NOT NULL,
    canonical_bytes BLOB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('prepared', 'written', 'applied', 'blocked')),
    safe_error_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, snapshot_id),
    UNIQUE (vault_id, card_revision_id),
    FOREIGN KEY (vault_id, extraction_set_id, source_id)
        REFERENCES semantic_extraction_sets(vault_id, extraction_set_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, card_id, source_id)
        REFERENCES semantic_memory_cards(vault_id, card_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, card_revision_id, card_id, source_id)
        REFERENCES semantic_card_revisions(vault_id, card_revision_id, card_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE INDEX semantic_prepared_snapshots_recovery_idx
    ON semantic_prepared_snapshots(vault_id, status, created_at, snapshot_id);

-- Reserved for M3. M1 creates stable, Vault-scoped storage without exposing
-- write operations or applying rules yet.
CREATE TABLE semantic_corrections (
    vault_id TEXT NOT NULL,
    correction_id TEXT NOT NULL,
    semantic_target_key TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    authorized_by TEXT NOT NULL,
    correction_json TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, correction_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE TABLE semantic_suppressions (
    vault_id TEXT NOT NULL,
    suppression_id TEXT NOT NULL,
    semantic_target_key TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    authorized_by TEXT NOT NULL,
    policy_json TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, suppression_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);
