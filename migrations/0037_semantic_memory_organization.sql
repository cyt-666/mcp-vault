-- M2-only organization namespace.  M1 source cards, observations and
-- evidence remain immutable inputs; composed cards are a separate projection.

CREATE TABLE semantic_organization_jobs (
    vault_id TEXT NOT NULL,
    organization_job_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('running', 'prepared', 'applied', 'blocked', 'failed', 'cancelled')),
    input_hash TEXT NOT NULL,
    policy_revision INTEGER NOT NULL CHECK (policy_revision >= 0),
    profile_id TEXT NOT NULL,
    decision_revision INTEGER NOT NULL CHECK (decision_revision >= 0),
    safe_error_code TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    PRIMARY KEY (vault_id, organization_job_id)
);

CREATE TABLE semantic_organization_idempotency (
    vault_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    organization_job_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, idempotency_key),
    FOREIGN KEY (vault_id, organization_job_id)
        REFERENCES semantic_organization_jobs(vault_id, organization_job_id) ON DELETE CASCADE
);

CREATE TABLE semantic_organization_job_sources (
    vault_id TEXT NOT NULL,
    organization_job_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    authorization_revision INTEGER NOT NULL CHECK (authorization_revision >= 0),
    source_generation INTEGER NOT NULL CHECK (source_generation >= 0),
    extraction_commit_sequence INTEGER NOT NULL CHECK (extraction_commit_sequence >= 0),
    PRIMARY KEY (vault_id, organization_job_id, source_id),
    FOREIGN KEY (vault_id, organization_job_id)
        REFERENCES semantic_organization_jobs(vault_id, organization_job_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_id)
        REFERENCES semantic_sources(vault_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_relation_candidates (
    vault_id TEXT NOT NULL,
    relation_candidate_id TEXT NOT NULL,
    organization_job_id TEXT NOT NULL,
    left_observation_id TEXT NOT NULL,
    left_source_id TEXT NOT NULL,
    left_source_revision_id TEXT NOT NULL,
    right_observation_id TEXT NOT NULL,
    right_source_id TEXT NOT NULL,
    right_source_revision_id TEXT NOT NULL,
    candidate_input_hash TEXT NOT NULL,
    similarity_hint REAL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'decided', 'rejected', 'expired')),
    profile_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, relation_candidate_id),
    FOREIGN KEY (vault_id, organization_job_id)
        REFERENCES semantic_organization_jobs(vault_id, organization_job_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, left_observation_id, left_source_id)
        REFERENCES semantic_observations(vault_id, observation_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, left_source_revision_id, left_source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, right_observation_id, right_source_id)
        REFERENCES semantic_observations(vault_id, observation_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, right_source_revision_id, right_source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_relation_decisions (
    vault_id TEXT NOT NULL,
    relation_decision_id TEXT NOT NULL,
    relation_candidate_id TEXT NOT NULL,
    decision_revision INTEGER NOT NULL CHECK (decision_revision >= 1),
    relation_kind TEXT NOT NULL CHECK (relation_kind IN ('equivalent', 'supplements', 'different_scope', 'conflicts', 'supersedes', 'related')),
    decision_state TEXT NOT NULL CHECK (decision_state IN ('accepted', 'rejected', 'stale', 'blocked')),
    decision_reason_code TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, relation_decision_id),
    UNIQUE (vault_id, relation_candidate_id, decision_revision),
    FOREIGN KEY (vault_id, relation_candidate_id)
        REFERENCES semantic_relation_candidates(vault_id, relation_candidate_id) ON DELETE CASCADE
);

CREATE TABLE semantic_relation_evidence (
    vault_id TEXT NOT NULL,
    relation_decision_id TEXT NOT NULL,
    evidence_ref_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('left', 'right', 'decision')),
    PRIMARY KEY (vault_id, relation_decision_id, evidence_ref_id, role),
    FOREIGN KEY (vault_id, relation_decision_id)
        REFERENCES semantic_relation_decisions(vault_id, relation_decision_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, evidence_ref_id, source_id)
        REFERENCES semantic_evidence_refs(vault_id, evidence_ref_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_composed_cards (
    vault_id TEXT NOT NULL,
    composed_card_id TEXT NOT NULL,
    identity_key TEXT NOT NULL,
    topic_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    current_revision_id TEXT,
    eligibility TEXT NOT NULL CHECK (eligibility IN ('pending', 'readable', 'invalidated', 'stale', 'redirected')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, composed_card_id),
    UNIQUE (vault_id, identity_key),
    UNIQUE (vault_id, composed_card_id, current_revision_id),
    FOREIGN KEY (vault_id, current_revision_id)
        REFERENCES semantic_composed_card_revisions(vault_id, composed_card_revision_id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE semantic_composed_card_revisions (
    vault_id TEXT NOT NULL,
    composed_card_revision_id TEXT NOT NULL,
    composed_card_id TEXT NOT NULL,
    organization_job_id TEXT NOT NULL,
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
    PRIMARY KEY (vault_id, composed_card_revision_id),
    UNIQUE (vault_id, composed_card_id, revision_number),
    FOREIGN KEY (vault_id, composed_card_id)
        REFERENCES semantic_composed_cards(vault_id, composed_card_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, organization_job_id)
        REFERENCES semantic_organization_jobs(vault_id, organization_job_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, canonical_file_id) REFERENCES file_entries(vault_id, id)
);

CREATE TABLE semantic_composed_card_dependencies (
    vault_id TEXT NOT NULL,
    composed_card_revision_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    PRIMARY KEY (vault_id, composed_card_revision_id, source_id),
    FOREIGN KEY (vault_id, composed_card_revision_id)
        REFERENCES semantic_composed_card_revisions(vault_id, composed_card_revision_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_composed_card_items (
    vault_id TEXT NOT NULL,
    composed_card_item_id TEXT NOT NULL,
    composed_card_revision_id TEXT NOT NULL,
    item_kind TEXT NOT NULL CHECK (item_kind IN ('core_assertion', 'required_qualifier', 'optional_detail', 'unresolved_item', 'condition', 'exception', 'ordered_step')),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    content TEXT NOT NULL,
    PRIMARY KEY (vault_id, composed_card_item_id),
    UNIQUE (vault_id, composed_card_revision_id, item_kind, ordinal),
    FOREIGN KEY (vault_id, composed_card_revision_id)
        REFERENCES semantic_composed_card_revisions(vault_id, composed_card_revision_id) ON DELETE CASCADE
);

CREATE TABLE semantic_composed_support_groups (
    vault_id TEXT NOT NULL,
    support_group_id TEXT NOT NULL,
    composed_card_item_id TEXT NOT NULL,
    operator TEXT NOT NULL CHECK (operator IN ('and', 'or')),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (vault_id, support_group_id),
    UNIQUE (vault_id, composed_card_item_id, ordinal),
    FOREIGN KEY (vault_id, composed_card_item_id)
        REFERENCES semantic_composed_card_items(vault_id, composed_card_item_id) ON DELETE CASCADE
);

CREATE TABLE semantic_composed_support_members (
    vault_id TEXT NOT NULL,
    support_member_id TEXT NOT NULL,
    support_group_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    evidence_ref_id TEXT NOT NULL,
    member_role TEXT NOT NULL CHECK (member_role IN ('body', 'context', 'complete')),
    PRIMARY KEY (vault_id, support_member_id),
    UNIQUE (vault_id, support_group_id, observation_id, evidence_ref_id),
    FOREIGN KEY (vault_id, support_group_id)
        REFERENCES semantic_composed_support_groups(vault_id, support_group_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, observation_id, source_id)
        REFERENCES semantic_observations(vault_id, observation_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, evidence_ref_id, source_id)
        REFERENCES semantic_evidence_refs(vault_id, evidence_ref_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_revision_id, source_id)
        REFERENCES semantic_source_revisions(vault_id, source_revision_id, source_id) ON DELETE CASCADE
);

CREATE TABLE semantic_organization_snapshots (
    vault_id TEXT NOT NULL,
    organization_snapshot_id TEXT NOT NULL,
    organization_job_id TEXT NOT NULL,
    composed_card_id TEXT NOT NULL,
    composed_card_revision_id TEXT NOT NULL,
    expected_card_revision_id TEXT,
    proposed_revision_number INTEGER NOT NULL CHECK (proposed_revision_number >= 1),
    target_path TEXT NOT NULL,
    proposed_file_hash TEXT NOT NULL,
    canonical_bytes BLOB NOT NULL,
    published_file_id TEXT,
    published_file_revision INTEGER,
    status TEXT NOT NULL CHECK (status IN ('prepared', 'written', 'applied', 'blocked')),
    safe_error_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, organization_snapshot_id),
    UNIQUE (vault_id, composed_card_revision_id),
    FOREIGN KEY (vault_id, organization_job_id)
        REFERENCES semantic_organization_jobs(vault_id, organization_job_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, composed_card_id)
        REFERENCES semantic_composed_cards(vault_id, composed_card_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, composed_card_revision_id)
        REFERENCES semantic_composed_card_revisions(vault_id, composed_card_revision_id) ON DELETE CASCADE
);

CREATE TABLE semantic_composed_card_aliases (
    vault_id TEXT NOT NULL,
    alias_id TEXT NOT NULL,
    old_composed_card_id TEXT NOT NULL,
    replacement_composed_card_id TEXT NOT NULL,
    decision_revision INTEGER NOT NULL CHECK (decision_revision >= 1),
    reason TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, alias_id),
    UNIQUE (vault_id, old_composed_card_id),
    FOREIGN KEY (vault_id, old_composed_card_id)
        REFERENCES semantic_composed_cards(vault_id, composed_card_id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, replacement_composed_card_id)
        REFERENCES semantic_composed_cards(vault_id, composed_card_id) ON DELETE CASCADE
);

CREATE INDEX semantic_organization_jobs_state_idx
    ON semantic_organization_jobs(vault_id, state, created_at);
CREATE INDEX semantic_relation_candidates_lookup_idx
    ON semantic_relation_candidates(vault_id, left_observation_id, right_observation_id, state);
CREATE INDEX semantic_composed_cards_eligibility_idx
    ON semantic_composed_cards(vault_id, eligibility, updated_at);
CREATE INDEX semantic_composed_dependencies_source_idx
    ON semantic_composed_card_dependencies(vault_id, source_id, source_revision_id);
CREATE INDEX semantic_organization_snapshots_recovery_idx
    ON semantic_organization_snapshots(vault_id, status, created_at);
