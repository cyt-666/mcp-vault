//! Vault-scoped storage for the independent semantic-memory contract.

use mcp_vault_domain::{
    CardItemId, CardRevisionId, ComposedCardId, EvidenceRefId, ExtractionSetId, FileId,
    MemoryCardId, ObservationId, PreparedSnapshotId, Revision, SemanticSourceId, SourceRevisionId,
    VaultContext, VaultPath,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::{StateError, files::FileRecord, now_millis};

type PublicationSourceSnapshot = (
    String,
    String,
    Option<String>,
    String,
    String,
    i64,
    i64,
    Option<String>,
    i64,
    i64,
    i64,
    i64,
    i64,
    String,
);
type SemanticFingerprintObservation = (
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
pub struct SemanticSourceRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub source_id: SemanticSourceId,
    pub file_id: FileId,
    pub current_revision_id: Option<SourceRevisionId>,
    pub source_path: VaultPath,
    pub content_hash: String,
    pub source_generation: i64,
    pub extraction_commit_sequence: i64,
    pub authorization_revision: i64,
    pub eligible: bool,
    pub pending_rebuild: bool,
    pub invalid_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSourceRevisionRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub source_revision_id: SourceRevisionId,
    pub source_id: SemanticSourceId,
    pub file_id: FileId,
    pub file_revision: Revision,
    pub content_hash: String,
    pub source_path: VaultPath,
    pub availability: String,
    pub source_generation: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSpanInput {
    pub start_byte: u64,
    pub end_byte: u64,
    pub content_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEvidenceInput {
    pub id: EvidenceRefId,
    pub body_spans: Vec<SemanticSpanInput>,
    pub context_spans: Vec<SemanticSpanInput>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticObservationInput {
    pub id: ObservationId,
    pub local_key: String,
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
    pub admission_reason: String,
    pub value_for_future_work: String,
    pub evidence: SemanticEvidenceInput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticCardItemInput {
    pub id: CardItemId,
    pub observation_id: ObservationId,
    pub kind: String,
    pub ordinal: u32,
    pub content: String,
    pub evidence_ref_ids: Vec<EvidenceRefId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticCardRevisionInput {
    pub card_id: MemoryCardId,
    pub card_revision_id: CardRevisionId,
    pub expected_card_revision_id: Option<CardRevisionId>,
    pub revision_number: u32,
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
    pub items: Vec<SemanticCardItemInput>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticExtractionRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub id: ExtractionSetId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub state: String,
    pub input_hash: String,
    pub profile_id: String,
    pub authorization_revision: i64,
    pub source_generation: i64,
    pub extraction_commit_sequence: i64,
    pub observation_count: u32,
    pub card_count: u32,
    pub safe_error_code: Option<String>,
    pub semantic_target_key: String,
    pub rules_revision: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticExtractionBatchSpec {
    pub batch_index: u32,
    pub batch_count: u32,
    pub source_revision_id: SourceRevisionId,
    pub batch_input_hash: String,
    pub batch_catalog_hash: String,
    pub prompt_id: String,
    pub schema_id: String,
    pub provider_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticExtractionBatchRecord {
    pub extraction_set_id: ExtractionSetId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub batch_index: u32,
    pub batch_count: u32,
    pub batch_input_hash: String,
    pub batch_catalog_hash: String,
    pub prompt_id: String,
    pub schema_id: String,
    pub provider_fingerprint: String,
    pub state: String,
    pub attempt_count: u32,
    pub normalized_proposal_json: Option<String>,
    pub normalized_proposal_hash: Option<String>,
    pub advisory_normalization_counts: Value,
    pub safe_error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticObservationRecord {
    pub id: ObservationId,
    pub source_revision_id: SourceRevisionId,
    pub extraction_set_id: ExtractionSetId,
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
    pub admission_reason: String,
    pub value_for_future_work: String,
    pub evidence_ref_id: EvidenceRefId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticCardItemRecord {
    pub kind: String,
    pub ordinal: u32,
    pub content: String,
    pub observation_id: ObservationId,
    pub observation_kind: String,
    pub scope: String,
    pub assertion_status: String,
    pub source_time_scope: Value,
    pub evidence_ref_ids: Vec<EvidenceRefId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticCardRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub id: MemoryCardId,
    pub revision_id: CardRevisionId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub source_path: VaultPath,
    pub title: String,
    pub kind: String,
    pub scope_ref: String,
    pub assertion_status: String,
    pub temporal_scope: Value,
    pub revision_number: u32,
    pub canonical_path: VaultPath,
    pub canonical_file_id: FileId,
    pub canonical_file_revision: Revision,
    pub canonical_markdown_hash: String,
    pub items: Vec<SemanticCardItemRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticCardHead {
    pub id: MemoryCardId,
    pub current_revision_id: Option<CardRevisionId>,
    pub revision_number: u32,
    pub canonical_file_id: Option<FileId>,
    pub canonical_file_revision: Option<Revision>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEvidenceRecord {
    pub id: EvidenceRefId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub body_spans: Vec<SemanticSpanInput>,
    pub context_spans: Vec<SemanticSpanInput>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticPreparedSnapshot {
    pub id: PreparedSnapshotId,
    pub extraction_set_id: ExtractionSetId,
    pub card_id: MemoryCardId,
    pub card_revision_id: CardRevisionId,
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub source_content_hash: String,
    pub authorization_revision: i64,
    pub source_generation: i64,
    pub extraction_commit_sequence: i64,
    pub expected_card_revision_id: Option<CardRevisionId>,
    pub proposed_card_revision_number: u32,
    pub expected_file_id: Option<FileId>,
    pub expected_file_revision: Option<Revision>,
    pub proposed_file_revision: Revision,
    pub published_file_id: Option<FileId>,
    pub published_file_revision: Option<Revision>,
    pub target_path: VaultPath,
    pub proposed_file_hash: String,
    pub canonical_bytes: Vec<u8>,
    pub status: String,
    pub semantic_target_key: String,
    pub rules_revision: i64,
}

#[derive(Clone)]
pub struct SemanticMemoryRepository {
    pool: SqlitePool,
}

#[derive(FromRow)]
struct SourceRow {
    vault_id: String,
    source_id: String,
    file_id: String,
    current_revision_id: Option<String>,
    source_path: String,
    current_content_hash: String,
    source_generation: i64,
    extraction_commit_sequence: i64,
    authorization_revision: i64,
    eligible: i64,
    invalid_reason: Option<String>,
    pending_rebuild: i64,
}

#[derive(FromRow)]
struct SourceRevisionRow {
    vault_id: String,
    source_revision_id: String,
    source_id: String,
    file_id: String,
    file_revision: i64,
    content_hash: String,
    source_path: String,
    availability: String,
    source_generation: i64,
}

#[derive(FromRow)]
struct ExtractionRow {
    vault_id: String,
    extraction_set_id: String,
    source_id: String,
    source_revision_id: String,
    state: String,
    input_hash: String,
    profile_id: String,
    authorization_revision: i64,
    source_generation: i64,
    extraction_commit_sequence: i64,
    observation_count: i64,
    card_count: i64,
    safe_error_code: Option<String>,
    semantic_target_key: String,
    rules_revision: i64,
}

#[derive(FromRow)]
struct ExtractionBatchRow {
    extraction_set_id: String,
    source_id: String,
    source_revision_id: String,
    batch_index: i64,
    batch_count: i64,
    batch_input_hash: String,
    batch_catalog_hash: String,
    prompt_id: String,
    schema_id: String,
    provider_fingerprint: String,
    state: String,
    attempt_count: i64,
    normalized_proposal_json: Option<String>,
    normalized_proposal_hash: Option<String>,
    advisory_normalization_counts_json: String,
    safe_error_code: Option<String>,
}

#[derive(FromRow)]
struct SnapshotRow {
    snapshot_id: String,
    extraction_set_id: String,
    card_id: String,
    card_revision_id: String,
    source_id: String,
    source_revision_id: String,
    source_content_hash: String,
    authorization_revision: i64,
    source_generation: i64,
    extraction_commit_sequence: i64,
    expected_card_revision_id: Option<String>,
    proposed_card_revision_number: i64,
    expected_file_id: Option<String>,
    expected_file_revision: Option<i64>,
    proposed_file_revision: i64,
    published_file_id: Option<String>,
    published_file_revision: Option<i64>,
    target_path: String,
    proposed_file_hash: String,
    canonical_bytes: Vec<u8>,
    status: String,
    semantic_target_key: String,
    rules_revision: i64,
}

#[derive(FromRow)]
struct ObservationRow {
    observation_id: String,
    source_revision_id: String,
    extraction_set_id: String,
    kind: String,
    statement: String,
    scope: String,
    assertion_status: String,
    source_time_scope_json: String,
    conditions_json: String,
    exceptions_json: String,
    ordered_steps_json: String,
    result_json: Option<String>,
    uncertainty_json: Option<String>,
    admission_reason: String,
    value_for_future_work: String,
    evidence_ref_id: String,
    advisory_kind_unknown: i64,
}

type CardHeadRow = (
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
);
type ExtractionFenceRow = (
    String,
    String,
    Option<String>,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
);

impl SemanticMemoryRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Idempotently bind a running source ExtractionSet to the exact A80
    /// provider batches. The persisted rows contain hashes and validated
    /// proposals only; source text remains in the Vault and invalid model
    /// responses are never stored here.
    pub async fn prepare_extraction_batches(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        specs: &[SemanticExtractionBatchSpec],
    ) -> Result<Vec<SemanticExtractionBatchRecord>, StateError> {
        if specs.is_empty()
            || specs.len() > 4096
            || specs.iter().any(|spec| {
                spec.batch_count as usize != specs.len()
                    || spec.batch_input_hash.len() != 64
                    || spec.batch_catalog_hash.len() != 64
                    || spec.prompt_id.trim().is_empty()
                    || spec.schema_id.trim().is_empty()
                    || spec.provider_fingerprint.trim().is_empty()
            })
            || specs
                .iter()
                .enumerate()
                .any(|(index, spec)| spec.batch_index as usize != index)
        {
            return Err(StateError::InvalidInput(
                "semantic extraction batch contract is invalid",
            ));
        }
        let vault = context.id().to_string();
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let extraction: Option<(String, String, String)> = sqlx::query_as(
            "SELECT source_id,source_revision_id,state FROM semantic_extraction_sets
             WHERE vault_id=? AND extraction_set_id=?",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((source_id, revision_id, state)) = extraction else {
            return Err(StateError::Conflict);
        };
        let completed = matches!(state.as_str(), "success_nonempty" | "success_empty");
        if (state != "running" && !completed)
            || specs
                .iter()
                .any(|spec| spec.source_revision_id.to_string() != revision_id)
        {
            return Err(StateError::Conflict);
        }
        let existing = sqlx::query_as::<_, ExtractionBatchRow>(
            "SELECT extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                    batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                    state,attempt_count,normalized_proposal_json,normalized_proposal_hash,
                    advisory_normalization_counts_json,safe_error_code
             FROM semantic_extraction_batches
             WHERE vault_id=? AND extraction_set_id=? ORDER BY batch_index",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        if existing.is_empty() {
            if completed {
                return Err(StateError::Conflict);
            }
            for spec in specs {
                sqlx::query(
                    "INSERT INTO semantic_extraction_batches
                     (vault_id,extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                      batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                      state,attempt_count,advisory_normalization_counts_json,created_at,updated_at)
                     VALUES(?,?,?,?,?,?,?,?,?,?,?,'ready',0,'{}',?,?)",
                )
                .bind(&vault)
                .bind(extraction_set_id.to_string())
                .bind(&source_id)
                .bind(spec.source_revision_id.to_string())
                .bind(i64::from(spec.batch_index))
                .bind(i64::from(spec.batch_count))
                .bind(&spec.batch_input_hash)
                .bind(&spec.batch_catalog_hash)
                .bind(&spec.prompt_id)
                .bind(&spec.schema_id)
                .bind(&spec.provider_fingerprint)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
        } else {
            if existing.len() != specs.len()
                || existing.iter().zip(specs).any(|(row, spec)| {
                    row.source_id != source_id
                        || row.source_revision_id != spec.source_revision_id.to_string()
                        || row.batch_index != i64::from(spec.batch_index)
                        || row.batch_count != i64::from(spec.batch_count)
                        || row.batch_input_hash != spec.batch_input_hash
                        || row.batch_catalog_hash != spec.batch_catalog_hash
                        || row.prompt_id != spec.prompt_id
                        || row.schema_id != spec.schema_id
                        || row.provider_fingerprint != spec.provider_fingerprint
                })
            {
                return Err(StateError::Conflict);
            }
        }
        if completed && existing.iter().any(|row| row.state != "validated") {
            return Err(StateError::Conflict);
        }
        let rows = sqlx::query_as::<_, ExtractionBatchRow>(
            "SELECT extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                    batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                    state,attempt_count,normalized_proposal_json,normalized_proposal_hash,
                    advisory_normalization_counts_json,safe_error_code
             FROM semantic_extraction_batches
             WHERE vault_id=? AND extraction_set_id=? ORDER BY batch_index",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(extraction_batch_row_to_record)
            .collect()
    }

    /// Reserve an attempt before dispatch. A row left `dispatching` by a
    /// process crash is converted to `uncertain` and never silently replayed.
    pub async fn reserve_extraction_batch_attempt(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        batch_index: u32,
        expected_input_hash: &str,
    ) -> Result<SemanticExtractionBatchRecord, StateError> {
        let vault = context.id().to_string();
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query_as::<_, ExtractionBatchRow>(
            "SELECT extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                    batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                    state,attempt_count,normalized_proposal_json,normalized_proposal_hash,
                    advisory_normalization_counts_json,safe_error_code
             FROM semantic_extraction_batches
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=?",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StateError::Conflict)?;
        if row.batch_input_hash != expected_input_hash {
            return Err(StateError::Conflict);
        }
        if row.state == "validated" {
            tx.commit().await?;
            return extraction_batch_row_to_record(row);
        }
        if row.state == "dispatching" {
            sqlx::query(
                "UPDATE semantic_extraction_batches SET state='uncertain',
                 safe_error_code='provider_attempt_outcome_uncertain',updated_at=?
                 WHERE vault_id=? AND extraction_set_id=? AND batch_index=? AND state='dispatching'",
            )
            .bind(now)
            .bind(&vault)
            .bind(extraction_set_id.to_string())
            .bind(i64::from(batch_index))
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Err(StateError::Conflict);
        }
        if row.state != "ready" || row.attempt_count != 0 {
            return Err(StateError::Conflict);
        }
        sqlx::query(
            "UPDATE semantic_extraction_batches SET state='dispatching',attempt_count=attempt_count+1,
             safe_error_code=NULL,updated_at=?
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=? AND state='ready' AND attempt_count=0",
        )
        .bind(now)
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .execute(&mut *tx)
        .await?;
        let updated = sqlx::query_as::<_, ExtractionBatchRow>(
            "SELECT extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                    batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                    state,attempt_count,normalized_proposal_json,normalized_proposal_hash,
                    advisory_normalization_counts_json,safe_error_code
             FROM semantic_extraction_batches
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=?",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        extraction_batch_row_to_record(updated)
    }

    pub async fn store_validated_extraction_batch(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        batch_index: u32,
        expected_input_hash: &str,
        normalized_proposal_json: &str,
        advisory_normalization_counts: &Value,
    ) -> Result<(), StateError> {
        let proposal_hash = sha256_hex(normalized_proposal_json.as_bytes());
        let counts_json = serde_json::to_string(advisory_normalization_counts)
            .map_err(|_| StateError::InvalidInput("advisory counts are invalid"))?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE semantic_extraction_batches SET state='validated',normalized_proposal_json=?,
             normalized_proposal_hash=?,advisory_normalization_counts_json=?,safe_error_code=NULL,updated_at=?
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=? AND batch_input_hash=? AND state='dispatching'",
        )
        .bind(normalized_proposal_json)
        .bind(proposal_hash)
        .bind(counts_json)
        .bind(now)
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .bind(expected_input_hash)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    pub async fn fail_extraction_batch_attempt(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        batch_index: u32,
        expected_input_hash: &str,
        safe_error_code: &str,
    ) -> Result<(), StateError> {
        if safe_error_code.is_empty()
            || safe_error_code.len() > 96
            || !safe_error_code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(StateError::InvalidInput("batch error code is invalid"));
        }
        let result = sqlx::query(
            "UPDATE semantic_extraction_batches SET state='failed',safe_error_code=?,updated_at=?
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=? AND batch_input_hash=? AND state='dispatching'",
        )
        .bind(safe_error_code)
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .bind(expected_input_hash)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    /// Atomically spend the source's single A80 regeneration token and reserve
    /// its already-failed batch for the second and final Provider attempt.
    /// Keeping both changes in one Vault-scoped transaction prevents a crash
    /// or concurrent runner from restoring the token.
    pub async fn reserve_extraction_batch_regen_attempt(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        batch_index: u32,
        expected_input_hash: &str,
    ) -> Result<SemanticExtractionBatchRecord, StateError> {
        let vault = context.id().to_string();
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query_as::<_, ExtractionBatchRow>(
            "SELECT extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                    batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                    state,attempt_count,normalized_proposal_json,normalized_proposal_hash,
                    advisory_normalization_counts_json,safe_error_code
             FROM semantic_extraction_batches
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=?",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StateError::Conflict)?;
        let extraction_state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM semantic_extraction_sets
             WHERE vault_id=? AND extraction_set_id=? AND source_id=?",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(&row.source_id)
        .fetch_optional(&mut *tx)
        .await?;
        if row.batch_input_hash != expected_input_hash
            || row.state != "failed"
            || row.attempt_count != 1
            || extraction_state.as_deref() != Some("running")
        {
            return Err(StateError::Conflict);
        }
        sqlx::query(
            "INSERT INTO semantic_extraction_regen_tokens
             (vault_id,extraction_set_id,source_id,token_kind,batch_index,consumed_at)
             VALUES(?,?,?,'observation_regen',?,?)",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(&row.source_id)
        .bind(i64::from(batch_index))
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .is_some_and(|database| database.is_unique_violation())
            {
                StateError::Conflict
            } else {
                StateError::Database(error)
            }
        })?;
        let updated = sqlx::query(
            "UPDATE semantic_extraction_batches
             SET state='dispatching',attempt_count=2,safe_error_code=NULL,updated_at=?
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=?
               AND batch_input_hash=? AND state='failed' AND attempt_count=1",
        )
        .bind(now)
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .bind(expected_input_hash)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        let updated = sqlx::query_as::<_, ExtractionBatchRow>(
            "SELECT extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                    batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                    state,attempt_count,normalized_proposal_json,normalized_proposal_hash,
                    advisory_normalization_counts_json,safe_error_code
             FROM semantic_extraction_batches
             WHERE vault_id=? AND extraction_set_id=? AND batch_index=?",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .bind(i64::from(batch_index))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        extraction_batch_row_to_record(updated)
    }

    pub async fn list_extraction_batches(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<Vec<SemanticExtractionBatchRecord>, StateError> {
        let rows = sqlx::query_as::<_, ExtractionBatchRow>(
            "SELECT extraction_set_id,source_id,source_revision_id,batch_index,batch_count,
                    batch_input_hash,batch_catalog_hash,prompt_id,schema_id,provider_fingerprint,
                    state,attempt_count,normalized_proposal_json,normalized_proposal_hash,
                    advisory_normalization_counts_json,safe_error_code
             FROM semantic_extraction_batches WHERE vault_id=? AND extraction_set_id=? ORDER BY batch_index",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(extraction_batch_row_to_record)
            .collect()
    }

    /// Bind or advance a Markdown source to an immutable content revision.
    /// Same File ID plus same hash updates navigation only.
    pub async fn upsert_source_revision(
        &self,
        context: &VaultContext,
        file_id: FileId,
        source_path: &VaultPath,
        file_revision: Revision,
        content_hash: &str,
        authorization_revision: i64,
    ) -> Result<(SemanticSourceRecord, SemanticSourceRevisionRecord, bool), StateError> {
        self.upsert_source_revision_inner(
            context,
            file_id,
            source_path,
            file_revision,
            content_hash,
            authorization_revision,
            None,
        )
        .await
    }

    /// Bind a source only when the FileRecord read by the caller is still the
    /// authoritative row. The fence is checked inside the same State
    /// transaction that updates semantic source metadata, so a concurrent
    /// move/replace cannot write an old path or revision back into the source.
    pub async fn upsert_source_revision_fenced(
        &self,
        context: &VaultContext,
        expected_file: &FileRecord,
        content_hash: &str,
        authorization_revision: i64,
    ) -> Result<(SemanticSourceRecord, SemanticSourceRevisionRecord, bool), StateError> {
        if expected_file.vault_id != context.id()
            || expected_file.content_hash.as_deref().is_none()
            || !content_hash_matches(expected_file.content_hash.as_deref(), content_hash)
        {
            return Err(StateError::Conflict);
        }
        self.upsert_source_revision_inner(
            context,
            expected_file.id,
            &expected_file.path,
            expected_file.current_revision,
            content_hash,
            authorization_revision,
            Some(expected_file),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn upsert_source_revision_inner(
        &self,
        context: &VaultContext,
        file_id: FileId,
        source_path: &VaultPath,
        file_revision: Revision,
        content_hash: &str,
        authorization_revision: i64,
        fence: Option<&FileRecord>,
    ) -> Result<(SemanticSourceRecord, SemanticSourceRevisionRecord, bool), StateError> {
        if content_hash.is_empty() {
            return Err(StateError::InvalidInput("semantic source hash is required"));
        }
        let now = now_millis()?;
        // The fenced variant must reserve SQLite's writer slot before reading
        // file_entries. Otherwise a concurrent FileState commit could advance
        // the row after our snapshot and turn the later semantic write into a
        // stale-snapshot error rather than a deterministic Conflict.
        let mut tx = if fence.is_some() {
            self.pool.begin_with("BEGIN IMMEDIATE").await?
        } else {
            self.pool.begin().await?
        };
        let vault = context.id().to_string();
        let file = file_id.to_string();
        if let Some(expected) = fence {
            let current: Option<(String, i64, Option<String>)> = sqlx::query_as(
                "SELECT path, current_revision, content_hash
                 FROM file_entries WHERE vault_id=? AND id=?",
            )
            .bind(&vault)
            .bind(&file)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((current_path, current_revision, current_hash)) = current else {
                return Err(StateError::Conflict);
            };
            if current_path != expected.path.as_str()
                || current_revision != expected.current_revision.as_i64()?
                || !content_hash_matches(current_hash.as_deref(), content_hash)
                || !content_hash_matches(
                    current_hash.as_deref(),
                    expected.content_hash.as_deref().unwrap_or_default(),
                )
            {
                return Err(StateError::Conflict);
            }
        }
        let existing = sqlx::query_as::<_, SourceRow>(
            "SELECT vault_id, source_id, file_id, current_revision_id, source_path,
                    current_content_hash, source_generation, extraction_commit_sequence,
                    authorization_revision, eligible, invalid_reason, pending_rebuild
             FROM semantic_sources WHERE vault_id=? AND file_id=?",
        )
        .bind(&vault)
        .bind(&file)
        .fetch_optional(&mut *tx)
        .await?;

        let (source_id, content_changed) = if let Some(source) = existing {
            let source_id = SemanticSourceId::parse(&source.source_id)?;
            let old_revision = source.current_revision_id;
            if source.current_content_hash == content_hash {
                let revision_id = old_revision.ok_or(StateError::IntegrityFailure)?;
                let existing_revision = load_source_revision_tx(
                    &mut tx,
                    context,
                    SourceRevisionId::parse(&revision_id)?,
                )
                .await?;
                let authorization_changed = source.authorization_revision != authorization_revision;
                let generation_reopened =
                    existing_revision.source_generation != source.source_generation;
                let next_generation = if authorization_changed {
                    source
                        .source_generation
                        .checked_add(1)
                        .ok_or(StateError::IntegrityFailure)?
                } else {
                    source.source_generation
                };
                sqlx::query(
                    "UPDATE semantic_sources SET source_path=?, updated_at=?
                     WHERE vault_id=? AND source_id=?",
                )
                .bind(source_path.as_str())
                .bind(now)
                .bind(&vault)
                .bind(source_id.to_string())
                .execute(&mut *tx)
                .await?;
                if authorization_changed || generation_reopened {
                    sqlx::query(
                        "UPDATE semantic_sources SET authorization_revision=?,source_generation=?,eligible=0,
                            invalid_reason=?,pending_rebuild=1,updated_at=?
                         WHERE vault_id=? AND source_id=?",
                    )
                    .bind(authorization_revision)
                    .bind(next_generation)
                    .bind(if authorization_changed {
                        "authorization_changed"
                    } else {
                        "source_not_extracted"
                    })
                    .bind(now)
                    .bind(&vault)
                    .bind(source_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        "UPDATE semantic_source_revisions SET source_generation=?
                         WHERE vault_id=? AND source_revision_id=? AND source_id=?",
                    )
                    .bind(next_generation)
                    .bind(&vault)
                    .bind(&revision_id)
                    .bind(source_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        "UPDATE semantic_memory_cards SET eligibility='invalidated',updated_at=?
                         WHERE vault_id=? AND source_id=?",
                    )
                    .bind(now)
                    .bind(&vault)
                    .bind(source_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        "UPDATE semantic_evidence_refs SET validation_status='invalidated'
                         WHERE vault_id=? AND source_id=?",
                    )
                    .bind(&vault)
                    .bind(source_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                }
                let source = load_source_tx(&mut tx, context, source_id).await?;
                let revision = load_source_revision_tx(
                    &mut tx,
                    context,
                    SourceRevisionId::parse(&revision_id)?,
                )
                .await?;
                tx.commit().await?;
                return Ok((source, revision, false));
            }
            sqlx::query(
                "UPDATE semantic_source_revisions SET availability='historical'
                 WHERE vault_id=? AND source_id=? AND availability='current'",
            )
            .bind(&vault)
            .bind(source_id.to_string())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE semantic_sources SET current_revision_id=NULL, current_content_hash=?,
                    source_path=?, authorization_revision=?,source_generation=source_generation+1,
                    eligible=0, invalid_reason='source_changed', pending_rebuild=1,
                    updated_at=? WHERE vault_id=? AND source_id=?",
            )
            .bind(content_hash)
            .bind(source_path.as_str())
            .bind(authorization_revision)
            .bind(now)
            .bind(&vault)
            .bind(source_id.to_string())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE semantic_memory_cards SET eligibility='invalidated', updated_at=?
                 WHERE vault_id=? AND source_id=?",
            )
            .bind(now)
            .bind(&vault)
            .bind(source_id.to_string())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE semantic_evidence_refs SET validation_status='invalidated'
                 WHERE vault_id=? AND source_id=?",
            )
            .bind(&vault)
            .bind(source_id.to_string())
            .execute(&mut *tx)
            .await?;
            (source_id, true)
        } else {
            let source_id = SemanticSourceId::new();
            sqlx::query(
                "INSERT INTO semantic_sources
                 (vault_id,source_id,file_id,current_revision_id,source_path,current_content_hash,
                  authorization_revision,eligible,invalid_reason,pending_rebuild,created_at,updated_at)
                 VALUES(?,?,?,NULL,?,?,?,0,'source_not_extracted',1,?,?)",
            )
            .bind(&vault)
            .bind(source_id.to_string())
            .bind(&file)
            .bind(source_path.as_str())
            .bind(content_hash)
            .bind(authorization_revision)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            (source_id, true)
        };

        let revision_id = SourceRevisionId::new();
        let revision_number = file_revision.as_i64()?;
        let source_generation: i64 = sqlx::query_scalar(
            "SELECT source_generation FROM semantic_sources WHERE vault_id=? AND source_id=?",
        )
        .bind(&vault)
        .bind(source_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO semantic_source_revisions
             (vault_id,source_revision_id,source_id,file_id,file_revision,content_hash,
              source_path,source_time_scope_json,availability,source_generation,created_at)
             VALUES(?,?,?,?,?,?,?,'{}','current',?,?)",
        )
        .bind(&vault)
        .bind(revision_id.to_string())
        .bind(source_id.to_string())
        .bind(&file)
        .bind(revision_number)
        .bind(content_hash)
        .bind(source_path.as_str())
        .bind(source_generation)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_sources SET current_revision_id=?, current_content_hash=?,
                source_path=?,authorization_revision=?,eligible=0, invalid_reason=?, pending_rebuild=1, updated_at=?
             WHERE vault_id=? AND source_id=?",
        )
        .bind(revision_id.to_string())
        .bind(content_hash)
        .bind(source_path.as_str())
        .bind(authorization_revision)
        .bind(if content_changed {
            "source_changed"
        } else {
            "source_not_extracted"
        })
        .bind(now)
        .bind(&vault)
        .bind(source_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        let source = self
            .get_source(context, source_id)
            .await?
            .ok_or(StateError::IntegrityFailure)?;
        let revision = self
            .get_source_revision(context, revision_id)
            .await?
            .ok_or(StateError::IntegrityFailure)?;
        Ok((source, revision, content_changed))
    }

    pub async fn get_source(
        &self,
        context: &VaultContext,
        source_id: SemanticSourceId,
    ) -> Result<Option<SemanticSourceRecord>, StateError> {
        let row = sqlx::query_as::<_, SourceRow>(
            "SELECT vault_id,source_id,file_id,current_revision_id,source_path,current_content_hash,
                    source_generation,extraction_commit_sequence,authorization_revision,eligible,
                    invalid_reason,pending_rebuild
             FROM semantic_sources WHERE vault_id=? AND source_id=?",
        )
        .bind(context.id().to_string())
        .bind(source_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(source_row_to_record).transpose()
    }

    pub async fn get_source_by_file(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<Option<SemanticSourceRecord>, StateError> {
        let row = sqlx::query_as::<_, SourceRow>(
            "SELECT vault_id,source_id,file_id,current_revision_id,source_path,current_content_hash,
                    source_generation,extraction_commit_sequence,authorization_revision,eligible,
                    invalid_reason,pending_rebuild
             FROM semantic_sources WHERE vault_id=? AND file_id=?",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(source_row_to_record).transpose()
    }

    pub async fn get_source_revision(
        &self,
        context: &VaultContext,
        revision_id: SourceRevisionId,
    ) -> Result<Option<SemanticSourceRevisionRecord>, StateError> {
        let row = sqlx::query_as::<_, SourceRevisionRow>(
            "SELECT vault_id,source_revision_id,source_id,file_id,file_revision,
                    content_hash,source_path,availability,source_generation
             FROM semantic_source_revisions WHERE vault_id=? AND source_revision_id=?",
        )
        .bind(context.id().to_string())
        .bind(revision_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(source_revision_row_to_record).transpose()
    }

    /// List every revision identity for a Vault. Transient model-facing block
    /// namespaces use this Vault-scoped set to detect short-hash collisions,
    /// including revisions that are no longer current.
    pub async fn list_source_revision_ids(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<SourceRevisionId>, StateError> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT source_revision_id FROM semantic_source_revisions
             WHERE vault_id=? ORDER BY source_revision_id",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|id| SourceRevisionId::parse(id).map_err(|_| StateError::IntegrityFailure))
            .collect()
    }

    pub async fn card_head(
        &self,
        context: &VaultContext,
        source_id: SemanticSourceId,
        topic_key: &str,
    ) -> Result<Option<SemanticCardHead>, StateError> {
        let row: Option<CardHeadRow> = sqlx::query_as(
            "SELECT c.card_id,c.current_revision_id,r.revision_number,
                        r.canonical_file_id,r.canonical_revision
                 FROM semantic_memory_cards c LEFT JOIN semantic_card_revisions r
                   ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
                 WHERE c.vault_id=? AND c.source_id=? AND c.topic_key=?",
        )
        .bind(context.id().to_string())
        .bind(source_id.to_string())
        .bind(topic_key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(
            |(card_id, revision_id, revision_number, file_id, file_revision)| {
                Ok(SemanticCardHead {
                    id: MemoryCardId::parse(&card_id)?,
                    current_revision_id: revision_id
                        .as_deref()
                        .map(CardRevisionId::parse)
                        .transpose()?,
                    revision_number: u32::try_from(revision_number.unwrap_or(0))
                        .map_err(|_| StateError::IntegrityFailure)?,
                    canonical_file_id: file_id.as_deref().map(FileId::parse).transpose()?,
                    canonical_file_revision: file_revision.map(Revision::try_from).transpose()?,
                })
            },
        )
        .transpose()
    }

    pub async fn get_extraction(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<Option<SemanticExtractionRecord>, StateError> {
        let row = sqlx::query_as::<_, ExtractionRow>(
            "SELECT vault_id,extraction_set_id,source_id,source_revision_id,state,input_hash,profile_id,
                    authorization_revision,source_generation,extraction_commit_sequence,
                    observation_count,card_count,safe_error_code,semantic_target_key,rules_revision
             FROM semantic_extraction_sets WHERE vault_id=? AND extraction_set_id=?",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(extraction_row_to_record).transpose()
    }

    /// Return the newest completed extraction for one historical source
    /// revision. This is intentionally independent of current source
    /// eligibility so a caller can perform a conservative, evidence-based
    /// local rebind before rebuilding the source.
    pub async fn latest_reusable_extraction(
        &self,
        context: &VaultContext,
        source_id: SemanticSourceId,
        source_revision_id: SourceRevisionId,
        profile_id: &str,
    ) -> Result<Option<SemanticExtractionRecord>, StateError> {
        let row = sqlx::query_as::<_, ExtractionRow>(
            "SELECT vault_id,extraction_set_id,source_id,source_revision_id,state,input_hash,profile_id,
                    authorization_revision,source_generation,extraction_commit_sequence,
                    observation_count,card_count,safe_error_code,semantic_target_key,rules_revision
             FROM semantic_extraction_sets
             WHERE vault_id=? AND source_id=? AND source_revision_id=?
               AND state='success_nonempty' AND profile_id=?
             ORDER BY completed_at DESC,created_at DESC,extraction_set_id DESC LIMIT 1",
        )
        .bind(context.id().to_string())
        .bind(source_id.to_string())
        .bind(source_revision_id.to_string())
        .bind(profile_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(extraction_row_to_record).transpose()
    }

    pub async fn list_observations(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<Vec<SemanticObservationRecord>, StateError> {
        let rows = sqlx::query_as::<_, ObservationRow>(
            "SELECT o.observation_id,o.source_revision_id,o.extraction_set_id,o.kind,o.statement,
                    o.advisory_kind_unknown,
                    o.scope,o.assertion_status,o.source_time_scope_json,o.conditions_json,
                    o.exceptions_json,o.ordered_steps_json,o.result_json,o.uncertainty_json,
                    o.admission_reason,o.value_for_future_work,e.evidence_ref_id
             FROM semantic_observations o JOIN semantic_observation_evidence e
               ON e.vault_id=o.vault_id AND e.observation_id=o.observation_id
             WHERE o.vault_id=? AND o.extraction_set_id=?
             ORDER BY o.local_observation_key,e.evidence_ref_id",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(SemanticObservationRecord {
                    id: ObservationId::parse(&row.observation_id)?,
                    source_revision_id: SourceRevisionId::parse(&row.source_revision_id)?,
                    extraction_set_id: ExtractionSetId::parse(&row.extraction_set_id)?,
                    kind: if row.advisory_kind_unknown == 1 {
                        "unknown".to_owned()
                    } else {
                        row.kind
                    },
                    statement: row.statement,
                    scope: row.scope,
                    assertion_status: row.assertion_status,
                    source_time_scope: serde_json::from_str(&row.source_time_scope_json)?,
                    conditions: serde_json::from_str(&row.conditions_json)?,
                    exceptions: serde_json::from_str(&row.exceptions_json)?,
                    ordered_steps: serde_json::from_str(&row.ordered_steps_json)?,
                    result: row.result_json,
                    uncertainty: row.uncertainty_json,
                    admission_reason: row.admission_reason,
                    value_for_future_work: row.value_for_future_work,
                    evidence_ref_id: EvidenceRefId::parse(&row.evidence_ref_id)?,
                })
            })
            .collect()
    }

    /// Load published cards from a historical extraction without applying
    /// current source eligibility. The caller must independently prove every
    /// evidence span before reusing any returned card.
    pub async fn list_historical_cards_for_extraction(
        &self,
        context: &VaultContext,
        extraction: &SemanticExtractionRecord,
    ) -> Result<Vec<SemanticCardRecord>, StateError> {
        let rows = sqlx::query_as::<_, CardHeaderRow>(
            "SELECT c.card_id,r.card_revision_id,c.source_id,r.source_revision_id,s.source_path,
                    r.title,r.kind,r.scope_ref,r.assertion_status,r.temporal_scope_json,r.revision_number,
                    r.canonical_path,r.canonical_file_id,r.canonical_revision,r.canonical_markdown_hash
             FROM semantic_memory_cards c
             JOIN semantic_card_revisions r
               ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
             JOIN semantic_sources s ON s.vault_id=c.vault_id AND s.source_id=c.source_id
             JOIN semantic_extraction_sets x
               ON x.vault_id=r.vault_id AND x.extraction_set_id=r.extraction_set_id
              AND x.source_id=r.source_id AND x.source_revision_id=r.source_revision_id
             JOIN semantic_source_revisions sr
               ON sr.vault_id=x.vault_id AND sr.source_revision_id=x.source_revision_id
              AND sr.source_id=x.source_id
             WHERE c.vault_id=? AND r.extraction_set_id=?
               AND x.source_id=? AND x.source_revision_id=? AND x.profile_id=?
               AND x.state='success_nonempty' AND r.publication_state='published'
               AND r.composition_profile_id=?
             ORDER BY r.created_at,c.card_id",
        )
        .bind(context.id().to_string())
        .bind(extraction.id.to_string())
        .bind(extraction.source_id.to_string())
        .bind(extraction.source_revision_id.to_string())
        .bind(&extraction.profile_id)
        .bind(&extraction.profile_id)
        .fetch_all(&self.pool)
        .await?;
        self.build_card_records(context, rows).await
    }

    /// Start or return an idempotent complete-result-set extraction.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_extraction(
        &self,
        context: &VaultContext,
        id: ExtractionSetId,
        source_id: SemanticSourceId,
        source_revision_id: SourceRevisionId,
        input_hash: &str,
        profile_id: &str,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<(SemanticExtractionRecord, bool), StateError> {
        let mut tx = self.pool.begin().await?;
        let vault = context.id().to_string();
        if let Some((existing_id, existing_hash)) = sqlx::query_as::<_, (String, String)>(
            "SELECT extraction_set_id,request_hash FROM semantic_extraction_idempotency
             WHERE vault_id=? AND idempotency_key=?",
        )
        .bind(&vault)
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            if existing_hash != request_hash {
                return Err(StateError::Conflict);
            }
            let row = sqlx::query_as::<_, ExtractionRow>(
                "SELECT vault_id,extraction_set_id,source_id,source_revision_id,state,input_hash,
                        profile_id,authorization_revision,source_generation,extraction_commit_sequence,
                        observation_count,card_count,safe_error_code,semantic_target_key,rules_revision
                 FROM semantic_extraction_sets WHERE vault_id=? AND extraction_set_id=?",
            )
            .bind(&vault)
            .bind(existing_id)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok((extraction_row_to_record(row)?, false));
        }
        let current = sqlx::query_as::<_, (Option<String>, String, i64, Option<String>, i64, i64, i64)>(
            "SELECT current_revision_id,current_content_hash,eligible,invalid_reason,authorization_revision,source_generation,extraction_commit_sequence FROM semantic_sources
             WHERE vault_id=? AND source_id=?",
        )
        .bind(&vault)
        .bind(source_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StateError::InvalidInput("semantic source is unavailable"))?;
        if current.0.as_deref() != Some(&source_revision_id.to_string()) {
            return Err(StateError::Conflict);
        }
        let revision_fence: (String, i64) = sqlx::query_as(
            "SELECT content_hash,source_generation FROM semantic_source_revisions
             WHERE vault_id=? AND source_revision_id=? AND source_id=?",
        )
        .bind(&vault)
        .bind(source_revision_id.to_string())
        .bind(source_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if revision_fence.0 != current.1 || revision_fence.1 != current.5 {
            return Err(StateError::Conflict);
        }
        let rules_revision =
            crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?;
        let target = crate::semantic_rules::ensure_target_tx(
            &mut tx,
            context,
            "source_extraction",
            "source",
            1,
            &format!("{source_id}:{source_revision_id}:{profile_id}"),
        )
        .await?;
        sqlx::query(
            "UPDATE semantic_sources SET extraction_commit_sequence=extraction_commit_sequence+1
             WHERE vault_id=? AND source_id=?",
        )
        .bind(&vault)
        .bind(source_id.to_string())
        .execute(&mut *tx)
        .await?;
        let extraction_commit_sequence: i64 = sqlx::query_scalar(
            "SELECT extraction_commit_sequence FROM semantic_sources
             WHERE vault_id=? AND source_id=?",
        )
        .bind(&vault)
        .bind(source_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let now = now_millis()?;
        sqlx::query(
            "INSERT INTO semantic_extraction_sets
             (vault_id,extraction_set_id,source_id,source_revision_id,state,input_hash,profile_id,
              authorization_revision,source_generation,extraction_commit_sequence,request_hash,created_at,
              semantic_target_key,rules_revision)
             VALUES(?,?,?,?,'running',?,?,?,?,?,?,?, ?,?)",
        )
        .bind(&vault)
        .bind(id.to_string())
        .bind(source_id.to_string())
        .bind(source_revision_id.to_string())
        .bind(input_hash)
        .bind(profile_id)
        .bind(current.4)
        .bind(current.5)
        .bind(extraction_commit_sequence)
        .bind(request_hash)
        .bind(now)
        .bind(&target.target_key)
        .bind(rules_revision)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO semantic_extraction_idempotency
             (vault_id,idempotency_key,extraction_set_id,request_hash,created_at) VALUES(?,?,?,?,?)",
        )
        .bind(&vault)
        .bind(idempotency_key)
        .bind(id.to_string())
        .bind(request_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query_as::<_, ExtractionRow>(
            "SELECT vault_id,extraction_set_id,source_id,source_revision_id,state,input_hash,
                    profile_id,authorization_revision,source_generation,extraction_commit_sequence,
                    observation_count,card_count,safe_error_code,semantic_target_key,rules_revision
             FROM semantic_extraction_sets WHERE vault_id=? AND extraction_set_id=?",
        )
        .bind(&vault)
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((extraction_row_to_record(row)?, true))
    }

    /// Record failure, partial completion, or cancellation without converting it to an empty set.
    pub async fn finish_empty_or_nonpublishable(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        state: &str,
        safe_error_code: Option<&str>,
    ) -> Result<(), StateError> {
        if !matches!(state, "success_empty" | "partial" | "failed" | "cancelled") {
            return Err(StateError::InvalidInput(
                "invalid semantic extraction terminal state",
            ));
        }
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let row: Option<ExtractionFenceRow> = sqlx::query_as(
            "SELECT e.source_id,e.source_revision_id,s.current_revision_id,
                    e.authorization_revision,s.authorization_revision,
                    e.source_generation,r.source_generation,s.source_generation,
                    e.extraction_commit_sequence,s.extraction_commit_sequence
             FROM semantic_extraction_sets e JOIN semantic_sources s
               ON s.vault_id=e.vault_id AND s.source_id=e.source_id
             JOIN semantic_source_revisions r
               ON r.vault_id=e.vault_id AND r.source_revision_id=e.source_revision_id
              AND r.source_id=e.source_id
             WHERE e.vault_id=? AND e.extraction_set_id=? AND e.state='running'",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((
            source_id,
            revision_id,
            current_revision_id,
            extraction_auth,
            source_auth,
            extraction_generation,
            revision_generation,
            source_generation,
            extraction_commit_sequence,
            source_commit_sequence,
        )) = row
        else {
            return Err(StateError::Conflict);
        };
        let extraction_rules_revision: i64 = sqlx::query_scalar(
            "SELECT rules_revision FROM semantic_extraction_sets
             WHERE vault_id=? AND extraction_set_id=?",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if extraction_rules_revision
            != crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?
        {
            return Err(StateError::Conflict);
        }
        if state == "success_empty"
            && (Some(revision_id.as_str()) != current_revision_id.as_deref()
                || extraction_auth != source_auth
                || extraction_generation != source_generation
                || revision_generation != source_generation
                || extraction_commit_sequence != source_commit_sequence)
        {
            return Err(StateError::Conflict);
        }
        sqlx::query(
            "UPDATE semantic_extraction_sets SET state=?,safe_error_code=?,observation_count=0,
                card_count=0,completed_at=? WHERE vault_id=? AND extraction_set_id=? AND state='running'",
        )
        .bind(state)
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        if state == "success_empty" {
            sqlx::query(
                "UPDATE semantic_memory_cards SET eligibility='invalidated',updated_at=?
                 WHERE vault_id=? AND source_id=?",
            )
            .bind(now)
            .bind(context.id().to_string())
            .bind(&source_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE semantic_evidence_refs SET validation_status='invalidated'
                 WHERE vault_id=? AND source_id=?",
            )
            .bind(context.id().to_string())
            .bind(&source_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE semantic_sources SET eligible=1,invalid_reason=NULL,pending_rebuild=0,updated_at=?
                WHERE vault_id=? AND source_id=? AND current_revision_id=? AND authorization_revision=?",
            )
            .bind(now)
            .bind(context.id().to_string())
            .bind(source_id)
            .bind(revision_id)
            .bind(source_auth)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Persist observations, card revisions and exact canonical bytes before any Core write.
    pub async fn prepare_publication(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        observations: &[SemanticObservationInput],
        cards: &[SemanticCardRevisionInput],
    ) -> Result<Vec<SemanticPreparedSnapshot>, StateError> {
        if observations.is_empty() || cards.is_empty() {
            return Err(StateError::InvalidInput(
                "nonempty semantic publication requires observations and cards",
            ));
        }
        let vault = context.id().to_string();
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let (
            source_text,
            revision_text,
            current_revision,
            input_hash,
            current_hash,
            extraction_auth,
            source_auth,
            _invalid_reason,
            extraction_generation,
            source_generation,
            revision_generation,
            extraction_commit_sequence,
            source_commit_sequence,
            extraction_profile,
        ): PublicationSourceSnapshot = sqlx::query_as(
            "SELECT e.source_id,e.source_revision_id,s.current_revision_id,e.input_hash,s.current_content_hash,
                    e.authorization_revision,s.authorization_revision,s.invalid_reason,
                    e.source_generation,s.source_generation,r.source_generation,
                    e.extraction_commit_sequence,s.extraction_commit_sequence,e.profile_id
             FROM semantic_extraction_sets e JOIN semantic_sources s
               ON s.vault_id=e.vault_id AND s.source_id=e.source_id
             JOIN semantic_source_revisions r
               ON r.vault_id=e.vault_id AND r.source_revision_id=e.source_revision_id
              AND r.source_id=e.source_id
             WHERE e.vault_id=? AND e.extraction_set_id=? AND e.state='running'",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StateError::Conflict)?;
        if revision_text != current_revision.unwrap_or_default() {
            return Err(StateError::Conflict);
        }
        if input_hash != current_hash {
            return Err(StateError::Conflict);
        }
        if extraction_auth != source_auth
            || extraction_generation != source_generation
            || revision_generation != source_generation
            || extraction_commit_sequence != source_commit_sequence
        {
            return Err(StateError::Conflict);
        }
        let source_id = SemanticSourceId::parse(&source_text)?;
        let source_revision_id = SourceRevisionId::parse(&revision_text)?;
        let (_extraction_target_key, extraction_rules_revision): (String, i64) = sqlx::query_as(
            "SELECT semantic_target_key,rules_revision FROM semantic_extraction_sets
             WHERE vault_id=? AND extraction_set_id=?",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if extraction_rules_revision
            != crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?
        {
            return Err(StateError::Conflict);
        }
        if cards
            .iter()
            .any(|card| card.composition_profile_id != extraction_profile)
        {
            return Err(StateError::Conflict);
        }

        for observation in observations {
            sqlx::query(
                "INSERT INTO semantic_evidence_refs
                 (vault_id,evidence_ref_id,source_id,source_revision_id,validation_status,created_at)
                 VALUES(?,?,?,?,'validated',?)",
            )
            .bind(&vault)
            .bind(observation.evidence.id.to_string())
            .bind(source_id.to_string())
            .bind(source_revision_id.to_string())
            .bind(now)
            .execute(&mut *tx)
            .await?;
            insert_spans(
                &mut tx,
                context,
                observation.evidence.id,
                source_revision_id,
                "body",
                &observation.evidence.body_spans,
            )
            .await?;
            insert_spans(
                &mut tx,
                context,
                observation.evidence.id,
                source_revision_id,
                "context",
                &observation.evidence.context_spans,
            )
            .await?;
            sqlx::query(
                "INSERT INTO semantic_observations
                 (vault_id,observation_id,source_id,source_revision_id,extraction_set_id,
                  local_observation_key,kind,advisory_kind_unknown,statement,scope,assertion_status,source_time_scope_json,
                  conditions_json,exceptions_json,ordered_steps_json,result_json,uncertainty_json,
                  admission_reason,value_for_future_work,created_at)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(&vault)
            .bind(observation.id.to_string())
            .bind(source_id.to_string())
            .bind(source_revision_id.to_string())
            .bind(extraction_set_id.to_string())
            .bind(&observation.local_key)
            .bind(if observation.kind == "unknown" {
                "state"
            } else {
                observation.kind.as_str()
            })
            .bind(i64::from(observation.kind == "unknown"))
            .bind(&observation.statement)
            .bind(&observation.scope)
            .bind(&observation.assertion_status)
            .bind(serde_json::to_string(&observation.source_time_scope)?)
            .bind(serde_json::to_string(&observation.conditions)?)
            .bind(serde_json::to_string(&observation.exceptions)?)
            .bind(serde_json::to_string(&observation.ordered_steps)?)
            .bind(observation.result.as_deref())
            .bind(observation.uncertainty.as_deref())
            .bind(&observation.admission_reason)
            .bind(&observation.value_for_future_work)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO semantic_observation_evidence(vault_id,observation_id,source_id,evidence_ref_id)
                 VALUES(?,?,?,?)",
            )
            .bind(&vault)
            .bind(observation.id.to_string())
            .bind(source_id.to_string())
            .bind(observation.evidence.id.to_string())
            .execute(&mut *tx)
            .await?;
        }

        let mut snapshots = Vec::with_capacity(cards.len());
        for card in cards {
            let semantic_fingerprint =
                semantic_card_fingerprint(&mut tx, context, card, source_id).await?;
            let existing_target: Option<(String, String)> = sqlx::query_as(
                "SELECT r.semantic_target_key,t.fingerprint FROM semantic_memory_cards c
                 JOIN semantic_card_revisions r ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
                 JOIN semantic_targets t ON t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key
                 WHERE c.vault_id=? AND c.card_id=? AND r.semantic_target_key<>''",
            )
            .bind(&vault)
            .bind(card.card_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            let card_target_key = if let Some((existing, existing_fingerprint)) = existing_target {
                if existing_fingerprint == semantic_fingerprint {
                    existing
                } else {
                    crate::semantic_rules::ensure_target_tx(
                        &mut tx,
                        context,
                        "memory_card",
                        &card.scope_ref,
                        2,
                        &semantic_fingerprint,
                    )
                    .await?
                    .target_key
                }
            } else {
                crate::semantic_rules::ensure_target_tx(
                    &mut tx,
                    context,
                    "memory_card",
                    &card.scope_ref,
                    2,
                    &semantic_fingerprint,
                )
                .await?
                .target_key
            };
            let dependencies = vec![(source_id.to_string(), revision_text.clone())];
            let observations = card
                .items
                .iter()
                .map(|item| item.observation_id.to_string())
                .collect::<Vec<_>>();
            if crate::semantic_rules::target_blocks_generation(
                &mut tx,
                context,
                "memory_card",
                &card_target_key,
                &card.scope_ref,
                &dependencies,
                &observations,
                Some(&card.card_id.to_string()),
                None,
                std::str::from_utf8(&card.canonical_bytes).unwrap_or_default(),
            )
            .await?
            {
                return Err(StateError::Conflict);
            }
            if card.canonical_bytes.is_empty() || card.canonical_markdown_hash.is_empty() {
                return Err(StateError::InvalidInput(
                    "semantic card snapshot bytes and hash are required",
                ));
            }
            let current_row: Option<(Option<String>,)> = sqlx::query_as(
                "SELECT current_revision_id FROM semantic_memory_cards
                 WHERE vault_id=? AND card_id=? AND source_id=?",
            )
            .bind(&vault)
            .bind(card.card_id.to_string())
            .bind(source_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            let current = current_row.and_then(|(revision_id,)| revision_id);
            if current.as_deref()
                != card
                    .expected_card_revision_id
                    .map(|id| id.to_string())
                    .as_deref()
            {
                return Err(StateError::Conflict);
            }
            // A card item and every support edge must resolve to the exact
            // source revision fenced by this extraction.  The schema's
            // source-id foreign keys intentionally do not substitute for
            // this revision-level check.
            for item in &card.items {
                let observation_row: Option<(String, String)> = sqlx::query_as(
                    "SELECT source_revision_id,extraction_set_id
                     FROM semantic_observations
                     WHERE vault_id=? AND observation_id=? AND source_id=?",
                )
                .bind(&vault)
                .bind(item.observation_id.to_string())
                .bind(source_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
                let Some((observation_revision, observation_extraction)) = observation_row else {
                    return Err(StateError::Conflict);
                };
                if observation_revision != revision_text
                    || observation_extraction != extraction_set_id.to_string()
                {
                    return Err(StateError::Conflict);
                }
                for evidence_id in &item.evidence_ref_ids {
                    let evidence_revision: Option<String> = sqlx::query_scalar(
                        "SELECT source_revision_id FROM semantic_evidence_refs
                         WHERE vault_id=? AND evidence_ref_id=? AND source_id=?",
                    )
                    .bind(&vault)
                    .bind(evidence_id.to_string())
                    .bind(source_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await?;
                    if evidence_revision.as_deref() != Some(revision_text.as_str()) {
                        return Err(StateError::Conflict);
                    }
                    let linked: Option<i64> = sqlx::query_scalar(
                        "SELECT 1 FROM semantic_observation_evidence
                         WHERE vault_id=? AND observation_id=? AND source_id=? AND evidence_ref_id=?",
                    )
                    .bind(&vault)
                    .bind(item.observation_id.to_string())
                    .bind(source_id.to_string())
                    .bind(evidence_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await?;
                    if linked != Some(1) {
                        return Err(StateError::Conflict);
                    }
                    let span_mismatch: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM semantic_evidence_spans
                         WHERE vault_id=? AND evidence_ref_id=? AND source_revision_id<>?",
                    )
                    .bind(&vault)
                    .bind(evidence_id.to_string())
                    .bind(&revision_text)
                    .fetch_one(&mut *tx)
                    .await?;
                    if span_mismatch != 0 {
                        return Err(StateError::Conflict);
                    }
                }
            }
            if current.is_none() {
                sqlx::query(
                    "INSERT INTO semantic_memory_cards
                     (vault_id,card_id,source_id,topic_key,current_revision_id,eligibility,created_at,updated_at)
                     VALUES(?,?,?,?,NULL,'pending',?,?)",
                )
                .bind(&vault)
                .bind(card.card_id.to_string())
                .bind(source_id.to_string())
                .bind(&card.topic_key)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
            let prior_file: Option<(Option<String>, Option<i64>)> = if card
                .expected_card_revision_id
                .is_some()
            {
                sqlx::query_as(
                    "SELECT canonical_file_id,canonical_revision FROM semantic_card_revisions
                     WHERE vault_id=? AND card_revision_id=? AND card_id=? AND publication_state='published'",
                )
                .bind(&vault)
                .bind(card.expected_card_revision_id.map(|id| id.to_string()))
                .bind(card.card_id.to_string())
                .fetch_optional(&mut *tx)
                .await?
            } else {
                None
            };
            if card.expected_card_revision_id.is_some() && prior_file.is_none() {
                return Err(StateError::Conflict);
            }
            let expected_file_id = prior_file.as_ref().and_then(|(id, _)| id.clone());
            let expected_file_revision = prior_file.as_ref().and_then(|(_, revision)| *revision);
            if card.expected_card_revision_id.is_some()
                && (expected_file_id.is_none() || expected_file_revision.is_none())
            {
                return Err(StateError::Conflict);
            }
            if let Some(expected_id) = card.expected_card_revision_id {
                let prior_number: i64 = sqlx::query_scalar(
                    "SELECT revision_number FROM semantic_card_revisions
                     WHERE vault_id=? AND card_revision_id=? AND card_id=? AND publication_state='published'",
                )
                .bind(&vault)
                .bind(expected_id.to_string())
                .bind(card.card_id.to_string())
                .fetch_one(&mut *tx)
                .await?;
                if i64::from(card.revision_number) != prior_number + 1 {
                    return Err(StateError::Conflict);
                }
            } else if card.revision_number != 1 {
                return Err(StateError::Conflict);
            }
            let proposed_file_revision = expected_file_revision.unwrap_or(0) + 1;
            let snapshot_id = PreparedSnapshotId::new();
            sqlx::query(
                "INSERT INTO semantic_card_revisions
                 (vault_id,card_revision_id,card_id,source_id,source_revision_id,extraction_set_id,
                  revision_number,title,kind,scope_ref,assertion_status,temporal_scope_json,
                  composition_profile_id,canonical_markdown_hash,canonical_file_id,canonical_path,
                 canonical_revision,publication_state,created_at,semantic_target_key,rules_revision)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,NULL,?,NULL,'prepared',?,?,?)",
            )
            .bind(&vault)
            .bind(card.card_revision_id.to_string())
            .bind(card.card_id.to_string())
            .bind(source_id.to_string())
            .bind(source_revision_id.to_string())
            .bind(extraction_set_id.to_string())
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
            .bind(&card_target_key)
            .bind(extraction_rules_revision)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO semantic_card_dependencies
                 (vault_id,card_revision_id,card_id,source_id,source_revision_id) VALUES(?,?,?,?,?)",
            )
            .bind(&vault)
            .bind(card.card_revision_id.to_string())
            .bind(card.card_id.to_string())
            .bind(source_id.to_string())
            .bind(source_revision_id.to_string())
            .execute(&mut *tx)
            .await?;
            for item in &card.items {
                if item.evidence_ref_ids.is_empty() {
                    return Err(StateError::InvalidInput(
                        "each semantic card item requires evidence",
                    ));
                }
                sqlx::query(
                    "INSERT INTO semantic_card_items
                     (vault_id,item_id,card_revision_id,card_id,source_id,observation_id,item_kind,ordinal,content)
                     VALUES(?,?,?,?,?,?,?,?,?)",
                )
                .bind(&vault)
                .bind(item.id.to_string())
                .bind(card.card_revision_id.to_string())
                .bind(card.card_id.to_string())
                .bind(source_id.to_string())
                .bind(item.observation_id.to_string())
                .bind(&item.kind)
                .bind(i64::from(item.ordinal))
                .bind(&item.content)
                .execute(&mut *tx)
                .await?;
                for evidence_id in &item.evidence_ref_ids {
                    sqlx::query(
                        "INSERT INTO semantic_card_assertion_supports
                         (vault_id,item_id,card_revision_id,source_id,observation_id,evidence_ref_id,support_kind)
                         VALUES(?,?,?,?,?,?,'single')",
                    )
                    .bind(&vault)
                    .bind(item.id.to_string())
                    .bind(card.card_revision_id.to_string())
                    .bind(source_id.to_string())
                    .bind(item.observation_id.to_string())
                    .bind(evidence_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                }
            }
            sqlx::query(
                "INSERT INTO semantic_prepared_snapshots
                (vault_id,snapshot_id,extraction_set_id,card_id,card_revision_id,source_id,source_revision_id,
                  source_content_hash,authorization_revision,source_generation,extraction_commit_sequence,
                  expected_card_revision_id,proposed_card_revision_number,
                  expected_file_id,expected_file_revision,proposed_file_revision,
                  target_path,proposed_file_hash,canonical_bytes,status,created_at,updated_at,semantic_target_key,rules_revision)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,'prepared',?,?,?,?)",
            )
            .bind(&vault)
            .bind(snapshot_id.to_string())
            .bind(extraction_set_id.to_string())
            .bind(card.card_id.to_string())
            .bind(card.card_revision_id.to_string())
            .bind(source_id.to_string())
            .bind(source_revision_id.to_string())
            .bind(&current_hash)
            .bind(extraction_auth)
            .bind(source_generation)
            .bind(extraction_commit_sequence)
            .bind(card.expected_card_revision_id.map(|id| id.to_string()))
            .bind(i64::from(card.revision_number))
            .bind(expected_file_id.as_deref())
            .bind(expected_file_revision)
            .bind(proposed_file_revision)
            .bind(card.canonical_path.as_str())
            .bind(&card.canonical_markdown_hash)
            .bind(&card.canonical_bytes)
            .bind(now)
            .bind(now)
            .bind(&card_target_key)
            .bind(extraction_rules_revision)
            .execute(&mut *tx)
            .await?;
            snapshots.push(SemanticPreparedSnapshot {
                id: snapshot_id,
                extraction_set_id,
                card_id: card.card_id,
                card_revision_id: card.card_revision_id,
                source_id,
                source_revision_id,
                source_content_hash: current_hash.clone(),
                authorization_revision: extraction_auth,
                source_generation,
                extraction_commit_sequence,
                expected_card_revision_id: card.expected_card_revision_id,
                proposed_card_revision_number: card.revision_number,
                expected_file_id: expected_file_id.as_deref().map(FileId::parse).transpose()?,
                expected_file_revision: expected_file_revision
                    .map(Revision::try_from)
                    .transpose()?,
                proposed_file_revision: Revision::try_from(proposed_file_revision)?,
                published_file_id: None,
                published_file_revision: None,
                target_path: card.canonical_path.clone(),
                proposed_file_hash: card.canonical_markdown_hash.clone(),
                canonical_bytes: card.canonical_bytes.clone(),
                status: "prepared".to_owned(),
                semantic_target_key: card_target_key,
                rules_revision: extraction_rules_revision,
            });
        }
        sqlx::query(
            "UPDATE semantic_extraction_sets SET state='prepared',observation_count=?,card_count=?
             WHERE vault_id=? AND extraction_set_id=? AND state='running'",
        )
        .bind(i64::try_from(observations.len()).unwrap_or(i64::MAX))
        .bind(i64::try_from(cards.len()).unwrap_or(i64::MAX))
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(snapshots)
    }

    pub async fn pending_snapshots(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<Vec<SemanticPreparedSnapshot>, StateError> {
        let rows = sqlx::query_as::<_, SnapshotRow>(
            "SELECT snapshot_id,extraction_set_id,card_id,card_revision_id,source_id,source_revision_id,
                    source_content_hash,authorization_revision,source_generation,extraction_commit_sequence,
                    expected_card_revision_id,proposed_card_revision_number,
                    expected_file_id,expected_file_revision,proposed_file_revision,
                    published_file_id,published_file_revision,target_path,proposed_file_hash,canonical_bytes,status,
                    semantic_target_key,rules_revision
             FROM semantic_prepared_snapshots p
             WHERE vault_id=? AND extraction_set_id=?
               AND (status IN ('prepared','written') OR (status='blocked' AND EXISTS (
                   SELECT 1 FROM operation_journal j
                   WHERE j.vault_id=p.vault_id
                     AND (j.destination_path=p.target_path OR j.source_path=p.target_path)
                     AND j.state IN ('prepared','file_committed','needs_review')
               )))
             ORDER BY snapshot_id",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(snapshot_row_to_record).collect()
    }

    pub async fn pending_extractions(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<ExtractionSetId>, StateError> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT extraction_set_id FROM semantic_extraction_sets
             WHERE vault_id=? AND (
                 state='prepared'
                 OR (state='partial' AND EXISTS (
                     SELECT 1
                     FROM semantic_prepared_snapshots p
                     JOIN operation_journal j
                       ON j.vault_id=p.vault_id
                      AND (j.destination_path=p.target_path OR j.source_path=p.target_path)
                      AND j.state IN ('prepared','file_committed','needs_review')
                     WHERE p.vault_id=semantic_extraction_sets.vault_id
                       AND p.extraction_set_id=semantic_extraction_sets.extraction_set_id
                       AND p.status='blocked'
                 ))
             ) ORDER BY created_at,extraction_set_id",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|id| ExtractionSetId::parse(id).map_err(StateError::from))
            .collect()
    }

    /// Record the Core write witness after independently verifying exact bytes in the caller.
    pub async fn mark_snapshot_written(
        &self,
        context: &VaultContext,
        snapshot_id: PreparedSnapshotId,
        file_id: FileId,
        revision: Revision,
    ) -> Result<(), StateError> {
        let revision_i64 = revision.as_i64()?;
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let row: Option<(Option<String>, i64, String, i64, i64, i64)> = sqlx::query_as(
            "SELECT p.expected_file_id,p.proposed_file_revision,p.source_id,
                    p.extraction_commit_sequence,s.extraction_commit_sequence,p.rules_revision
             FROM semantic_prepared_snapshots p JOIN semantic_sources s
               ON s.vault_id=p.vault_id AND s.source_id=p.source_id
             WHERE p.vault_id=? AND p.snapshot_id=? AND p.status IN ('prepared','written')",
        )
        .bind(context.id().to_string())
        .bind(snapshot_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((
            expected_file_id,
            proposed_revision,
            _source_id,
            snapshot_sequence,
            source_sequence,
            snapshot_rules_revision,
        )) = row
        else {
            return Err(StateError::Conflict);
        };
        let written_file_id = file_id.to_string();
        let current_rules_revision =
            crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?;
        if snapshot_sequence != source_sequence
            || snapshot_rules_revision != current_rules_revision
            || revision_i64 != proposed_revision
            || expected_file_id
                .as_deref()
                .is_some_and(|expected| expected != written_file_id)
        {
            return Err(StateError::Conflict);
        }
        sqlx::query(
            "UPDATE semantic_prepared_snapshots SET status='written',published_file_id=?,
                published_file_revision=?,updated_at=? WHERE vault_id=? AND snapshot_id=?",
        )
        .bind(file_id.to_string())
        .bind(revision_i64)
        .bind(now)
        .bind(context.id().to_string())
        .bind(snapshot_id.to_string())
        .execute(&mut *tx)
        .await?;
        let card_revision_id: String = sqlx::query_scalar(
            "SELECT card_revision_id FROM semantic_prepared_snapshots WHERE vault_id=? AND snapshot_id=?",
        )
        .bind(context.id().to_string())
        .bind(snapshot_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_card_revisions SET canonical_file_id=?,canonical_revision=?
             WHERE vault_id=? AND card_revision_id=? AND publication_state='prepared'",
        )
        .bind(written_file_id)
        .bind(revision_i64)
        .bind(context.id().to_string())
        .bind(card_revision_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically publish every card in one complete extraction after all Core writes are witnessed.
    pub async fn apply_publication(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<(), StateError> {
        let now = now_millis()?;
        let vault = context.id().to_string();
        let mut tx = self.pool.begin().await?;
        let snapshots = sqlx::query_as::<_, SnapshotRow>(
            "SELECT snapshot_id,extraction_set_id,card_id,card_revision_id,source_id,source_revision_id,
                    source_content_hash,authorization_revision,source_generation,extraction_commit_sequence,
                    expected_card_revision_id,proposed_card_revision_number,
                    expected_file_id,expected_file_revision,proposed_file_revision,
                    published_file_id,published_file_revision,target_path,proposed_file_hash,canonical_bytes,status,
                    semantic_target_key,rules_revision
             FROM semantic_prepared_snapshots WHERE vault_id=? AND extraction_set_id=? ORDER BY snapshot_id",
        )
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        if snapshots.is_empty()
            || snapshots
                .iter()
                .any(|snapshot| snapshot.status != "written")
        {
            return Err(StateError::Conflict);
        }
        let current_rules_revision =
            crate::semantic_rules::current_rules_revision_tx(&mut tx, context).await?;
        if snapshots
            .iter()
            .any(|snapshot| snapshot.rules_revision != current_rules_revision)
        {
            return Err(StateError::Conflict);
        }
        let source_id = &snapshots[0].source_id;
        let source_revision_id = &snapshots[0].source_revision_id;
        if snapshots.iter().any(|snapshot| {
            snapshot.source_id != *source_id
                || snapshot.source_revision_id != *source_revision_id
                || snapshot.authorization_revision != snapshots[0].authorization_revision
                || snapshot.source_generation != snapshots[0].source_generation
                || snapshot.extraction_commit_sequence != snapshots[0].extraction_commit_sequence
        }) {
            return Err(StateError::Conflict);
        }
        // A complete result set replaces this source's current card set as one
        // projection. Cards omitted from the new set must not remain readable.
        sqlx::query(
            "UPDATE semantic_memory_cards SET eligibility='invalidated',updated_at=?
             WHERE vault_id=? AND source_id=? AND card_id NOT IN (
               SELECT card_id FROM semantic_prepared_snapshots
               WHERE vault_id=? AND extraction_set_id=?
             )",
        )
        .bind(now)
        .bind(&vault)
        .bind(source_id.to_string())
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        for snapshot in &snapshots {
            let source_current: Option<(Option<String>, String, i64, i64, i64)> = sqlx::query_as(
                "SELECT current_revision_id,current_content_hash,authorization_revision,source_generation,extraction_commit_sequence FROM semantic_sources
                 WHERE vault_id=? AND source_id=?",
            )
            .bind(&vault)
            .bind(&snapshot.source_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((
                current_revision,
                current_hash,
                source_auth,
                source_generation,
                source_commit_sequence,
            )) = source_current
            else {
                return Err(StateError::Conflict);
            };
            let (prepared_hash, revision_generation): (String, i64) = sqlx::query_as(
                "SELECT content_hash,source_generation FROM semantic_source_revisions
                 WHERE vault_id=? AND source_revision_id=? AND source_id=?",
            )
            .bind(&vault)
            .bind(&snapshot.source_revision_id)
            .bind(&snapshot.source_id)
            .fetch_one(&mut *tx)
            .await?;
            if current_revision.as_deref() != Some(snapshot.source_revision_id.as_str())
                || current_hash != snapshot.source_content_hash
                || prepared_hash != snapshot.source_content_hash
                || source_auth != snapshot.authorization_revision
                || source_generation != snapshot.source_generation
                || revision_generation != source_generation
                || source_commit_sequence != snapshot.extraction_commit_sequence
                || snapshot.published_file_revision != Some(snapshot.proposed_file_revision)
                || snapshot.published_file_id.is_none()
            {
                return Err(StateError::Conflict);
            }
            let current_card_row: Option<(Option<String>,)> = sqlx::query_as(
                "SELECT current_revision_id FROM semantic_memory_cards
                 WHERE vault_id=? AND card_id=? AND source_id=?",
            )
            .bind(&vault)
            .bind(&snapshot.card_id)
            .bind(&snapshot.source_id)
            .fetch_optional(&mut *tx)
            .await?;
            let current_card = current_card_row.and_then(|(revision_id,)| revision_id);
            if current_card != snapshot.expected_card_revision_id {
                return Err(StateError::Conflict);
            }
        }
        for snapshot in &snapshots {
            let result = sqlx::query(
                "UPDATE semantic_memory_cards SET current_revision_id=?,eligibility='readable',updated_at=?
                 WHERE vault_id=? AND card_id=? AND source_id=? AND current_revision_id IS ?",
            )
            .bind(&snapshot.card_revision_id)
            .bind(now)
            .bind(&vault)
            .bind(&snapshot.card_id)
            .bind(&snapshot.source_id)
            .bind(&snapshot.expected_card_revision_id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() != 1 {
                return Err(StateError::Conflict);
            }
            sqlx::query(
                "UPDATE semantic_card_revisions SET publication_state='published',canonical_file_id=?,canonical_revision=?
                 WHERE vault_id=? AND card_revision_id=? AND publication_state='prepared'",
            )
            .bind(&snapshot.published_file_id)
            .bind(snapshot.published_file_revision)
            .bind(&vault)
            .bind(&snapshot.card_revision_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE semantic_prepared_snapshots SET status='applied',updated_at=?
                 WHERE vault_id=? AND snapshot_id=? AND status='written'",
            )
            .bind(now)
            .bind(&vault)
            .bind(&snapshot.snapshot_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE semantic_sources SET eligible=1,invalid_reason=NULL,pending_rebuild=0,updated_at=?
             WHERE vault_id=? AND source_id=? AND current_revision_id=? AND authorization_revision=?",
        )
        .bind(now)
        .bind(&vault)
        .bind(source_id)
        .bind(source_revision_id)
        .bind(snapshots[0].authorization_revision)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_extraction_sets SET state='success_nonempty',completed_at=?
             WHERE vault_id=? AND extraction_set_id=? AND state='prepared'",
        )
        .bind(now)
        .bind(&vault)
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn reject_publication(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        safe_error_code: &str,
    ) -> Result<(), StateError> {
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE semantic_prepared_snapshots SET status='blocked',safe_error_code=?,updated_at=?
             WHERE vault_id=? AND extraction_set_id=? AND status IN ('prepared','written')",
        )
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_card_revisions SET publication_state='blocked'
             WHERE vault_id=? AND extraction_set_id=? AND publication_state='prepared'",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_extraction_sets SET state='partial',safe_error_code=?,completed_at=?
             WHERE vault_id=? AND extraction_set_id=? AND state='prepared'",
        )
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Cancel an extraction before publication. Prepared/written work is not
    /// cancellable here: once a canonical write may have started, recovery
    /// must use the existing witness/barrier path instead of pretending that
    /// cancellation erased it.
    pub async fn cancel_extraction(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        safe_error_code: &str,
    ) -> Result<(), StateError> {
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM semantic_extraction_sets
             WHERE vault_id=? AND extraction_set_id=?",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        match state.as_deref() {
            Some("cancelled") => {
                tx.commit().await?;
                return Ok(());
            }
            Some("running") => {}
            Some("prepared" | "partial") => {
                return Err(StateError::Conflict);
            }
            Some("success_empty" | "success_nonempty" | "failed") | None => {
                return Err(StateError::Conflict);
            }
            Some(_) => return Err(StateError::IntegrityFailure),
        }
        sqlx::query(
            "UPDATE semantic_extraction_sets
             SET state='cancelled',safe_error_code=?,observation_count=0,card_count=0,completed_at=?
             WHERE vault_id=? AND extraction_set_id=? AND state='running'",
        )
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_prepared_snapshots SET status='blocked',safe_error_code=?,updated_at=?
             WHERE vault_id=? AND extraction_set_id=? AND status IN ('prepared','written')",
        )
        .bind(safe_error_code)
        .bind(now)
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_card_revisions SET publication_state='blocked'
             WHERE vault_id=? AND extraction_set_id=? AND publication_state='prepared'",
        )
        .bind(context.id().to_string())
        .bind(extraction_set_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Immediately remove one source and every dependent card/evidence object from read eligibility.
    pub async fn invalidate_source(
        &self,
        context: &VaultContext,
        file_id: FileId,
        reason: &str,
        authorization_revision: Option<i64>,
    ) -> Result<bool, StateError> {
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        let changed = invalidate_source_tx(
            &mut tx,
            &context.id().to_string(),
            &file_id.to_string(),
            reason,
            authorization_revision,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn invalidate_all_sources(
        &self,
        context: &VaultContext,
        reason: &str,
        authorization_revision: Option<i64>,
    ) -> Result<(), StateError> {
        let now = now_millis()?;
        let mut tx = self.pool.begin().await?;
        invalidate_all_sources_tx(
            &mut tx,
            &context.id().to_string(),
            reason,
            authorization_revision,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// List only currently eligible cards; hidden-card counts are never exposed here.
    pub async fn list_cards(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<SemanticCardRecord>, StateError> {
        if limit == 0 || limit > 200 {
            return Err(StateError::InvalidInput(
                "semantic card limit must be between 1 and 200",
            ));
        }
        let rows = sqlx::query_as::<_, CardHeaderRow>(
            "SELECT c.card_id,r.card_revision_id,c.source_id,r.source_revision_id,s.source_path,
                    r.title,r.kind,r.scope_ref,r.assertion_status,r.temporal_scope_json,r.revision_number,
                    r.canonical_path,r.canonical_file_id,r.canonical_revision,r.canonical_markdown_hash
             FROM semantic_memory_cards c
             JOIN semantic_card_revisions r ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
             JOIN semantic_sources s ON s.vault_id=c.vault_id AND s.source_id=c.source_id
             JOIN semantic_card_dependencies d ON d.vault_id=r.vault_id AND d.card_revision_id=r.card_revision_id
             WHERE c.vault_id=? AND c.eligibility='readable' AND r.publication_state='published'
               AND s.eligible=1 AND s.pending_rebuild=0 AND s.current_revision_id=d.source_revision_id
               AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                 WHERE x.vault_id=r.vault_id AND x.active=1
                   AND x.action IN ('suppress_read','forget_current')
                   AND (
                     (x.semantic_target_key=r.semantic_target_key
                       AND (x.source_id IS NULL OR x.source_id=c.source_id)
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.card_id IS NULL OR x.card_id=c.card_id))
                     OR (x.card_id IS NOT NULL
                       AND x.card_id=c.card_id
                       AND (x.source_id IS NULL OR x.source_id=c.source_id)
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.observation_id IS NULL OR EXISTS (
                         SELECT 1 FROM semantic_card_items ci
                         WHERE ci.vault_id=r.vault_id AND ci.card_revision_id=r.card_revision_id
                           AND ci.observation_id=x.observation_id)))
                     OR (x.card_id IS NULL AND x.source_id=c.source_id
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.observation_id IS NULL OR EXISTS (
                         SELECT 1 FROM semantic_card_items ci
                         WHERE ci.vault_id=r.vault_id AND ci.card_revision_id=r.card_revision_id
                           AND ci.observation_id=x.observation_id)))
                   ))
               AND NOT EXISTS (SELECT 1 FROM semantic_task_states t
                 WHERE t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key AND t.state='completed')
             ORDER BY r.title COLLATE NOCASE,c.card_id LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        self.build_card_records(context, rows).await
    }

    pub async fn get_card(
        &self,
        context: &VaultContext,
        card_id: MemoryCardId,
    ) -> Result<Option<SemanticCardRecord>, StateError> {
        let rows = sqlx::query_as::<_, CardHeaderRow>(
            "SELECT c.card_id,r.card_revision_id,c.source_id,r.source_revision_id,s.source_path,
                    r.title,r.kind,r.scope_ref,r.assertion_status,r.temporal_scope_json,r.revision_number,
                    r.canonical_path,r.canonical_file_id,r.canonical_revision,r.canonical_markdown_hash
             FROM semantic_memory_cards c
             JOIN semantic_card_revisions r ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
             JOIN semantic_sources s ON s.vault_id=c.vault_id AND s.source_id=c.source_id
             JOIN semantic_card_dependencies d ON d.vault_id=r.vault_id AND d.card_revision_id=r.card_revision_id
             WHERE c.vault_id=? AND c.card_id=? AND c.eligibility='readable' AND r.publication_state='published'
               AND s.eligible=1 AND s.pending_rebuild=0 AND s.current_revision_id=d.source_revision_id
               AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                 WHERE x.vault_id=r.vault_id AND x.active=1
                   AND x.action IN ('suppress_read','forget_current')
                   AND (
                     (x.semantic_target_key=r.semantic_target_key
                       AND (x.source_id IS NULL OR x.source_id=c.source_id)
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.card_id IS NULL OR x.card_id=c.card_id))
                     OR (x.card_id IS NOT NULL
                       AND x.card_id=c.card_id
                       AND (x.source_id IS NULL OR x.source_id=c.source_id)
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.observation_id IS NULL OR EXISTS (
                         SELECT 1 FROM semantic_card_items ci
                         WHERE ci.vault_id=r.vault_id AND ci.card_revision_id=r.card_revision_id
                           AND ci.observation_id=x.observation_id)))
                     OR (x.card_id IS NULL AND x.source_id=c.source_id
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.observation_id IS NULL OR EXISTS (
                         SELECT 1 FROM semantic_card_items ci
                         WHERE ci.vault_id=r.vault_id AND ci.card_revision_id=r.card_revision_id
                           AND ci.observation_id=x.observation_id)))
                   ))
               AND NOT EXISTS (SELECT 1 FROM semantic_task_states t
                 WHERE t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key AND t.state='completed')",
        )
        .bind(context.id().to_string())
        .bind(card_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(self
            .build_card_records(context, rows)
            .await?
            .into_iter()
            .next())
    }

    /// Load all currently readable cards for one source revision without a
    /// presentation-page limit.  Evaluation and source-scoped maintenance
    /// must not guess that the target card is in the first 200 global cards.
    pub async fn list_cards_for_source_revision(
        &self,
        context: &VaultContext,
        source_id: SemanticSourceId,
        source_revision_id: SourceRevisionId,
    ) -> Result<Vec<SemanticCardRecord>, StateError> {
        let rows = sqlx::query_as::<_, CardHeaderRow>(
            "SELECT c.card_id,r.card_revision_id,c.source_id,r.source_revision_id,s.source_path,
                    r.title,r.kind,r.scope_ref,r.assertion_status,r.temporal_scope_json,r.revision_number,
                    r.canonical_path,r.canonical_file_id,r.canonical_revision,r.canonical_markdown_hash
             FROM semantic_memory_cards c
             JOIN semantic_card_revisions r ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
             JOIN semantic_sources s ON s.vault_id=c.vault_id AND s.source_id=c.source_id
             JOIN semantic_card_dependencies d ON d.vault_id=r.vault_id AND d.card_revision_id=r.card_revision_id
             WHERE c.vault_id=? AND c.source_id=? AND r.source_revision_id=?
               AND c.eligibility='readable' AND r.publication_state='published'
               AND s.eligible=1 AND s.pending_rebuild=0 AND s.current_revision_id=d.source_revision_id
               AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                 WHERE x.vault_id=r.vault_id AND x.active=1
                   AND x.action IN ('suppress_read','forget_current')
                   AND ((x.semantic_target_key=r.semantic_target_key
                     AND (x.source_id IS NULL OR x.source_id=c.source_id)
                     AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                     AND x.composed_card_id IS NULL
                     AND (x.card_id IS NULL OR x.card_id=c.card_id))
                     OR (x.card_id IS NOT NULL AND x.card_id=c.card_id
                       AND (x.source_id IS NULL OR x.source_id=c.source_id)
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.observation_id IS NULL OR EXISTS (SELECT 1 FROM semantic_card_items ci
                         WHERE ci.vault_id=r.vault_id AND ci.card_revision_id=r.card_revision_id
                           AND ci.observation_id=x.observation_id)))
                     OR (x.card_id IS NULL AND x.source_id=c.source_id
                       AND (x.source_revision_id IS NULL OR x.source_revision_id=r.source_revision_id)
                       AND x.composed_card_id IS NULL
                       AND (x.observation_id IS NULL OR EXISTS (SELECT 1 FROM semantic_card_items ci
                         WHERE ci.vault_id=r.vault_id AND ci.card_revision_id=r.card_revision_id
                           AND ci.observation_id=x.observation_id)))))
               AND NOT EXISTS (SELECT 1 FROM semantic_task_states t
                 WHERE t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key AND t.state='completed')
             ORDER BY r.title COLLATE NOCASE,c.card_id",
        )
        .bind(context.id().to_string())
        .bind(source_id.to_string())
        .bind(source_revision_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        self.build_card_records(context, rows).await
    }

    pub async fn get_evidence(
        &self,
        context: &VaultContext,
        evidence_ref_id: EvidenceRefId,
    ) -> Result<Option<SemanticEvidenceRecord>, StateError> {
        let mut row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT e.source_id,e.source_revision_id,s.current_revision_id
             FROM semantic_evidence_refs e JOIN semantic_sources s
               ON s.vault_id=e.vault_id AND s.source_id=e.source_id
             WHERE e.vault_id=? AND e.evidence_ref_id=? AND e.validation_status='validated'
               AND s.eligible=1 AND s.pending_rebuild=0
               AND EXISTS (
                 SELECT 1 FROM semantic_card_assertion_supports a
                 JOIN semantic_card_items i ON i.vault_id=a.vault_id AND i.item_id=a.item_id
                 JOIN semantic_memory_cards c ON c.vault_id=i.vault_id AND c.card_id=i.card_id
                 JOIN semantic_card_revisions r ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
                 WHERE a.vault_id=e.vault_id AND a.evidence_ref_id=e.evidence_ref_id
                   AND c.eligibility='readable' AND r.publication_state='published'
                   AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                     WHERE x.vault_id=r.vault_id AND x.active=1
                       AND x.action IN ('suppress_read','forget_current')
                       AND (
                         (x.semantic_target_key=r.semantic_target_key
                           AND (x.source_id IS NULL OR x.source_id=e.source_id)
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=e.source_revision_id)
                           AND x.composed_card_id IS NULL
                           AND (x.card_id IS NULL OR x.card_id=c.card_id))
                         OR (x.card_id IS NOT NULL
                           AND x.card_id=c.card_id
                           AND (x.source_id IS NULL OR x.source_id=e.source_id)
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=e.source_revision_id)
                           AND x.composed_card_id IS NULL
                           AND (x.observation_id IS NULL OR EXISTS (
                             SELECT 1 FROM semantic_card_items ci
                             WHERE ci.vault_id=r.vault_id AND ci.card_revision_id=r.card_revision_id
                               AND ci.observation_id=x.observation_id)))
                         OR (x.card_id IS NULL AND x.source_id=e.source_id
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=e.source_revision_id)
                           AND x.composed_card_id IS NULL
                           AND (x.observation_id IS NULL OR x.observation_id=a.observation_id))
                       ))
                   AND NOT EXISTS (SELECT 1 FROM semantic_task_states t
                     WHERE t.vault_id=r.vault_id AND t.target_key=r.semantic_target_key AND t.state='completed')
                   AND r.source_revision_id=e.source_revision_id
               )",
        )
        .bind(context.id().to_string())
        .bind(evidence_ref_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        if row.is_none() {
            let candidates = sqlx::query_as::<_, (String, String, String, String)>(
                "SELECT e.source_id,e.source_revision_id,s.current_revision_id,c.composed_card_id
                 FROM semantic_evidence_refs e
                 JOIN semantic_sources s
                   ON s.vault_id=e.vault_id AND s.source_id=e.source_id
                 JOIN semantic_composed_support_members m
                   ON m.vault_id=e.vault_id AND m.evidence_ref_id=e.evidence_ref_id
                  AND m.source_id=e.source_id AND m.source_revision_id=e.source_revision_id
                 JOIN semantic_observations o
                   ON o.vault_id=m.vault_id AND o.observation_id=m.observation_id
                  AND o.source_id=m.source_id AND o.source_revision_id=m.source_revision_id
                 JOIN semantic_extraction_sets es
                   ON es.vault_id=o.vault_id AND es.extraction_set_id=o.extraction_set_id
                  AND es.source_id=o.source_id
                 JOIN semantic_composed_support_groups g
                   ON g.vault_id=m.vault_id AND g.support_group_id=m.support_group_id
                 JOIN semantic_composed_card_items i
                   ON i.vault_id=g.vault_id AND i.composed_card_item_id=g.composed_card_item_id
                 JOIN semantic_composed_card_revisions r
                   ON r.vault_id=i.vault_id AND r.composed_card_revision_id=i.composed_card_revision_id
                 JOIN semantic_composed_cards c
                   ON c.vault_id=r.vault_id AND c.composed_card_id=r.composed_card_id
                  AND c.current_revision_id=r.composed_card_revision_id
                 WHERE e.vault_id=? AND e.evidence_ref_id=? AND e.validation_status='validated'
                   AND s.eligible=1 AND s.pending_rebuild=0
                   AND s.current_revision_id=e.source_revision_id
                   AND es.state='success_nonempty'
                   AND es.extraction_commit_sequence=s.extraction_commit_sequence
                   AND r.publication_state='published' AND c.eligibility='readable'
                   AND NOT EXISTS (SELECT 1 FROM semantic_suppressions x
                     WHERE x.vault_id=m.vault_id AND x.active=1
                       AND x.action IN ('suppress_read','forget_current')
                       AND (
                         (x.source_id=m.source_id
                           AND (x.source_revision_id IS NULL OR x.source_revision_id=m.source_revision_id)
                           AND (x.composed_card_id IS NULL OR x.composed_card_id=c.composed_card_id)
                           AND (x.observation_id IS NULL OR x.observation_id=m.observation_id))
                         OR (x.composed_card_id=c.composed_card_id AND x.source_id IS NULL)
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
            .bind(evidence_ref_id.to_string())
            .fetch_all(&self.pool)
            .await?;
            for (source_text, revision_text, current_text, card_id) in candidates {
                let card_id = match ComposedCardId::parse(&card_id) {
                    Ok(card_id) => card_id,
                    Err(_) => return Err(StateError::IntegrityFailure),
                };
                if revision_text == current_text
                    && crate::semantic_organization::card_is_currently_readable(
                        &self.pool, context, card_id,
                    )
                    .await?
                {
                    row = Some((source_text, revision_text, current_text));
                    break;
                }
            }
        }
        let Some((source_text, revision_text, current_text)) = row else {
            return Ok(None);
        };
        if revision_text != current_text {
            return Ok(None);
        }
        let spans = sqlx::query_as::<_, (String, String, i64, i64, String)>(
            "SELECT source_revision_id,span_role,start_byte,end_byte,content_hash FROM semantic_evidence_spans
             WHERE vault_id=? AND evidence_ref_id=? ORDER BY span_role,ordinal",
        )
        .bind(context.id().to_string())
        .bind(evidence_ref_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut body_spans = Vec::new();
        let mut context_spans = Vec::new();
        for (span_revision, role, start, end, hash) in spans {
            if span_revision != revision_text {
                return Err(StateError::IntegrityFailure);
            }
            let span = SemanticSpanInput {
                start_byte: u64::try_from(start).map_err(|_| StateError::IntegrityFailure)?,
                end_byte: u64::try_from(end).map_err(|_| StateError::IntegrityFailure)?,
                content_hash: hash,
            };
            match role.as_str() {
                "body" => body_spans.push(span),
                "context" => context_spans.push(span),
                _ => return Err(StateError::IntegrityFailure),
            }
        }
        Ok(Some(SemanticEvidenceRecord {
            id: evidence_ref_id,
            source_id: SemanticSourceId::parse(&source_text)?,
            source_revision_id: SourceRevisionId::parse(&revision_text)?,
            body_spans,
            context_spans,
        }))
    }

    /// Load an evidence reference and its immutable spans without requiring
    /// current-source eligibility. This is used only by the deterministic
    /// historical rebind proof; normal reads must use `get_evidence`.
    pub async fn get_historical_evidence(
        &self,
        context: &VaultContext,
        evidence_ref_id: EvidenceRefId,
        source_id: SemanticSourceId,
        source_revision_id: SourceRevisionId,
    ) -> Result<Option<SemanticEvidenceRecord>, StateError> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT e.source_id,e.source_revision_id FROM semantic_evidence_refs e
             JOIN semantic_source_revisions r
               ON r.vault_id=e.vault_id AND r.source_revision_id=e.source_revision_id
              AND r.source_id=e.source_id
             WHERE e.vault_id=? AND e.evidence_ref_id=? AND e.source_id=?
               AND e.source_revision_id=?",
        )
        .bind(context.id().to_string())
        .bind(evidence_ref_id.to_string())
        .bind(source_id.to_string())
        .bind(source_revision_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some((source_text, revision_text)) = row else {
            return Ok(None);
        };
        let spans = sqlx::query_as::<_, (String, String, i64, i64, String)>(
            "SELECT source_revision_id,span_role,start_byte,end_byte,content_hash
             FROM semantic_evidence_spans
             WHERE vault_id=? AND evidence_ref_id=? ORDER BY span_role,ordinal",
        )
        .bind(context.id().to_string())
        .bind(evidence_ref_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut body_spans = Vec::new();
        let mut context_spans = Vec::new();
        for (span_revision, role, start, end, hash) in spans {
            if span_revision != revision_text {
                return Err(StateError::IntegrityFailure);
            }
            let span = SemanticSpanInput {
                start_byte: u64::try_from(start).map_err(|_| StateError::IntegrityFailure)?,
                end_byte: u64::try_from(end).map_err(|_| StateError::IntegrityFailure)?,
                content_hash: hash,
            };
            match role.as_str() {
                "body" => body_spans.push(span),
                "context" => context_spans.push(span),
                _ => return Err(StateError::IntegrityFailure),
            }
        }
        Ok(Some(SemanticEvidenceRecord {
            id: evidence_ref_id,
            source_id: SemanticSourceId::parse(&source_text)?,
            source_revision_id: SourceRevisionId::parse(&revision_text)?,
            body_spans,
            context_spans,
        }))
    }

    async fn build_card_records(
        &self,
        context: &VaultContext,
        rows: Vec<CardHeaderRow>,
    ) -> Result<Vec<SemanticCardRecord>, StateError> {
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let item_rows = sqlx::query_as::<_, CardItemRow>(
                "SELECT i.item_id,i.item_kind,i.ordinal,i.content,i.observation_id,o.source_revision_id AS observation_source_revision_id,
                        CASE WHEN o.advisory_kind_unknown=1 THEN 'unknown' ELSE o.kind END AS observation_kind,
                        o.scope,o.assertion_status,o.source_time_scope_json
                 FROM semantic_card_items i JOIN semantic_observations o
                   ON o.vault_id=i.vault_id AND o.observation_id=i.observation_id
                 WHERE i.vault_id=? AND i.card_revision_id=?
                 ORDER BY i.item_kind,i.ordinal",
            )
            .bind(context.id().to_string())
            .bind(&row.card_revision_id)
            .fetch_all(&self.pool)
            .await?;
            let mut items = Vec::new();
            for item in item_rows {
                if item.observation_source_revision_id != row.source_revision_id {
                    return Err(StateError::IntegrityFailure);
                }
                let evidence_rows = sqlx::query_as::<_, (String, String, String)>(
                    "SELECT a.evidence_ref_id,e.source_revision_id,a.observation_id
                     FROM semantic_card_assertion_supports a
                     JOIN semantic_evidence_refs e
                       ON e.vault_id=a.vault_id AND e.evidence_ref_id=a.evidence_ref_id
                     WHERE a.vault_id=? AND a.item_id=? ORDER BY a.evidence_ref_id",
                )
                .bind(context.id().to_string())
                .bind(&item.item_id)
                .fetch_all(&self.pool)
                .await?;
                if evidence_rows.is_empty() {
                    return Err(StateError::IntegrityFailure);
                }
                if evidence_rows
                    .iter()
                    .any(|(_, evidence_revision, observation_id)| {
                        evidence_revision != &row.source_revision_id
                            || observation_id != &item.observation_id
                    })
                {
                    return Err(StateError::IntegrityFailure);
                }
                let evidence_ids = evidence_rows
                    .iter()
                    .map(|(evidence_id, _, _)| evidence_id)
                    .collect::<Vec<_>>();
                items.push(SemanticCardItemRecord {
                    kind: item.item_kind,
                    ordinal: u32::try_from(item.ordinal)
                        .map_err(|_| StateError::IntegrityFailure)?,
                    content: item.content,
                    observation_id: ObservationId::parse(&item.observation_id)?,
                    observation_kind: item.observation_kind,
                    scope: item.scope,
                    assertion_status: item.assertion_status,
                    source_time_scope: serde_json::from_str(&item.source_time_scope_json)?,
                    evidence_ref_ids: evidence_ids
                        .iter()
                        .map(|id| EvidenceRefId::parse(id))
                        .collect::<Result<_, _>>()?,
                });
            }
            records.push(SemanticCardRecord {
                vault_id: context.id(),
                id: MemoryCardId::parse(&row.card_id)?,
                revision_id: CardRevisionId::parse(&row.card_revision_id)?,
                source_id: SemanticSourceId::parse(&row.source_id)?,
                source_revision_id: SourceRevisionId::parse(&row.source_revision_id)?,
                source_path: VaultPath::parse(&row.source_path)?,
                title: row.title,
                kind: row.kind,
                scope_ref: row.scope_ref,
                assertion_status: row.assertion_status,
                temporal_scope: serde_json::from_str(&row.temporal_scope_json)?,
                revision_number: u32::try_from(row.revision_number)
                    .map_err(|_| StateError::IntegrityFailure)?,
                canonical_path: VaultPath::parse(&row.canonical_path)?,
                canonical_file_id: FileId::parse(
                    row.canonical_file_id
                        .as_deref()
                        .ok_or(StateError::IntegrityFailure)?,
                )?,
                canonical_file_revision: Revision::try_from(
                    row.canonical_revision.ok_or(StateError::IntegrityFailure)?,
                )?,
                canonical_markdown_hash: row.canonical_markdown_hash,
                items,
            });
        }
        Ok(records)
    }
}

/// Apply the semantic qualification consequence of one committed file
/// mutation inside the caller's metadata transaction. Vault Core owns the
/// canonical file commit; this State-only seam keeps semantic SQL atomic with
/// the file row and its outbox event.
pub(crate) async fn apply_file_mutation_qualification_tx(
    tx: &mut Transaction<'_, Sqlite>,
    vault_id: &str,
    file_id: &str,
    path: &VaultPath,
    operation: &str,
    content_hash: Option<&str>,
    now: i64,
) -> Result<(), StateError> {
    if !semantic_schema_present_tx(tx).await? {
        return Ok(());
    }
    let current_hash: Option<String> = sqlx::query_scalar(
        "SELECT current_content_hash FROM semantic_sources
         WHERE vault_id=? AND file_id=?",
    )
    .bind(vault_id)
    .bind(file_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(current_hash) = current_hash else {
        // A new File ID has no semantic identity to inherit, even when its
        // path matches a deleted/tombstoned file.
        return Ok(());
    };

    if operation == "move" && content_hash == Some(current_hash.as_str()) {
        // A same-identity, same-content move only changes navigation. Do not
        // advance source generation or hide an otherwise readable card.
        sqlx::query(
            "UPDATE semantic_sources SET source_path=?,updated_at=?
             WHERE vault_id=? AND file_id=? AND current_content_hash=?",
        )
        .bind(path.as_str())
        .bind(now)
        .bind(vault_id)
        .bind(file_id)
        .bind(&current_hash)
        .execute(&mut **tx)
        .await?;
        return Ok(());
    }

    // Delete/restore and every actual content change fail closed. A restore
    // is deliberately conservative even when the bytes happen to match: it
    // follows an explicit identity/lifecycle transition and must be
    // re-confirmed by the semantic pipeline.
    let must_invalidate = operation == "delete"
        || operation == "restore"
        || content_hash != Some(current_hash.as_str());
    if must_invalidate {
        invalidate_source_tx(tx, vault_id, file_id, "source_changed", None, now).await?;
    }
    Ok(())
}

pub(crate) async fn invalidate_source_tx(
    tx: &mut Transaction<'_, Sqlite>,
    vault_id: &str,
    file_id: &str,
    reason: &str,
    authorization_revision: Option<i64>,
    now: i64,
) -> Result<bool, StateError> {
    if !semantic_schema_present_tx(tx).await? {
        return Ok(false);
    }
    let result = sqlx::query(
        "UPDATE semantic_sources SET eligible=0,pending_rebuild=1,invalid_reason=?,
            source_generation=source_generation+1,
            authorization_revision=COALESCE(?,authorization_revision),updated_at=?
         WHERE vault_id=? AND file_id=?",
    )
    .bind(reason)
    .bind(authorization_revision)
    .bind(now)
    .bind(vault_id)
    .bind(file_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE semantic_memory_cards SET eligibility='invalidated',updated_at=?
         WHERE vault_id=? AND source_id=(SELECT source_id FROM semantic_sources WHERE vault_id=? AND file_id=?)",
    )
    .bind(now)
    .bind(vault_id)
    .bind(vault_id)
    .bind(file_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE semantic_evidence_refs SET validation_status='invalidated'
         WHERE vault_id=? AND source_id=(SELECT source_id FROM semantic_sources WHERE vault_id=? AND file_id=?)",
    )
    .bind(vault_id)
    .bind(vault_id)
    .bind(file_id)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}

pub(crate) async fn invalidate_all_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    vault_id: &str,
    reason: &str,
    authorization_revision: Option<i64>,
    now: i64,
) -> Result<(), StateError> {
    if !semantic_schema_present_tx(tx).await? {
        return Ok(());
    }
    sqlx::query(
        "UPDATE semantic_sources SET eligible=0,pending_rebuild=1,invalid_reason=?,
            source_generation=source_generation+1,
            authorization_revision=COALESCE(?,authorization_revision),updated_at=?
         WHERE vault_id=?",
    )
    .bind(reason)
    .bind(authorization_revision)
    .bind(now)
    .bind(vault_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE semantic_memory_cards SET eligibility='invalidated',updated_at=? WHERE vault_id=?",
    )
    .bind(now)
    .bind(vault_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE semantic_evidence_refs SET validation_status='invalidated' WHERE vault_id=?",
    )
    .bind(vault_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn semantic_schema_present_tx(tx: &mut Transaction<'_, Sqlite>) -> Result<bool, StateError> {
    // File/settings repositories are also used by predecessor-schema tests
    // before semantic migrations have been applied. Those databases must not
    // fail merely because the new namespace is absent. Once migration 0034
    // (or newer) is recorded, however, a missing semantic table is a broken
    // migrated database and must fail closed rather than silently skipping
    // qualification invalidation.
    let migration_table_present: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type='table' AND name='_sqlx_migrations'",
    )
    .fetch_one(&mut **tx)
    .await?;
    if migration_table_present == 0 {
        return Ok(false);
    }
    let migration_version: Option<i64> =
        sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success=1")
            .fetch_one(&mut **tx)
            .await?;
    if migration_version.is_none_or(|version| version < 34) {
        return Ok(false);
    }

    for table in [
        "semantic_sources",
        "semantic_memory_cards",
        "semantic_evidence_refs",
    ] {
        let present: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&mut **tx)
                .await?;
        if present != 1 {
            return Err(StateError::IntegrityFailure);
        }
    }
    Ok(true)
}

#[derive(FromRow)]
struct CardHeaderRow {
    card_id: String,
    card_revision_id: String,
    source_id: String,
    source_revision_id: String,
    source_path: String,
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

#[derive(FromRow)]
struct CardItemRow {
    item_id: String,
    item_kind: String,
    ordinal: i64,
    content: String,
    observation_id: String,
    observation_source_revision_id: String,
    observation_kind: String,
    scope: String,
    assertion_status: String,
    source_time_scope_json: String,
}

async fn insert_spans(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    context: &VaultContext,
    evidence_id: EvidenceRefId,
    source_revision_id: SourceRevisionId,
    role: &str,
    spans: &[SemanticSpanInput],
) -> Result<(), StateError> {
    for (ordinal, span) in spans.iter().enumerate() {
        let start = i64::try_from(span.start_byte)
            .map_err(|_| StateError::InvalidInput("semantic span is too large"))?;
        let end = i64::try_from(span.end_byte)
            .map_err(|_| StateError::InvalidInput("semantic span is too large"))?;
        if start < 0 || end <= start || span.content_hash.is_empty() {
            return Err(StateError::InvalidInput("semantic span is invalid"));
        }
        sqlx::query(
            "INSERT INTO semantic_evidence_spans
             (vault_id,evidence_ref_id,source_revision_id,span_role,ordinal,start_byte,end_byte,content_hash)
             VALUES(?,?,?,?,?,?,?,?)",
        )
        .bind(context.id().to_string())
        .bind(evidence_id.to_string())
        .bind(source_revision_id.to_string())
        .bind(role)
        .bind(i64::try_from(ordinal).map_err(|_| StateError::InvalidInput("too many semantic spans"))?)
        .bind(start)
        .bind(end)
        .bind(&span.content_hash)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn load_source_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    context: &VaultContext,
    source_id: SemanticSourceId,
) -> Result<SemanticSourceRecord, StateError> {
    let row = sqlx::query_as::<_, SourceRow>(
        "SELECT vault_id,source_id,file_id,current_revision_id,source_path,current_content_hash,
                source_generation,extraction_commit_sequence,authorization_revision,eligible,
                invalid_reason,pending_rebuild
         FROM semantic_sources WHERE vault_id=? AND source_id=?",
    )
    .bind(context.id().to_string())
    .bind(source_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    source_row_to_record(row)
}

async fn load_source_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    context: &VaultContext,
    source_revision_id: SourceRevisionId,
) -> Result<SemanticSourceRevisionRecord, StateError> {
    let row = sqlx::query_as::<_, SourceRevisionRow>(
        "SELECT vault_id,source_revision_id,source_id,file_id,file_revision,content_hash,source_path,availability,source_generation
         FROM semantic_source_revisions WHERE vault_id=? AND source_revision_id=?",
    )
    .bind(context.id().to_string())
    .bind(source_revision_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    source_revision_row_to_record(row)
}

fn source_row_to_record(row: SourceRow) -> Result<SemanticSourceRecord, StateError> {
    Ok(SemanticSourceRecord {
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        source_id: SemanticSourceId::parse(&row.source_id)?,
        file_id: FileId::parse(&row.file_id)?,
        current_revision_id: row
            .current_revision_id
            .as_deref()
            .map(SourceRevisionId::parse)
            .transpose()?,
        source_path: VaultPath::parse(&row.source_path)?,
        content_hash: row.current_content_hash,
        source_generation: row.source_generation,
        extraction_commit_sequence: row.extraction_commit_sequence,
        authorization_revision: row.authorization_revision,
        eligible: row.eligible == 1,
        pending_rebuild: row.pending_rebuild == 1,
        invalid_reason: row.invalid_reason,
    })
}

fn source_revision_row_to_record(
    row: SourceRevisionRow,
) -> Result<SemanticSourceRevisionRecord, StateError> {
    Ok(SemanticSourceRevisionRecord {
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        source_revision_id: SourceRevisionId::parse(&row.source_revision_id)?,
        source_id: SemanticSourceId::parse(&row.source_id)?,
        file_id: FileId::parse(&row.file_id)?,
        file_revision: Revision::try_from(row.file_revision)?,
        content_hash: row.content_hash,
        source_path: VaultPath::parse(&row.source_path)?,
        availability: row.availability,
        source_generation: row.source_generation,
    })
}

fn extraction_row_to_record(row: ExtractionRow) -> Result<SemanticExtractionRecord, StateError> {
    Ok(SemanticExtractionRecord {
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        id: ExtractionSetId::parse(&row.extraction_set_id)?,
        source_id: SemanticSourceId::parse(&row.source_id)?,
        source_revision_id: SourceRevisionId::parse(&row.source_revision_id)?,
        state: row.state,
        input_hash: row.input_hash,
        profile_id: row.profile_id,
        authorization_revision: row.authorization_revision,
        source_generation: row.source_generation,
        extraction_commit_sequence: row.extraction_commit_sequence,
        observation_count: u32::try_from(row.observation_count)
            .map_err(|_| StateError::IntegrityFailure)?,
        card_count: u32::try_from(row.card_count).map_err(|_| StateError::IntegrityFailure)?,
        safe_error_code: row.safe_error_code,
        semantic_target_key: row.semantic_target_key,
        rules_revision: row.rules_revision,
    })
}

fn extraction_batch_row_to_record(
    row: ExtractionBatchRow,
) -> Result<SemanticExtractionBatchRecord, StateError> {
    let normalized_proposal_json = row.normalized_proposal_json;
    let normalized_proposal_hash = row.normalized_proposal_hash;
    if let Some(json) = normalized_proposal_json.as_deref()
        && normalized_proposal_hash.as_deref() != Some(sha256_hex(json.as_bytes()).as_str())
    {
        return Err(StateError::IntegrityFailure);
    }
    if (row.state == "validated") != normalized_proposal_json.is_some()
        || (row.state == "validated") != normalized_proposal_hash.is_some()
    {
        return Err(StateError::IntegrityFailure);
    }
    Ok(SemanticExtractionBatchRecord {
        extraction_set_id: ExtractionSetId::parse(&row.extraction_set_id)?,
        source_id: SemanticSourceId::parse(&row.source_id)?,
        source_revision_id: SourceRevisionId::parse(&row.source_revision_id)?,
        batch_index: u32::try_from(row.batch_index).map_err(|_| StateError::IntegrityFailure)?,
        batch_count: u32::try_from(row.batch_count).map_err(|_| StateError::IntegrityFailure)?,
        batch_input_hash: row.batch_input_hash,
        batch_catalog_hash: row.batch_catalog_hash,
        prompt_id: row.prompt_id,
        schema_id: row.schema_id,
        provider_fingerprint: row.provider_fingerprint,
        state: row.state,
        attempt_count: u32::try_from(row.attempt_count)
            .map_err(|_| StateError::IntegrityFailure)?,
        normalized_proposal_json,
        normalized_proposal_hash,
        advisory_normalization_counts: serde_json::from_str(
            &row.advisory_normalization_counts_json,
        )
        .map_err(|_| StateError::IntegrityFailure)?,
        safe_error_code: row.safe_error_code,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn semantic_card_fingerprint(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    card: &SemanticCardRevisionInput,
    source_id: SemanticSourceId,
) -> Result<String, StateError> {
    let mut parts = Vec::new();
    for item in &card.items {
        let row: SemanticFingerprintObservation = sqlx::query_as(
            "SELECT CASE WHEN advisory_kind_unknown=1 THEN 'unknown' ELSE kind END,
                        statement,scope,assertion_status,conditions_json,exceptions_json,
                        ordered_steps_json,source_time_scope_json,result_json,uncertainty_json
                 FROM semantic_observations
                 WHERE vault_id=? AND observation_id=? AND source_id=?",
        )
        .bind(context.id().to_string())
        .bind(item.observation_id.to_string())
        .bind(source_id.to_string())
        .fetch_one(&mut **tx)
        .await?;
        parts.push(
            [
                source_id.to_string(),
                card.kind.clone(),
                card.scope_ref.clone(),
                card.assertion_status.clone(),
                item.kind.clone(),
                item.ordinal.to_string(),
                item.content.clone(),
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
            ]
            .join("|"),
        );
        let mut span_identity = Vec::new();
        for evidence_ref_id in &item.evidence_ref_ids {
            let spans = sqlx::query_as::<_, (String, i64, String)>(
                "SELECT span_role,ordinal,content_hash FROM semantic_evidence_spans
                 WHERE vault_id=? AND evidence_ref_id=? ORDER BY span_role,ordinal",
            )
            .bind(context.id().to_string())
            .bind(evidence_ref_id.to_string())
            .fetch_all(&mut **tx)
            .await?;
            span_identity.extend(
                spans
                    .into_iter()
                    .map(|(role, ordinal, hash)| format!("{role}:{ordinal}:{hash}")),
            );
        }
        span_identity.sort();
        if let Some(last) = parts.last_mut() {
            last.push('|');
            last.push_str(&span_identity.join(","));
        }
    }
    parts.sort();
    Ok(parts.join("||"))
}

fn snapshot_row_to_record(row: SnapshotRow) -> Result<SemanticPreparedSnapshot, StateError> {
    Ok(SemanticPreparedSnapshot {
        id: PreparedSnapshotId::parse(&row.snapshot_id)?,
        extraction_set_id: ExtractionSetId::parse(&row.extraction_set_id)?,
        card_id: MemoryCardId::parse(&row.card_id)?,
        card_revision_id: CardRevisionId::parse(&row.card_revision_id)?,
        source_id: SemanticSourceId::parse(&row.source_id)?,
        source_revision_id: SourceRevisionId::parse(&row.source_revision_id)?,
        source_content_hash: row.source_content_hash,
        authorization_revision: row.authorization_revision,
        source_generation: row.source_generation,
        extraction_commit_sequence: row.extraction_commit_sequence,
        expected_card_revision_id: row
            .expected_card_revision_id
            .as_deref()
            .map(CardRevisionId::parse)
            .transpose()?,
        proposed_card_revision_number: u32::try_from(row.proposed_card_revision_number)
            .map_err(|_| StateError::IntegrityFailure)?,
        expected_file_id: row
            .expected_file_id
            .as_deref()
            .map(FileId::parse)
            .transpose()?,
        expected_file_revision: row
            .expected_file_revision
            .map(Revision::try_from)
            .transpose()?,
        proposed_file_revision: Revision::try_from(row.proposed_file_revision)?,
        published_file_id: row
            .published_file_id
            .as_deref()
            .map(FileId::parse)
            .transpose()?,
        published_file_revision: row
            .published_file_revision
            .map(Revision::try_from)
            .transpose()?,
        target_path: VaultPath::parse(&row.target_path)?,
        proposed_file_hash: row.proposed_file_hash,
        canonical_bytes: row.canonical_bytes,
        status: row.status,
        semantic_target_key: row.semantic_target_key,
        rules_revision: row.rules_revision,
    })
}

fn content_hash_matches(expected: Option<&str>, actual: &str) -> bool {
    expected
        .map(|value| value.strip_prefix("sha256:").unwrap_or(value))
        .is_some_and(|value| {
            value.eq_ignore_ascii_case(actual.strip_prefix("sha256:").unwrap_or(actual))
        })
}
