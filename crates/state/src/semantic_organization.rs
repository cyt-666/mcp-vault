//! Vault-scoped M2 organization storage.
//!
//! This repository stores bounded relation decisions and composed-card
//! projections. It deliberately does not call a provider or write through
//! Vault Core; those responsibilities belong to the later organization
//! service.

use mcp_vault_domain::{
    ComposedCardId, ComposedCardItemId, ComposedCardRevisionId, EvidenceRefId, ObservationId,
    OrganizationJobId, OrganizationSnapshotId, RelationCandidateId, RelationDecisionId,
    SemanticSourceId, SourceRevisionId, SupportGroupId, SupportMemberId, VaultContext, VaultPath,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::{StateError, now_millis};

type ComposedFingerprintObservation = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationSourceFence {
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub content_hash: String,
    pub authorization_revision: i64,
    pub source_generation: i64,
    pub extraction_commit_sequence: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationJobRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub id: OrganizationJobId,
    pub state: String,
    pub lifecycle_state: String,
    pub input_hash: String,
    pub policy_revision: i64,
    pub profile_id: String,
    pub decision_revision: i64,
    pub safe_error_code: Option<String>,
    pub semantic_target_key: String,
    pub rules_revision: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrganizationObservation {
    pub id: ObservationId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub evidence_ref_id: EvidenceRefId,
    pub kind: String,
    pub statement: String,
    pub scope: String,
    pub assertion_status: String,
    pub source_time_scope: Value,
    pub conditions: Vec<String>,
    pub exceptions: Vec<String>,
    pub ordered_steps: Vec<String>,
    pub result: Option<String>,
    pub uncertainty: Option<String>,
    pub value_for_future_work: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationActionAuditInput {
    pub ordinal: u32,
    pub action_kind: String,
    pub candidate_ids: Vec<RelationCandidateId>,
    pub reason: Option<String>,
}

type SnapshotWitnessRow = (
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<i64>,
    String,
    String,
    i64,
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationEvidenceInput {
    pub evidence_ref_id: EvidenceRefId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub role: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationCandidateInput {
    pub id: RelationCandidateId,
    pub left_observation_id: ObservationId,
    pub left_source_id: SemanticSourceId,
    pub left_source_revision_id: SourceRevisionId,
    pub right_observation_id: ObservationId,
    pub right_source_id: SemanticSourceId,
    pub right_source_revision_id: SourceRevisionId,
    pub candidate_input_hash: String,
    pub similarity_hint: Option<i64>,
    pub profile_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationDecisionInput {
    pub id: RelationDecisionId,
    pub candidate_id: RelationCandidateId,
    pub relation_kind: String,
    pub decision_state: String,
    pub decision_reason_code: String,
    pub profile_id: String,
    pub evidence: Vec<RelationEvidenceInput>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticRelationRecord {
    pub id: RelationDecisionId,
    pub relation_kind: String,
    pub reason_code: String,
    pub left_source_id: SemanticSourceId,
    pub right_source_id: SemanticSourceId,
    pub left_observation_id: ObservationId,
    pub right_observation_id: ObservationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupportMemberInput {
    pub id: SupportMemberId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub observation_id: ObservationId,
    pub evidence_ref_id: EvidenceRefId,
    pub member_role: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupportGroupInput {
    pub id: SupportGroupId,
    pub operator: String,
    pub ordinal: u32,
    pub members: Vec<SupportMemberInput>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposedCardItemInput {
    pub id: ComposedCardItemId,
    pub kind: String,
    pub ordinal: u32,
    pub content: String,
    pub support_groups: Vec<SupportGroupInput>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComposedCardRevisionInput {
    pub card_id: ComposedCardId,
    pub card_revision_id: ComposedCardRevisionId,
    pub expected_card_revision_id: Option<ComposedCardRevisionId>,
    pub revision_number: u32,
    pub identity_key: String,
    pub topic_key: String,
    pub title: String,
    pub kind: String,
    pub scope_ref: String,
    pub assertion_status: String,
    pub temporal_scope: Value,
    pub composition_profile_id: String,
    pub canonical_path: VaultPath,
    pub canonical_markdown_hash: String,
    pub canonical_bytes: Vec<u8>,
    pub items: Vec<ComposedCardItemInput>,
    pub dependencies: Vec<(SemanticSourceId, SourceRevisionId)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationPreparedSnapshot {
    pub id: OrganizationSnapshotId,
    pub job_id: OrganizationJobId,
    pub card_id: ComposedCardId,
    pub card_revision_id: ComposedCardRevisionId,
    pub expected_card_revision_id: Option<ComposedCardRevisionId>,
    pub proposed_revision_number: u32,
    pub target_path: VaultPath,
    pub proposed_file_hash: String,
    pub canonical_bytes: Vec<u8>,
    pub status: String,
    pub expected_file_id: Option<mcp_vault_domain::FileId>,
    pub expected_file_revision: Option<mcp_vault_domain::Revision>,
    pub actual_file_id: Option<mcp_vault_domain::FileId>,
    pub actual_file_revision: Option<mcp_vault_domain::Revision>,
    pub actual_file_hash: Option<String>,
    pub source_fence_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposedSupportMemberRecord {
    pub id: SupportMemberId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub source_path: VaultPath,
    pub observation_id: ObservationId,
    pub evidence_ref_id: EvidenceRefId,
    pub member_role: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposedSupportGroupRecord {
    pub id: SupportGroupId,
    pub operator: String,
    pub ordinal: u32,
    pub members: Vec<ComposedSupportMemberRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposedCardItemRecord {
    pub id: ComposedCardItemId,
    pub kind: String,
    pub ordinal: u32,
    pub content: String,
    pub support_groups: Vec<ComposedSupportGroupRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComposedCardRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub id: ComposedCardId,
    pub revision_id: ComposedCardRevisionId,
    pub title: String,
    pub kind: String,
    pub scope_ref: String,
    pub assertion_status: String,
    pub temporal_scope: Value,
    pub revision_number: u32,
    pub canonical_path: VaultPath,
    pub canonical_file_id: Option<mcp_vault_domain::FileId>,
    pub canonical_file_revision: Option<mcp_vault_domain::Revision>,
    pub canonical_markdown_hash: String,
    pub items: Vec<ComposedCardItemRecord>,
}

#[derive(Clone)]
pub struct SemanticOrganizationRepository {
    pool: SqlitePool,
}

#[derive(FromRow)]
struct JobRow {
    vault_id: String,
    organization_job_id: String,
    state: String,
    lifecycle_state: String,
    input_hash: String,
    policy_revision: i64,
    profile_id: String,
    decision_revision: i64,
    safe_error_code: Option<String>,
    semantic_target_key: String,
    rules_revision: i64,
}

#[derive(FromRow)]
struct CardHeaderRow {
    vault_id: String,
    composed_card_id: String,
    composed_card_revision_id: String,
    title: String,
    kind: String,
    scope_ref: String,
    assertion_status: String,
    temporal_scope_json: String,
    revision_number: i64,
    canonical_path: String,
    canonical_file_id: Option<String>,
    canonical_revision: Option<i64>,
    canonical_markdown_hash: String,
}

impl SemanticOrganizationRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Return accepted, current relation decisions for MemoryPack conflict
    /// presentation. Source qualification remains in this repository.
    pub async fn list_current_relations(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<SemanticRelationRecord>, StateError> {
        if limit == 0 || limit > 200 {
            return Err(StateError::InvalidInput(
                "semantic relation limit must be between 1 and 200",
            ));
        }
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
            "SELECT d.relation_decision_id,d.relation_kind,d.decision_reason_code,
                    c.left_source_id,c.right_source_id,c.left_observation_id,c.right_observation_id
             FROM semantic_relation_decisions d
             JOIN semantic_relation_candidates c
               ON c.vault_id=d.vault_id AND c.relation_candidate_id=d.relation_candidate_id
             JOIN semantic_sources sl
               ON sl.vault_id=c.vault_id AND sl.source_id=c.left_source_id
              AND sl.current_revision_id=c.left_source_revision_id
             JOIN semantic_sources sr
               ON sr.vault_id=c.vault_id AND sr.source_id=c.right_source_id
              AND sr.current_revision_id=c.right_source_revision_id
             WHERE d.vault_id=? AND d.decision_state='accepted'
               AND sl.eligible=1 AND sl.pending_rebuild=0
               AND sr.eligible=1 AND sr.pending_rebuild=0
             ORDER BY d.relation_decision_id LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(
                    id,
                    relation_kind,
                    reason_code,
                    left_source_id,
                    right_source_id,
                    left_observation_id,
                    right_observation_id,
                )| {
                    Ok(SemanticRelationRecord {
                        id: RelationDecisionId::parse(&id)?,
                        relation_kind,
                        reason_code,
                        left_source_id: SemanticSourceId::parse(&left_source_id)?,
                        right_source_id: SemanticSourceId::parse(&right_source_id)?,
                        left_observation_id: ObservationId::parse(&left_observation_id)?,
                        right_observation_id: ObservationId::parse(&right_observation_id)?,
                    })
                },
            )
            .collect()
    }

    pub async fn current_source_fences(
        &self,
        context: &VaultContext,
        source_ids: &[SemanticSourceId],
    ) -> Result<Vec<OrganizationSourceFence>, StateError> {
        let mut result = Vec::with_capacity(source_ids.len());
        for source_id in source_ids {
            let row: Option<(String, String, i64, i64, i64, i64)> = sqlx::query_as(
                "SELECT current_revision_id,current_content_hash,authorization_revision,
                        source_generation,extraction_commit_sequence,eligible
                 FROM semantic_sources WHERE vault_id=? AND source_id=? AND pending_rebuild=0",
            )
            .bind(context.id().to_string())
            .bind(source_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
            let Some((revision, hash, auth, generation, sequence, eligible)) = row else {
                return Err(StateError::Conflict);
            };
            if eligible != 1 {
                return Err(StateError::Conflict);
            }
            result.push(OrganizationSourceFence {
                source_id: *source_id,
                source_revision_id: SourceRevisionId::parse(&revision)?,
                content_hash: hash,
                authorization_revision: auth,
                source_generation: generation,
                extraction_commit_sequence: sequence,
            });
        }
        Ok(result)
    }

    pub async fn current_observations(
        &self,
        context: &VaultContext,
        source_ids: &[SemanticSourceId],
    ) -> Result<Vec<OrganizationObservation>, StateError> {
        let mut result = Vec::new();
        for source_id in source_ids {
            let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String, String, String, String, String, String, Option<String>, Option<String>, String)>(
                "SELECT o.observation_id,o.source_id,o.source_revision_id,e.evidence_ref_id,
                        CASE WHEN o.advisory_kind_unknown=1 THEN 'unknown' ELSE o.kind END,
                        o.statement,o.scope,o.assertion_status,o.source_time_scope_json,
                        o.conditions_json,o.exceptions_json,o.ordered_steps_json,o.result_json,
                        o.uncertainty_json,o.value_for_future_work
                 FROM semantic_observations o
                 JOIN semantic_extraction_sets es
                   ON es.vault_id=o.vault_id AND es.extraction_set_id=o.extraction_set_id
                  AND es.source_id=o.source_id
                 JOIN semantic_sources s ON s.vault_id=o.vault_id AND s.source_id=o.source_id
                 JOIN semantic_observation_evidence oe ON oe.vault_id=o.vault_id AND oe.observation_id=o.observation_id AND oe.source_id=o.source_id
                 JOIN semantic_evidence_refs e ON e.vault_id=oe.vault_id AND e.evidence_ref_id=oe.evidence_ref_id AND e.source_id=oe.source_id
                 WHERE o.vault_id=? AND o.source_id=? AND s.eligible=1 AND s.pending_rebuild=0
                   AND s.current_revision_id=o.source_revision_id AND e.validation_status='validated'
                   AND es.state='success_nonempty'
                   AND es.extraction_commit_sequence=s.extraction_commit_sequence
                   AND NOT EXISTS (
                     SELECT 1 FROM semantic_suppressions x
                     WHERE x.vault_id=o.vault_id AND x.active=1
                       AND x.action IN ('suppress_read','forget_current')
                       AND x.composed_card_id IS NULL
                       AND (
                         (x.card_id IS NULL
                           AND x.source_id=o.source_id
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=o.source_revision_id)
                           AND (x.observation_id IS NULL OR x.observation_id=o.observation_id))
                         OR (x.card_id IS NOT NULL
                           AND (x.source_id IS NULL OR x.source_id=o.source_id)
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=o.source_revision_id)
                           AND (x.observation_id IS NULL OR x.observation_id=o.observation_id)
                           AND EXISTS (
                             SELECT 1 FROM semantic_memory_cards mc
                             JOIN semantic_card_revisions mr
                               ON mr.vault_id=mc.vault_id AND mr.card_revision_id=mc.current_revision_id
                             JOIN semantic_card_items mi
                               ON mi.vault_id=mr.vault_id AND mi.card_revision_id=mr.card_revision_id
                             WHERE mc.vault_id=o.vault_id AND mc.card_id=x.card_id
                               AND mc.source_id=o.source_id
                               AND mr.source_revision_id=o.source_revision_id
                               AND mi.observation_id=o.observation_id))
                       )
                   )
                 ORDER BY o.local_observation_key,e.evidence_ref_id",
            )
            .bind(context.id().to_string())
            .bind(source_id.to_string())
            .fetch_all(&self.pool)
            .await?;
            for (
                id,
                source,
                revision,
                evidence,
                kind,
                statement,
                scope,
                status,
                time_scope,
                conditions,
                exceptions,
                steps,
                result_json,
                uncertainty,
                value,
            ) in rows
            {
                result.push(OrganizationObservation {
                    id: ObservationId::parse(&id)?,
                    source_id: SemanticSourceId::parse(&source)?,
                    source_revision_id: SourceRevisionId::parse(&revision)?,
                    evidence_ref_id: EvidenceRefId::parse(&evidence)?,
                    kind,
                    statement,
                    scope,
                    assertion_status: status,
                    source_time_scope: serde_json::from_str(&time_scope)?,
                    conditions: serde_json::from_str(&conditions)?,
                    exceptions: serde_json::from_str(&exceptions)?,
                    ordered_steps: serde_json::from_str(&steps)?,
                    result: result_json,
                    uncertainty,
                    value_for_future_work: value,
                });
            }
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_job(
        &self,
        context: &VaultContext,
        id: OrganizationJobId,
        input_hash: &str,
        policy_revision: i64,
        profile_id: &str,
        idempotency_key: &str,
        request_hash: &str,
        sources: &[OrganizationSourceFence],
    ) -> Result<(OrganizationJobRecord, bool), StateError> {
        if input_hash.is_empty() || request_hash.is_empty() || sources.is_empty() {
            return Err(StateError::InvalidInput(
                "organization job hash and source fences are required",
            ));
        }
        let mut tx = self.pool.begin().await?;
        let vault = context.id().to_string();
        if let Some((existing_id, existing_hash)) = sqlx::query_as::<_, (String, String)>(
            "SELECT organization_job_id,request_hash
             FROM semantic_organization_idempotency WHERE vault_id=? AND idempotency_key=?",
        )
        .bind(&vault)
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            if existing_hash != request_hash {
                return Err(StateError::Conflict);
            }
            let existing_job_id = OrganizationJobId::parse(&existing_id)?;
            if !job_source_set_matches(&mut tx, context, existing_job_id, sources).await? {
                return Err(StateError::Conflict);
            }
            validate_job_sources(&mut tx, context, existing_job_id).await?;
            let row = load_job_tx(&mut tx, context, existing_job_id).await?;
            if row.input_hash != input_hash
                || row.policy_revision != policy_revision
                || row.profile_id != profile_id
            {
                return Err(StateError::Conflict);
            }
            if matches!(
                row.lifecycle_state.as_str(),
                "failed" | "blocked" | "cancelled"
            ) {
                return Err(StateError::Conflict);
            }
            tx.commit().await?;
            return Ok((job_row_to_record(row)?, false));
        }
        let now = now_millis()?;
        for source in sources {
            validate_source_fence(&mut tx, context, source, true).await?;
        }
        let rules_revision =
            crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?;
        let mut source_fingerprint = sources
            .iter()
            .map(|source| {
                format!(
                    "{}:{}:{}:{}:{}:{}",
                    source.source_id,
                    source.source_revision_id,
                    source.content_hash,
                    source.authorization_revision,
                    source.source_generation,
                    source.extraction_commit_sequence
                )
            })
            .collect::<Vec<_>>();
        source_fingerprint.sort();
        let target = crate::semantic_rules::ensure_target_tx(
            &mut tx,
            context,
            "organization_job",
            "vault",
            1,
            &format!("{input_hash}|{}", source_fingerprint.join("|")),
        )
        .await?;
        sqlx::query(
            "INSERT INTO semantic_organization_jobs
             (vault_id,organization_job_id,state,input_hash,policy_revision,profile_id,decision_revision,created_at,semantic_target_key,rules_revision)
             VALUES(?,?, 'running',?,?,?,0,?,?,?)",
        )
        .bind(&vault)
        .bind(id.to_string())
        .bind(input_hash)
        .bind(policy_revision)
        .bind(profile_id)
        .bind(now)
        .bind(&target.target_key)
        .bind(rules_revision)
        .execute(&mut *tx)
        .await?;
        for source in sources {
            sqlx::query(
                "INSERT INTO semantic_organization_job_sources
                 (vault_id,organization_job_id,source_id,source_revision_id,content_hash,
                  authorization_revision,source_generation,extraction_commit_sequence)
                 VALUES(?,?,?,?,?,?,?,?)",
            )
            .bind(&vault)
            .bind(id.to_string())
            .bind(source.source_id.to_string())
            .bind(source.source_revision_id.to_string())
            .bind(&source.content_hash)
            .bind(source.authorization_revision)
            .bind(source.source_generation)
            .bind(source.extraction_commit_sequence)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO semantic_organization_idempotency
             (vault_id,idempotency_key,organization_job_id,request_hash,created_at)
             VALUES(?,?,?,?,?)",
        )
        .bind(&vault)
        .bind(idempotency_key)
        .bind(id.to_string())
        .bind(request_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let row = load_job_tx(&mut tx, context, id).await?;
        tx.commit().await?;
        Ok((job_row_to_record(row)?, true))
    }

    pub async fn record_candidate(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
        candidate: &RelationCandidateInput,
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let vault = context.id().to_string();
        ensure_running_job(&mut tx, context, job_id).await?;
        ensure_job_source(
            &mut tx,
            context,
            job_id,
            candidate.left_source_id,
            candidate.left_source_revision_id,
        )
        .await?;
        ensure_job_source(
            &mut tx,
            context,
            job_id,
            candidate.right_source_id,
            candidate.right_source_revision_id,
        )
        .await?;
        validate_observation_evidence(
            &mut tx,
            context,
            candidate.left_observation_id,
            candidate.left_source_id,
            candidate.left_source_revision_id,
            None,
        )
        .await?;
        validate_observation_evidence(
            &mut tx,
            context,
            candidate.right_observation_id,
            candidate.right_source_id,
            candidate.right_source_revision_id,
            None,
        )
        .await?;
        if candidate.left_observation_id == candidate.right_observation_id {
            return Err(StateError::InvalidInput(
                "relation candidate endpoints must differ",
            ));
        }
        let now = now_millis()?;
        sqlx::query(
            "INSERT INTO semantic_relation_candidates
             (vault_id,relation_candidate_id,organization_job_id,left_observation_id,left_source_id,left_source_revision_id,
              right_observation_id,right_source_id,right_source_revision_id,candidate_input_hash,similarity_hint,state,profile_id,created_at,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,'pending',?,?,?)",
        )
        .bind(&vault)
        .bind(candidate.id.to_string())
        .bind(job_id.to_string())
        .bind(candidate.left_observation_id.to_string())
        .bind(candidate.left_source_id.to_string())
        .bind(candidate.left_source_revision_id.to_string())
        .bind(candidate.right_observation_id.to_string())
        .bind(candidate.right_source_id.to_string())
        .bind(candidate.right_source_revision_id.to_string())
        .bind(&candidate.candidate_input_hash)
        .bind(candidate.similarity_hint)
        .bind(&candidate.profile_id)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn record_action_audits(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
        audits: &[OrganizationActionAuditInput],
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let vault = context.id().to_string();
        ensure_running_job(&mut tx, context, job_id).await?;
        let now = now_millis()?;
        for audit in audits {
            if !matches!(
                audit.action_kind.as_str(),
                "create_composed_card"
                    | "attach_equivalent_evidence"
                    | "add_supported_information"
                    | "supersede_with_evidence"
                    | "record_conflict"
                    | "link_related_only"
                    | "keep_separate_scope"
                    | "no_change"
            ) {
                return Err(StateError::InvalidInput("invalid organization action"));
            }
            for candidate_id in &audit.candidate_ids {
                let exists: Option<i64> = sqlx::query_scalar(
                    "SELECT 1 FROM semantic_relation_candidates
                     WHERE vault_id=? AND organization_job_id=? AND relation_candidate_id=?",
                )
                .bind(&vault)
                .bind(job_id.to_string())
                .bind(candidate_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
                if exists != Some(1) {
                    return Err(StateError::Conflict);
                }
            }
            sqlx::query(
                "INSERT INTO semantic_organization_action_audits
                 (vault_id,organization_job_id,action_ordinal,action_kind,candidate_ids_json,reason,outcome,created_at)
                 VALUES(?,?,?,?,?,?, 'accepted',?)",
            )
            .bind(&vault)
            .bind(job_id.to_string())
            .bind(i64::from(audit.ordinal))
            .bind(&audit.action_kind)
            .bind(serde_json::to_string(
                &audit.candidate_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
            )?)
            .bind(&audit.reason)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn prepare_organization(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
        decisions: &[RelationDecisionInput],
        cards: &[ComposedCardRevisionInput],
    ) -> Result<Vec<OrganizationPreparedSnapshot>, StateError> {
        if cards.is_empty() && decisions.is_empty() {
            let audited: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM semantic_organization_action_audits
                 WHERE vault_id=? AND organization_job_id=?",
            )
            .bind(context.id().to_string())
            .bind(job_id.to_string())
            .fetch_one(&self.pool)
            .await?;
            if audited > 0 {
                // A no-change-only proposal is a successful relation-free job.
            } else {
                return Err(StateError::InvalidInput(
                    "organization requires a card or relation decision",
                ));
            }
        }
        let mut tx = self.pool.begin().await?;
        let vault = context.id().to_string();
        ensure_running_job(&mut tx, context, job_id).await?;
        validate_job_sources(&mut tx, context, job_id).await?;
        let job_rules_revision: i64 = sqlx::query_scalar(
            "SELECT rules_revision FROM semantic_organization_jobs
             WHERE vault_id=? AND organization_job_id=?",
        )
        .bind(&vault)
        .bind(job_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if job_rules_revision
            != crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?
        {
            return Err(StateError::Conflict);
        }
        for decision in decisions {
            insert_relation_decision(&mut tx, context, job_id, decision).await?;
        }
        let mut snapshots = Vec::with_capacity(cards.len());
        for card in cards {
            snapshots.push(insert_composed_card(&mut tx, context, job_id, card).await?);
        }
        // Every discovered candidate must leave the running state with an
        // explicit terminal outcome. Selected candidates become `decided`
        // above; unselected/no_change candidates are deliberately rejected,
        // never left hanging for a later replay to reuse.
        sqlx::query(
            "UPDATE semantic_relation_candidates SET state='rejected',updated_at=?
             WHERE vault_id=? AND organization_job_id=? AND state='pending'",
        )
        .bind(now_millis()?)
        .bind(&vault)
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        let now = now_millis()?;
        sqlx::query(
            "UPDATE semantic_organization_jobs SET state='prepared',lifecycle_state='prepared',completed_at=?
             WHERE vault_id=? AND organization_job_id=? AND state='running'",
        )
        .bind(now)
        .bind(&vault)
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(snapshots)
    }

    /// Record an exact Core write witness. This does not write files; the
    /// future organization service will call it after Core has committed.
    pub async fn mark_snapshot_written(
        &self,
        context: &VaultContext,
        snapshot_id: OrganizationSnapshotId,
        file_id: mcp_vault_domain::FileId,
        revision: mcp_vault_domain::Revision,
        actual_hash: &str,
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let row: Option<SnapshotWitnessRow> = sqlx::query_as(
            "SELECT s.organization_job_id,s.composed_card_revision_id,s.composed_card_id,s.proposed_revision_number,
                    s.expected_file_id,s.expected_file_revision,s.source_fence_hash,s.proposed_file_hash,j.rules_revision
             FROM semantic_organization_snapshots s
             JOIN semantic_organization_jobs j ON j.vault_id=s.vault_id AND j.organization_job_id=s.organization_job_id
             WHERE s.vault_id=? AND s.organization_snapshot_id=? AND s.status IN ('prepared','written')",
        )
        .bind(context.id().to_string())
        .bind(snapshot_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((
            job_id,
            revision_id,
            _card_id,
            proposed_revision,
            expected_file_id,
            expected_file_revision,
            source_fence_hash,
            proposed_hash,
            snapshot_rules_revision,
        )) = row
        else {
            return Err(StateError::Conflict);
        };
        let job_id = OrganizationJobId::parse(&job_id)?;
        if snapshot_rules_revision
            != crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?
        {
            return Err(StateError::Conflict);
        }
        validate_job_sources(&mut tx, context, job_id).await?;
        let revision_number = revision.as_i64()?;
        if revision_number != proposed_revision
            || actual_hash.is_empty()
            || actual_hash != proposed_hash
            || expected_file_id
                .as_deref()
                .is_some_and(|expected| expected != file_id.to_string())
            || expected_file_revision
                .is_some_and(|expected| expected.checked_add(1) != Some(revision_number))
            || source_fence_hash != organization_source_fence_hash(&mut tx, context, job_id).await?
        {
            return Err(StateError::Conflict);
        }
        let now = now_millis()?;
        sqlx::query(
            "UPDATE semantic_organization_snapshots SET status='written',published_file_id=?,published_file_revision=?,actual_file_id=?,actual_file_revision=?,actual_file_hash=?,updated_at=?
             WHERE vault_id=? AND organization_snapshot_id=?",
        )
        .bind(file_id.to_string())
        .bind(revision_number)
        .bind(file_id.to_string())
        .bind(revision_number)
        .bind(actual_hash)
        .bind(now)
        .bind(context.id().to_string())
        .bind(snapshot_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_composed_card_revisions SET canonical_file_id=?,canonical_revision=?
             WHERE vault_id=? AND composed_card_revision_id=? AND publication_state='prepared'",
        )
        .bind(file_id.to_string())
        .bind(revision_number)
        .bind(context.id().to_string())
        .bind(revision_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_organization_jobs SET lifecycle_state='written'
             WHERE vault_id=? AND organization_job_id=? AND lifecycle_state='prepared'",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn mark_job_failed(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
        safe_error_code: &str,
        blocked: bool,
    ) -> Result<(), StateError> {
        let state = if blocked { "blocked" } else { "failed" };
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE semantic_organization_jobs SET state=?,lifecycle_state=?,safe_error_code=?,completed_at=?
             WHERE vault_id=? AND organization_job_id=? AND lifecycle_state NOT IN ('applied','failed','blocked','cancelled')",
        )
        .bind(state)
        .bind(state)
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_organization_snapshots SET status='blocked',safe_error_code=?,updated_at=?
             WHERE vault_id=? AND organization_job_id=? AND status IN ('prepared','written')",
        )
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        let candidate_terminal_state = if blocked { "expired" } else { "rejected" };
        sqlx::query(
            "UPDATE semantic_relation_candidates SET state=?,updated_at=?
             WHERE vault_id=? AND organization_job_id=? AND state IN ('pending','decided')",
        )
        .bind(candidate_terminal_state)
        .bind(now)
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_relation_decisions SET decision_state='blocked'
             WHERE vault_id=? AND relation_candidate_id IN (
                 SELECT relation_candidate_id FROM semantic_relation_candidates
                 WHERE vault_id=? AND organization_job_id=?
             ) AND decision_state='accepted'",
        )
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_organization_action_audits SET outcome='blocked'
             WHERE vault_id=? AND organization_job_id=? AND outcome='accepted'",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Cancel an organization job before its prepared snapshot can reach
    /// Vault Core. Once a job is prepared/written, callers must use the
    /// existing recovery barrier instead of silently discarding a possible
    /// canonical write.
    pub async fn cancel_job(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
        safe_error_code: &str,
    ) -> Result<(), StateError> {
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let lifecycle: Option<String> = sqlx::query_scalar(
            "SELECT lifecycle_state FROM semantic_organization_jobs
             WHERE vault_id=? AND organization_job_id=?",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        match lifecycle.as_deref() {
            Some("cancelled") => {
                tx.commit().await?;
                return Ok(());
            }
            Some("running") => {}
            Some("prepared" | "written") => return Err(StateError::Conflict),
            Some("applied" | "failed" | "blocked") | None => return Err(StateError::Conflict),
            Some(_) => return Err(StateError::IntegrityFailure),
        }
        sqlx::query(
            "UPDATE semantic_organization_jobs
             SET state='cancelled',lifecycle_state='cancelled',safe_error_code=?,completed_at=?
             WHERE vault_id=? AND organization_job_id=? AND lifecycle_state='running'",
        )
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_organization_snapshots SET status='blocked',safe_error_code=?,updated_at=?
             WHERE vault_id=? AND organization_job_id=? AND status IN ('prepared','written')",
        )
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_composed_card_revisions SET publication_state='blocked'
             WHERE vault_id=? AND organization_job_id=? AND publication_state='prepared'",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_relation_candidates SET state='expired',updated_at=?
             WHERE vault_id=? AND organization_job_id=? AND state IN ('pending','decided')",
        )
        .bind(now)
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_relation_decisions SET decision_state='blocked'
             WHERE vault_id=? AND relation_candidate_id IN (
                 SELECT relation_candidate_id FROM semantic_relation_candidates
                 WHERE vault_id=? AND organization_job_id=?
             ) AND decision_state='accepted'",
        )
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_organization_action_audits SET outcome='blocked'
             WHERE vault_id=? AND organization_job_id=? AND outcome='accepted'",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Validate the complete source/rules fence immediately before a Core
    /// recovery or composed-card write. This is intentionally separate from
    /// the later witness/CAS check: known-stale jobs must not enter Core.
    pub async fn validate_job_write_fence(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let job_rules_revision: i64 = sqlx::query_scalar(
            "SELECT rules_revision FROM semantic_organization_jobs
             WHERE vault_id=? AND organization_job_id=?",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if job_rules_revision
            != crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?
        {
            return Err(StateError::Conflict);
        }
        validate_job_sources(&mut tx, context, job_id).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn pending_jobs(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<OrganizationJobId>, StateError> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT organization_job_id FROM semantic_organization_jobs
             WHERE vault_id=? AND (
                 lifecycle_state IN ('running','prepared','written')
                 OR (lifecycle_state='blocked' AND EXISTS (
                     SELECT 1
                     FROM semantic_organization_snapshots s
                     JOIN operation_journal j
                       ON j.vault_id=s.vault_id
                      AND (j.destination_path=s.target_path OR j.source_path=s.target_path)
                      AND j.state IN ('prepared','file_committed','needs_review')
                     WHERE s.vault_id=semantic_organization_jobs.vault_id
                       AND s.organization_job_id=semantic_organization_jobs.organization_job_id
                       AND s.status='blocked'
                 ))
             )
             ORDER BY created_at,organization_job_id",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|value| OrganizationJobId::parse(&value).map_err(StateError::from))
            .collect()
    }

    pub async fn pending_snapshots(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
    ) -> Result<Vec<OrganizationPreparedSnapshot>, StateError> {
        let rows = sqlx::query_as::<_, (
            String, String, String, String, Option<String>, i64, String, String, Vec<u8>, String,
            Option<String>, Option<i64>, Option<String>, Option<i64>, Option<String>, String,
        )>(
            "SELECT organization_snapshot_id,organization_job_id,composed_card_id,composed_card_revision_id,
                    expected_card_revision_id,proposed_revision_number,target_path,proposed_file_hash,canonical_bytes,status,
                    expected_file_id,expected_file_revision,actual_file_id,actual_file_revision,actual_file_hash,source_fence_hash
             FROM semantic_organization_snapshots s
             WHERE vault_id=? AND organization_job_id=?
               AND (status IN ('prepared','written') OR (status='blocked' AND EXISTS (
                   SELECT 1 FROM operation_journal j
                   WHERE j.vault_id=s.vault_id
                     AND (j.destination_path=s.target_path OR j.source_path=s.target_path)
                     AND j.state IN ('prepared','file_committed','needs_review')
               )))
             ORDER BY organization_snapshot_id",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(
                    snapshot_id,
                    job_id,
                    card_id,
                    revision_id,
                    expected,
                    proposed_number,
                    path,
                    hash,
                    bytes,
                    status,
                    expected_file_id,
                    expected_file_revision,
                    actual_file_id,
                    actual_file_revision,
                    actual_file_hash,
                    fence_hash,
                )| {
                    Ok(OrganizationPreparedSnapshot {
                        id: OrganizationSnapshotId::parse(&snapshot_id)?,
                        job_id: OrganizationJobId::parse(&job_id)?,
                        card_id: ComposedCardId::parse(&card_id)?,
                        card_revision_id: ComposedCardRevisionId::parse(&revision_id)?,
                        expected_card_revision_id: expected
                            .as_deref()
                            .map(ComposedCardRevisionId::parse)
                            .transpose()?,
                        proposed_revision_number: u32::try_from(proposed_number)
                            .map_err(|_| StateError::IntegrityFailure)?,
                        target_path: VaultPath::parse(&path)?,
                        proposed_file_hash: hash,
                        canonical_bytes: bytes,
                        status,
                        expected_file_id: expected_file_id
                            .as_deref()
                            .map(mcp_vault_domain::FileId::parse)
                            .transpose()?,
                        expected_file_revision: expected_file_revision
                            .map(mcp_vault_domain::Revision::try_from)
                            .transpose()?,
                        actual_file_id: actual_file_id
                            .as_deref()
                            .map(mcp_vault_domain::FileId::parse)
                            .transpose()?,
                        actual_file_revision: actual_file_revision
                            .map(mcp_vault_domain::Revision::try_from)
                            .transpose()?,
                        actual_file_hash,
                        source_fence_hash: fence_hash,
                    })
                },
            )
            .collect()
    }

    /// Return lifecycle projections for audit tests and recovery diagnostics.
    /// Both queries remain Vault- and job-scoped; no candidate payload is
    /// exposed to callers.
    pub async fn organization_candidate_states(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
    ) -> Result<Vec<String>, StateError> {
        sqlx::query_scalar(
            "SELECT state FROM semantic_relation_candidates
             WHERE vault_id=? AND organization_job_id=? ORDER BY relation_candidate_id",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(StateError::from)
    }

    pub async fn organization_action_outcomes(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
    ) -> Result<Vec<String>, StateError> {
        sqlx::query_scalar(
            "SELECT outcome FROM semantic_organization_action_audits
             WHERE vault_id=? AND organization_job_id=? ORDER BY action_ordinal",
        )
        .bind(context.id().to_string())
        .bind(job_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(StateError::from)
    }

    /// Apply a fully witnessed State projection. Core file I/O remains outside
    /// this repository, while source fences and card CAS are enforced here.
    pub async fn apply_organization(
        &self,
        context: &VaultContext,
        job_id: OrganizationJobId,
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let vault = context.id().to_string();
        let source_rows = sqlx::query_as::<_, (String, String, String, i64, i64, i64, i64)>(
            "SELECT j.source_id,j.source_revision_id,j.content_hash,j.authorization_revision,
                    j.source_generation,j.extraction_commit_sequence,s.eligible
             FROM semantic_organization_job_sources j JOIN semantic_sources s
               ON s.vault_id=j.vault_id AND s.source_id=j.source_id
             WHERE j.vault_id=? AND j.organization_job_id=?",
        )
        .bind(&vault)
        .bind(job_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        if source_rows.is_empty() {
            return Err(StateError::Conflict);
        }
        let job_rules_revision: i64 = sqlx::query_scalar(
            "SELECT rules_revision FROM semantic_organization_jobs
             WHERE vault_id=? AND organization_job_id=?",
        )
        .bind(&vault)
        .bind(job_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if job_rules_revision
            != crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?
        {
            return Err(StateError::Conflict);
        }
        for (source_id, revision_id, content_hash, auth, generation, sequence, eligible) in
            source_rows
        {
            if eligible != 1 {
                return Err(StateError::Conflict);
            }
            validate_source_fence(
                &mut tx,
                context,
                &OrganizationSourceFence {
                    source_id: SemanticSourceId::parse(&source_id)?,
                    source_revision_id: SourceRevisionId::parse(&revision_id)?,
                    content_hash,
                    authorization_revision: auth,
                    source_generation: generation,
                    extraction_commit_sequence: sequence,
                },
                true,
            )
            .await?;
        }
        let snapshots = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<i64>,
                i64,
            ),
        >(
            "SELECT organization_snapshot_id,composed_card_id,composed_card_revision_id,
                    expected_card_revision_id,published_file_id,published_file_revision,rules_revision
             FROM semantic_organization_snapshots
             WHERE vault_id=? AND organization_job_id=? ORDER BY organization_snapshot_id",
        )
        .bind(&vault)
        .bind(job_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        if sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM semantic_organization_snapshots
                 WHERE vault_id=? AND organization_job_id=? AND status<>'written'",
        )
        .bind(&vault)
        .bind(job_id.to_string())
        .fetch_one(&mut *tx)
        .await?
            != 0
        {
            if snapshots.is_empty() {
                sqlx::query(
                    "UPDATE semantic_organization_jobs SET state='applied',lifecycle_state='applied',completed_at=?
                     WHERE vault_id=? AND organization_job_id=? AND state='prepared'",
                )
                .bind(now_millis()?)
                .bind(&vault)
                .bind(job_id.to_string())
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(());
            }
            return Err(StateError::Conflict);
        }
        for (snapshot_id, card_id, revision_id, expected, file_id, file_revision, rules_revision) in
            &snapshots
        {
            if *rules_revision != job_rules_revision {
                return Err(StateError::Conflict);
            }
            if file_id.is_none() || file_revision.is_none() {
                return Err(StateError::Conflict);
            }
            let current: Option<(Option<String>,)> = sqlx::query_as(
                "SELECT current_revision_id FROM semantic_composed_cards
                 WHERE vault_id=? AND composed_card_id=?",
            )
            .bind(&vault)
            .bind(card_id)
            .fetch_optional(&mut *tx)
            .await?;
            if current.and_then(|(value,)| value) != *expected {
                return Err(StateError::Conflict);
            }
            let result = sqlx::query(
                "UPDATE semantic_composed_cards SET current_revision_id=?,eligibility='readable',updated_at=?
                 WHERE vault_id=? AND composed_card_id=? AND current_revision_id IS ?",
            )
            .bind(revision_id)
            .bind(now_millis()?)
            .bind(&vault)
            .bind(card_id)
            .bind(expected)
            .execute(&mut *tx)
            .await?
            ;
            if result.rows_affected() != 1 {
                return Err(StateError::Conflict);
            }
            sqlx::query(
                "UPDATE semantic_composed_card_revisions SET publication_state='published'
                 WHERE vault_id=? AND composed_card_revision_id=? AND publication_state='prepared'",
            )
            .bind(&vault)
            .bind(revision_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE semantic_organization_snapshots SET status='applied',updated_at=?
                 WHERE vault_id=? AND organization_snapshot_id=? AND status='written'",
            )
            .bind(now_millis()?)
            .bind(&vault)
            .bind(snapshot_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE semantic_organization_jobs SET state='applied',lifecycle_state='applied',completed_at=?
             WHERE vault_id=? AND organization_job_id=? AND state='prepared'",
        )
        .bind(now_millis()?)
        .bind(&vault)
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_job(
        &self,
        context: &VaultContext,
        id: OrganizationJobId,
    ) -> Result<Option<OrganizationJobRecord>, StateError> {
        let row = sqlx::query_as::<_, JobRow>(
            "SELECT vault_id,organization_job_id,state,lifecycle_state,input_hash,policy_revision,profile_id,decision_revision,safe_error_code,semantic_target_key,rules_revision
             FROM semantic_organization_jobs WHERE vault_id=? AND organization_job_id=?",
        )
        .bind(context.id().to_string())
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(job_row_to_record).transpose()
    }

    pub async fn list_jobs(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<OrganizationJobRecord>, StateError> {
        if limit == 0 || limit > 200 {
            return Err(StateError::InvalidInput(
                "organization job limit must be between 1 and 200",
            ));
        }
        let rows = sqlx::query_as::<_, JobRow>(
            "SELECT vault_id,organization_job_id,state,lifecycle_state,input_hash,policy_revision,profile_id,decision_revision,safe_error_code,semantic_target_key,rules_revision
             FROM semantic_organization_jobs WHERE vault_id=? ORDER BY created_at,organization_job_id LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(job_row_to_record).collect()
    }

    pub async fn get_composed_card(
        &self,
        context: &VaultContext,
        id: ComposedCardId,
    ) -> Result<Option<ComposedCardRecord>, StateError> {
        let row = card_header(&self.pool, context, Some(id)).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if !card_is_currently_readable(&self.pool, context, id).await? {
            return Ok(None);
        }
        Ok(Some(build_card_record(&self.pool, row).await?))
    }

    pub async fn current_composed_card_identity(
        &self,
        context: &VaultContext,
        identity_key: &str,
    ) -> Result<Option<(ComposedCardId, ComposedCardRevisionId, u32)>, StateError> {
        let row: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT c.composed_card_id,c.current_revision_id,r.revision_number
             FROM semantic_composed_cards c
             JOIN semantic_composed_card_revisions r
               ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
             WHERE c.vault_id=? AND c.identity_key=? AND r.publication_state='published'",
        )
        .bind(context.id().to_string())
        .bind(identity_key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(card_id, revision_id, revision_number)| {
            Ok((
                ComposedCardId::parse(&card_id)?,
                ComposedCardRevisionId::parse(&revision_id)?,
                u32::try_from(revision_number).map_err(|_| StateError::IntegrityFailure)?,
            ))
        })
        .transpose()
    }

    pub async fn list_composed_cards(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<ComposedCardRecord>, StateError> {
        if limit == 0 || limit > 200 {
            return Err(StateError::InvalidInput(
                "composed card limit must be between 1 and 200",
            ));
        }
        let rows = sqlx::query_as::<_, CardHeaderRow>(
            "SELECT c.vault_id,c.composed_card_id,r.composed_card_revision_id,r.title,r.kind,r.scope_ref,
                    r.assertion_status,r.temporal_scope_json,r.revision_number,r.canonical_path,
                    r.canonical_file_id,r.canonical_revision,r.canonical_markdown_hash
             FROM semantic_composed_cards c JOIN semantic_composed_card_revisions r
               ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
             WHERE c.vault_id=? AND c.eligibility='readable' AND r.publication_state='published'
               AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                 WHERE x.vault_id=r.vault_id AND x.active=1
                   AND x.action IN ('suppress_read','forget_current')
                   AND (
                     (x.semantic_target_key=r.semantic_target_key
                       AND x.source_id IS NULL
                       AND (x.composed_card_id IS NULL OR x.composed_card_id=c.composed_card_id))
                   ))
               AND NOT EXISTS (SELECT 1 FROM semantic_task_states t
                 WHERE t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key AND t.state='completed')
             ORDER BY r.title COLLATE NOCASE,c.composed_card_id LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        let mut result = Vec::new();
        for row in rows {
            let id = ComposedCardId::parse(&row.composed_card_id)?;
            if card_is_currently_readable(&self.pool, context, id).await? {
                result.push(build_card_record(&self.pool, row).await?);
            }
        }
        Ok(result)
    }
}

async fn load_job_tx(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    id: OrganizationJobId,
) -> Result<JobRow, StateError> {
    sqlx::query_as::<_, JobRow>(
        "SELECT vault_id,organization_job_id,state,lifecycle_state,input_hash,policy_revision,profile_id,decision_revision,safe_error_code,semantic_target_key,rules_revision
         FROM semantic_organization_jobs WHERE vault_id=? AND organization_job_id=?",
    )
    .bind(context.id().to_string())
    .bind(id.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(StateError::from)
}

async fn ensure_running_job(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    job_id: OrganizationJobId,
) -> Result<(), StateError> {
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM semantic_organization_jobs WHERE vault_id=? AND organization_job_id=?",
    )
    .bind(context.id().to_string())
    .bind(job_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    if state.as_deref() != Some("running") {
        return Err(StateError::Conflict);
    }
    Ok(())
}

type SourceFenceRow = (Option<String>, String, i64, i64, i64, i64, i64);

async fn validate_source_fence(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    fence: &OrganizationSourceFence,
    require_readable: bool,
) -> Result<(), StateError> {
    let row: Option<SourceFenceRow> = sqlx::query_as(
        "SELECT s.current_revision_id,s.current_content_hash,s.authorization_revision,s.source_generation,
                s.extraction_commit_sequence,s.eligible,s.pending_rebuild
         FROM semantic_sources s JOIN semantic_source_revisions r
           ON r.vault_id=s.vault_id AND r.source_revision_id=? AND r.source_id=s.source_id
         WHERE s.vault_id=? AND s.source_id=? AND r.content_hash=?",
    )
    .bind(fence.source_revision_id.to_string())
    .bind(context.id().to_string())
    .bind(fence.source_id.to_string())
    .bind(&fence.content_hash)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((current_revision, content_hash, auth, generation, sequence, eligible, pending)) = row
    else {
        return Err(StateError::Conflict);
    };
    if current_revision.as_deref() != Some(&fence.source_revision_id.to_string())
        || content_hash != fence.content_hash
        || auth != fence.authorization_revision
        || generation != fence.source_generation
        || sequence != fence.extraction_commit_sequence
        || (require_readable && (eligible != 1 || pending != 0))
    {
        return Err(StateError::Conflict);
    }
    Ok(())
}

async fn ensure_job_source(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    job_id: OrganizationJobId,
    source_id: SemanticSourceId,
    revision_id: SourceRevisionId,
) -> Result<(), StateError> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM semantic_organization_job_sources
         WHERE vault_id=? AND organization_job_id=? AND source_id=? AND source_revision_id=?",
    )
    .bind(context.id().to_string())
    .bind(job_id.to_string())
    .bind(source_id.to_string())
    .bind(revision_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    if exists != Some(1) {
        return Err(StateError::Conflict);
    }
    Ok(())
}

async fn validate_job_sources(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    job_id: OrganizationJobId,
) -> Result<(), StateError> {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
        "SELECT source_id,source_revision_id,content_hash,authorization_revision,source_generation,extraction_commit_sequence
         FROM semantic_organization_job_sources WHERE vault_id=? AND organization_job_id=?",
    )
    .bind(context.id().to_string())
    .bind(job_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Err(StateError::Conflict);
    }
    for (
        source_id,
        revision_id,
        content_hash,
        authorization_revision,
        source_generation,
        sequence,
    ) in rows
    {
        validate_source_fence(
            tx,
            context,
            &OrganizationSourceFence {
                source_id: SemanticSourceId::parse(&source_id)?,
                source_revision_id: SourceRevisionId::parse(&revision_id)?,
                content_hash,
                authorization_revision,
                source_generation,
                extraction_commit_sequence: sequence,
            },
            true,
        )
        .await?;
    }
    Ok(())
}

async fn job_source_set_matches(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    job_id: OrganizationJobId,
    sources: &[OrganizationSourceFence],
) -> Result<bool, StateError> {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
        "SELECT source_id,source_revision_id,content_hash,authorization_revision,source_generation,extraction_commit_sequence
         FROM semantic_organization_job_sources WHERE vault_id=? AND organization_job_id=? ORDER BY source_id",
    )
    .bind(context.id().to_string())
    .bind(job_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    let mut expected = sources
        .iter()
        .map(|source| {
            (
                source.source_id.to_string(),
                source.source_revision_id.to_string(),
                source.content_hash.clone(),
                source.authorization_revision,
                source.source_generation,
                source.extraction_commit_sequence,
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    Ok(rows == expected)
}

async fn validate_observation_evidence(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    observation_id: ObservationId,
    source_id: SemanticSourceId,
    revision_id: SourceRevisionId,
    evidence_id: Option<EvidenceRefId>,
) -> Result<(), StateError> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM semantic_observations o
         JOIN semantic_source_revisions r ON r.vault_id=o.vault_id AND r.source_revision_id=o.source_revision_id AND r.source_id=o.source_id
         WHERE o.vault_id=? AND o.observation_id=? AND o.source_id=? AND o.source_revision_id=?",
    )
    .bind(context.id().to_string())
    .bind(observation_id.to_string())
    .bind(source_id.to_string())
    .bind(revision_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    if exists != Some(1) {
        return Err(StateError::Conflict);
    }
    let suppressed: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM semantic_suppressions
         WHERE vault_id=? AND active=1 AND action IN ('suppress_read','forget_current')
           AND composed_card_id IS NULL
           AND (
             (card_id IS NULL
               AND source_id=?
               AND (source_revision_id IS NULL OR source_revision_id=?)
               AND (observation_id IS NULL OR observation_id=?))
             OR (card_id IS NOT NULL
               AND (source_id IS NULL OR source_id=?)
               AND (source_revision_id IS NULL OR source_revision_id=?)
               AND (observation_id IS NULL OR observation_id=?)
               AND EXISTS (
                 SELECT 1 FROM semantic_memory_cards mc
                 JOIN semantic_card_revisions mr
                   ON mr.vault_id=mc.vault_id AND mr.card_revision_id=mc.current_revision_id
                 JOIN semantic_card_items mi
                   ON mi.vault_id=mr.vault_id AND mi.card_revision_id=mr.card_revision_id
                 WHERE mc.vault_id=? AND mc.card_id=semantic_suppressions.card_id
                   AND mc.source_id=?
                   AND mr.source_revision_id=?
                   AND mi.observation_id=?))
           )
         LIMIT 1",
    )
    .bind(context.id().to_string())
    .bind(source_id.to_string())
    .bind(revision_id.to_string())
    .bind(observation_id.to_string())
    .bind(source_id.to_string())
    .bind(revision_id.to_string())
    .bind(observation_id.to_string())
    .bind(context.id().to_string())
    .bind(source_id.to_string())
    .bind(revision_id.to_string())
    .bind(observation_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    if suppressed == Some(1) {
        return Err(StateError::Conflict);
    }
    if let Some(evidence_id) = evidence_id {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM semantic_observation_evidence oe
             JOIN semantic_evidence_refs e ON e.vault_id=oe.vault_id AND e.evidence_ref_id=oe.evidence_ref_id AND e.source_id=oe.source_id
             WHERE oe.vault_id=? AND oe.observation_id=? AND oe.source_id=? AND oe.evidence_ref_id=?
               AND e.source_revision_id=?",
        )
        .bind(context.id().to_string())
        .bind(observation_id.to_string())
        .bind(source_id.to_string())
        .bind(evidence_id.to_string())
        .bind(revision_id.to_string())
        .fetch_optional(&mut **tx)
        .await?;
        if exists != Some(1) {
            return Err(StateError::Conflict);
        }
    }
    Ok(())
}

async fn insert_relation_decision(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    job_id: OrganizationJobId,
    decision: &RelationDecisionInput,
) -> Result<(), StateError> {
    if !matches!(
        decision.relation_kind.as_str(),
        "equivalent" | "supplements" | "different_scope" | "conflicts" | "supersedes" | "related"
    ) || !matches!(
        decision.decision_state.as_str(),
        "accepted" | "rejected" | "stale" | "blocked"
    ) {
        return Err(StateError::InvalidInput("invalid relation decision"));
    }
    let candidate: Option<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT left_observation_id,left_source_id,left_source_revision_id,right_observation_id,right_source_id,right_source_revision_id
         FROM semantic_relation_candidates WHERE vault_id=? AND relation_candidate_id=? AND organization_job_id=? AND state='pending'",
    )
    .bind(context.id().to_string())
    .bind(decision.candidate_id.to_string())
    .bind(job_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((left_obs, left_source, left_revision, right_obs, right_source, right_revision)) =
        candidate
    else {
        return Err(StateError::Conflict);
    };
    let now = now_millis()?;
    sqlx::query(
        "INSERT INTO semantic_relation_decisions
         (vault_id,relation_decision_id,relation_candidate_id,decision_revision,relation_kind,decision_state,decision_reason_code,profile_id,created_at)
         VALUES(?,?,?,1,?,?,?,?,?)",
    )
    .bind(context.id().to_string())
    .bind(decision.id.to_string())
    .bind(decision.candidate_id.to_string())
    .bind(&decision.relation_kind)
    .bind(&decision.decision_state)
    .bind(&decision.decision_reason_code)
    .bind(&decision.profile_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    for evidence in &decision.evidence {
        if !matches!(evidence.role.as_str(), "left" | "right" | "decision") {
            return Err(StateError::InvalidInput("invalid relation evidence role"));
        }
        let expected = match evidence.role.as_str() {
            "left" => (&left_source, &left_revision),
            "right" => (&right_source, &right_revision),
            _ => (&left_source, &left_revision),
        };
        if evidence.source_id.to_string() != *expected.0
            || evidence.source_revision_id.to_string() != *expected.1
        {
            return Err(StateError::Conflict);
        }
        validate_observation_evidence(
            tx,
            context,
            if evidence.role == "right" {
                ObservationId::parse(&right_obs)?
            } else {
                ObservationId::parse(&left_obs)?
            },
            evidence.source_id,
            evidence.source_revision_id,
            Some(evidence.evidence_ref_id),
        )
        .await?;
        sqlx::query(
            "INSERT INTO semantic_relation_evidence
             (vault_id,relation_decision_id,evidence_ref_id,source_id,source_revision_id,role)
             VALUES(?,?,?,?,?,?)",
        )
        .bind(context.id().to_string())
        .bind(decision.id.to_string())
        .bind(evidence.evidence_ref_id.to_string())
        .bind(evidence.source_id.to_string())
        .bind(evidence.source_revision_id.to_string())
        .bind(&evidence.role)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        "UPDATE semantic_relation_candidates SET state='decided',updated_at=?
         WHERE vault_id=? AND relation_candidate_id=? AND state='pending'",
    )
    .bind(now)
    .bind(context.id().to_string())
    .bind(decision.candidate_id.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_composed_card(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    job_id: OrganizationJobId,
    card: &ComposedCardRevisionInput,
) -> Result<OrganizationPreparedSnapshot, StateError> {
    if card.canonical_bytes.is_empty()
        || hash_bytes(&card.canonical_bytes) != card.canonical_markdown_hash
    {
        return Err(StateError::InvalidInput(
            "composed card bytes and hash do not match",
        ));
    }
    if card.items.is_empty() || card.dependencies.is_empty() {
        return Err(StateError::InvalidInput(
            "composed card items and dependencies are required",
        ));
    }
    let vault = context.id().to_string();
    let rules_revision = crate::semantic_rules::current_rules_revision_tx(tx, context).await?;
    let semantic_fingerprint = composed_card_fingerprint(tx, context, card).await?;
    let existing_target: Option<(String, String)> = sqlx::query_as(
        "SELECT r.semantic_target_key,t.fingerprint FROM semantic_composed_cards c
         JOIN semantic_composed_card_revisions r ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
         JOIN semantic_targets t ON t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key
         WHERE c.vault_id=? AND c.composed_card_id=? AND r.semantic_target_key<>''",
    )
    .bind(&vault)
    .bind(card.card_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let target_key = if let Some((existing, existing_fingerprint)) = existing_target {
        if existing_fingerprint == semantic_fingerprint {
            existing
        } else {
            crate::semantic_rules::ensure_target_tx(
                tx,
                context,
                "composed_card",
                &card.scope_ref,
                2,
                &semantic_fingerprint,
            )
            .await?
            .target_key
        }
    } else {
        crate::semantic_rules::ensure_target_tx(
            tx,
            context,
            "composed_card",
            &card.scope_ref,
            2,
            &semantic_fingerprint,
        )
        .await?
        .target_key
    };
    for (source_id, revision_id) in &card.dependencies {
        ensure_job_source(tx, context, job_id, *source_id, *revision_id).await?;
    }
    let dependencies = card
        .dependencies
        .iter()
        .map(|(source_id, revision_id)| (source_id.to_string(), revision_id.to_string()))
        .collect::<Vec<_>>();
    let observations = card
        .items
        .iter()
        .flat_map(|item| item.support_groups.iter())
        .flat_map(|group| group.members.iter())
        .map(|member| member.observation_id.to_string())
        .collect::<Vec<_>>();
    if crate::semantic_rules::target_blocks_generation(
        tx,
        context,
        "composed_card",
        &target_key,
        &card.scope_ref,
        &dependencies,
        &observations,
        None,
        Some(&card.card_id.to_string()),
        std::str::from_utf8(&card.canonical_bytes).unwrap_or_default(),
    )
    .await?
    {
        return Err(StateError::Conflict);
    }
    let current: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT current_revision_id FROM semantic_composed_cards
         WHERE vault_id=? AND composed_card_id=? AND identity_key=?",
    )
    .bind(&vault)
    .bind(card.card_id.to_string())
    .bind(&card.identity_key)
    .fetch_optional(&mut **tx)
    .await?;
    let current_is_none = current.is_none();
    let current_revision = current.and_then(|(value,)| value);
    if current_revision.as_deref()
        != card
            .expected_card_revision_id
            .map(|id| id.to_string())
            .as_deref()
    {
        return Err(StateError::Conflict);
    }
    let expected_revision_number = if let Some(expected) = card.expected_card_revision_id {
        let prior: i64 = sqlx::query_scalar(
            "SELECT revision_number FROM semantic_composed_card_revisions
             WHERE vault_id=? AND composed_card_revision_id=? AND composed_card_id=? AND publication_state='published'",
        )
        .bind(&vault)
        .bind(expected.to_string())
        .bind(card.card_id.to_string())
        .fetch_one(&mut **tx)
        .await?;
        u32::try_from(prior)
            .map_err(|_| StateError::IntegrityFailure)?
            .checked_add(1)
            .ok_or(StateError::Conflict)?
    } else {
        1
    };
    if card.revision_number != expected_revision_number {
        return Err(StateError::Conflict);
    }
    let expected_file: (Option<String>, Option<i64>) = if let Some(expected) =
        card.expected_card_revision_id
    {
        sqlx::query_as(
            "SELECT canonical_file_id,canonical_revision FROM semantic_composed_card_revisions
             WHERE vault_id=? AND composed_card_revision_id=? AND composed_card_id=? AND publication_state='published'",
        )
        .bind(&vault)
        .bind(expected.to_string())
        .bind(card.card_id.to_string())
        .fetch_one(&mut **tx)
        .await?
    } else {
        (None, None)
    };
    if current_is_none {
        sqlx::query(
            "INSERT INTO semantic_composed_cards
             (vault_id,composed_card_id,identity_key,topic_key,kind,scope_ref,current_revision_id,eligibility,created_at,updated_at)
             VALUES(?,?,?,?,?,?,NULL,'pending',?,?)",
        )
        .bind(&vault)
        .bind(card.card_id.to_string())
        .bind(&card.identity_key)
        .bind(&card.topic_key)
        .bind(&card.kind)
        .bind(&card.scope_ref)
        .bind(now_millis()?)
        .bind(now_millis()?)
        .execute(&mut **tx)
        .await?;
    }
    let now = now_millis()?;
    sqlx::query(
        "INSERT INTO semantic_composed_card_revisions
         (vault_id,composed_card_revision_id,composed_card_id,organization_job_id,revision_number,title,kind,scope_ref,assertion_status,temporal_scope_json,composition_profile_id,canonical_markdown_hash,canonical_path,publication_state,created_at,semantic_target_key,rules_revision)
         VALUES(?,?,?,?,?,?,?,?,?,?,?, ?,?,'prepared',?,?,?)",
    )
    .bind(&vault)
    .bind(card.card_revision_id.to_string())
    .bind(card.card_id.to_string())
    .bind(job_id.to_string())
    .bind(i64::from(card.revision_number))
    .bind(&card.title)
    .bind(&card.kind)
    .bind(&card.scope_ref)
    .bind(&card.assertion_status)
    .bind(serde_json::to_string(&card.temporal_scope)?)
    .bind(&card.composition_profile_id)
    .bind(&card.canonical_markdown_hash)
    .bind(card.canonical_path.as_str())
    .bind(now)
    .bind(&target_key)
    .bind(rules_revision)
    .execute(&mut **tx)
    .await?;
    for (source_id, revision_id) in &card.dependencies {
        sqlx::query(
            "INSERT INTO semantic_composed_card_dependencies
             (vault_id,composed_card_revision_id,source_id,source_revision_id) VALUES(?,?,?,?)",
        )
        .bind(&vault)
        .bind(card.card_revision_id.to_string())
        .bind(source_id.to_string())
        .bind(revision_id.to_string())
        .execute(&mut **tx)
        .await?;
    }
    for item in &card.items {
        sqlx::query(
            "INSERT INTO semantic_composed_card_items
             (vault_id,composed_card_item_id,composed_card_revision_id,item_kind,ordinal,content) VALUES(?,?,?,?,?,?)",
        )
        .bind(&vault)
        .bind(item.id.to_string())
        .bind(card.card_revision_id.to_string())
        .bind(&item.kind)
        .bind(i64::from(item.ordinal))
        .bind(&item.content)
        .execute(&mut **tx)
        .await?;
        for group in &item.support_groups {
            if group.members.is_empty() || !matches!(group.operator.as_str(), "and" | "or") {
                return Err(StateError::InvalidInput("invalid composed support group"));
            }
            sqlx::query(
                "INSERT INTO semantic_composed_support_groups
                 (vault_id,support_group_id,composed_card_item_id,operator,ordinal) VALUES(?,?,?,?,?)",
            )
            .bind(&vault)
            .bind(group.id.to_string())
            .bind(item.id.to_string())
            .bind(&group.operator)
            .bind(i64::from(group.ordinal))
            .execute(&mut **tx)
            .await?;
            for member in &group.members {
                ensure_job_source(
                    tx,
                    context,
                    job_id,
                    member.source_id,
                    member.source_revision_id,
                )
                .await?;
                validate_observation_evidence(
                    tx,
                    context,
                    member.observation_id,
                    member.source_id,
                    member.source_revision_id,
                    Some(member.evidence_ref_id),
                )
                .await?;
                sqlx::query(
                    "INSERT INTO semantic_composed_support_members
                     (vault_id,support_member_id,support_group_id,source_id,source_revision_id,observation_id,evidence_ref_id,member_role)
                     VALUES(?,?,?,?,?,?,?,?)",
                )
                .bind(&vault)
                .bind(member.id.to_string())
                .bind(group.id.to_string())
                .bind(member.source_id.to_string())
                .bind(member.source_revision_id.to_string())
                .bind(member.observation_id.to_string())
                .bind(member.evidence_ref_id.to_string())
                .bind(&member.member_role)
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    let snapshot_id = OrganizationSnapshotId::new();
    let source_fence_hash = organization_source_fence_hash(tx, context, job_id).await?;
    sqlx::query(
        "INSERT INTO semantic_organization_snapshots
         (vault_id,organization_snapshot_id,organization_job_id,composed_card_id,composed_card_revision_id,expected_card_revision_id,proposed_revision_number,target_path,proposed_file_hash,canonical_bytes,status,created_at,updated_at,expected_file_id,expected_file_revision,source_fence_hash,semantic_target_key,rules_revision)
         VALUES(?,?,?,?,?,?,?,?,?,?,'prepared',?,?,?,?,?,?,?)",
    )
    .bind(&vault)
    .bind(snapshot_id.to_string())
    .bind(job_id.to_string())
    .bind(card.card_id.to_string())
    .bind(card.card_revision_id.to_string())
    .bind(card.expected_card_revision_id.map(|id| id.to_string()))
    .bind(i64::from(card.revision_number))
    .bind(card.canonical_path.as_str())
    .bind(&card.canonical_markdown_hash)
    .bind(&card.canonical_bytes)
    .bind(now)
    .bind(now)
    .bind(expected_file.0.as_deref())
    .bind(expected_file.1)
    .bind(&source_fence_hash)
    .bind(&target_key)
    .bind(rules_revision)
    .execute(&mut **tx)
    .await?;
    Ok(OrganizationPreparedSnapshot {
        id: snapshot_id,
        job_id,
        card_id: card.card_id,
        card_revision_id: card.card_revision_id,
        expected_card_revision_id: card.expected_card_revision_id,
        proposed_revision_number: card.revision_number,
        target_path: card.canonical_path.clone(),
        proposed_file_hash: card.canonical_markdown_hash.clone(),
        canonical_bytes: card.canonical_bytes.clone(),
        status: "prepared".to_owned(),
        expected_file_id: expected_file
            .0
            .as_deref()
            .map(mcp_vault_domain::FileId::parse)
            .transpose()?,
        expected_file_revision: expected_file
            .1
            .map(mcp_vault_domain::Revision::try_from)
            .transpose()?,
        actual_file_id: None,
        actual_file_revision: None,
        actual_file_hash: None,
        source_fence_hash,
    })
}

async fn organization_source_fence_hash(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    job_id: OrganizationJobId,
) -> Result<String, StateError> {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
        "SELECT source_id,source_revision_id,content_hash,authorization_revision,source_generation,extraction_commit_sequence
         FROM semantic_organization_job_sources WHERE vault_id=? AND organization_job_id=? ORDER BY source_id",
    )
    .bind(context.id().to_string())
    .bind(job_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    let material = rows
        .into_iter()
        .map(|(source, revision, hash, auth, generation, sequence)| {
            format!("{source}:{revision}:{hash}:{auth}:{generation}:{sequence}")
        })
        .collect::<Vec<_>>()
        .join("|");
    Ok(hash_bytes(material.as_bytes()))
}

async fn composed_card_fingerprint(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    card: &ComposedCardRevisionInput,
) -> Result<String, StateError> {
    let mut members = Vec::new();
    for item in &card.items {
        for group in &item.support_groups {
            for member in &group.members {
                members.push((
                    member.observation_id,
                    member.source_id,
                    member.evidence_ref_id,
                    item.kind.clone(),
                    item.ordinal,
                    item.content.clone(),
                    group.operator.clone(),
                    group.ordinal,
                    member.member_role.clone(),
                ));
            }
        }
    }
    members.sort_by_key(
        |(_, source_id, evidence_id, item_kind, item_ordinal, _, operator, group_ordinal, role)| {
            (
                source_id.to_string(),
                evidence_id.to_string(),
                item_kind.clone(),
                *item_ordinal,
                operator.clone(),
                *group_ordinal,
                role.clone(),
            )
        },
    );
    let mut parts = Vec::new();
    for (
        observation_id,
        source_id,
        evidence_id,
        item_kind,
        item_ordinal,
        item_content,
        group_operator,
        group_ordinal,
        member_role,
    ) in members
    {
        let row: ComposedFingerprintObservation = sqlx::query_as(
            "SELECT kind,statement,scope,assertion_status,conditions_json,exceptions_json,
                        ordered_steps_json,source_time_scope_json,result_json,uncertainty_json
                 FROM semantic_observations
                 WHERE vault_id=? AND observation_id=? AND source_id=?",
        )
        .bind(context.id().to_string())
        .bind(observation_id.to_string())
        .bind(source_id.to_string())
        .fetch_one(&mut **tx)
        .await?;
        parts.push(
            serde_json::json!([
                source_id.to_string(),
                card.scope_ref.clone(),
                card.assertion_status.clone(),
                item_kind,
                item_ordinal,
                item_content,
                group_operator,
                group_ordinal,
                member_role,
                row.0,
                row.1,
                row.2,
                row.3,
                row.4,
                row.5,
                row.6,
                row.7,
                row.8.unwrap_or_default(),
                row.9.unwrap_or_default(),
            ])
            .to_string(),
        );
        let span_identity = sqlx::query_as::<_, (String, i64, String)>(
            "SELECT span_role,ordinal,content_hash FROM semantic_evidence_spans
             WHERE vault_id=? AND evidence_ref_id=? ORDER BY span_role,ordinal",
        )
        .bind(context.id().to_string())
        .bind(evidence_id.to_string())
        .fetch_all(&mut **tx)
        .await?;
        if let Some(last) = parts.last_mut() {
            last.push('|');
            last.push_str(
                &span_identity
                    .into_iter()
                    .map(|(role, ordinal, hash)| format!("{role}:{ordinal}:{hash}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
    }
    parts.sort();
    Ok(parts.join("||"))
}

async fn card_header(
    pool: &SqlitePool,
    context: &VaultContext,
    id: Option<ComposedCardId>,
) -> Result<Option<CardHeaderRow>, StateError> {
    let query = "SELECT c.vault_id,c.composed_card_id,r.composed_card_revision_id,r.title,r.kind,r.scope_ref,
                        r.assertion_status,r.temporal_scope_json,r.revision_number,r.canonical_path,
                        r.canonical_file_id,r.canonical_revision,r.canonical_markdown_hash
                 FROM semantic_composed_cards c JOIN semantic_composed_card_revisions r
                   ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
                 WHERE c.vault_id=? AND c.composed_card_id=? AND c.eligibility='readable'
                   AND r.publication_state='published'
                   AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                     WHERE x.vault_id=r.vault_id AND x.active=1
                       AND x.action IN ('suppress_read','forget_current')
                       AND (
                         (x.semantic_target_key=r.semantic_target_key
                           AND x.source_id IS NULL
                           AND (x.composed_card_id IS NULL OR x.composed_card_id=c.composed_card_id))
                         OR (x.composed_card_id=c.composed_card_id AND x.source_id IS NULL)
                       ))
                   AND NOT EXISTS (SELECT 1 FROM semantic_task_states t
                     WHERE t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key AND t.state='completed')";
    sqlx::query_as::<_, CardHeaderRow>(query)
        .bind(context.id().to_string())
        .bind(id.map(|value| value.to_string()).unwrap_or_default())
        .fetch_optional(pool)
        .await
        .map_err(StateError::from)
}

pub(crate) async fn card_is_currently_readable(
    pool: &SqlitePool,
    context: &VaultContext,
    id: ComposedCardId,
) -> Result<bool, StateError> {
    let revision: Option<String> = sqlx::query_scalar(
        "SELECT current_revision_id FROM semantic_composed_cards
         WHERE vault_id=? AND composed_card_id=? AND eligibility='readable'",
    )
    .bind(context.id().to_string())
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    let Some(revision) = revision else {
        return Ok(false);
    };
    let hidden: Option<i64> = sqlx::query_scalar(
        "SELECT 1
         FROM semantic_composed_cards c
         JOIN semantic_composed_card_revisions r
           ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
         WHERE c.vault_id=? AND c.composed_card_id=?
           AND (EXISTS (SELECT 1 FROM semantic_suppressions x
                  WHERE x.vault_id=r.vault_id AND x.active=1
                    AND x.action IN ('suppress_read','forget_current')
                    AND ((x.semantic_target_key=r.semantic_target_key
                          AND x.source_id IS NULL
                          AND (x.composed_card_id IS NULL OR x.composed_card_id=c.composed_card_id))
                         OR (x.composed_card_id=c.composed_card_id AND x.source_id IS NULL)))
                OR EXISTS (SELECT 1 FROM semantic_task_states t
                  WHERE t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key
                    AND t.state='completed'))",
    )
    .bind(context.id().to_string())
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    if hidden == Some(1) {
        return Ok(false);
    }
    let groups = sqlx::query_as::<_, (String, String)>(
        "SELECT support_group_id,operator FROM semantic_composed_support_groups g
         JOIN semantic_composed_card_items i ON i.vault_id=g.vault_id AND i.composed_card_item_id=g.composed_card_item_id
         WHERE g.vault_id=? AND i.composed_card_revision_id=?",
    )
    .bind(context.id().to_string())
    .bind(&revision)
    .fetch_all(pool)
    .await?;
    if groups.is_empty() {
        return Ok(false);
    }
    for (group_id, operator) in groups {
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM semantic_composed_support_members WHERE vault_id=? AND support_group_id=?",
        )
        .bind(context.id().to_string())
        .bind(&group_id)
        .fetch_one(pool)
        .await?;
        let valid: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM semantic_composed_support_members m
             JOIN semantic_sources s ON s.vault_id=m.vault_id AND s.source_id=m.source_id
             JOIN semantic_observations o
               ON o.vault_id=m.vault_id AND o.observation_id=m.observation_id
              AND o.source_id=m.source_id AND o.source_revision_id=m.source_revision_id
             JOIN semantic_extraction_sets es
               ON es.vault_id=o.vault_id AND es.extraction_set_id=o.extraction_set_id
              AND es.source_id=o.source_id
             JOIN semantic_evidence_refs e ON e.vault_id=m.vault_id AND e.evidence_ref_id=m.evidence_ref_id
             WHERE m.vault_id=? AND m.support_group_id=? AND s.eligible=1 AND s.pending_rebuild=0
               AND s.current_revision_id=m.source_revision_id
               AND e.source_id=m.source_id AND e.source_revision_id=m.source_revision_id
               AND e.validation_status='validated'
               AND es.state='success_nonempty'
               AND es.extraction_commit_sequence=s.extraction_commit_sequence
               AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                 WHERE x.vault_id=m.vault_id AND x.active=1
                   AND x.action IN ('suppress_read','forget_current')
                   AND (
                     (x.source_id=m.source_id
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=m.source_revision_id)
                       AND (x.composed_card_id IS NULL OR x.composed_card_id=?)
                       AND (x.observation_id IS NULL OR x.observation_id=m.observation_id))
                     OR (x.composed_card_id=? AND x.source_id IS NULL)
                     OR (x.card_id IS NOT NULL
                       AND (x.source_id IS NULL OR x.source_id=m.source_id)
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=m.source_revision_id)
                       AND EXISTS (
                         SELECT 1 FROM semantic_memory_cards mc
                         JOIN semantic_card_revisions mr
                           ON mr.vault_id=mc.vault_id AND mr.card_revision_id=mc.current_revision_id
                         WHERE mc.vault_id=m.vault_id AND mc.card_id=x.card_id
                           AND mc.source_id=m.source_id
                           AND mr.source_revision_id=m.source_revision_id
                           AND (x.observation_id IS NULL OR EXISTS (
                             SELECT 1 FROM semantic_card_items mi
                             WHERE mi.vault_id=mr.vault_id
                               AND mi.card_revision_id=mr.card_revision_id
                               AND mi.observation_id=m.observation_id))))
                   ))",
        )
        .bind(context.id().to_string())
        .bind(&group_id)
        .bind(id.to_string())
        .bind(id.to_string())
        .fetch_one(pool)
        .await?;
        let supported = match operator.as_str() {
            "and" => total > 0 && valid == total,
            "or" => valid > 0,
            _ => false,
        };
        if !supported {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn build_card_record(
    pool: &SqlitePool,
    row: CardHeaderRow,
) -> Result<ComposedCardRecord, StateError> {
    let groups = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT support_group_id,operator,ordinal FROM semantic_composed_support_groups
         WHERE vault_id=? AND composed_card_item_id IN (
           SELECT composed_card_item_id FROM semantic_composed_card_items WHERE vault_id=? AND composed_card_revision_id=?
         ) ORDER BY composed_card_item_id,ordinal",
    )
    .bind(&row.vault_id)
    .bind(&row.vault_id)
    .bind(&row.composed_card_revision_id)
    .fetch_all(pool)
    .await?;
    let items = sqlx::query_as::<_, (String, String, i64, String)>(
        "SELECT composed_card_item_id,item_kind,ordinal,content FROM semantic_composed_card_items
         WHERE vault_id=? AND composed_card_revision_id=? ORDER BY item_kind,ordinal",
    )
    .bind(&row.vault_id)
    .bind(&row.composed_card_revision_id)
    .fetch_all(pool)
    .await?;
    let mut item_records = Vec::new();
    for (item_id, kind, ordinal, content) in items {
        let mut group_records = Vec::new();
        for (group_id, operator, group_ordinal) in groups.iter().filter(|(_, _, _)| true) {
            let belongs: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM semantic_composed_support_groups WHERE vault_id=? AND support_group_id=? AND composed_card_item_id=?",
            )
            .bind(&row.vault_id)
            .bind(group_id)
            .bind(&item_id)
            .fetch_optional(pool)
            .await?;
            if belongs != Some(1) {
                continue;
            }
            let members = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
                "SELECT m.support_member_id,m.source_id,m.source_revision_id,s.source_path,m.observation_id,m.evidence_ref_id,m.member_role
                 FROM semantic_composed_support_members m
                 JOIN semantic_sources s ON s.vault_id=m.vault_id AND s.source_id=m.source_id
                 JOIN semantic_observations o
                   ON o.vault_id=m.vault_id AND o.observation_id=m.observation_id
                  AND o.source_id=m.source_id AND o.source_revision_id=m.source_revision_id
                 JOIN semantic_extraction_sets es
                   ON es.vault_id=o.vault_id AND es.extraction_set_id=o.extraction_set_id
                  AND es.source_id=o.source_id
                 JOIN semantic_evidence_refs e ON e.vault_id=m.vault_id AND e.evidence_ref_id=m.evidence_ref_id
                 WHERE m.vault_id=? AND m.support_group_id=?
                   AND s.eligible=1 AND s.pending_rebuild=0 AND s.current_revision_id=m.source_revision_id
                   AND e.source_id=m.source_id AND e.source_revision_id=m.source_revision_id
                   AND e.validation_status='validated'
                   AND es.state='success_nonempty'
                   AND es.extraction_commit_sequence=s.extraction_commit_sequence
                   AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                     WHERE x.vault_id=m.vault_id AND x.active=1
                       AND x.action IN ('suppress_read','forget_current')
                       AND (
                         (x.source_id=m.source_id
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=m.source_revision_id)
                           AND (x.observation_id IS NULL OR x.observation_id=m.observation_id)
                           AND (x.composed_card_id IS NULL OR x.composed_card_id=?))
                         OR (x.composed_card_id=? AND x.source_id IS NULL)
                         OR (x.card_id IS NOT NULL
                           AND (x.source_id IS NULL OR x.source_id=m.source_id)
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=m.source_revision_id)
                           AND EXISTS (
                             SELECT 1 FROM semantic_memory_cards mc
                             JOIN semantic_card_revisions mr
                               ON mr.vault_id=mc.vault_id AND mr.card_revision_id=mc.current_revision_id
                             WHERE mc.vault_id=m.vault_id AND mc.card_id=x.card_id
                               AND mc.source_id=m.source_id
                               AND mr.source_revision_id=m.source_revision_id
                               AND (x.observation_id IS NULL OR EXISTS (
                                 SELECT 1 FROM semantic_card_items mi
                                 WHERE mi.vault_id=mr.vault_id
                                   AND mi.card_revision_id=mr.card_revision_id
                                   AND mi.observation_id=m.observation_id))))
                       ))
                 ORDER BY support_member_id",
            )
            .bind(&row.vault_id)
            .bind(group_id)
            .bind(&row.composed_card_id)
            .bind(&row.composed_card_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id, source, revision, path, observation, evidence, role)| {
                Ok(ComposedSupportMemberRecord {
                    id: SupportMemberId::parse(&id)?,
                    source_id: SemanticSourceId::parse(&source)?,
                    source_revision_id: SourceRevisionId::parse(&revision)?,
                    source_path: VaultPath::parse(&path)?,
                    observation_id: ObservationId::parse(&observation)?,
                    evidence_ref_id: EvidenceRefId::parse(&evidence)?,
                    member_role: role,
                })
            })
            .collect::<Result<Vec<_>, StateError>>()?;
            group_records.push(ComposedSupportGroupRecord {
                id: SupportGroupId::parse(group_id)?,
                operator: operator.clone(),
                ordinal: u32::try_from(*group_ordinal).map_err(|_| StateError::IntegrityFailure)?,
                members,
            });
        }
        item_records.push(ComposedCardItemRecord {
            id: ComposedCardItemId::parse(&item_id)?,
            kind,
            ordinal: u32::try_from(ordinal).map_err(|_| StateError::IntegrityFailure)?,
            content,
            support_groups: group_records,
        });
    }
    Ok(ComposedCardRecord {
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        id: ComposedCardId::parse(&row.composed_card_id)?,
        revision_id: ComposedCardRevisionId::parse(&row.composed_card_revision_id)?,
        title: row.title,
        kind: row.kind,
        scope_ref: row.scope_ref,
        assertion_status: row.assertion_status,
        temporal_scope: serde_json::from_str(&row.temporal_scope_json)?,
        revision_number: u32::try_from(row.revision_number)
            .map_err(|_| StateError::IntegrityFailure)?,
        canonical_path: VaultPath::parse(&row.canonical_path)?,
        canonical_file_id: row
            .canonical_file_id
            .as_deref()
            .map(mcp_vault_domain::FileId::parse)
            .transpose()?,
        canonical_file_revision: row
            .canonical_revision
            .map(mcp_vault_domain::Revision::try_from)
            .transpose()?,
        canonical_markdown_hash: row.canonical_markdown_hash,
        items: item_records,
    })
}

fn job_row_to_record(row: JobRow) -> Result<OrganizationJobRecord, StateError> {
    Ok(OrganizationJobRecord {
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        id: OrganizationJobId::parse(&row.organization_job_id)?,
        state: row.state,
        lifecycle_state: row.lifecycle_state,
        input_hash: row.input_hash,
        policy_revision: row.policy_revision,
        profile_id: row.profile_id,
        decision_revision: row.decision_revision,
        safe_error_code: row.safe_error_code,
        semantic_target_key: row.semantic_target_key,
        rules_revision: row.rules_revision,
    })
}

fn hash_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
