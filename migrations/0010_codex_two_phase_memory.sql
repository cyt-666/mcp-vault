-- Codex-style two-phase memory staging and consolidation state.
--
-- Phase 1 outputs are derived, Vault-scoped inputs. They are never final
-- memory. Phase 2 proposals remain untrusted until application validation and
-- Vault Core materialization complete.

CREATE TABLE memory_stage1_outputs (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_type TEXT NOT NULL CHECK (
        source_type IN ('note', 'explicit_agent', 'explicit_admin', 'direct_markdown', 'import')
    ),
    source_key TEXT NOT NULL,
    source_file_id TEXT,
    source_path TEXT,
    source_revision INTEGER CHECK (source_revision IS NULL OR source_revision >= 0),
    profile_hash TEXT NOT NULL,
    pipeline_version INTEGER NOT NULL CHECK (pipeline_version >= 1),
    prompt_version TEXT NOT NULL,
    raw_memory TEXT NOT NULL,
    source_summary TEXT NOT NULL,
    source_slug TEXT,
    evidence_json TEXT NOT NULL DEFAULT '[]',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    output_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ready', 'no_output', 'withdrawn')),
    generated_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    usage_count INTEGER NOT NULL DEFAULT 0 CHECK (usage_count >= 0),
    last_usage INTEGER,
    selected_for_phase2 INTEGER NOT NULL DEFAULT 0 CHECK (selected_for_phase2 IN (0, 1)),
    selected_for_phase2_hash TEXT,
    selected_for_phase2_at INTEGER,
    UNIQUE (vault_id, id),
    UNIQUE (vault_id, source_type, source_key),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, source_file_id)
        REFERENCES file_entries(vault_id, id)
);

CREATE INDEX memory_stage1_outputs_dirty_idx
    ON memory_stage1_outputs(vault_id, selected_for_phase2, status, updated_at, id);

CREATE INDEX memory_stage1_outputs_source_idx
    ON memory_stage1_outputs(vault_id, source_file_id, source_revision);

CREATE UNIQUE INDEX jobs_one_active_memory_consolidation_per_vault_idx
    ON jobs(vault_id)
    WHERE job_type = 'memory.consolidate'
      AND status IN ('queued', 'running', 'retry_wait');

CREATE TABLE memory_consolidation_proposals (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    proposal_json TEXT NOT NULL,
    model_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('prepared', 'applied', 'rejected')),
    created_at INTEGER NOT NULL,
    applied_at INTEGER,
    UNIQUE (vault_id, id),
    UNIQUE (vault_id, input_hash),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE INDEX memory_consolidation_proposals_status_idx
    ON memory_consolidation_proposals(vault_id, status, created_at DESC);

CREATE TABLE memory_consolidation_state (
    vault_id TEXT PRIMARY KEY,
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    memory_summary TEXT NOT NULL DEFAULT '',
    last_input_hash TEXT,
    last_proposal_id TEXT,
    last_success_at INTEGER,
    legacy_reset_version INTEGER NOT NULL DEFAULT 0 CHECK (legacy_reset_version >= 0),
    legacy_reextract_pending INTEGER NOT NULL DEFAULT 0 CHECK (legacy_reextract_pending IN (0, 1)),
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id, last_proposal_id)
        REFERENCES memory_consolidation_proposals(vault_id, id)
);
