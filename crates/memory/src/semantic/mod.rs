//! Single-source semantic memory: local evidence, complete extraction sets,
//! portable cards, and conservative current-source qualification.

pub mod organize;
pub mod public;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use mcp_vault_core::{ManagedReadResult, VaultCore, VaultError};
use mcp_vault_domain::{
    Actor as CoreActor, CardItemId, CardRevisionId, EvidenceRefId, ExtractionSetId, FileId,
    MemoryCardId, ObservationId, Revision, SemanticSourceId, SourcePlane, SourceRevisionId,
    VaultContext, VaultId, VaultPath,
};
use mcp_vault_state::{
    EntryType, SemanticCardItemInput, SemanticCardRecord, SemanticEvidenceInput,
    SemanticEvidenceRecord, SemanticExtractionBatchSpec, SemanticObservationInput,
    SemanticObservationRecord, SemanticPreparedSnapshot, SemanticSourceRecord,
    SemanticSourceRevisionRecord, SemanticSpanInput, StateStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::MemoryError;

const EXTRACTION_POLICY_KEY: &str = "memory.units.policy";
const SEMANTIC_PROFILE: &str = "semantic-memory-m1-v1";
pub const A80_MAX_BLOCKS_PER_BATCH: usize = 80;

pub fn a80_batch_count(logical_block_count: u32) -> u32 {
    logical_block_count.div_ceil(A80_MAX_BLOCKS_PER_BATCH as u32)
}

/// One source block made available to an extraction model. IDs are temporary
/// and bound to one Vault source revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticSourceBlock {
    pub local_id: String,
    pub text: String,
    pub line_number: u32,
    pub kind: String,
}

/// Local extraction input contains no Vault, path, durable ID or permission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticModelInput {
    pub blocks: Vec<SemanticSourceBlock>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticProposalOutcome {
    SuccessNonempty,
    SuccessEmpty,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticKind {
    Preference,
    Constraint,
    Decision,
    Experience,
    Procedure,
    State,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticScope {
    User,
    Project,
    Task,
    Unspecified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticAssertionStatus {
    SourceAsserted,
    Proposed,
    Adopted,
    Committed,
    Observed,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticTimeScopeStatus {
    Unknown,
    SourceStated,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticTimeScopeProposal {
    pub status: SemanticTimeScopeStatus,
    pub value: Option<String>,
    #[serde(default)]
    pub evidence_block_ids: Vec<String>,
}

/// Complete, single-source model proposal. All durable identity and storage
/// information is assigned by the service after validation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticExtractionProposal {
    pub outcome: SemanticProposalOutcome,
    #[serde(default)]
    pub observations: Vec<SemanticObservationProposal>,
    #[serde(default)]
    pub cards: Vec<SemanticCardProposal>,
}

/// Observation-stage input. The outcome is optional because it is fully
/// determined by whether the validated observation set is empty; cards are
/// deliberately outside this stage's contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticObservationPhaseInput {
    pub observations: Vec<SemanticObservationProposal>,
    #[serde(default, deserialize_with = "deserialize_present_outcome")]
    pub outcome: Option<SemanticProposalOutcome>,
}

fn deserialize_present_outcome<'de, D>(
    deserializer: D,
) -> Result<Option<SemanticProposalOutcome>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    SemanticProposalOutcome::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticObservationProposal {
    pub kind: SemanticKind,
    pub statement: String,
    pub scope: SemanticScope,
    pub assertion_status: SemanticAssertionStatus,
    #[serde(default)]
    pub source_time_scope: Option<SemanticTimeScopeProposal>,
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub exceptions: Vec<String>,
    #[serde(default)]
    pub ordered_steps: Vec<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub uncertainty: Option<String>,
    pub admission_reason: String,
    pub value_for_future_work: String,
    pub body_block_ids: Vec<String>,
    #[serde(default)]
    pub context_block_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCardProposal {
    pub title: String,
    pub kind: SemanticKind,
    pub scope: SemanticScope,
    pub assertion_status: SemanticAssertionStatus,
    /// Indices into this response's observation array, never durable IDs.
    pub observation_indices: Vec<u32>,
}

/// A deterministic compatibility partition for the second semantic-card
/// stage. Cards may be split by the model inside one group, but references
/// must never cross one of these boundaries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticCompatibilityGroup {
    pub group_id: String,
    pub kind: SemanticKind,
    pub scope: SemanticScope,
    pub assertion_status: SemanticAssertionStatus,
    pub source_time_scope: Option<SemanticTimeScopeProposal>,
    /// Zero-based indices into the first-stage observation array.
    pub observation_indices: Vec<u32>,
}

/// The only model-owned fields in the composition stage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCardSelection {
    pub title: String,
    /// Zero-based indices from one compatibility group's allowed references.
    pub observation_indices: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCompositionProposal {
    pub cards: Vec<SemanticCardSelection>,
}

/// Prepared, non-published first-stage result. The extraction fence remains
/// running in State until composition is accepted or explicitly failed.
pub struct SemanticObservationPhase {
    pub extraction: mcp_vault_state::SemanticExtractionRecord,
    pub observations: Vec<SemanticObservationProposal>,
    pub groups: Vec<SemanticCompatibilityGroup>,
    prepared: SemanticPreparedGeneration,
}

/// Source and State fence captured before the first Provider request. The
/// loaded source is intentionally private so callers cannot forge it.
#[derive(Clone)]
pub struct SemanticPreparedGeneration {
    pub extraction: mcp_vault_state::SemanticExtractionRecord,
    pub model_input: SemanticModelInput,
    loaded: LoadedSource,
}

/// One bounded A80 source-local request. Evidence indices retain their
/// 1-based positions in the complete frozen source catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticObservationBatch {
    pub index: u32,
    pub count: u32,
    pub input: Value,
    pub input_hash: String,
    pub catalog_hash: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SemanticAdvisoryNormalizationCounts {
    pub kind_unknown: u32,
    pub scope_unspecified: u32,
    pub status_unknown: u32,
    pub time_scope_unknown: u32,
}

impl SemanticPreparedGeneration {
    /// Return the temporary namespace that binds model-visible evidence
    /// indexes to this exact Vault source revision.
    pub fn evidence_namespace(&self) -> &str {
        &self.loaded.block_id_namespace
    }

    /// Build deterministic, at-most-80-block provider fragments. The
    /// complete source catalog remains locally available for strict mapping.
    pub fn observation_batches(&self) -> Result<Vec<SemanticObservationBatch>, MemoryError> {
        let count = self
            .model_input
            .blocks
            .len()
            .div_ceil(A80_MAX_BLOCKS_PER_BATCH);
        let count_u32 = u32::try_from(count).map_err(|_| MemoryError::Conflict)?;
        let catalog_hash =
            hash_bytes(&serde_json::to_vec(&self.model_input).map_err(|_| MemoryError::Conflict)?);
        self.model_input
            .blocks
            .chunks(A80_MAX_BLOCKS_PER_BATCH)
            .enumerate()
            .map(|(batch_index, blocks)| {
                let start_index = batch_index * A80_MAX_BLOCKS_PER_BATCH;
                let visible_blocks = blocks
                    .iter()
                    .enumerate()
                    .map(|(offset, block)| {
                        json!({
                            "evidence_index": start_index + offset + 1,
                            "text": block.text,
                            "line_number": block.line_number,
                            "kind": block.kind,
                        })
                    })
                    .collect::<Vec<_>>();
                let input = json!({
                    "batch_index": batch_index,
                    "batch_count": count,
                    "blocks": visible_blocks,
                });
                let encoded = serde_json::to_vec(&input).map_err(|_| MemoryError::Conflict)?;
                Ok(SemanticObservationBatch {
                    index: u32::try_from(batch_index).map_err(|_| MemoryError::Conflict)?,
                    count: count_u32,
                    input_hash: hash_bytes(&encoded),
                    catalog_hash: catalog_hash.clone(),
                    input,
                })
            })
            .collect()
    }
}

/// Partition observations by the complete set of fields that must agree on a
/// durable card. This is intentionally pure and deterministic so it can be
/// used to build a bounded second-stage request before any publication.
pub fn compatibility_groups(
    observations: &[SemanticObservationProposal],
) -> Vec<SemanticCompatibilityGroup> {
    let mut grouped = BTreeMap::<String, SemanticCompatibilityGroup>::new();
    for (index, observation) in observations.iter().enumerate() {
        let key = serde_json::to_string(&(
            observation.kind,
            observation.scope,
            observation.assertion_status,
            &observation.source_time_scope,
        ))
        .expect("semantic compatibility key is serializable");
        let group_number = grouped.len() + 1;
        grouped
            .entry(key)
            .and_modify(|group| group.observation_indices.push(index as u32))
            .or_insert_with(|| SemanticCompatibilityGroup {
                group_id: format!("group-{group_number:04}"),
                kind: observation.kind,
                scope: observation.scope,
                assertion_status: observation.assertion_status,
                source_time_scope: observation.source_time_scope.clone(),
                observation_indices: vec![index as u32],
            });
    }
    let mut groups = grouped.into_values().collect::<Vec<_>>();
    groups.sort_by_key(|group| group.observation_indices[0]);
    for (index, group) in groups.iter_mut().enumerate() {
        group.group_id = format!("group-{:04}", index + 1);
    }
    groups
}

/// Assemble legacy card proposals from model-selected titles and references.
/// Card metadata is derived from the referenced observations; mixed groups,
/// duplicate consumption, and incomplete coverage remain hard failures.
pub fn assemble_card_proposals(
    observations: &[SemanticObservationProposal],
    selections: &[SemanticCardSelection],
) -> Result<Vec<SemanticCardProposal>, MemoryError> {
    let mut used = HashSet::new();
    let mut cards = Vec::with_capacity(selections.len());
    for selection in selections {
        if selection.title.trim().is_empty() || selection.title.contains(['\n', '\r']) {
            return Err(MemoryError::GeneratedOutput("semantic_card_title_invalid"));
        }
        if selection.observation_indices.is_empty() {
            return Err(MemoryError::GeneratedOutput(
                "semantic_card_without_observations",
            ));
        }
        let indices = unique_indices(&selection.observation_indices, observations.len())?;
        let first = &observations[indices[0] as usize];
        if indices.iter().any(|index| {
            let observation = &observations[*index];
            observation.kind != first.kind
                || observation.scope != first.scope
                || observation.assertion_status != first.assertion_status
                || observation.source_time_scope != first.source_time_scope
        }) {
            return Err(MemoryError::GeneratedOutput(
                "semantic_card_selection_crosses_compatibility_group",
            ));
        }
        if indices.iter().any(|index| !used.insert(*index)) {
            return Err(MemoryError::GeneratedOutput("semantic_observation_reused"));
        }
        cards.push(SemanticCardProposal {
            title: selection.title.clone(),
            kind: first.kind,
            scope: first.scope,
            assertion_status: first.assertion_status,
            observation_indices: indices.into_iter().map(|index| index as u32).collect(),
        });
    }
    if used.len() != observations.len() {
        return Err(MemoryError::GeneratedOutput(
            "semantic_observation_not_carded",
        ));
    }
    Ok(cards)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticSubmission {
    pub extraction: mcp_vault_state::SemanticExtractionRecord,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEvidenceText {
    pub source_id: SemanticSourceId,
    pub source_revision_id: SourceRevisionId,
    pub source_path: VaultPath,
    pub body_spans: Vec<String>,
    pub context_spans: Vec<String>,
    pub body_span_records: Vec<SemanticEvidenceSpanText>,
    pub context_span_records: Vec<SemanticEvidenceSpanText>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEvidenceSpanText {
    pub start_byte: u64,
    pub end_byte: u64,
    pub content_hash: String,
    pub text: String,
}

/// Safe disposition returned after reconciling one current file event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticSourceEventDisposition {
    /// The same File ID and content hash still qualify; only navigation changed.
    NavigationOnly,
    /// One or more complete historical cards were deterministically rebound
    /// through the normal extraction/publication path.
    Rebound,
    /// The current source is known and must be re-extracted before it is readable.
    NeedsRebuild,
    /// The event cannot be an authorized Markdown source and remains invalidated.
    Invalidated,
    /// Extraction is disabled by the current Vault policy.
    PolicyDisabled,
    /// The policy exists but does not satisfy the local configuration contract.
    PolicyInvalid,
}

/// Content-free result of one source-event reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSourceEventReport {
    pub file_id: FileId,
    pub disposition: SemanticSourceEventDisposition,
    pub source_id: Option<SemanticSourceId>,
    pub source_revision_id: Option<SourceRevisionId>,
    pub file_revision: Option<Revision>,
    pub source_generation: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalRebindOutcome {
    None,
    Complete,
}

#[derive(Clone)]
pub struct SemanticMemoryService {
    state: StateStore,
}

#[derive(Clone)]
struct LoadedSource {
    source: SemanticSourceRecord,
    revision: SemanticSourceRevisionRecord,
    block_id_namespace: String,
    content: String,
    blocks: Vec<LocalBlock>,
    model_input: SemanticModelInput,
}

#[derive(Clone)]
struct LocalBlock {
    display: SemanticSourceBlock,
    start_byte: u64,
    end_byte: u64,
    hash: String,
    heading_context: Vec<String>,
}

struct ValidatedProposal {
    observations: Vec<SemanticObservationInput>,
    cards: Vec<mcp_vault_state::SemanticCardRevisionInput>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ExtractionPolicy {
    enabled: bool,
    request_timeout_seconds: u64,
}

impl Default for ExtractionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            request_timeout_seconds: 300,
        }
    }
}

impl SemanticMemoryService {
    pub fn new(state: StateStore) -> Self {
        Self { state }
    }

    /// Persist cancellation before any semantic publication is prepared.
    pub async fn cancel_extraction(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<(), MemoryError> {
        self.state
            .semantic_memory()
            .cancel_extraction(context, extraction_set_id, "semantic_extraction_cancelled")
            .await?;
        Ok(())
    }

    async fn fail_running_extraction(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
        safe_error_code: &str,
    ) -> Result<(), MemoryError> {
        let repository = self.state.semantic_memory();
        if repository
            .finish_empty_or_nonpublishable(
                context,
                extraction_set_id,
                "failed",
                Some(safe_error_code),
            )
            .await
            .is_ok()
        {
            return Ok(());
        }

        // A fence change can make the failure transition reject the stale
        // extraction. It is still safe to cancel while the set is running,
        // before any publication snapshot has been prepared.
        repository
            .cancel_extraction(context, extraction_set_id, safe_error_code)
            .await?;
        Ok(())
    }

    /// Prepare this authorized Markdown source for a model. Ordinary files are
    /// read only through Vault Core; managed semantic files are excluded.
    pub async fn prepare_source(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
    ) -> Result<SemanticModelInput, MemoryError> {
        Ok(self.load_source(context, core, path).await?.model_input)
    }

    /// Capture the source and State fence before the first Provider request.
    pub async fn begin_generation(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
    ) -> Result<SemanticPreparedGeneration, MemoryError> {
        self.begin_generation_inner(context, core, path, "composition", false)
            .await
    }

    /// Reopen the exact in-flight A80 extraction after process restart. State
    /// batch reservations, not this handle, prevent concurrent/replayed calls.
    pub async fn begin_generation_resumable(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
    ) -> Result<SemanticPreparedGeneration, MemoryError> {
        self.begin_generation_inner(context, core, path, "a80", true)
            .await
    }

    async fn begin_generation_inner(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
        idempotency_namespace: &str,
        allow_running_resume: bool,
    ) -> Result<SemanticPreparedGeneration, MemoryError> {
        let loaded = self.load_source(context, core, path).await?;
        let request_hash = hash_bytes(
            format!(
                "{}:{}:{}",
                loaded.source.source_id, loaded.revision.source_revision_id, SEMANTIC_PROFILE
            )
            .as_bytes(),
        );
        let (extraction, created) = self
            .state
            .semantic_memory()
            .start_extraction(
                context,
                ExtractionSetId::new(),
                loaded.source.source_id,
                loaded.revision.source_revision_id,
                &loaded.revision.content_hash,
                SEMANTIC_PROFILE,
                &format!("{idempotency_namespace}:{request_hash}"),
                &request_hash,
            )
            .await?;
        let published_a80 = allow_running_resume
            && matches!(
                extraction.state.as_str(),
                "success_nonempty" | "success_empty"
            );
        if (extraction.state != "running" && !published_a80) || (!created && !allow_running_resume)
        {
            return Err(MemoryError::Conflict);
        }
        if let Err(error) = self
            .verify_generation_fence(context, core, &loaded, &extraction)
            .await
        {
            self.fail_running_extraction(context, extraction.id, error.code())
                .await?;
            return Err(error);
        }
        if let Err(error) = self.verify_block_id_namespace(context, &loaded).await {
            self.fail_running_extraction(context, extraction.id, error.code())
                .await?;
            return Err(error);
        }
        Ok(SemanticPreparedGeneration {
            model_input: loaded.model_input.clone(),
            loaded,
            extraction,
        })
    }

    /// Persist the immutable catalog of bounded A80 requests before any
    /// Provider call. Provider identity and template IDs participate in the
    /// resume fence.
    pub async fn bind_observation_batches(
        &self,
        context: &VaultContext,
        prepared: &SemanticPreparedGeneration,
        batches: &[SemanticObservationBatch],
        provider_fingerprint: &str,
        prompt_id: &str,
        schema_id: &str,
    ) -> Result<Vec<mcp_vault_state::SemanticExtractionBatchRecord>, MemoryError> {
        if batches.is_empty() || provider_fingerprint.trim().is_empty() {
            return Err(MemoryError::Conflict);
        }
        let specs = batches
            .iter()
            .map(|batch| SemanticExtractionBatchSpec {
                batch_index: batch.index,
                batch_count: batch.count,
                source_revision_id: prepared.extraction.source_revision_id,
                batch_input_hash: batch.input_hash.clone(),
                batch_catalog_hash: batch.catalog_hash.clone(),
                prompt_id: prompt_id.to_owned(),
                schema_id: schema_id.to_owned(),
                provider_fingerprint: provider_fingerprint.to_owned(),
            })
            .collect::<Vec<_>>();
        let records = self
            .state
            .semantic_memory()
            .prepare_extraction_batches(context, prepared.extraction.id, &specs)
            .await?;
        Ok(records)
    }

    /// Strictly validate one flat claim batch, map its source-local indices
    /// back to the existing block IDs, and persist only the normalized
    /// proposal after the ordinary source/span/hash validator accepts it.
    pub async fn accept_observation_batch(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        prepared: &SemanticPreparedGeneration,
        batch: &SemanticObservationBatch,
        response_json: &str,
    ) -> Result<SemanticAdvisoryNormalizationCounts, MemoryError> {
        self.verify_generation_fence(context, core, &prepared.loaded, &prepared.extraction)
            .await?;
        self.verify_block_id_namespace(context, &prepared.loaded)
            .await?;
        let start = usize::try_from(batch.index)
            .map_err(|_| MemoryError::Conflict)?
            .checked_mul(A80_MAX_BLOCKS_PER_BATCH)
            .ok_or(MemoryError::Conflict)?;
        let end = (start + A80_MAX_BLOCKS_PER_BATCH).min(prepared.model_input.blocks.len());
        if start >= end
            || batch.count as usize
                != prepared
                    .model_input
                    .blocks
                    .len()
                    .div_ceil(A80_MAX_BLOCKS_PER_BATCH)
        {
            return Err(MemoryError::Conflict);
        }
        let (observations, counts) = parse_a80_observation_response(
            response_json,
            &prepared.model_input.blocks,
            start,
            end,
        )?;
        let outcome = if observations.is_empty() {
            SemanticProposalOutcome::SuccessEmpty
        } else {
            SemanticProposalOutcome::SuccessNonempty
        };
        self.validate_proposal(
            context,
            core,
            &prepared.loaded,
            SemanticExtractionProposal {
                outcome,
                observations: observations.clone(),
                cards: Vec::new(),
            },
            false,
        )
        .await?;
        let normalized = serde_json::to_string(&observations).map_err(|_| MemoryError::Conflict)?;
        self.state
            .semantic_memory()
            .store_validated_extraction_batch(
                context,
                prepared.extraction.id,
                batch.index,
                &batch.input_hash,
                &normalized,
                &serde_json::to_value(&counts).map_err(|_| MemoryError::Conflict)?,
            )
            .await?;
        Ok(counts)
    }

    pub async fn reserve_observation_batch_attempt(
        &self,
        context: &VaultContext,
        prepared: &SemanticPreparedGeneration,
        batch: &SemanticObservationBatch,
    ) -> Result<String, MemoryError> {
        let record = self
            .state
            .semantic_memory()
            .reserve_extraction_batch_attempt(
                context,
                prepared.extraction.id,
                batch.index,
                &batch.input_hash,
            )
            .await?;
        Ok(record.state)
    }

    /// Record a definitive local/schema failure without ending the source set.
    /// The source fence is checked again so a stale response can never earn a
    /// regeneration token.
    pub async fn fail_observation_batch_attempt(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        prepared: &SemanticPreparedGeneration,
        batch: &SemanticObservationBatch,
        safe_error_code: &str,
    ) -> Result<(), MemoryError> {
        self.verify_generation_fence(context, core, &prepared.loaded, &prepared.extraction)
            .await?;
        self.verify_block_id_namespace(context, &prepared.loaded)
            .await?;
        self.state
            .semantic_memory()
            .fail_extraction_batch_attempt(
                context,
                prepared.extraction.id,
                batch.index,
                &batch.input_hash,
                safe_error_code,
            )
            .await?;
        Ok(())
    }

    /// Spend the one source-wide A80 regen token and reserve the failed batch
    /// for its second attempt in one State transaction.
    pub async fn reserve_observation_batch_regen_attempt(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        prepared: &SemanticPreparedGeneration,
        batch: &SemanticObservationBatch,
    ) -> Result<String, MemoryError> {
        self.verify_generation_fence(context, core, &prepared.loaded, &prepared.extraction)
            .await?;
        self.verify_block_id_namespace(context, &prepared.loaded)
            .await?;
        let record = self
            .state
            .semantic_memory()
            .reserve_extraction_batch_regen_attempt(
                context,
                prepared.extraction.id,
                batch.index,
                &batch.input_hash,
            )
            .await?;
        Ok(record.state)
    }

    /// Publish only after every fragment is validated and the complete
    /// source/revision fence still matches. Each observation becomes exactly
    /// one deterministic card; the existing atomic full-set publisher owns
    /// canonical writes and State CAS.
    pub async fn finalize_observation_batches(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
        prepared: &SemanticPreparedGeneration,
        expected_batch_count: u32,
    ) -> Result<SemanticSubmission, MemoryError> {
        if path != &prepared.loaded.revision.source_path {
            return Err(MemoryError::Conflict);
        }
        self.verify_generation_fence(context, core, &prepared.loaded, &prepared.extraction)
            .await?;
        self.verify_block_id_namespace(context, &prepared.loaded)
            .await?;
        let batches = self
            .state
            .semantic_memory()
            .list_extraction_batches(context, prepared.extraction.id)
            .await?;
        if batches.len() != expected_batch_count as usize
            || batches.iter().enumerate().any(|(index, batch)| {
                batch.batch_index as usize != index
                    || batch.batch_count != expected_batch_count
                    || batch.state != "validated"
                    || batch.source_revision_id != prepared.extraction.source_revision_id
            })
        {
            return Err(MemoryError::Conflict);
        }
        let mut observations = Vec::new();
        for batch in batches {
            let json = batch
                .normalized_proposal_json
                .ok_or(MemoryError::Conflict)?;
            let mut part: Vec<SemanticObservationProposal> =
                serde_json::from_str(&json).map_err(|_| MemoryError::Conflict)?;
            observations.append(&mut part);
        }
        if observations.is_empty() {
            self.state
                .semantic_memory()
                .finish_empty_or_nonpublishable(
                    context,
                    prepared.extraction.id,
                    "success_empty",
                    None,
                )
                .await?;
            return self.submission_for(context, prepared.extraction.id).await;
        }
        let cards = observations
            .iter()
            .enumerate()
            .map(|(index, observation)| {
                Ok(SemanticCardProposal {
                    title: deterministic_card_title(index, &observation.statement),
                    kind: observation.kind,
                    scope: observation.scope,
                    assertion_status: observation.assertion_status,
                    observation_indices: vec![
                        u32::try_from(index).map_err(|_| MemoryError::Conflict)?,
                    ],
                })
            })
            .collect::<Result<Vec<_>, MemoryError>>()?;
        let validated = self
            .validate_proposal(
                context,
                core,
                &prepared.loaded,
                SemanticExtractionProposal {
                    outcome: SemanticProposalOutcome::SuccessNonempty,
                    observations,
                    cards,
                },
                true,
            )
            .await?;
        self.publish_validated(context, core, prepared.extraction.id, validated)
            .await
    }

    /// Reconstruct the runner result after State publication committed but
    /// before its artifact checkpoint did. This accepts only the same
    /// idempotent source extraction with a fully validated batch catalog.
    pub async fn resume_published_observation_batches(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
        prepared: &SemanticPreparedGeneration,
        expected_batch_count: u32,
    ) -> Result<SemanticSubmission, MemoryError> {
        if path != &prepared.loaded.revision.source_path
            || !matches!(
                prepared.extraction.state.as_str(),
                "success_nonempty" | "success_empty"
            )
        {
            return Err(MemoryError::Conflict);
        }
        self.verify_generation_fence(context, core, &prepared.loaded, &prepared.extraction)
            .await?;
        self.verify_block_id_namespace(context, &prepared.loaded)
            .await?;
        let batches = self
            .state
            .semantic_memory()
            .list_extraction_batches(context, prepared.extraction.id)
            .await?;
        if batches.len() != expected_batch_count as usize
            || batches.iter().enumerate().any(|(index, batch)| {
                batch.batch_index as usize != index
                    || batch.batch_count != expected_batch_count
                    || batch.state != "validated"
                    || batch.source_id != prepared.extraction.source_id
                    || batch.source_revision_id != prepared.extraction.source_revision_id
            })
        {
            return Err(MemoryError::Conflict);
        }
        self.submission_for(context, prepared.extraction.id).await
    }

    /// Validate the first Provider result against the source captured by
    /// `begin_generation`, then produce bounded composition groups.
    pub async fn accept_observation_result(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        prepared: SemanticPreparedGeneration,
        proposal_json: &str,
    ) -> Result<SemanticObservationPhase, MemoryError> {
        let extraction_id = prepared.extraction.id;
        let input: SemanticObservationPhaseInput = match serde_json::from_str(proposal_json) {
            Ok(input) => input,
            Err(_) => {
                let error = MemoryError::GeneratedOutput("semantic_proposal_schema");
                self.fail_running_extraction(context, extraction_id, error.code())
                    .await?;
                return Err(error);
            }
        };
        let derived_outcome = if input.observations.is_empty() {
            SemanticProposalOutcome::SuccessEmpty
        } else {
            SemanticProposalOutcome::SuccessNonempty
        };
        if input
            .outcome
            .is_some_and(|outcome| outcome != derived_outcome)
        {
            let error = MemoryError::GeneratedOutput("semantic_observation_outcome_mismatch");
            self.fail_running_extraction(context, extraction_id, error.code())
                .await?;
            return Err(error);
        }
        if let Err(error) = self
            .verify_generation_fence(context, core, &prepared.loaded, &prepared.extraction)
            .await
        {
            self.fail_running_extraction(context, extraction_id, error.code())
                .await?;
            return Err(error);
        }
        if let Err(error) = self
            .verify_block_id_namespace(context, &prepared.loaded)
            .await
        {
            self.fail_running_extraction(context, extraction_id, error.code())
                .await?;
            return Err(error);
        }
        let observations = input.observations;
        let proposal = SemanticExtractionProposal {
            outcome: derived_outcome,
            observations: observations.clone(),
            cards: Vec::new(),
        };
        if let Err(error) = self
            .validate_proposal(context, core, &prepared.loaded, proposal, false)
            .await
        {
            self.fail_running_extraction(context, extraction_id, error.code())
                .await?;
            return Err(error);
        }
        if observations.is_empty() {
            self.state
                .semantic_memory()
                .finish_empty_or_nonpublishable(
                    context,
                    prepared.extraction.id,
                    "success_empty",
                    None,
                )
                .await?;
        }
        Ok(SemanticObservationPhase {
            groups: compatibility_groups(&observations),
            extraction: prepared.extraction.clone(),
            observations,
            prepared,
        })
    }

    /// Bind a composition response to a prepared observation phase and send
    /// it through the existing full publication validator atomically.
    pub async fn submit_composition(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
        phase: &SemanticObservationPhase,
        composition_json: &str,
    ) -> Result<SemanticSubmission, MemoryError> {
        if phase.observations.is_empty() {
            return Err(MemoryError::Conflict);
        }
        if path != &phase.prepared.loaded.revision.source_path {
            let error = MemoryError::Conflict;
            self.fail_running_extraction(context, phase.extraction.id, error.code())
                .await?;
            return Err(error);
        }
        if let Err(error) = self
            .verify_generation_fence(context, core, &phase.prepared.loaded, &phase.extraction)
            .await
        {
            self.fail_running_extraction(context, phase.extraction.id, error.code())
                .await?;
            return Err(error);
        }
        let composition: SemanticCompositionProposal = match serde_json::from_str(composition_json)
        {
            Ok(composition) => composition,
            Err(_) => {
                let error = MemoryError::GeneratedOutput("semantic_composition_schema");
                self.fail_running_extraction(context, phase.extraction.id, error.code())
                    .await?;
                return Err(error);
            }
        };
        let cards = match assemble_card_proposals(&phase.observations, &composition.cards) {
            Ok(cards) => cards,
            Err(error) => {
                self.fail_running_extraction(context, phase.extraction.id, error.code())
                    .await?;
                return Err(error);
            }
        };
        let proposal = SemanticExtractionProposal {
            outcome: SemanticProposalOutcome::SuccessNonempty,
            observations: phase.observations.clone(),
            cards,
        };
        let validated = match self
            .validate_proposal(context, core, &phase.prepared.loaded, proposal, true)
            .await
        {
            Ok(validated) => validated,
            Err(error) => {
                self.fail_running_extraction(context, phase.extraction.id, error.code())
                    .await?;
                return Err(error);
            }
        };
        self.publish_validated(context, core, phase.extraction.id, validated)
            .await
    }

    async fn verify_generation_fence(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        loaded: &LoadedSource,
        extraction: &mcp_vault_state::SemanticExtractionRecord,
    ) -> Result<(), MemoryError> {
        let current = self
            .state
            .semantic_memory()
            .get_extraction(context, extraction.id)
            .await?
            .ok_or(MemoryError::Conflict)?;
        let source = self
            .state
            .semantic_memory()
            .get_source(context, extraction.source_id)
            .await?
            .ok_or(MemoryError::Conflict)?;
        let policy_record = self
            .state
            .settings()
            .get_vault(context, EXTRACTION_POLICY_KEY)
            .await?
            .ok_or(MemoryError::Conflict)?;
        let policy: ExtractionPolicy = serde_json::from_value(policy_record.value)
            .map_err(|_| MemoryError::Configuration("semantic_source_policy_invalid"))?;
        let policy_revision = policy_record
            .revision
            .as_i64()
            .map_err(|_| MemoryError::Configuration("semantic_source_policy_revision_invalid"))?;
        let mut actual_file = core
            .read(context, &loaded.revision.source_path)
            .await
            .map_err(|_| MemoryError::Conflict)?;
        let mut actual_bytes = Vec::new();
        actual_file
            .reader
            .read_to_end(&mut actual_bytes)
            .await
            .map_err(|_| MemoryError::Conflict)?;
        let actual_hash = hash_bytes(&actual_bytes);
        let rules_revision = self
            .state
            .semantic_rules()
            .current_rules_revision(context)
            .await?;
        if !matches!(
            current.state.as_str(),
            "running" | "success_nonempty" | "success_empty"
        ) || current.source_id != extraction.source_id
            || current.source_revision_id != extraction.source_revision_id
            || current.authorization_revision != extraction.authorization_revision
            || current.source_generation != extraction.source_generation
            || current.extraction_commit_sequence != extraction.extraction_commit_sequence
            || current.rules_revision != extraction.rules_revision
            || rules_revision != extraction.rules_revision
            || !policy.enabled
            || policy_revision != extraction.authorization_revision
            || source.current_revision_id != Some(extraction.source_revision_id)
            || source.content_hash != extraction.input_hash
            || source.authorization_revision != extraction.authorization_revision
            || source.source_generation != extraction.source_generation
            || source.extraction_commit_sequence != extraction.extraction_commit_sequence
            || loaded.source.source_id != extraction.source_id
            || loaded.revision.source_revision_id != extraction.source_revision_id
            || loaded.revision.content_hash != extraction.input_hash
            || actual_file.file.id != loaded.revision.file_id
            || actual_file.file.current_revision != loaded.revision.file_revision
            || actual_hash != extraction.input_hash
        {
            if current.state == "running" {
                let finished = self
                    .state
                    .semantic_memory()
                    .finish_empty_or_nonpublishable(
                        context,
                        extraction.id,
                        "failed",
                        Some("semantic_source_fence_conflict"),
                    )
                    .await;
                if finished.is_err() {
                    let _ = self.cancel_extraction(context, extraction.id).await;
                }
            }
            return Err(MemoryError::Conflict);
        }
        Ok(())
    }

    async fn verify_block_id_namespace(
        &self,
        context: &VaultContext,
        loaded: &LoadedSource,
    ) -> Result<(), MemoryError> {
        let revision_ids = self
            .state
            .semantic_memory()
            .list_source_revision_ids(context)
            .await?;
        let current_namespace = block_id_namespace(
            context.id(),
            loaded.revision.source_revision_id,
            &revision_ids,
        )?;
        ensure_block_id_namespace_matches(&loaded.block_id_namespace, &current_namespace)
    }

    async fn publish_validated(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        extraction_id: ExtractionSetId,
        validated: ValidatedProposal,
    ) -> Result<SemanticSubmission, MemoryError> {
        let repository = self.state.semantic_memory();
        if validated.observations.is_empty() {
            repository
                .finish_empty_or_nonpublishable(context, extraction_id, "success_empty", None)
                .await?;
            return self.submission_for(context, extraction_id).await;
        }
        let snapshots = match repository
            .prepare_publication(
                context,
                extraction_id,
                &validated.observations,
                &validated.cards,
            )
            .await
        {
            Ok(snapshots) => snapshots,
            Err(error) => {
                let _ = repository
                    .finish_empty_or_nonpublishable(
                        context,
                        extraction_id,
                        "failed",
                        Some("semantic_snapshot_prepare_failed"),
                    )
                    .await;
                return Err(error.into());
            }
        };
        if let Err(error) = self
            .write_and_apply(context, core, extraction_id, snapshots)
            .await
        {
            if !error.retryable() {
                repository
                    .reject_publication(context, extraction_id, error.code())
                    .await?;
            }
            return Err(error);
        }
        self.submission_for(context, extraction_id).await
    }

    /// Validate and publish one full model result. This method has no Provider
    /// client and performs no network or paid operation.
    pub async fn submit_proposal_json(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
        proposal_json: &str,
    ) -> Result<SemanticSubmission, MemoryError> {
        let loaded = self.load_source(context, core, path).await?;
        let proposal_hash = hash_bytes(proposal_json.as_bytes());
        let idempotency_material = format!(
            "{}:{}:{}:{}",
            loaded.source.source_id,
            loaded.revision.source_revision_id,
            SEMANTIC_PROFILE,
            proposal_hash,
        );
        let request_hash = hash_bytes(idempotency_material.as_bytes());
        let repository = self.state.semantic_memory();
        let (extraction, created) = repository
            .start_extraction(
                context,
                ExtractionSetId::new(),
                loaded.source.source_id,
                loaded.revision.source_revision_id,
                &loaded.revision.content_hash,
                SEMANTIC_PROFILE,
                &request_hash,
                &request_hash,
            )
            .await?;
        if !created && extraction.state != "running" && extraction.state != "prepared" {
            return Ok(SemanticSubmission { extraction });
        }

        if extraction.state == "prepared" {
            self.recover_publication(context, core, extraction.id)
                .await?;
            return self.submission_for(context, extraction.id).await;
        }

        let proposal: SemanticExtractionProposal = match serde_json::from_str(proposal_json) {
            Ok(proposal) => proposal,
            Err(_) => {
                repository
                    .finish_empty_or_nonpublishable(
                        context,
                        extraction.id,
                        "failed",
                        Some("semantic_proposal_schema"),
                    )
                    .await?;
                return Err(MemoryError::GeneratedOutput("semantic_proposal_schema"));
            }
        };
        let validated = match self
            .validate_proposal(context, core, &loaded, proposal, true)
            .await
        {
            Ok(validated) => validated,
            Err(error) => {
                repository
                    .finish_empty_or_nonpublishable(
                        context,
                        extraction.id,
                        "failed",
                        Some(error.code()),
                    )
                    .await?;
                return Err(error);
            }
        };

        if validated.observations.is_empty() {
            repository
                .finish_empty_or_nonpublishable(context, extraction.id, "success_empty", None)
                .await?;
            return self.submission_for(context, extraction.id).await;
        }

        let snapshots = match repository
            .prepare_publication(
                context,
                extraction.id,
                &validated.observations,
                &validated.cards,
            )
            .await
        {
            Ok(snapshots) => snapshots,
            Err(error) => {
                let _ = repository
                    .finish_empty_or_nonpublishable(
                        context,
                        extraction.id,
                        "failed",
                        Some("semantic_snapshot_prepare_failed"),
                    )
                    .await;
                return Err(error.into());
            }
        };
        if let Err(error) = self
            .write_and_apply(context, core, extraction.id, snapshots)
            .await
        {
            if !error.retryable() {
                repository
                    .reject_publication(context, extraction.id, error.code())
                    .await?;
            }
            return Err(error);
        }
        self.submission_for(context, extraction.id).await
    }

    pub async fn submit_proposal(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
        proposal: &SemanticExtractionProposal,
    ) -> Result<SemanticSubmission, MemoryError> {
        let json = serde_json::to_string(proposal)
            .map_err(|_| MemoryError::InvalidInput("semantic proposal cannot be serialized"))?;
        self.submit_proposal_json(context, core, path, &json).await
    }

    pub async fn list_cards(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        limit: u32,
    ) -> Result<Vec<SemanticCardRecord>, MemoryError> {
        if !self.extraction_enabled(context).await? {
            return Ok(Vec::new());
        }
        let cards = self
            .state
            .semantic_memory()
            .list_cards(context, limit)
            .await?;
        let mut readable = Vec::with_capacity(cards.len());
        for card in cards {
            if self.verify_card_file(context, core, &card).await? {
                readable.push(card);
            }
        }
        Ok(readable)
    }

    pub async fn get_card(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        card_id: MemoryCardId,
    ) -> Result<Option<SemanticCardRecord>, MemoryError> {
        if !self.extraction_enabled(context).await? {
            return Ok(None);
        }
        let Some(card) = self
            .state
            .semantic_memory()
            .get_card(context, card_id)
            .await?
        else {
            return Ok(None);
        };
        if self.verify_card_file(context, core, &card).await? {
            Ok(Some(card))
        } else {
            Ok(None)
        }
    }

    /// Read the complete current card set for one source revision. This is a
    /// source-scoped lookup, not the bounded global browse page.
    pub async fn list_cards_for_source_revision(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        source_id: SemanticSourceId,
        source_revision_id: SourceRevisionId,
    ) -> Result<Vec<SemanticCardRecord>, MemoryError> {
        if !self.extraction_enabled(context).await? {
            return Ok(Vec::new());
        }
        let cards = self
            .state
            .semantic_memory()
            .list_cards_for_source_revision(context, source_id, source_revision_id)
            .await?;
        let mut readable = Vec::with_capacity(cards.len());
        for card in cards {
            if self.verify_card_file(context, core, &card).await? {
                readable.push(card);
            }
        }
        Ok(readable)
    }

    /// Check the same Vault policy used by current card reads. Internal
    /// consumers such as MemoryPack must not qualify M2 projections alone.
    pub async fn ensure_current_policy(&self, context: &VaultContext) -> Result<bool, MemoryError> {
        self.extraction_enabled(context).await
    }

    /// Expand only the exact cited spans, rechecking current source identity,
    /// hash, and Core access at read time.
    pub async fn read_evidence(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        evidence_ref_id: EvidenceRefId,
    ) -> Result<Option<SemanticEvidenceText>, MemoryError> {
        if !self.extraction_enabled(context).await? {
            return Ok(None);
        }
        let repository = self.state.semantic_memory();
        let Some(evidence) = repository.get_evidence(context, evidence_ref_id).await? else {
            return Ok(None);
        };
        let Some(source) = repository.get_source(context, evidence.source_id).await? else {
            return Ok(None);
        };
        let Some(revision) = repository
            .get_source_revision(context, evidence.source_revision_id)
            .await?
        else {
            return Ok(None);
        };
        if !source.eligible
            || source.pending_rebuild
            || source.current_revision_id != Some(evidence.source_revision_id)
            || source.content_hash != revision.content_hash
        {
            return Ok(None);
        }
        let mut read = match core.read(context, &source.source_path).await {
            Ok(read) => read,
            Err(VaultError::NotFound) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if read.file.id != revision.file_id
            || !content_hash_matches(read.file.content_hash.as_deref(), &revision.content_hash)
        {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        read.reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| MemoryError::SourceIngestion("semantic_evidence_read_failed"))?;
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => return Ok(None),
        };
        let Some(body_spans) = validate_stored_spans(&content, &evidence.body_spans) else {
            return Ok(None);
        };
        let Some(context_spans) = validate_stored_spans(&content, &evidence.context_spans) else {
            return Ok(None);
        };
        let Some(body_span_records) = validated_span_records(&content, &evidence.body_spans) else {
            return Ok(None);
        };
        let Some(context_span_records) = validated_span_records(&content, &evidence.context_spans)
        else {
            return Ok(None);
        };
        Ok(Some(SemanticEvidenceText {
            source_id: evidence.source_id,
            source_revision_id: evidence.source_revision_id,
            source_path: source.source_path,
            body_spans,
            context_spans,
            body_span_records,
            context_span_records,
        }))
    }

    /// Synchronous qualification entry for content, deletion, or permission
    /// changes. Rebuild work remains pending until a fresh complete set commits.
    pub async fn invalidate_source(
        &self,
        context: &VaultContext,
        file_id: FileId,
        reason: &str,
        authorization_revision: Option<i64>,
    ) -> Result<bool, MemoryError> {
        Ok(self
            .state
            .semantic_memory()
            .invalidate_source(context, file_id, reason, authorization_revision)
            .await?)
    }

    /// Reconcile one event against the current Vault file row.
    ///
    /// The event payload is intentionally not trusted for path, revision, or
    /// content. A late or repeated event always reloads the current FileRecord;
    /// only an active, non-managed Markdown file is read through Vault Core.
    /// This method never invokes a Provider or publishes a semantic result.
    pub async fn reconcile_source_event(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        file_id: FileId,
    ) -> Result<SemanticSourceEventReport, MemoryError> {
        let repository = self.state.semantic_memory();
        let existing = repository.get_source_by_file(context, file_id).await?;

        let policy = self
            .state
            .settings()
            .get_vault(context, EXTRACTION_POLICY_KEY)
            .await?;
        let Some(policy) = policy else {
            repository
                .invalidate_all_sources(context, "permission_revoked", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::PolicyDisabled,
                existing.as_ref(),
                None,
                None,
            ));
        };
        let parsed: ExtractionPolicy = match serde_json::from_value(policy.value) {
            Ok(value) => value,
            Err(_) => {
                repository
                    .invalidate_all_sources(
                        context,
                        "policy_invalid",
                        Some(policy.revision.as_i64().map_err(|_| {
                            MemoryError::Configuration("semantic_source_policy_revision_invalid")
                        })?),
                    )
                    .await?;
                return Ok(source_event_report(
                    file_id,
                    SemanticSourceEventDisposition::PolicyInvalid,
                    existing.as_ref(),
                    None,
                    None,
                ));
            }
        };
        if !parsed.enabled {
            repository
                .invalidate_all_sources(
                    context,
                    "permission_revoked",
                    Some(policy.revision.as_i64().map_err(|_| {
                        MemoryError::Configuration("semantic_source_policy_revision_invalid")
                    })?),
                )
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::PolicyDisabled,
                existing.as_ref(),
                None,
                None,
            ));
        }
        let authorization_revision = policy
            .revision
            .as_i64()
            .map_err(|_| MemoryError::Configuration("semantic_source_policy_revision_invalid"))?;

        let Some(file) = self.state.files().get_by_id(context, file_id).await? else {
            repository
                .invalidate_source(context, file_id, "source_deleted", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::Invalidated,
                existing.as_ref(),
                None,
                None,
            ));
        };
        if !file.is_active()
            || file.entry_type != EntryType::File
            || core.is_managed_path(&file.path)
            || !file.path.as_str().to_ascii_lowercase().ends_with(".md")
        {
            repository
                .invalidate_source(context, file_id, "source_not_authorized", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::Invalidated,
                existing.as_ref(),
                None,
                Some(file.current_revision),
            ));
        }

        let mut read = match core.read(context, &file.path).await {
            Ok(read) => read,
            Err(VaultError::NotFound | VaultError::ExternalMismatch | VaultError::NeedsReview) => {
                repository
                    .invalidate_source(context, file_id, "source_unavailable", None)
                    .await?;
                return Ok(source_event_report(
                    file_id,
                    SemanticSourceEventDisposition::Invalidated,
                    existing.as_ref(),
                    None,
                    Some(file.current_revision),
                ));
            }
            Err(error) => return Err(error.into()),
        };
        if read.file.id != file.id || read.file.current_revision != file.current_revision {
            repository
                .invalidate_source(context, file_id, "source_changed", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::NeedsRebuild,
                existing.as_ref(),
                None,
                Some(file.current_revision),
            ));
        }
        let mut bytes = Vec::new();
        read.reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| MemoryError::SourceIngestion("semantic_source_read_failed"))?;
        let actual_hash = hash_bytes(&bytes);
        if !content_hash_matches(file.content_hash.as_deref(), &actual_hash)
            || !content_hash_matches(read.file.content_hash.as_deref(), &actual_hash)
        {
            repository
                .invalidate_source(context, file_id, "source_hash_mismatch", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::NeedsRebuild,
                existing.as_ref(),
                None,
                Some(file.current_revision),
            ));
        }

        // The first FileRecord may have become stale while Core was opening
        // and streaming the file. Re-read the authoritative row before
        // rebinding so a late event can never write an old path/revision back
        // over a newer commit.
        let Some(latest) = self.state.files().get_by_id(context, file_id).await? else {
            repository
                .invalidate_source(context, file_id, "source_deleted", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::Invalidated,
                existing.as_ref(),
                None,
                None,
            ));
        };
        if !latest.is_active()
            || latest.entry_type != EntryType::File
            || core.is_managed_path(&latest.path)
            || !latest.path.as_str().to_ascii_lowercase().ends_with(".md")
        {
            repository
                .invalidate_source(context, file_id, "source_not_authorized", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::Invalidated,
                existing.as_ref(),
                None,
                Some(latest.current_revision),
            ));
        }
        if latest.path != file.path
            || latest.current_revision != file.current_revision
            || !content_hash_matches(latest.content_hash.as_deref(), &actual_hash)
        {
            repository
                .invalidate_source(context, file_id, "source_changed", None)
                .await?;
            return Ok(source_event_report(
                file_id,
                SemanticSourceEventDisposition::NeedsRebuild,
                existing.as_ref(),
                None,
                Some(latest.current_revision),
            ));
        }

        let content = String::from_utf8(bytes)
            .map_err(|_| MemoryError::SourceIngestion("semantic_source_not_utf8"))?;
        let prior_source = existing.clone();
        let prior_revision = if let Some(revision_id) = prior_source
            .as_ref()
            .and_then(|source| source.current_revision_id)
        {
            repository.get_source_revision(context, revision_id).await?
        } else {
            None
        };
        let old_content = if let (Some(source), Some(revision)) =
            (prior_source.as_ref(), prior_revision.as_ref())
        {
            let mut historical = match core
                .read_revision(context, &latest.path, revision.file_revision)
                .await
            {
                Ok(read) => read,
                Err(_) => {
                    repository
                        .invalidate_source(context, file_id, "source_changed", None)
                        .await?;
                    return Ok(source_event_report(
                        file_id,
                        SemanticSourceEventDisposition::NeedsRebuild,
                        existing.as_ref(),
                        None,
                        Some(latest.current_revision),
                    ));
                }
            };
            if historical.file.id != source.file_id
                || historical.file.id != revision.file_id
                || !content_hash_matches(
                    historical.revision.content_hash.as_deref(),
                    &revision.content_hash,
                )
            {
                None
            } else {
                let mut bytes = Vec::new();
                historical
                    .reader
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| {
                        MemoryError::SourceIngestion("semantic_historical_source_read_failed")
                    })?;
                let actual = hash_bytes(&bytes);
                content_hash_matches(Some(&revision.content_hash), &actual)
                    .then(|| String::from_utf8(bytes).ok())
                    .flatten()
            }
        } else {
            None
        };
        let (source, revision, content_changed) = repository
            .upsert_source_revision_fenced(context, &latest, &actual_hash, authorization_revision)
            .await?;
        let disposition =
            if existing.is_some() && !content_changed && source.eligible && !source.pending_rebuild
            {
                SemanticSourceEventDisposition::NavigationOnly
            } else if content_changed {
                let outcome = if let (Some(old_source), Some(old_revision), Some(old_content)) = (
                    prior_source.as_ref(),
                    prior_revision.as_ref(),
                    old_content.as_deref(),
                ) {
                    self.try_local_rebind(
                        context,
                        core,
                        old_source,
                        old_revision,
                        old_content,
                        &content,
                        &source,
                        &revision,
                        authorization_revision,
                    )
                    .await
                    .unwrap_or(LocalRebindOutcome::None)
                } else {
                    LocalRebindOutcome::None
                };
                if matches!(outcome, LocalRebindOutcome::Complete) {
                    SemanticSourceEventDisposition::Rebound
                } else {
                    SemanticSourceEventDisposition::NeedsRebuild
                }
            } else {
                SemanticSourceEventDisposition::NeedsRebuild
            };
        Ok(source_event_report(
            file_id,
            disposition,
            Some(&source),
            Some(&revision),
            Some(file.current_revision),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn try_local_rebind(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        old_source: &SemanticSourceRecord,
        old_revision: &SemanticSourceRevisionRecord,
        old_content: &str,
        new_content: &str,
        new_source: &SemanticSourceRecord,
        new_revision: &SemanticSourceRevisionRecord,
        authorization_revision: i64,
    ) -> Result<LocalRebindOutcome, MemoryError> {
        let repository = self.state.semantic_memory();
        let extraction = repository
            .latest_reusable_extraction(
                context,
                old_source.source_id,
                old_revision.source_revision_id,
                SEMANTIC_PROFILE,
            )
            .await?;
        let Some(extraction) = extraction else {
            return Ok(LocalRebindOutcome::None);
        };
        if extraction.vault_id != context.id()
            || old_source.vault_id != context.id()
            || new_source.vault_id != context.id()
            || old_revision.vault_id != context.id()
            || new_revision.vault_id != context.id()
            || extraction.source_id != old_source.source_id
            || extraction.source_revision_id != old_revision.source_revision_id
            || old_revision.source_id != old_source.source_id
            || old_revision.file_id != old_source.file_id
            || new_revision.source_id != new_source.source_id
            || new_revision.file_id != new_source.file_id
            || extraction.authorization_revision != old_source.authorization_revision
            || extraction.source_generation != old_revision.source_generation
            || extraction.extraction_commit_sequence != old_source.extraction_commit_sequence
            || extraction.rules_revision
                != self
                    .state
                    .semantic_rules()
                    .current_rules_revision(context)
                    .await?
            || new_source.current_revision_id != Some(new_revision.source_revision_id)
            || new_source.content_hash != new_revision.content_hash
            || new_source.authorization_revision != authorization_revision
        {
            return Ok(LocalRebindOutcome::None);
        }
        let observations = repository.list_observations(context, extraction.id).await?;
        if observations.iter().any(|observation| {
            observation.extraction_set_id != extraction.id
                || observation.source_revision_id != extraction.source_revision_id
        }) {
            return Ok(LocalRebindOutcome::None);
        }
        let cards = repository
            .list_historical_cards_for_extraction(context, &extraction)
            .await?;
        if observations.is_empty()
            || cards.is_empty()
            || observations.len() != usize::try_from(extraction.observation_count).unwrap_or(0)
            || cards.len() != usize::try_from(extraction.card_count).unwrap_or(0)
        {
            return Ok(LocalRebindOutcome::None);
        }
        if cards.iter().any(|card| {
            card.vault_id != context.id()
                || card.source_id != old_source.source_id
                || card.source_revision_id != old_revision.source_revision_id
        }) {
            return Ok(LocalRebindOutcome::None);
        }
        let revision_ids = repository.list_source_revision_ids(context).await?;
        let old_namespace =
            block_id_namespace(context.id(), old_revision.source_revision_id, &revision_ids)?;
        let new_namespace =
            block_id_namespace(context.id(), new_revision.source_revision_id, &revision_ids)?;
        let old_blocks = make_blocks(old_content, &old_namespace)?;
        let blocks = make_blocks(new_content, &new_namespace)?;
        let mut mapped = HashMap::new();
        for observation in &observations {
            let evidence = repository
                .get_historical_evidence(
                    context,
                    observation.evidence_ref_id,
                    old_source.source_id,
                    old_revision.source_revision_id,
                )
                .await?;
            let Some(evidence) = evidence else {
                continue;
            };
            if evidence.source_id != old_source.source_id
                || evidence.source_revision_id != old_revision.source_revision_id
            {
                continue;
            }
            let Some(mapped_evidence) =
                map_rebind_evidence(old_content, new_content, &old_blocks, &blocks, &evidence)
            else {
                continue;
            };
            mapped.insert(observation.id, (observation.clone(), mapped_evidence));
        }
        let mut selected_ids = HashSet::new();
        let mut selected_cards = Vec::new();
        for card in cards {
            let complete = card.items.iter().all(|item| {
                mapped.contains_key(&item.observation_id)
                    && item
                        .evidence_ref_ids
                        .iter()
                        .all(|id| mapped.values().any(|(_, evidence)| evidence.id == *id))
            });
            if complete {
                for item in &card.items {
                    selected_ids.insert(item.observation_id);
                }
                selected_cards.push(card);
            }
        }
        if selected_cards.is_empty() || selected_cards.len() != extraction.card_count as usize {
            return Ok(LocalRebindOutcome::None);
        }
        if selected_ids.len() != observations.len() {
            return Ok(LocalRebindOutcome::None);
        }
        let ordered = observations
            .into_iter()
            .filter(|observation| selected_ids.contains(&observation.id))
            .collect::<Vec<_>>();
        let mut index_by_id = HashMap::new();
        let mut proposal_observations = Vec::with_capacity(ordered.len());
        for (index, observation) in ordered.iter().enumerate() {
            let Some((_, evidence)) = mapped.get(&observation.id) else {
                return Ok(LocalRebindOutcome::None);
            };
            index_by_id.insert(observation.id, index as u32);
            proposal_observations.push(rebind_observation_proposal(observation, evidence)?);
        }
        let proposal_cards = selected_cards
            .iter()
            .map(|card| {
                let mut seen_observations = HashSet::new();
                let observation_indices = card
                    .items
                    .iter()
                    .filter(|item| seen_observations.insert(item.observation_id))
                    .map(|item| {
                        index_by_id
                            .get(&item.observation_id)
                            .copied()
                            .ok_or(MemoryError::Conflict)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(SemanticCardProposal {
                    title: card.title.clone(),
                    kind: parse_semantic_kind(&card.kind)?,
                    scope: parse_semantic_scope(&card.scope_ref)?,
                    assertion_status: parse_semantic_status(&card.assertion_status)?,
                    observation_indices,
                })
            })
            .collect::<Result<Vec<_>, MemoryError>>()?;
        let proposal = SemanticExtractionProposal {
            outcome: SemanticProposalOutcome::SuccessNonempty,
            observations: proposal_observations,
            cards: proposal_cards,
        };
        let json = serde_json::to_string(&proposal).map_err(|_| {
            MemoryError::InvalidInput("semantic rebind proposal serialization failed")
        })?;
        let result = self
            .submit_proposal_json(context, core, &new_source.source_path, &json)
            .await;
        if result.is_err() {
            return Ok(LocalRebindOutcome::None);
        }
        Ok(LocalRebindOutcome::Complete)
    }

    /// Resume all prepared publications after Vault Core has recovered its own
    /// journals. State and canonical Markdown are reconciled by exact hash and
    /// revision, never by path or ID alone.
    pub async fn recover_pending_publications(
        &self,
        context: &VaultContext,
        core: &VaultCore,
    ) -> Result<(), MemoryError> {
        self.preflight_all_semantic_recovery(context).await?;
        core.recover(context).await?;
        self.recover_pending_publications_after_core(context, core)
            .await
    }

    /// Check all pending publication source/rules fences without touching
    /// Vault Core. Startup uses this before generic Core recovery so a stale
    /// semantic journal cannot be finalized by the generic recovery pass.
    pub async fn preflight_pending_publications(
        &self,
        context: &VaultContext,
    ) -> Result<(), MemoryError> {
        let ids = self
            .state
            .semantic_memory()
            .pending_extractions(context)
            .await?;
        let repository = self.state.semantic_memory();
        let rules_revision = self
            .state
            .semantic_rules()
            .current_rules_revision(context)
            .await?;
        for id in &ids {
            let snapshots = repository.pending_snapshots(context, *id).await?;
            if snapshots
                .iter()
                .any(|snapshot| snapshot.rules_revision != rules_revision)
            {
                repository
                    .reject_publication(context, *id, "semantic_rules_changed")
                    .await?;
                return Err(MemoryError::Conflict);
            }
            if self.preflight_recovery_sources(context, &snapshots).await? {
                repository
                    .reject_publication(context, *id, "semantic_source_fence_conflict")
                    .await?;
                return Err(MemoryError::Conflict);
            }
        }
        Ok(())
    }

    /// Preflight every semantic recovery barrier in this Vault before any
    /// caller enters generic Vault Core recovery. Startup and service-level
    /// recovery share this coordinator so one semantic type cannot recover
    /// around a stale journal owned by the other type.
    pub async fn preflight_all_semantic_recovery(
        &self,
        context: &VaultContext,
    ) -> Result<(), MemoryError> {
        preflight_all_semantic_recovery(&self.state, context).await
    }

    /// Apply pending publications after the caller has already completed the
    /// generic Vault Core maintenance recovery pass.
    pub async fn recover_pending_publications_after_core(
        &self,
        context: &VaultContext,
        core: &VaultCore,
    ) -> Result<(), MemoryError> {
        let ids = self
            .state
            .semantic_memory()
            .pending_extractions(context)
            .await?;
        for id in ids {
            self.recover_publication_after_core(context, core, id)
                .await?;
        }
        Ok(())
    }

    pub async fn recover_publication(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        extraction_set_id: ExtractionSetId,
    ) -> Result<(), MemoryError> {
        self.preflight_all_semantic_recovery(context).await?;
        core.recover(context).await?;
        self.recover_publication_after_core(context, core, extraction_set_id)
            .await
    }

    /// Apply one pending publication after the caller has completed generic
    /// Core recovery. The source and rules fences are checked again here.
    pub async fn recover_publication_after_core(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        extraction_set_id: ExtractionSetId,
    ) -> Result<(), MemoryError> {
        self.preflight_publication(context, extraction_set_id)
            .await?;
        let repository = self.state.semantic_memory();
        let snapshots = repository
            .pending_snapshots(context, extraction_set_id)
            .await?;
        if snapshots.is_empty() {
            let status = repository
                .get_extraction(context, extraction_set_id)
                .await?;
            return if status.is_some_and(|record| record.state == "success_nonempty") {
                Ok(())
            } else {
                Err(MemoryError::NotFound)
            };
        }
        self.write_and_apply(context, core, extraction_set_id, snapshots)
            .await
    }

    async fn preflight_publication(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<(), MemoryError> {
        let repository = self.state.semantic_memory();
        let snapshots = repository
            .pending_snapshots(context, extraction_set_id)
            .await?;
        let rules_revision = self
            .state
            .semantic_rules()
            .current_rules_revision(context)
            .await?;
        if snapshots
            .iter()
            .any(|snapshot| snapshot.rules_revision != rules_revision)
        {
            repository
                .reject_publication(context, extraction_set_id, "semantic_rules_changed")
                .await?;
            return Err(MemoryError::Conflict);
        }
        if self.preflight_recovery_sources(context, &snapshots).await? {
            repository
                .reject_publication(context, extraction_set_id, "semantic_source_fence_conflict")
                .await?;
            return Err(MemoryError::Conflict);
        }
        Ok(())
    }

    async fn preflight_recovery_sources(
        &self,
        context: &VaultContext,
        snapshots: &[SemanticPreparedSnapshot],
    ) -> Result<bool, MemoryError> {
        let repository = self.state.semantic_memory();
        for snapshot in snapshots {
            let source = repository
                .get_source(context, snapshot.source_id)
                .await?
                .ok_or(MemoryError::Conflict)?;
            if source.current_revision_id != Some(snapshot.source_revision_id)
                || source.content_hash != snapshot.source_content_hash
                || source.authorization_revision != snapshot.authorization_revision
                || source.source_generation != snapshot.source_generation
                || source.extraction_commit_sequence != snapshot.extraction_commit_sequence
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn write_and_apply(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        extraction_set_id: ExtractionSetId,
        snapshots: Vec<SemanticPreparedSnapshot>,
    ) -> Result<(), MemoryError> {
        let repository = self.state.semantic_memory();
        for snapshot in snapshots {
            if self
                .state
                .semantic_rules()
                .current_rules_revision(context)
                .await?
                != snapshot.rules_revision
            {
                repository
                    .reject_publication(context, extraction_set_id, "semantic_rules_changed")
                    .await?;
                return Err(MemoryError::Conflict);
            }
            if hash_bytes(&snapshot.canonical_bytes) != snapshot.proposed_file_hash {
                return Err(MemoryError::Quarantined);
            }
            if self
                .preflight_recovery_sources(context, std::slice::from_ref(&snapshot))
                .await?
            {
                repository
                    .reject_publication(
                        context,
                        extraction_set_id,
                        "semantic_source_fence_conflict",
                    )
                    .await?;
                return Err(MemoryError::Conflict);
            }
            let file = self.ensure_snapshot_file(context, core, &snapshot).await?;
            repository
                .mark_snapshot_written(context, snapshot.id, file.id, file.current_revision)
                .await?;
        }
        if let Err(error) = repository
            .apply_publication(context, extraction_set_id)
            .await
        {
            return Err(error.into());
        }
        Ok(())
    }

    async fn ensure_snapshot_file(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        snapshot: &SemanticPreparedSnapshot,
    ) -> Result<mcp_vault_state::FileRecord, MemoryError> {
        let mut current = match core.read_managed(context, &snapshot.target_path).await {
            Ok(read) => Some(read_managed_bytes(read).await?),
            Err(VaultError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(existing) = current.as_ref()
            && existing.bytes == snapshot.canonical_bytes
            && hash_bytes(&existing.bytes) == snapshot.proposed_file_hash
            && existing.file.current_revision == snapshot.proposed_file_revision
            && snapshot
                .expected_file_id
                .is_none_or(|expected| expected == existing.file.id)
        {
            return Ok(existing.file.clone());
        }

        let idempotency_key = format!("semantic-card:{}", snapshot.id);
        let write = match current.take() {
            Some(existing)
                if snapshot.expected_file_id == Some(existing.file.id)
                    && snapshot.expected_file_revision == Some(existing.file.current_revision) =>
            {
                core.replace_managed_bytes(
                    context,
                    &snapshot.target_path,
                    existing.file.current_revision,
                    &snapshot.canonical_bytes,
                    CoreActor::system(),
                    SourcePlane::System,
                    Some(&idempotency_key),
                )
                .await
            }
            None if snapshot.expected_file_id.is_none() => {
                core.create_managed_bytes(
                    context,
                    &snapshot.target_path,
                    &snapshot.canonical_bytes,
                    CoreActor::system(),
                    SourcePlane::System,
                    Some(&idempotency_key),
                )
                .await
            }
            _ => return Err(MemoryError::Conflict),
        };
        match write {
            Ok(result) => {
                let verified = core
                    .read_managed(context, &snapshot.target_path)
                    .await
                    .map_err(MemoryError::Core)?;
                let verified = read_managed_bytes(verified).await?;
                let file = verified.file;
                let bytes = verified.bytes;
                if file.id != result.file.id
                    || file.current_revision != snapshot.proposed_file_revision
                    || bytes != snapshot.canonical_bytes
                    || hash_bytes(&bytes) != snapshot.proposed_file_hash
                {
                    return Err(MemoryError::Conflict);
                }
                Ok(file)
            }
            Err(VaultError::AlreadyExists | VaultError::RevisionConflict { .. }) => {
                // A competing/replayed Core operation is adopted only after the
                // exact bytes, stable file identity, and proposed revision match.
                let verified = core
                    .read_managed(context, &snapshot.target_path)
                    .await
                    .map_err(MemoryError::Core)?;
                let verified = read_managed_bytes(verified).await?;
                let file = verified.file;
                let bytes = verified.bytes;
                if bytes == snapshot.canonical_bytes
                    && hash_bytes(&bytes) == snapshot.proposed_file_hash
                    && file.current_revision == snapshot.proposed_file_revision
                    && snapshot
                        .expected_file_id
                        .is_none_or(|expected| expected == file.id)
                {
                    Ok(file)
                } else {
                    Err(MemoryError::Conflict)
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn verify_card_file(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        card: &SemanticCardRecord,
    ) -> Result<bool, MemoryError> {
        let read = match core.read_managed(context, &card.canonical_path).await {
            Ok(read) => read,
            Err(VaultError::NotFound) => return Ok(false),
            Err(VaultError::ExternalMismatch | VaultError::NeedsReview) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let verified = read_managed_bytes(read).await?;
        Ok(verified.file.id == card.canonical_file_id
            && verified.file.current_revision == card.canonical_file_revision
            && content_hash_matches(
                verified.file.content_hash.as_deref(),
                &card.canonical_markdown_hash,
            )
            && hash_bytes(&verified.bytes) == card.canonical_markdown_hash)
    }

    async fn submission_for(
        &self,
        context: &VaultContext,
        extraction_set_id: ExtractionSetId,
    ) -> Result<SemanticSubmission, MemoryError> {
        let extraction = self
            .state
            .semantic_memory()
            .get_extraction(context, extraction_set_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        Ok(SemanticSubmission { extraction })
    }

    async fn extraction_enabled(&self, context: &VaultContext) -> Result<bool, MemoryError> {
        let policy = self
            .state
            .settings()
            .get_vault(context, EXTRACTION_POLICY_KEY)
            .await?;
        let Some(policy_record) = policy else {
            self.state
                .semantic_memory()
                .invalidate_all_sources(context, "permission_revoked", None)
                .await?;
            return Ok(false);
        };
        let policy_revision = policy_record.revision;
        let policy: ExtractionPolicy = serde_json::from_value(policy_record.value)
            .map_err(|_| MemoryError::Configuration("semantic_source_policy_invalid"))?;
        let _timeout = policy.request_timeout_seconds;
        if !policy.enabled {
            self.state
                .semantic_memory()
                .invalidate_all_sources(
                    context,
                    "permission_revoked",
                    Some(policy_revision.as_i64().map_err(|_| {
                        MemoryError::Configuration("semantic_source_policy_revision_invalid")
                    })?),
                )
                .await?;
        }
        Ok(policy.enabled)
    }

    async fn load_source(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
    ) -> Result<LoadedSource, MemoryError> {
        let policy_record = self
            .state
            .settings()
            .get_vault(context, EXTRACTION_POLICY_KEY)
            .await?;
        let (enabled, authorization_revision) = match policy_record {
            Some(record) => {
                let policy: ExtractionPolicy = serde_json::from_value(record.value)
                    .map_err(|_| MemoryError::Configuration("semantic_source_policy_invalid"))?;
                (
                    policy.enabled,
                    record.revision.as_i64().map_err(|_| {
                        MemoryError::Configuration("semantic_source_policy_revision_invalid")
                    })?,
                )
            }
            None => (false, 0),
        };
        if !enabled {
            self.state
                .semantic_memory()
                .invalidate_all_sources(context, "permission_revoked", Some(authorization_revision))
                .await?;
            return Err(MemoryError::Configuration(
                "semantic_extraction_not_enabled",
            ));
        }
        if core.is_managed_path(path) || !path.as_str().to_ascii_lowercase().ends_with(".md") {
            return Err(MemoryError::SourceIngestion(
                "semantic_source_not_authorized",
            ));
        }
        let mut read = match core.read(context, path).await {
            Ok(read) => read,
            Err(VaultError::NotFound) => {
                return Err(MemoryError::SourceIngestion("semantic_source_not_found"));
            }
            Err(error) => return Err(error.into()),
        };
        let recorded_hash = read
            .file
            .content_hash
            .clone()
            .ok_or(MemoryError::SourceIngestion("semantic_source_hash_missing"))?;
        let mut bytes = Vec::new();
        read.reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| MemoryError::SourceIngestion("semantic_source_read_failed"))?;
        let actual_hash = hash_bytes(&bytes);
        if !content_hash_matches(Some(&recorded_hash), &actual_hash) {
            return Err(MemoryError::SourceIngestion(
                "semantic_source_hash_mismatch",
            ));
        }
        let content = String::from_utf8(bytes.clone())
            .map_err(|_| MemoryError::SourceIngestion("semantic_source_not_utf8"))?;
        let path_hash = actual_hash;
        let (source, revision, _) = self
            .state
            .semantic_memory()
            .upsert_source_revision_fenced(context, &read.file, &path_hash, authorization_revision)
            .await?;
        if source.invalid_reason.as_deref() == Some("permission_revoked") {
            return Err(MemoryError::Configuration("semantic_source_access_revoked"));
        }
        if source.authorization_revision != authorization_revision {
            return Err(MemoryError::Conflict);
        }
        let revision_ids = self
            .state
            .semantic_memory()
            .list_source_revision_ids(context)
            .await?;
        let block_id_namespace =
            block_id_namespace(context.id(), revision.source_revision_id, &revision_ids)?;
        let blocks = make_blocks(&content, &block_id_namespace)?;
        let model_input = SemanticModelInput {
            blocks: blocks.iter().map(|block| block.display.clone()).collect(),
        };
        Ok(LoadedSource {
            source,
            revision,
            block_id_namespace,
            content,
            blocks,
            model_input,
        })
    }

    async fn validate_proposal(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        loaded: &LoadedSource,
        proposal: SemanticExtractionProposal,
        require_cards: bool,
    ) -> Result<ValidatedProposal, MemoryError> {
        let displayed: HashMap<&str, &LocalBlock> = loaded
            .blocks
            .iter()
            .map(|block| (block.display.local_id.as_str(), block))
            .collect();
        match proposal.outcome {
            SemanticProposalOutcome::SuccessEmpty => {
                if proposal.observations.is_empty() && proposal.cards.is_empty() {
                    return Ok(ValidatedProposal {
                        observations: Vec::new(),
                        cards: Vec::new(),
                    });
                }
                return Err(MemoryError::GeneratedOutput("semantic_empty_set_not_empty"));
            }
            SemanticProposalOutcome::SuccessNonempty => {
                if proposal.observations.is_empty() || (require_cards && proposal.cards.is_empty())
                {
                    return Err(MemoryError::GeneratedOutput(
                        "semantic_nonempty_set_incomplete",
                    ));
                }
            }
        }

        let mut observations = Vec::with_capacity(proposal.observations.len());
        for (index, observation) in proposal.observations.iter().enumerate() {
            validate_observation_text(observation)?;
            let body = resolve_block_ids(&observation.body_block_ids, &displayed)?;
            if body.is_empty() {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_evidence_body_missing",
                ));
            }
            let mut context_ids = resolve_block_ids(&observation.context_block_ids, &displayed)?;
            let body_ids = body
                .iter()
                .map(|block| block.display.local_id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            if let Some(time_scope) = &observation.source_time_scope {
                match time_scope.status {
                    SemanticTimeScopeStatus::Unknown => {
                        if time_scope.value.is_some() || !time_scope.evidence_block_ids.is_empty() {
                            return Err(MemoryError::GeneratedOutput(
                                "semantic_unknown_time_has_value",
                            ));
                        }
                    }
                    SemanticTimeScopeStatus::SourceStated => {
                        if time_scope
                            .value
                            .as_deref()
                            .is_none_or(|value| value.trim().is_empty())
                        {
                            return Err(MemoryError::GeneratedOutput(
                                "semantic_source_time_missing",
                            ));
                        }
                        for time_block in
                            resolve_block_ids(&time_scope.evidence_block_ids, &displayed)?
                        {
                            if !body_ids.contains(time_block.display.local_id.as_str()) {
                                context_ids.push(time_block);
                            }
                        }
                    }
                }
            }
            let body_blocks = unique_blocks(body)?;
            let mut context_blocks = unique_blocks(context_ids)?;
            if body_blocks.iter().any(|body_block| {
                context_blocks.iter().any(|context_block| {
                    context_block.display.local_id == body_block.display.local_id
                })
            }) {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_evidence_role_overlap",
                ));
            }
            for body_block in &body_blocks {
                for heading_id in &body_block.heading_context {
                    if let Some(heading) = displayed.get(heading_id.as_str()) {
                        context_blocks.push((*heading).clone());
                    }
                }
            }
            context_blocks.sort_by_key(|block| block.start_byte);
            context_blocks.dedup_by(|left, right| left.display.local_id == right.display.local_id);
            let evidence_id = EvidenceRefId::new();
            let evidence = SemanticEvidenceInput {
                id: evidence_id,
                body_spans: spans_for(&loaded.content, &body_blocks)?,
                context_spans: spans_for(&loaded.content, &context_blocks)?,
            };
            let observation_id = ObservationId::new();
            observations.push(SemanticObservationInput {
                id: observation_id,
                local_key: format!("observation-{:06}", index + 1),
                kind: observation.kind.as_str().to_owned(),
                statement: observation.statement.trim().to_owned(),
                scope: observation.scope.as_str().to_owned(),
                assertion_status: observation.assertion_status.as_str().to_owned(),
                source_time_scope: time_scope_value(observation.source_time_scope.as_ref())?,
                conditions: trim_strings(&observation.conditions),
                exceptions: trim_strings(&observation.exceptions),
                ordered_steps: trim_strings(&observation.ordered_steps),
                result: observation
                    .result
                    .as_deref()
                    .map(str::trim)
                    .map(str::to_owned),
                uncertainty: observation
                    .uncertainty
                    .as_deref()
                    .map(str::trim)
                    .map(str::to_owned),
                admission_reason: observation.admission_reason.trim().to_owned(),
                value_for_future_work: observation.value_for_future_work.trim().to_owned(),
                evidence,
            });
        }

        if !require_cards {
            return Ok(ValidatedProposal {
                observations,
                cards: Vec::new(),
            });
        }

        let mut used_observations = HashSet::new();
        let mut topics = HashSet::new();
        let mut cards = Vec::with_capacity(proposal.cards.len());
        for card_proposal in &proposal.cards {
            if card_proposal.title.trim().is_empty() || card_proposal.title.contains(['\n', '\r']) {
                return Err(MemoryError::GeneratedOutput("semantic_card_title_invalid"));
            }
            if card_proposal.observation_indices.is_empty() {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_card_without_observations",
                ));
            }
            let topic_key = normalize_topic(&card_proposal.title);
            if topic_key.is_empty() || !topics.insert(topic_key.clone()) {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_duplicate_card_topic",
                ));
            }
            let indices = unique_indices(&card_proposal.observation_indices, observations.len())?;
            for index in &indices {
                if !used_observations.insert(*index) {
                    return Err(MemoryError::GeneratedOutput("semantic_observation_reused"));
                }
                let proposal_observation = &proposal.observations[*index];
                if proposal_observation.kind != card_proposal.kind
                    || proposal_observation.scope != card_proposal.scope
                    || proposal_observation.assertion_status != card_proposal.assertion_status
                {
                    return Err(MemoryError::GeneratedOutput(
                        "semantic_card_kind_scope_or_status_mismatch",
                    ));
                }
            }
            let first_time = &proposal.observations[indices[0]].source_time_scope;
            if indices.iter().any(|index| {
                proposal.observations[*index].source_time_scope.as_ref() != first_time.as_ref()
            }) {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_card_mixed_time_scope",
                ));
            }
            let repository = self.state.semantic_memory();
            let head = repository
                .card_head(context, loaded.source.source_id, &topic_key)
                .await?;
            let (card_id, expected_card_revision_id, revision_number) = match head {
                Some(head) => {
                    let expected = head.current_revision_id.ok_or(MemoryError::Conflict)?;
                    let next = head
                        .revision_number
                        .checked_add(1)
                        .ok_or(MemoryError::Conflict)?;
                    (head.id, Some(expected), next)
                }
                None => (MemoryCardId::new(), None, 1),
            };
            let child = VaultPath::parse(&format!("semantic-memory/cards/{card_id}.md"))
                .map_err(|_| MemoryError::InvalidInput("semantic card path is invalid"))?;
            let canonical_path = core
                .managed_root()
                .join(&child)
                .map_err(|_| MemoryError::InvalidInput("semantic card path is invalid"))?;
            let mut items = Vec::new();
            for index in &indices {
                let observation = &observations[*index];
                push_card_item(
                    &mut items,
                    observation,
                    "core_assertion",
                    observation.statement.clone(),
                )?;
                for condition in &observation.conditions {
                    push_card_item(&mut items, observation, "condition", condition.clone())?;
                }
                for exception in &observation.exceptions {
                    push_card_item(&mut items, observation, "exception", exception.clone())?;
                }
                for step in &observation.ordered_steps {
                    push_card_item(&mut items, observation, "ordered_step", step.clone())?;
                }
                if let Some(result) = &observation.result {
                    push_card_item(&mut items, observation, "optional_detail", result.clone())?;
                }
                if let Some(uncertainty) = &observation.uncertainty {
                    push_card_item(
                        &mut items,
                        observation,
                        "unresolved_item",
                        uncertainty.clone(),
                    )?;
                }
                push_card_item(
                    &mut items,
                    observation,
                    "optional_detail",
                    observation.value_for_future_work.clone(),
                )?;
            }
            let card_revision_id = CardRevisionId::new();
            let temporal_scope = time_scope_value(first_time.as_ref())?;
            let markdown = render_card_markdown(
                card_id,
                card_revision_id,
                loaded.source.source_id,
                loaded.revision.source_revision_id,
                &loaded.revision.content_hash,
                &card_proposal.title,
                card_proposal.kind,
                card_proposal.scope,
                card_proposal.assertion_status,
                &items,
                &observations,
            )
            .into_bytes();
            cards.push(mcp_vault_state::SemanticCardRevisionInput {
                card_id,
                card_revision_id,
                expected_card_revision_id,
                revision_number,
                topic_key,
                title: card_proposal.title.trim().to_owned(),
                kind: card_proposal.kind.as_state_kind().to_owned(),
                scope_ref: card_proposal.scope.as_state_scope().to_owned(),
                assertion_status: card_proposal.assertion_status.as_state_status().to_owned(),
                temporal_scope,
                composition_profile_id: SEMANTIC_PROFILE.to_owned(),
                canonical_path,
                canonical_markdown_hash: hash_bytes(&markdown),
                canonical_bytes: markdown,
                items,
            });
        }
        if used_observations.len() != observations.len() {
            return Err(MemoryError::GeneratedOutput(
                "semantic_observation_not_carded",
            ));
        }
        Ok(ValidatedProposal {
            observations,
            cards,
        })
    }
}

struct MappedRebindEvidence {
    id: EvidenceRefId,
    body_block_ids: Vec<String>,
    context_block_ids: Vec<String>,
}

fn map_rebind_evidence(
    old_content: &str,
    new_content: &str,
    old_blocks: &[LocalBlock],
    new_blocks: &[LocalBlock],
    evidence: &SemanticEvidenceRecord,
) -> Option<MappedRebindEvidence> {
    let mut used = Vec::<(u64, u64)>::new();
    let mut map_spans = |spans: &[SemanticSpanInput]| -> Option<Vec<String>> {
        let mut ids = Vec::with_capacity(spans.len());
        for span in spans {
            let start = usize::try_from(span.start_byte).ok()?;
            let end = usize::try_from(span.end_byte).ok()?;
            let old_bytes = old_content.as_bytes().get(start..end)?;
            if hash_bytes(old_bytes) != span.content_hash || old_bytes.is_empty() || start > end {
                return None;
            }
            let old_block = old_blocks.iter().find(|block| {
                block.start_byte == span.start_byte
                    && block.end_byte == span.end_byte
                    && block.hash == span.content_hash
            })?;
            let mut matches = Vec::new();
            let mut cursor = 0usize;
            while let Some(relative) = new_content.as_bytes()[cursor..]
                .windows(old_bytes.len())
                .position(|window| window == old_bytes)
            {
                let match_start = cursor + relative;
                let match_end = match_start + old_bytes.len();
                if new_content.is_char_boundary(match_start)
                    && new_content.is_char_boundary(match_end)
                {
                    matches.push((match_start as u64, match_end as u64));
                }
                cursor = match_start.saturating_add(1);
                if cursor >= new_content.len() {
                    break;
                }
            }
            if matches.len() != 1 {
                return None;
            }
            let (new_start, new_end) = matches[0];
            if used
                .iter()
                .any(|(start, end)| new_start < *end && new_end > *start)
            {
                return None;
            }
            let block = new_blocks.iter().find(|block| {
                block.start_byte == new_start
                    && block.end_byte == new_end
                    && block.hash == span.content_hash
            })?;
            if block.display.kind != old_block.display.kind {
                return None;
            }
            used.push((new_start, new_end));
            ids.push(block.display.local_id.clone());
        }
        Some(ids)
    };
    let body_block_ids = map_spans(&evidence.body_spans)?;
    if body_block_ids.is_empty() {
        return None;
    }
    let context_block_ids = map_spans(&evidence.context_spans)?;
    Some(MappedRebindEvidence {
        id: evidence.id,
        body_block_ids,
        context_block_ids,
    })
}

fn rebind_observation_proposal(
    observation: &SemanticObservationRecord,
    evidence: &MappedRebindEvidence,
) -> Result<SemanticObservationProposal, MemoryError> {
    let time_scope = if observation.source_time_scope.is_object() {
        let status = observation
            .source_time_scope
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let status = match status {
            "source_stated" => SemanticTimeScopeStatus::SourceStated,
            _ => SemanticTimeScopeStatus::Unknown,
        };
        if matches!(status, SemanticTimeScopeStatus::SourceStated) {
            return Err(MemoryError::Conflict);
        }
        let value = observation
            .source_time_scope
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if matches!(status, SemanticTimeScopeStatus::SourceStated)
            && (value.as_deref().is_none_or(str::is_empty) || evidence.context_block_ids.is_empty())
        {
            return Err(MemoryError::Conflict);
        }
        Some(SemanticTimeScopeProposal {
            status,
            value,
            evidence_block_ids: if matches!(status, SemanticTimeScopeStatus::SourceStated) {
                evidence.context_block_ids.clone()
            } else {
                Vec::new()
            },
        })
    } else {
        None
    };
    Ok(SemanticObservationProposal {
        kind: parse_semantic_kind(&observation.kind)?,
        statement: observation.statement.clone(),
        scope: parse_semantic_scope(&observation.scope)?,
        assertion_status: parse_semantic_status(&observation.assertion_status)?,
        source_time_scope: time_scope,
        conditions: observation.conditions.clone(),
        exceptions: observation.exceptions.clone(),
        ordered_steps: observation.ordered_steps.clone(),
        result: observation.result.clone(),
        uncertainty: observation.uncertainty.clone(),
        admission_reason: observation.admission_reason.clone(),
        value_for_future_work: observation.value_for_future_work.clone(),
        body_block_ids: evidence.body_block_ids.clone(),
        context_block_ids: evidence.context_block_ids.clone(),
    })
}

fn parse_semantic_kind(value: &str) -> Result<SemanticKind, MemoryError> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(|_| MemoryError::Conflict)
}

fn parse_semantic_scope(value: &str) -> Result<SemanticScope, MemoryError> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(|_| MemoryError::Conflict)
}

fn parse_semantic_status(value: &str) -> Result<SemanticAssertionStatus, MemoryError> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(|_| MemoryError::Conflict)
}

/// Run the cross-type semantic recovery fence without touching Vault Core.
/// SQL remains inside the State repositories; this function only composes
/// their service-level preflight APIs.
pub(crate) async fn preflight_all_semantic_recovery(
    state: &StateStore,
    context: &VaultContext,
) -> Result<(), MemoryError> {
    state
        .preflight_semantic_recovery(context)
        .await
        .map_err(|error| {
            if matches!(error, mcp_vault_state::StateError::Conflict) {
                MemoryError::Conflict
            } else {
                MemoryError::State(error)
            }
        })
}

struct ManagedFileBytes {
    file: mcp_vault_state::FileRecord,
    bytes: Vec<u8>,
}

async fn read_managed_bytes(mut read: ManagedReadResult) -> Result<ManagedFileBytes, MemoryError> {
    let file = read.file.clone();
    let mut bytes = Vec::new();
    read.reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| MemoryError::SourceIngestion("semantic_card_read_failed"))?;
    Ok(ManagedFileBytes { file, bytes })
}

fn source_event_report(
    file_id: FileId,
    disposition: SemanticSourceEventDisposition,
    source: Option<&SemanticSourceRecord>,
    revision: Option<&SemanticSourceRevisionRecord>,
    file_revision: Option<Revision>,
) -> SemanticSourceEventReport {
    SemanticSourceEventReport {
        file_id,
        disposition,
        source_id: source.map(|value| value.source_id),
        source_revision_id: revision.map(|value| value.source_revision_id),
        file_revision,
        source_generation: source.map(|value| value.source_generation),
    }
}

const BLOCK_ID_MIN_HASH_HEX: usize = 12;

fn block_revision_digest(vault_id: VaultId, revision_id: SourceRevisionId) -> String {
    hash_bytes(
        format!(
            "mcp-vault-semantic-block-id-v1\0{}\0{}",
            vault_id, revision_id
        )
        .as_bytes(),
    )
}

fn block_id_namespace(
    vault_id: VaultId,
    revision_id: SourceRevisionId,
    revision_ids: &[SourceRevisionId],
) -> Result<String, MemoryError> {
    if !revision_ids.contains(&revision_id) {
        return Err(MemoryError::Conflict);
    }
    let target_digest = block_revision_digest(vault_id, revision_id);
    let other_digests = revision_ids
        .iter()
        .copied()
        .filter(|candidate| *candidate != revision_id)
        .map(|candidate| block_revision_digest(vault_id, candidate))
        .collect::<Vec<_>>();
    Ok(select_unique_block_namespace(
        revision_id,
        &target_digest,
        &other_digests,
    ))
}

fn select_unique_block_namespace(
    revision_id: SourceRevisionId,
    target_digest: &str,
    other_digests: &[String],
) -> String {
    for prefix_len in (BLOCK_ID_MIN_HASH_HEX..=target_digest.len()).step_by(2) {
        let prefix = &target_digest[..prefix_len];
        if other_digests
            .iter()
            .all(|candidate| !candidate.starts_with(prefix))
        {
            return format!("h{prefix}");
        }
    }
    // A full SHA-256 collision must not silently reuse a model-visible ID.
    // SourceRevisionId is unique per Vault by the State schema, so its exact
    // canonical UUID is a collision-free fallback namespace.
    format!("r{}", revision_id.to_string().replace('-', ""))
}

fn ensure_block_id_namespace_matches(
    prepared_namespace: &str,
    current_namespace: &str,
) -> Result<(), MemoryError> {
    if prepared_namespace == current_namespace {
        Ok(())
    } else {
        Err(MemoryError::Conflict)
    }
}

fn next_block_index(previous_count: usize) -> Result<usize, MemoryError> {
    previous_count
        .checked_add(1)
        .ok_or(MemoryError::GeneratedOutput(
            "semantic_block_index_overflow",
        ))
}

fn base36(mut value: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut encoded = Vec::new();
    loop {
        encoded.push(DIGITS[value % 36]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    encoded.reverse();
    String::from_utf8(encoded).expect("base36 alphabet is valid UTF-8")
}

fn model_block_id(namespace: &str, one_based_index: usize) -> Result<String, MemoryError> {
    if one_based_index == 0 {
        return Err(MemoryError::GeneratedOutput("semantic_block_index_invalid"));
    }
    Ok(format!("b{namespace}-{}", base36(one_based_index)))
}

fn make_blocks(content: &str, namespace: &str) -> Result<Vec<LocalBlock>, MemoryError> {
    let mut blocks = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut offset = 0usize;
    let mut line_number = 1u32;
    let mut in_code_fence = false;
    for line in content.split_inclusive('\n') {
        let left_trimmed = line.trim_start();
        let left_bytes = line.len() - left_trimmed.len();
        let text = left_trimmed.trim_end();
        let start = offset + left_bytes;
        let end = start + text.len();
        if !text.trim().is_empty() {
            let index = next_block_index(blocks.len())?;
            let hash = hash_bytes(&content.as_bytes()[start..end]);
            let local_id = model_block_id(namespace, index)?;
            let heading_level = heading_level(text);
            let heading_context = if let Some(level) = heading_level {
                heading_stack.retain(|(prior_level, _)| *prior_level < level);
                heading_stack.push((level, local_id.clone()));
                Vec::new()
            } else {
                heading_stack.iter().map(|(_, id)| id.clone()).collect()
            };
            let kind = block_kind(text, in_code_fence).to_owned();
            blocks.push(LocalBlock {
                display: SemanticSourceBlock {
                    local_id,
                    text: text.to_owned(),
                    line_number,
                    kind,
                },
                start_byte: start as u64,
                end_byte: end as u64,
                hash,
                heading_context,
            });
        }
        if text.trim_start().starts_with("```") || text.trim_start().starts_with("~~~") {
            in_code_fence = !in_code_fence;
        }
        offset += line.len();
        line_number = line_number.saturating_add(1);
    }
    Ok(blocks)
}

fn heading_level(text: &str) -> Option<usize> {
    let hashes = text.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6)
        .contains(&hashes)
        .then_some(hashes)
        .filter(|_| text.as_bytes().get(hashes) == Some(&b' '))
}

fn block_kind(text: &str, in_code_fence: bool) -> &'static str {
    if in_code_fence || text.trim_start().starts_with("```") || text.trim_start().starts_with("~~~")
    {
        "code"
    } else if heading_level(text).is_some() {
        "heading"
    } else if text.trim_start().starts_with('|') {
        "table_row"
    } else if text.trim_start().starts_with(['-', '*', '+'])
        || text
            .trim_start()
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    {
        "list_or_step"
    } else {
        "text"
    }
}

fn resolve_block_ids<'a>(
    ids: &[String],
    displayed: &'a HashMap<&str, &'a LocalBlock>,
) -> Result<Vec<LocalBlock>, MemoryError> {
    let mut seen = HashSet::new();
    ids.iter()
        .map(|id| {
            if !seen.insert(id.as_str()) {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_duplicate_evidence_id",
                ));
            }
            displayed
                .get(id.as_str())
                .map(|block| (*block).clone())
                .ok_or(MemoryError::GeneratedOutput("semantic_forged_evidence_id"))
        })
        .collect()
}

fn unique_blocks(blocks: Vec<LocalBlock>) -> Result<Vec<LocalBlock>, MemoryError> {
    let mut seen = HashSet::new();
    let mut unique = Vec::with_capacity(blocks.len());
    for block in blocks {
        if seen.insert(block.display.local_id.clone()) {
            unique.push(block);
        }
    }
    unique.sort_by_key(|block| block.start_byte);
    Ok(unique)
}

fn spans_for(content: &str, blocks: &[LocalBlock]) -> Result<Vec<SemanticSpanInput>, MemoryError> {
    blocks
        .iter()
        .map(|block| {
            let start = usize::try_from(block.start_byte)
                .map_err(|_| MemoryError::GeneratedOutput("semantic_span_out_of_bounds"))?;
            let end = usize::try_from(block.end_byte)
                .map_err(|_| MemoryError::GeneratedOutput("semantic_span_out_of_bounds"))?;
            let span = content.get(start..end).ok_or(MemoryError::GeneratedOutput(
                "semantic_span_not_utf8_boundary",
            ))?;
            let hash = hash_bytes(span.as_bytes());
            if hash != block.hash {
                return Err(MemoryError::GeneratedOutput("semantic_span_hash_mismatch"));
            }
            Ok(SemanticSpanInput {
                start_byte: block.start_byte,
                end_byte: block.end_byte,
                content_hash: hash,
            })
        })
        .collect()
}

fn validate_stored_spans(content: &str, spans: &[SemanticSpanInput]) -> Option<Vec<String>> {
    spans
        .iter()
        .map(|span| {
            let start = usize::try_from(span.start_byte).ok()?;
            let end = usize::try_from(span.end_byte).ok()?;
            let value = content.get(start..end)?;
            (hash_bytes(value.as_bytes()) == span.content_hash).then(|| value.to_owned())
        })
        .collect()
}

fn validated_span_records(
    content: &str,
    spans: &[SemanticSpanInput],
) -> Option<Vec<SemanticEvidenceSpanText>> {
    spans
        .iter()
        .map(|span| {
            let start = usize::try_from(span.start_byte).ok()?;
            let end = usize::try_from(span.end_byte).ok()?;
            let text = content.get(start..end)?;
            (hash_bytes(text.as_bytes()) == span.content_hash).then(|| SemanticEvidenceSpanText {
                start_byte: span.start_byte,
                end_byte: span.end_byte,
                content_hash: span.content_hash.clone(),
                text: text.to_owned(),
            })
        })
        .collect()
}

fn validate_observation_text(observation: &SemanticObservationProposal) -> Result<(), MemoryError> {
    if observation.statement.trim().is_empty()
        || observation.admission_reason.trim().is_empty()
        || observation.value_for_future_work.trim().is_empty()
    {
        return Err(MemoryError::GeneratedOutput(
            "semantic_observation_required_text_missing",
        ));
    }
    if observation
        .conditions
        .iter()
        .any(|value| value.trim().is_empty())
        || observation
            .exceptions
            .iter()
            .any(|value| value.trim().is_empty())
        || observation
            .ordered_steps
            .iter()
            .any(|value| value.trim().is_empty())
        || observation
            .result
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || observation
            .uncertainty
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err(MemoryError::GeneratedOutput(
            "semantic_observation_empty_detail",
        ));
    }
    Ok(())
}

fn unique_indices(indices: &[u32], observation_count: usize) -> Result<Vec<usize>, MemoryError> {
    let mut unique = BTreeSet::new();
    for index in indices {
        let index = usize::try_from(*index)
            .map_err(|_| MemoryError::GeneratedOutput("semantic_observation_index_invalid"))?;
        if index >= observation_count || !unique.insert(index) {
            return Err(MemoryError::GeneratedOutput(
                "semantic_observation_index_invalid",
            ));
        }
    }
    Ok(unique.into_iter().collect())
}

fn trim_strings(values: &[String]) -> Vec<String> {
    values.iter().map(|value| value.trim().to_owned()).collect()
}

fn time_scope_value(scope: Option<&SemanticTimeScopeProposal>) -> Result<Value, MemoryError> {
    scope.map_or_else(
        || Ok(json!({"status":"unknown"})),
        |scope| {
            serde_json::to_value(scope)
                .map_err(|_| MemoryError::GeneratedOutput("semantic_time_scope_invalid"))
        },
    )
}

fn push_card_item(
    items: &mut Vec<SemanticCardItemInput>,
    observation: &SemanticObservationInput,
    kind: &str,
    content: String,
) -> Result<(), MemoryError> {
    let ordinal = items.iter().filter(|item| item.kind == kind).count();
    let ordinal = u32::try_from(ordinal)
        .map_err(|_| MemoryError::GeneratedOutput("semantic_card_too_many_items"))?;
    items.push(SemanticCardItemInput {
        id: CardItemId::new(),
        observation_id: observation.id,
        kind: kind.to_owned(),
        ordinal,
        content,
        evidence_ref_ids: vec![observation.evidence.id],
    });
    Ok(())
}

fn normalize_topic(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn content_hash_matches(expected: Option<&str>, actual: &str) -> bool {
    expected
        .map(|expected| expected.strip_prefix("sha256:").unwrap_or(expected))
        .is_some_and(|expected| expected.eq_ignore_ascii_case(actual))
}

#[allow(clippy::too_many_arguments)]
fn render_card_markdown(
    card_id: MemoryCardId,
    card_revision_id: CardRevisionId,
    source_id: SemanticSourceId,
    source_revision_id: SourceRevisionId,
    source_content_hash: &str,
    title: &str,
    kind: SemanticKind,
    scope: SemanticScope,
    status: SemanticAssertionStatus,
    items: &[SemanticCardItemInput],
    observations: &[SemanticObservationInput],
) -> String {
    let mut output = format!(
        "---\nkind: semantic_memory_card\ncard_id: {card_id}\ncard_revision_id: {card_revision_id}\nsource_id: {source_id}\nsource_revision_id: {source_revision_id}\nsource_content_hash: {source_content_hash}\n---\n\n# {}\n\n- 内容类型：{}\n- 适用范围：{}\n- 来源状态：{}\n\n",
        title.trim(),
        kind.as_str(),
        scope.as_str(),
        status.as_str(),
    );
    for (heading, item_kind) in [
        ("核心断言", "core_assertion"),
        ("必要条件", "condition"),
        ("例外", "exception"),
        ("有序步骤", "ordered_step"),
        ("结果与补充", "optional_detail"),
        ("未解决与不确定项", "unresolved_item"),
    ] {
        let selected = items
            .iter()
            .filter(|item| item.kind == item_kind)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            continue;
        }
        output.push_str(&format!("## {heading}\n\n"));
        for item in selected {
            let observation = observations
                .iter()
                .find(|observation| observation.id == item.observation_id);
            if item_kind == "ordered_step" {
                output.push_str(&format!("{}. ", item.ordinal + 1));
            } else {
                output.push_str("- ");
            }
            let quoted = item
                .content
                .lines()
                .map(|line| format!("  > {line}\n"))
                .collect::<String>();
            output.push_str(&quoted);
            if let Some(observation) = observation {
                output.push_str(&format!(
                    "  > Observation `{}` · EvidenceRef `{}`\n",
                    observation.id, observation.evidence.id
                ));
            }
        }
        output.push('\n');
    }
    output
}

impl SemanticKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Constraint => "constraint",
            Self::Decision => "decision",
            Self::Experience => "experience",
            Self::Procedure => "procedure",
            Self::State => "state",
            Self::Unknown => "unknown",
        }
    }
}

fn deterministic_card_title(index: usize, statement: &str) -> String {
    let compact = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    let excerpt = compact.chars().take(64).collect::<String>();
    format!("Observation {:03}: {excerpt}", index.saturating_add(1))
}

fn parse_a80_observation_response(
    response_json: &str,
    blocks: &[SemanticSourceBlock],
    start: usize,
    end: usize,
) -> Result<
    (
        Vec<SemanticObservationProposal>,
        SemanticAdvisoryNormalizationCounts,
    ),
    MemoryError,
> {
    let value: Value = serde_json::from_str(response_json)
        .map_err(|_| MemoryError::GeneratedOutput("semantic_flat_observation_json"))?;
    let object = value.as_object().ok_or(MemoryError::GeneratedOutput(
        "semantic_flat_observation_shape",
    ))?;
    if object.len() != 1 || object.keys().any(|key| key != "claims") {
        return Err(MemoryError::GeneratedOutput(
            "semantic_flat_observation_namespace_or_fields",
        ));
    }
    let claims =
        object
            .get("claims")
            .and_then(Value::as_array)
            .ok_or(MemoryError::GeneratedOutput(
                "semantic_flat_observation_shape",
            ))?;
    let mut counts = SemanticAdvisoryNormalizationCounts::default();
    let mut observations = Vec::with_capacity(claims.len());
    for claim in claims {
        let claim = claim
            .as_object()
            .ok_or(MemoryError::GeneratedOutput("semantic_flat_claim_shape"))?;
        const FIELDS: &[&str] = &[
            "statement",
            "evidence_indices",
            "kind",
            "scope",
            "assertion_status",
            "source_time_scope",
        ];
        if claim.keys().any(|key| !FIELDS.contains(&key.as_str())) {
            return Err(MemoryError::GeneratedOutput(
                "semantic_flat_claim_unknown_field",
            ));
        }
        let statement = claim
            .get("statement")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or(MemoryError::GeneratedOutput(
                "semantic_flat_claim_statement_invalid",
            ))?
            .trim()
            .to_owned();
        let body_block_ids =
            map_a80_indices(claim.get("evidence_indices"), blocks, start, end, true)?;
        let (kind, kind_unknown) =
            advisory_enum::<SemanticKind>(claim.get("kind"), SemanticKind::Unknown);
        if kind_unknown {
            counts.kind_unknown = counts.kind_unknown.saturating_add(1);
        }
        let (scope, scope_unspecified) =
            advisory_enum::<SemanticScope>(claim.get("scope"), SemanticScope::Unspecified);
        if scope_unspecified {
            counts.scope_unspecified = counts.scope_unspecified.saturating_add(1);
        }
        let (assertion_status, status_unknown) = advisory_enum::<SemanticAssertionStatus>(
            claim.get("assertion_status"),
            SemanticAssertionStatus::Unknown,
        );
        if status_unknown {
            counts.status_unknown = counts.status_unknown.saturating_add(1);
        }
        let (source_time_scope, time_unknown) =
            parse_a80_time_scope(claim.get("source_time_scope"), blocks, start, end)?;
        if time_unknown {
            counts.time_scope_unknown = counts.time_scope_unknown.saturating_add(1);
        }
        observations.push(SemanticObservationProposal {
            kind,
            statement: statement.clone(),
            scope,
            assertion_status,
            source_time_scope: Some(source_time_scope),
            conditions: Vec::new(),
            exceptions: Vec::new(),
            ordered_steps: Vec::new(),
            result: None,
            uncertainty: None,
            admission_reason: "validated_source_supported_claim".to_owned(),
            value_for_future_work: "Source-supported claim selected for future work".to_owned(),
            body_block_ids,
            context_block_ids: Vec::new(),
        });
    }
    Ok((observations, counts))
}

fn advisory_enum<T>(value: Option<&Value>, fallback: T) -> (T, bool)
where
    T: for<'de> Deserialize<'de> + PartialEq + Clone,
{
    let parsed = value
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    match parsed {
        Some(parsed) => (parsed, false),
        None => (fallback, true),
    }
}

fn map_a80_indices(
    value: Option<&Value>,
    blocks: &[SemanticSourceBlock],
    start: usize,
    end: usize,
    required: bool,
) -> Result<Vec<String>, MemoryError> {
    let Some(value) = value else {
        return if required {
            Err(MemoryError::GeneratedOutput(
                "semantic_flat_evidence_indices_invalid",
            ))
        } else {
            Ok(Vec::new())
        };
    };
    let indices = value.as_array().ok_or(MemoryError::GeneratedOutput(
        "semantic_flat_evidence_indices_invalid",
    ))?;
    if required && indices.is_empty() {
        return Err(MemoryError::GeneratedOutput(
            "semantic_flat_evidence_indices_invalid",
        ));
    }
    let mut seen = BTreeSet::new();
    indices
        .iter()
        .map(|value| {
            let index = value
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| (start + 1..=end).contains(index))
                .ok_or(MemoryError::GeneratedOutput(
                    "semantic_flat_evidence_index_invalid",
                ))?;
            if !seen.insert(index) {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_flat_evidence_index_duplicate",
                ));
            }
            Ok(blocks[index - 1].local_id.clone())
        })
        .collect()
}

fn parse_a80_time_scope(
    value: Option<&Value>,
    blocks: &[SemanticSourceBlock],
    start: usize,
    end: usize,
) -> Result<(SemanticTimeScopeProposal, bool), MemoryError> {
    let unknown = || {
        (
            SemanticTimeScopeProposal {
                status: SemanticTimeScopeStatus::Unknown,
                value: None,
                evidence_block_ids: Vec::new(),
            },
            true,
        )
    };
    let Some(object) = value.and_then(Value::as_object) else {
        return Ok(unknown());
    };
    if object
        .keys()
        .any(|key| !["status", "value", "evidence_indices"].contains(&key.as_str()))
    {
        return Err(MemoryError::GeneratedOutput(
            "semantic_flat_claim_unknown_field",
        ));
    }
    if ["status", "value", "evidence_indices"]
        .iter()
        .any(|field| !object.contains_key(*field))
    {
        return Ok(unknown());
    }
    let status = object.get("status").and_then(Value::as_str);
    let time_value = object.get("value").and_then(Value::as_str);
    let evidence_value = object.get("evidence_indices");
    match (status, time_value) {
        (Some("unknown"), Some("")) => {
            let ids = map_a80_indices(evidence_value, blocks, start, end, false)?;
            if ids.is_empty() {
                Ok((
                    SemanticTimeScopeProposal {
                        status: SemanticTimeScopeStatus::Unknown,
                        value: None,
                        evidence_block_ids: ids,
                    },
                    false,
                ))
            } else {
                Ok(unknown())
            }
        }
        (Some("source_stated"), Some(text)) if !text.trim().is_empty() => {
            let ids = map_a80_indices(evidence_value, blocks, start, end, true)?;
            if ids.is_empty() {
                Ok(unknown())
            } else {
                Ok((
                    SemanticTimeScopeProposal {
                        status: SemanticTimeScopeStatus::SourceStated,
                        value: Some(text.trim().to_owned()),
                        evidence_block_ids: ids,
                    },
                    false,
                ))
            }
        }
        _ => Ok(unknown()),
    }
}

impl SemanticScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Task => "task",
            Self::Unspecified => "unspecified",
        }
    }
}

impl SemanticAssertionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::SourceAsserted => "source_asserted",
            Self::Proposed => "proposed",
            Self::Adopted => "adopted",
            Self::Committed => "committed",
            Self::Observed => "observed",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }
}

impl SemanticScope {
    fn as_state_scope(self) -> &'static str {
        self.as_str()
    }
}

impl SemanticKind {
    fn as_state_kind(self) -> &'static str {
        self.as_str()
    }
}

impl SemanticAssertionStatus {
    fn as_state_status(self) -> &'static str {
        self.as_str()
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn model_block_ids_are_short_revision_bound_and_duplicate_lines_stay_distinct() {
        let vault = VaultId::new();
        let first_revision = SourceRevisionId::new();
        let second_revision = SourceRevisionId::new();
        let revisions = [first_revision, second_revision];
        let first_namespace = block_id_namespace(vault, first_revision, &revisions).unwrap();
        let second_namespace = block_id_namespace(vault, second_revision, &revisions).unwrap();
        assert_ne!(first_namespace, second_namespace);
        assert_ne!(
            first_namespace,
            block_id_namespace(VaultId::new(), first_revision, &revisions).unwrap()
        );

        let blocks = make_blocks("Repeated line.\nRepeated line.\n", &first_namespace).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_ne!(blocks[0].display.local_id, blocks[1].display.local_id);
        assert!(blocks.iter().all(|block| block.display.local_id.len() < 25));
        assert_eq!(base36(36), "10");
        assert!(base36(usize::MAX).len() <= 13);
        assert!(next_block_index(usize::MAX).is_err());
    }

    #[test]
    fn model_block_namespace_extends_hash_collisions_and_has_exact_fallback() {
        let revision_id = SourceRevisionId::new();
        let target = "a".repeat(64);
        let colliding_prefix = format!("{}b{}", "a".repeat(12), "0".repeat(51));
        let extended = select_unique_block_namespace(revision_id, &target, &[colliding_prefix]);
        assert_eq!(extended, format!("h{}", "a".repeat(14)));

        let full_collision =
            select_unique_block_namespace(revision_id, &target, std::slice::from_ref(&target));
        assert_eq!(
            full_collision,
            format!("r{}", revision_id.to_string().replace('-', ""))
        );
    }

    #[test]
    fn a_new_revision_collision_invalidates_the_prepared_namespace() {
        let revision_id = SourceRevisionId::new();
        let target = "a".repeat(64);
        let colliding_prefix = format!("{}b{}", "a".repeat(12), "0".repeat(51));
        let prepared_namespace = select_unique_block_namespace(revision_id, &target, &[]);
        let current_namespace =
            select_unique_block_namespace(revision_id, &target, &[colliding_prefix]);
        assert_ne!(prepared_namespace, current_namespace);
        assert!(
            ensure_block_id_namespace_matches(&prepared_namespace, &current_namespace).is_err()
        );
    }

    fn observation(
        kind: SemanticKind,
        status: SemanticAssertionStatus,
    ) -> SemanticObservationProposal {
        SemanticObservationProposal {
            kind,
            statement: "statement".to_owned(),
            scope: SemanticScope::Project,
            assertion_status: status,
            source_time_scope: Some(SemanticTimeScopeProposal {
                status: SemanticTimeScopeStatus::Unknown,
                value: None,
                evidence_block_ids: Vec::new(),
            }),
            conditions: Vec::new(),
            exceptions: Vec::new(),
            ordered_steps: Vec::new(),
            result: None,
            uncertainty: None,
            admission_reason: "reason".to_owned(),
            value_for_future_work: "value".to_owned(),
            body_block_ids: vec!["block-1".to_owned()],
            context_block_ids: Vec::new(),
        }
    }

    #[test]
    fn groups_partition_by_kind_and_status() {
        let observations = vec![
            observation(SemanticKind::Constraint, SemanticAssertionStatus::Adopted),
            observation(SemanticKind::State, SemanticAssertionStatus::Adopted),
            observation(SemanticKind::Constraint, SemanticAssertionStatus::Adopted),
        ];
        let groups = compatibility_groups(&observations);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].observation_indices, vec![0, 2]);
        assert_eq!(groups[1].observation_indices, vec![1]);
    }

    #[test]
    fn assembly_derives_card_metadata_and_rejects_mixed_refs() {
        let observations = vec![
            observation(SemanticKind::Constraint, SemanticAssertionStatus::Adopted),
            observation(SemanticKind::State, SemanticAssertionStatus::Adopted),
        ];
        let mixed = vec![SemanticCardSelection {
            title: "mixed".to_owned(),
            observation_indices: vec![0, 1],
        }];
        assert_eq!(
            assemble_card_proposals(&observations, &mixed)
                .unwrap_err()
                .code(),
            "semantic_card_selection_crosses_compatibility_group"
        );

        let selections = vec![
            SemanticCardSelection {
                title: "constraint".to_owned(),
                observation_indices: vec![0],
            },
            SemanticCardSelection {
                title: "state".to_owned(),
                observation_indices: vec![1],
            },
        ];
        let cards = assemble_card_proposals(&observations, &selections).unwrap();
        assert_eq!(cards[0].kind, SemanticKind::Constraint);
        assert_eq!(cards[1].kind, SemanticKind::State);
    }
}

#[cfg(test)]
mod a80_flat_observation_tests {
    use super::*;

    fn blocks() -> Vec<SemanticSourceBlock> {
        vec![
            SemanticSourceBlock {
                local_id: "b000001-aaaa".to_owned(),
                text: "same source line".to_owned(),
                line_number: 1,
                kind: "paragraph".to_owned(),
            },
            SemanticSourceBlock {
                local_id: "b000002-bbbb".to_owned(),
                text: "same source line".to_owned(),
                line_number: 2,
                kind: "paragraph".to_owned(),
            },
        ]
    }

    #[test]
    fn flat_claims_reject_unknown_fields_and_require_unique_in_range_integer_evidence() {
        let valid = json!({
            "claims":[{"statement":"Keep the qualification.","evidence_indices":[2],"kind":"decision"}]
        });
        let (claims, counts) =
            parse_a80_observation_response(&valid.to_string(), &blocks(), 0, 2).unwrap();
        assert_eq!(claims[0].body_block_ids, ["b000002-bbbb"]);
        assert_eq!(claims[0].statement, "Keep the qualification.");
        assert_eq!(counts.kind_unknown, 0);

        let wrong_envelope = json!({"evidence_namespace":"untrusted","claims":[]});
        assert!(
            parse_a80_observation_response(&wrong_envelope.to_string(), &blocks(), 0, 2).is_err()
        );
        for indices in [
            json!([1, 1]),
            json!([3]),
            json!([1.0]),
            json!(["1"]),
            json!([true]),
        ] {
            let invalid = json!({"claims":[{
                "statement":"Keep the qualification.","evidence_indices":indices
            }]});
            assert!(parse_a80_observation_response(&invalid.to_string(), &blocks(), 0, 2).is_err());
        }
        let unknown_field = json!({"claims":[{
            "statement":"Keep the qualification.","evidence_indices":[1],"debug":"discarded"
        }]});
        assert!(
            parse_a80_observation_response(&unknown_field.to_string(), &blocks(), 0, 2).is_err()
        );
        let nested_unknown = json!({"claims":[{
            "statement":"Keep the qualification.","evidence_indices":[1],
            "source_time_scope":{"status":"unknown","value":"","evidence_indices":[],
                "synthetic_extra":"unsupported key"}
        }]});
        let error = parse_a80_observation_response(&nested_unknown.to_string(), &blocks(), 0, 2)
            .unwrap_err();
        assert_eq!(error.code(), "semantic_flat_claim_unknown_field");
    }

    #[test]
    fn invalid_advisory_enums_normalize_without_changing_supported_statement() {
        let input = json!({"claims":[{
            "statement":"Do not infer an unrecorded exception.",
            "evidence_indices":[1],
            "kind":"not_a_kind",
            "scope":"global",
            "assertion_status":"committed_by_model",
            "source_time_scope":{"status":"future_state","value":"later","evidence_indices":[]}
        }]});
        let (claims, counts) =
            parse_a80_observation_response(&input.to_string(), &blocks(), 0, 2).unwrap();
        assert_eq!(claims[0].statement, "Do not infer an unrecorded exception.");
        assert_eq!(claims[0].kind, SemanticKind::Unknown);
        assert_eq!(claims[0].scope, SemanticScope::Unspecified);
        assert_eq!(claims[0].assertion_status, SemanticAssertionStatus::Unknown);
        assert_eq!(
            claims[0].source_time_scope.as_ref().unwrap().status,
            SemanticTimeScopeStatus::Unknown
        );
        assert_eq!(counts.kind_unknown, 1);
        assert_eq!(counts.scope_unspecified, 1);
        assert_eq!(counts.status_unknown, 1);
        assert_eq!(counts.time_scope_unknown, 1);
    }
}
