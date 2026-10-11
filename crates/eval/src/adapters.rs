//! Production application-boundary adapters for the isolated M6 runner.
//!
//! This module contains no HTTP, SQL, or filesystem shortcuts.  Provider
//! generation is delegated to `ProviderService`; source reads and semantic
//! publication are delegated to `VaultCore` and `SemanticMemoryService`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    EvidenceRefId, ExtractionSetId, FileId, ModelId, Permission, PermissionSet, SemanticSourceId,
    SourceRevisionId, VaultContext, VaultPath,
};
use mcp_vault_indexer::{IndexService, NoteRetrievalMode, NoteRetrievalScope};
use mcp_vault_memory::{
    MemoryPackRequest, MemoryPackSourceScope, SemanticAccess, SemanticMemoryService,
    SemanticObservationBatch, SemanticObservationPhase, SemanticPreparedGeneration,
    SemanticPublicFacade,
    semantic::{SemanticSourceBlock, organize::SemanticOrganizationService},
};
use mcp_vault_providers::{
    ProviderError, ProviderRuntimeSnapshot, ProviderService, RequestBudget,
    StructuredGenerationRequest, validate_structured_value,
};
use mcp_vault_state::StateStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

use crate::{
    AuthoritativeSourceSnapshot, EvalError, EvalSource, EvalTask, EvaluationManifest,
    EvaluationRunConfig, LiveProviderError, LiveProviderOutput, LiveProviderRequest,
    M6_A80_INDEX_PROFILE_ID, M6_A80_PROMPT_ID, M6_A80_SCHEMA_ID, M6_PROMPT_ID, M6_SCHEMA_ID,
    ProviderAppBoundary, SemanticMemoryAppBoundary, SourceSnapshotVerifier,
};

/// Shared strict limit attached to ProviderTransport. Retries consume the
/// same budget as first attempts because every attempt reserves here.
#[derive(Clone)]
pub struct LiveTransportRequestBudget {
    limit: u32,
    used: Arc<AtomicU32>,
}

impl LiveTransportRequestBudget {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            used: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn new_with_used(limit: u32, used: u32) -> Result<Self, EvalError> {
        if used > limit {
            return Err(EvalError::LiveBlocked(
                "resumed provider request count exceeds sealed budget",
            ));
        }
        Ok(Self {
            limit,
            used: Arc::new(AtomicU32::new(used)),
        })
    }

    pub fn used(&self) -> u32 {
        self.used.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RequestBudget for LiveTransportRequestBudget {
    async fn reserve(&self, _body_bytes: usize) -> Result<(), ProviderError> {
        self.used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(1).filter(|next| *next <= self.limit)
            })
            .map(|_| ())
            .map_err(|_| ProviderError::RequestBudgetExhausted)
    }
}

/// Frozen, non-secret request material for one semantic evaluation stage.
/// Credentials are resolved only by `ProviderService` from its configured
/// encrypted secret store and never appear here.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderStageTemplate {
    pub stage: String,
    pub model_id: String,
    pub prompt_id: String,
    pub schema_id: String,
    pub index_profile_id: String,
    pub system: String,
    pub schema_name: String,
    pub schema: Value,
    pub max_output_tokens: u32,
    pub temperature: Option<f32>,
    pub timeout_seconds: u64,
}

pub fn validate_external_model_identity(
    run_config: &EvaluationRunConfig,
    templates: &[ProviderStageTemplate],
    actual_external_model_id: &str,
) -> Result<(), EvalError> {
    if actual_external_model_id.trim().is_empty()
        || run_config
            .comparisons
            .iter()
            .any(|comparison| comparison.answer_model_id != actual_external_model_id)
        || templates
            .iter()
            .any(|template| template.model_id != actual_external_model_id)
    {
        return Err(EvalError::LiveBlocked(
            "configured external model does not match the registered Provider model",
        ));
    }
    Ok(())
}

/// Adapter from the evaluator's fixed request contract to the existing
/// Provider application service.  No retry policy or transport is implemented
/// here; those remain inside the Provider service boundary.
#[derive(Clone)]
pub struct ProviderServiceAppBoundary {
    service: ProviderService,
    context: VaultContext,
    model_id: ModelId,
    templates: BTreeMap<String, ProviderStageTemplate>,
    runtime_snapshot: ProviderRuntimeSnapshot,
    request_budget: LiveTransportRequestBudget,
    prepared_extraction_cleanup: Option<SemanticMemoryServiceAppBoundary>,
}

impl ProviderServiceAppBoundary {
    pub fn new(
        service: ProviderService,
        context: VaultContext,
        model_id: ModelId,
        registered_external_model_id: &str,
        templates: impl IntoIterator<Item = ProviderStageTemplate>,
        runtime_snapshot: ProviderRuntimeSnapshot,
        request_budget: LiveTransportRequestBudget,
    ) -> Result<Self, EvalError> {
        let templates = templates
            .into_iter()
            .map(|template| (template.stage.clone(), template))
            .collect::<BTreeMap<_, _>>();
        if templates.is_empty() {
            return Err(EvalError::LiveConfig(
                "provider stage templates are required",
            ));
        }
        if registered_external_model_id.trim().is_empty()
            || templates
                .values()
                .any(|template| template.model_id != registered_external_model_id)
        {
            return Err(EvalError::LiveBlocked(
                "provider templates do not match the registered external model",
            ));
        }
        Ok(Self {
            service,
            context,
            model_id,
            templates,
            runtime_snapshot,
            request_budget,
            prepared_extraction_cleanup: None,
        })
    }

    /// Attach the arm-scoped semantic adapter so a failed observation or
    /// composition Provider call terminalizes its exact prepared extraction.
    pub fn with_prepared_extraction_cleanup(
        mut self,
        semantic: SemanticMemoryServiceAppBoundary,
    ) -> Self {
        self.prepared_extraction_cleanup = Some(semantic);
        self
    }

    async fn generate_inner(
        &self,
        request: LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        let template = self
            .templates
            .get(&request.stage)
            .ok_or_else(|| LiveProviderError {
                code: "provider_stage_not_configured".into(),
                ..Default::default()
            })?;
        if template.model_id != request.model_id
            || template.prompt_id != request.prompt_id
            || template.schema_id != request.schema_id
            || template.index_profile_id != request.index_profile_id
        {
            return Err(LiveProviderError {
                code: "provider_frozen_config_mismatch".into(),
                ..Default::default()
            });
        }
        let user = serde_json::to_string(&request.input).map_err(|_| LiveProviderError {
            code: "provider_input_serialization".into(),
            ..Default::default()
        })?;
        let schema = if request.stage == "observation" {
            resolve_observation_schema(&template.schema, &request.input).map_err(|code| {
                LiveProviderError {
                    code,
                    ..Default::default()
                }
            })?
        } else {
            template.schema.clone()
        };
        let m6_strict_function_stage = request.prompt_id == M6_PROMPT_ID
            && request.schema_id == M6_SCHEMA_ID
            && matches!(
                request.stage.as_str(),
                "observation" | "composition" | "relation" | "answer"
            );
        let a80_non_stream_json_object = self.runtime_snapshot.provider_type == "xiaomi_mimo"
            && ((request.prompt_id == M6_A80_PROMPT_ID && request.schema_id == M6_A80_SCHEMA_ID)
                || (request.prompt_id == "semantic-cards-tracked-adr-m6-v14"
                    && request.schema_id == "semantic-cards-m6-json-v10"))
            && matches!(
                request.stage.as_str(),
                "observation" | "relation" | "answer"
            );
        let strict_function_call =
            self.runtime_snapshot.provider_type == "xiaomi_mimo" && m6_strict_function_stage;
        let strict_function_schema = if strict_function_call && request.stage == "relation" {
            Some(
                strict_relation_wire_schema(&schema).map_err(|code| LiveProviderError {
                    code: code.to_owned(),
                    ..Default::default()
                })?,
            )
        } else {
            None
        };
        let system = if strict_function_call && request.stage == "relation" {
            format!(
                "{}\nIn each relation action, include card_ref, title, content, item_kind, support_operator, and reason. Use the empty string only when that field is not applicable; otherwise provide its actual value. Empty item_kind or support_operator means not applicable, not a category value.",
                template.system
            )
        } else {
            template.system.clone()
        };
        let generated = self
            .service
            .generate_structured_pinned(
                &self.context,
                self.model_id,
                &self.runtime_snapshot,
                &StructuredGenerationRequest {
                    model: template.model_id.clone(),
                    system,
                    user,
                    schema_name: template.schema_name.clone(),
                    schema,
                    strict_function_schema,
                    defer_local_schema_validation: strict_function_call
                        && request.stage == "relation",
                    strict_function_call,
                    non_stream_json_object: a80_non_stream_json_object,
                    allow_additional_output_properties: false,
                    missing_required_string_fallbacks: Vec::new(),
                    max_output_tokens: template.max_output_tokens,
                    temperature: template.temperature,
                    timeout: Some(Duration::from_secs(template.timeout_seconds)),
                },
            )
            .await
            .map_err(|error| {
                let (schema_issue, schema_path) = error
                    .schema_diagnostic()
                    .map(|(issue, path)| (Some(issue.to_owned()), Some(path.to_owned())))
                    .unwrap_or((None, None));
                LiveProviderError {
                    code: error.code().to_owned(),
                    schema_issue,
                    schema_path,
                    structured_json_diagnostic: error.structured_json_diagnostic().cloned(),
                    protocol_issue: error.strict_function_call_issue(),
                }
            })?;
        let mut value = generated.value;
        if strict_function_call && request.stage == "relation" {
            normalize_relation_empty_optional_fields(&mut value).map_err(|code| {
                LiveProviderError {
                    code: code.to_owned(),
                    ..Default::default()
                }
            })?;
            validate_structured_value(&value, &template.schema).map_err(|error| {
                let (schema_issue, schema_path) = error
                    .schema_diagnostic()
                    .map(|(issue, path)| (Some(issue.to_owned()), Some(path.to_owned())))
                    .unwrap_or((None, None));
                LiveProviderError {
                    code: error.code().to_owned(),
                    schema_issue,
                    schema_path,
                    ..Default::default()
                }
            })?;
        }
        Ok(LiveProviderOutput {
            output: value,
            usage: generated.usage,
            cost_minor: None,
        })
    }
}

#[async_trait]
impl ProviderAppBoundary for ProviderServiceAppBoundary {
    async fn generate(
        &self,
        request: LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        let cleanup_key = matches!(request.stage.as_str(), "observation" | "composition")
            .then(|| {
                request
                    .source_id
                    .as_ref()
                    .map(|source_id| (request.arm.clone(), source_id.clone()))
            })
            .flatten();
        let result = self.generate_inner(request).await;
        if result.is_err()
            && let (Some(semantic), Some((arm, source_id))) =
                (&self.prepared_extraction_cleanup, cleanup_key)
            && semantic
                .abort_pending_generation_for_source(arm, &source_id)
                .await
                .is_err()
        {
            return Err(LiveProviderError {
                code: "semantic_prepared_extraction_terminalization_failed".into(),
                ..Default::default()
            });
        }
        result
    }

    async fn verify_frozen_configuration(
        &self,
        expected: &ProviderRuntimeSnapshot,
    ) -> Result<(), LiveProviderError> {
        let current = self
            .service
            .runtime_snapshot(&self.context, self.model_id)
            .await
            .map_err(|error| LiveProviderError {
                code: error.code().to_owned(),
                ..Default::default()
            })?;
        if current.fingerprint != expected.fingerprint
            || current.fingerprint != self.runtime_snapshot.fingerprint
        {
            return Err(LiveProviderError {
                code: "provider_runtime_configuration_drift".into(),
                ..Default::default()
            });
        }
        Ok(())
    }

    fn transport_attempt_count(&self) -> Option<u32> {
        Some(self.request_budget.used())
    }
}

/// Application boundary for source preparation and semantic publication.
type RelationCandidateKey = (crate::ComparisonArm, String);
type RelationCandidateAliases = BTreeMap<RelationCandidateKey, BTreeMap<String, String>>;
type PreparedA80Batches = BTreeMap<RelationCandidateKey, Vec<SemanticObservationBatch>>;

#[derive(Clone)]
pub struct SemanticMemoryServiceAppBoundary {
    state: StateStore,
    context: VaultContext,
    arms: BTreeMap<crate::ComparisonArm, ArmSemanticRuntime>,
    source_bindings: Arc<Mutex<BTreeMap<(crate::ComparisonArm, String), ArmSourceBinding>>>,
    prepared_generations:
        Arc<Mutex<BTreeMap<(crate::ComparisonArm, String), SemanticPreparedGeneration>>>,
    prepared_a80_batches: Arc<Mutex<PreparedA80Batches>>,
    a80_normalization_counts: Arc<Mutex<BTreeMap<(crate::ComparisonArm, String), Value>>>,
    active_extractions: Arc<Mutex<BTreeMap<(crate::ComparisonArm, String), ExtractionSetId>>>,
    observation_phases:
        Arc<Mutex<BTreeMap<(crate::ComparisonArm, String), SemanticObservationPhase>>>,
    observation_context_overlap_removed: Arc<Mutex<BTreeMap<(crate::ComparisonArm, String), u64>>>,
    completed_extractions: Arc<Mutex<BTreeSet<(crate::ComparisonArm, String)>>>,
    relation_candidate_aliases: Arc<Mutex<RelationCandidateAliases>>,
    access: SemanticAccess,
}

#[derive(Clone)]
pub struct ArmSemanticRuntime {
    pub state: StateStore,
    pub context: VaultContext,
    pub core: VaultCore,
    pub source_root: PathBuf,
    pub state_db_path: PathBuf,
    pub history_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArmSourceBinding {
    source_id: SemanticSourceId,
    source_revision_id: SourceRevisionId,
    path: String,
    file_revision: u64,
}

impl SemanticMemoryServiceAppBoundary {
    pub fn new_isolated(
        state: StateStore,
        context: VaultContext,
        baseline_state_db_path: PathBuf,
        baseline_history_root: PathBuf,
        b: ArmSemanticRuntime,
        c: ArmSemanticRuntime,
    ) -> Result<Self, EvalError> {
        let baseline_source_root = context.content_root().to_path_buf();
        let isolation_paths = [
            baseline_source_root.as_path(),
            baseline_state_db_path.as_path(),
            baseline_history_root.as_path(),
            b.source_root.as_path(),
            b.state_db_path.as_path(),
            b.history_root.as_path(),
            c.source_root.as_path(),
            c.state_db_path.as_path(),
            c.history_root.as_path(),
        ];
        let paths_overlap = isolation_paths.iter().enumerate().any(|(index, path)| {
            isolation_paths
                .iter()
                .skip(index + 1)
                .any(|other| roots_overlap(path, other))
        });
        if b.context.id() == c.context.id()
            || b.context.content_root() == c.context.content_root()
            || b.state_db_path == c.state_db_path
            || b.history_root == c.history_root
            || b.context.id() == context.id()
            || c.context.id() == context.id()
            || b.context.content_root() == context.content_root()
            || c.context.content_root() == context.content_root()
            || b.state_db_path == c.state_db_path
            || paths_overlap
        {
            return Err(EvalError::LiveConfig(
                "B/C semantic contexts, roots, and state DBs must be independent",
            ));
        }
        for runtime in [&b, &c] {
            if runtime.source_root != runtime.context.content_root()
                || runtime.state_db_path.as_os_str().is_empty()
                || runtime.history_root.as_os_str().is_empty()
            {
                return Err(EvalError::LiveConfig(
                    "semantic arm runtime does not match its isolated Vault",
                ));
            }
        }
        let arms = BTreeMap::from([(crate::ComparisonArm::B, b), (crate::ComparisonArm::C, c)]);
        Ok(Self {
            state: state.clone(),
            context,
            arms,
            source_bindings: Arc::new(Mutex::new(BTreeMap::new())),
            prepared_generations: Arc::new(Mutex::new(BTreeMap::new())),
            prepared_a80_batches: Arc::new(Mutex::new(BTreeMap::new())),
            a80_normalization_counts: Arc::new(Mutex::new(BTreeMap::new())),
            active_extractions: Arc::new(Mutex::new(BTreeMap::new())),
            observation_phases: Arc::new(Mutex::new(BTreeMap::new())),
            observation_context_overlap_removed: Arc::new(Mutex::new(BTreeMap::new())),
            completed_extractions: Arc::new(Mutex::new(BTreeSet::new())),
            relation_candidate_aliases: Arc::new(Mutex::new(BTreeMap::new())),
            access: SemanticAccess::new(PermissionSet::from_iter([
                Permission::ReadMemory,
                Permission::ReadVault,
            ])),
        })
    }

    fn arm(&self, arm: crate::ComparisonArm) -> Result<&ArmSemanticRuntime, String> {
        self.arms
            .get(&arm)
            .ok_or_else(|| "semantic_arm_isolation_unavailable".to_owned())
    }

    async fn terminalize_extraction(
        runtime: &ArmSemanticRuntime,
        extraction_id: ExtractionSetId,
    ) -> Result<(), String> {
        let extraction = runtime
            .state
            .semantic_memory()
            .get_extraction(&runtime.context, extraction_id)
            .await
            .map_err(|_| "semantic_extraction_state_unavailable".to_owned())?
            .ok_or_else(|| "semantic_extraction_missing".to_owned())?;
        match extraction.state.as_str() {
            "running" => SemanticMemoryService::new(runtime.state.clone())
                .cancel_extraction(&runtime.context, extraction_id)
                .await
                .map_err(|error| error.code().to_owned()),
            "failed" | "cancelled" | "success_empty" | "success_nonempty" => Ok(()),
            "prepared" | "partial" => Err("semantic_extraction_publication_in_progress".to_owned()),
            _ => Err("semantic_extraction_state_invalid".to_owned()),
        }
    }

    /// Terminalize the extraction associated with one already-validated
    /// `(arm, logical source)` key. The adapter never accepts a Vault ID from
    /// the Provider request; the arm runtime supplies the Vault fence.
    async fn abort_pending_generation_for_source(
        &self,
        arm: crate::ComparisonArm,
        source_id: &str,
    ) -> Result<(), String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm, source_id.to_owned());
        let extraction_id = self.active_extractions.lock().await.get(&key).copied();
        if let Some(extraction_id) = extraction_id {
            Self::terminalize_extraction(runtime, extraction_id).await?;
            self.active_extractions.lock().await.remove(&key);
        }
        self.prepared_generations.lock().await.remove(&key);
        self.prepared_a80_batches.lock().await.remove(&key);
        self.a80_normalization_counts.lock().await.remove(&key);
        self.observation_phases.lock().await.remove(&key);
        Ok(())
    }

    async fn resolve_source_binding(
        runtime: &ArmSemanticRuntime,
        source: &EvalSource,
    ) -> Result<ArmSourceBinding, String> {
        let file_id = Self::verify_arm_file(runtime, source).await?;
        let semantic_source = runtime
            .state
            .semantic_memory()
            .get_source_by_file(&runtime.context, file_id)
            .await
            .map_err(|_| "semantic_source_unavailable".to_owned())?
            .ok_or_else(|| "semantic_source_unavailable".to_owned())?;
        let source_revision_id = semantic_source
            .current_revision_id
            .ok_or_else(|| "semantic_source_revision_unavailable".to_owned())?;
        let revision = runtime
            .state
            .semantic_memory()
            .get_source_revision(&runtime.context, source_revision_id)
            .await
            .map_err(|_| "semantic_source_revision_unavailable".to_owned())?
            .ok_or_else(|| "semantic_source_revision_unavailable".to_owned())?;
        if semantic_source.source_path.as_str() != source.path
            || semantic_source.file_id != file_id
            || revision.file_revision.value() != source.file_revision
            || revision.content_hash.trim_start_matches("sha256:")
                != source.content_hash.trim_start_matches("sha256:")
        {
            return Err("semantic_source_fence_mismatch".to_owned());
        }
        Ok(ArmSourceBinding {
            source_id: semantic_source.source_id,
            source_revision_id,
            path: source.path.clone(),
            file_revision: source.file_revision,
        })
    }

    async fn verify_arm_file(
        runtime: &ArmSemanticRuntime,
        source: &EvalSource,
    ) -> Result<FileId, String> {
        let path = VaultPath::parse(&source.path).map_err(|_| "source_path_invalid".to_owned())?;
        let file = runtime
            .state
            .files()
            .get_active(&runtime.context, &path)
            .await
            .map_err(|_| "semantic_source_unavailable".to_owned())?
            .ok_or_else(|| "semantic_source_unavailable".to_owned())?;
        if file.path.as_str() != source.path
            || file.current_revision.value() != source.file_revision
            || file.content_hash.as_deref().is_none_or(|hash| {
                hash.trim_start_matches("sha256:")
                    != source.content_hash.trim_start_matches("sha256:")
            })
        {
            return Err("semantic_source_fence_mismatch".to_owned());
        }
        let mut read = runtime
            .core
            .read(&runtime.context, &path)
            .await
            .map_err(|_| "semantic_source_unavailable".to_owned())?;
        if read.file.id != file.id || read.file.current_revision.value() != source.file_revision {
            return Err("semantic_source_fence_mismatch".to_owned());
        }
        let mut bytes = Vec::new();
        read.reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "semantic_source_unavailable".to_owned())?;
        if format!("{:x}", Sha256::digest(&bytes)) != source.content_hash {
            return Err("semantic_source_fence_mismatch".to_owned());
        }
        Ok(file.id)
    }
}

fn indexed_observation_model_input(prepared: &SemanticPreparedGeneration) -> Result<Value, String> {
    indexed_observation_model_input_for_catalog(
        prepared.evidence_namespace(),
        &prepared.model_input.blocks,
    )
}

fn indexed_observation_model_input_for_catalog(
    namespace: &str,
    blocks: &[SemanticSourceBlock],
) -> Result<Value, String> {
    if namespace.is_empty() || blocks.is_empty() {
        return Err("semantic_observation_catalog_invalid".to_owned());
    }
    let blocks = blocks
        .iter()
        .enumerate()
        .map(|(position, block)| {
            let evidence_index = u64::try_from(position)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or_else(|| "semantic_observation_catalog_invalid".to_owned())?;
            Ok::<_, String>(json!({
                "evidence_index": evidence_index,
                "text": block.text,
                "line_number": block.line_number,
                "kind": block.kind,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"evidence_namespace": namespace, "blocks": blocks}))
}

fn restore_observation_evidence_ids(
    proposal: &Value,
    prepared: &SemanticPreparedGeneration,
) -> Result<Value, String> {
    restore_observation_evidence_ids_for_catalog(
        proposal,
        prepared.evidence_namespace(),
        &prepared.model_input.blocks,
    )
}

fn restore_observation_evidence_ids_for_catalog(
    proposal: &Value,
    expected_namespace: &str,
    blocks: &[SemanticSourceBlock],
) -> Result<Value, String> {
    let namespace = proposal
        .get("evidence_namespace")
        .and_then(Value::as_str)
        .ok_or_else(|| "semantic_evidence_namespace_missing".to_owned())?;
    if namespace != expected_namespace {
        return Err("semantic_evidence_namespace_mismatch".to_owned());
    }
    let mut restored = proposal.clone();
    let object = restored
        .as_object_mut()
        .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
    let top_level = ["evidence_namespace", "outcome", "observations"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if object.keys().any(|key| !top_level.contains(key.as_str())) {
        return Err("semantic_evidence_contract_invalid".to_owned());
    }
    object.remove("evidence_namespace");
    let outcome = object
        .get("outcome")
        .and_then(Value::as_str)
        .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?
        .to_owned();
    let observations = object
        .get_mut("observations")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
    if (observations.is_empty() && outcome != "success_empty")
        || (!observations.is_empty() && outcome != "success_nonempty")
    {
        return Err("semantic_evidence_contract_invalid".to_owned());
    }
    for observation in observations {
        let observation = observation
            .as_object_mut()
            .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
        const ALLOWED_OBSERVATION_FIELDS: &[&str] = &[
            "kind",
            "statement",
            "scope",
            "assertion_status",
            "source_time_scope",
            "conditions",
            "exceptions",
            "ordered_steps",
            "result",
            "uncertainty",
            "admission_reason",
            "value_for_future_work",
            "body_block_indices",
            "context_block_indices",
        ];
        if observation
            .keys()
            .any(|key| !ALLOWED_OBSERVATION_FIELDS.contains(&key.as_str()))
            || ALLOWED_OBSERVATION_FIELDS
                .iter()
                .any(|field| !observation.contains_key(*field))
        {
            return Err("semantic_evidence_contract_invalid".to_owned());
        }
        remap_evidence_index_field(
            observation,
            "body_block_indices",
            "body_block_ids",
            blocks,
            true,
        )?;
        remap_evidence_index_field(
            observation,
            "context_block_indices",
            "context_block_ids",
            blocks,
            false,
        )?;
        for field in ["result", "uncertainty"] {
            if observation.get(field).and_then(Value::as_str) == Some("") {
                observation.insert(field.to_owned(), Value::Null);
            }
        }
        if let Some(time_scope) = observation.get_mut("source_time_scope") {
            let time_scope = time_scope
                .as_object_mut()
                .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
            if time_scope
                .keys()
                .any(|key| !["status", "value", "evidence_block_indices"].contains(&key.as_str()))
                || ["status", "value", "evidence_block_indices"]
                    .iter()
                    .any(|field| !time_scope.contains_key(*field))
            {
                return Err("semantic_evidence_contract_invalid".to_owned());
            }
            let time_status = time_scope
                .get("status")
                .and_then(Value::as_str)
                .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?
                .to_owned();
            let time_value = time_scope
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?
                .to_owned();
            let evidence_is_empty = time_scope
                .get("evidence_block_indices")
                .and_then(Value::as_array)
                .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?
                .is_empty();
            if time_status == "unknown" {
                if !time_value.is_empty() || !evidence_is_empty {
                    return Err("semantic_evidence_contract_invalid".to_owned());
                }
            } else if time_status == "source_stated" {
                if time_value.trim().is_empty() || evidence_is_empty {
                    return Err("semantic_evidence_contract_invalid".to_owned());
                }
            } else {
                return Err("semantic_evidence_contract_invalid".to_owned());
            }
            remap_evidence_index_field(
                time_scope,
                "evidence_block_indices",
                "evidence_block_ids",
                blocks,
                false,
            )?;
            if time_status == "unknown" {
                time_scope.insert("value".to_owned(), Value::Null);
            }
        } else {
            return Err("semantic_evidence_contract_invalid".to_owned());
        }
    }
    Ok(restored)
}

fn remap_evidence_index_field(
    object: &mut serde_json::Map<String, Value>,
    index_field: &str,
    id_field: &str,
    blocks: &[SemanticSourceBlock],
    required: bool,
) -> Result<(), String> {
    let Some(index_values) = object.remove(index_field) else {
        return if required {
            Err("semantic_evidence_indices_missing".to_owned())
        } else {
            Ok(())
        };
    };
    if object.contains_key(id_field) {
        return Err("semantic_evidence_contract_invalid".to_owned());
    }
    let index_values = index_values
        .as_array()
        .ok_or_else(|| "semantic_evidence_index_invalid".to_owned())?;
    if required && index_values.is_empty() {
        return Err("semantic_evidence_indices_missing".to_owned());
    }
    let mut seen = BTreeSet::new();
    let mut ids = Vec::with_capacity(index_values.len());
    for value in index_values {
        let index = value
            .as_u64()
            .filter(|index| *index > 0)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| "semantic_evidence_index_invalid".to_owned())?;
        if index > blocks.len() || !seen.insert(index) {
            return Err("semantic_evidence_index_invalid".to_owned());
        }
        ids.push(json!(blocks[index - 1].local_id));
    }
    object.insert(id_field.to_owned(), Value::Array(ids));
    Ok(())
}

const M6_RELATION_OPTIONAL_STRING_FIELDS: &[&str] = &[
    "card_ref",
    "title",
    "content",
    "item_kind",
    "support_operator",
    "reason",
];

fn strict_relation_wire_schema(local_schema: &Value) -> Result<Value, &'static str> {
    let mut wire_schema = local_schema.clone();
    let item = wire_schema
        .pointer_mut("/properties/actions/items")
        .and_then(Value::as_object_mut)
        .ok_or("semantic_relation_wire_schema_invalid")?;
    let properties = item
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .ok_or("semantic_relation_wire_schema_invalid")?;
    if M6_RELATION_OPTIONAL_STRING_FIELDS.iter().any(|field| {
        properties
            .get(*field)
            .and_then(|property| property.get("type"))
            .and_then(Value::as_str)
            != Some("string")
    }) {
        return Err("semantic_relation_wire_schema_invalid");
    }
    for field in ["item_kind", "support_operator"] {
        properties
            .get_mut(field)
            .and_then(Value::as_object_mut)
            .ok_or("semantic_relation_wire_schema_invalid")?
            .remove("enum");
    }
    let mut required = item
        .get("required")
        .and_then(Value::as_array)
        .ok_or("semantic_relation_wire_schema_invalid")?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    required.extend(
        M6_RELATION_OPTIONAL_STRING_FIELDS
            .iter()
            .map(|field| (*field).to_owned()),
    );
    item.insert(
        "required".to_owned(),
        json!(required.into_iter().collect::<Vec<_>>()),
    );
    Ok(wire_schema)
}

fn normalize_relation_empty_optional_fields(proposal: &mut Value) -> Result<u64, &'static str> {
    let actions = proposal
        .get_mut("actions")
        .and_then(Value::as_array_mut)
        .ok_or("semantic_relation_wire_result_invalid")?;
    let mut removed = 0_u64;
    for action in actions {
        let action = action
            .as_object_mut()
            .ok_or("semantic_relation_wire_result_invalid")?;
        for field in M6_RELATION_OPTIONAL_STRING_FIELDS {
            match action.get(*field) {
                Some(Value::String(value)) if value.is_empty() => {
                    action.remove(*field);
                    removed = removed
                        .checked_add(1)
                        .ok_or("semantic_relation_wire_empty_count_overflow")?;
                }
                Some(Value::String(_)) => {}
                _ => return Err("semantic_relation_wire_result_invalid"),
            }
        }
    }
    Ok(removed)
}

/// V7 observation wire normalization: one exact block may support the body,
/// while context retains only additional blocks. This changes no evidence
/// bytes and does not apply to legacy complete-proposal submissions.
fn normalize_v7_body_context_overlap(proposal: &mut Value) -> Result<u64, String> {
    let observations = proposal
        .get_mut("observations")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
    let mut removed = 0_u64;
    for observation in observations {
        let observation = observation
            .as_object_mut()
            .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
        let body_ids = observation
            .get("body_block_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let context_ids = observation
            .get_mut("context_block_ids")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
        let current = std::mem::take(context_ids);
        let mut retained = Vec::with_capacity(current.len());
        for context_id in current {
            let id = context_id
                .as_str()
                .ok_or_else(|| "semantic_evidence_contract_invalid".to_owned())?;
            if body_ids.contains(id) {
                removed = removed
                    .checked_add(1)
                    .ok_or_else(|| "semantic_evidence_overlap_count_overflow".to_owned())?;
            } else {
                retained.push(context_id);
            }
        }
        *context_ids = retained;
    }
    Ok(removed)
}

fn resolve_observation_schema(schema: &Value, input: &Value) -> Result<Value, String> {
    let properties = schema.get("properties").and_then(Value::as_object);
    if properties.is_some_and(|properties| properties.contains_key("claims")) {
        return resolve_a80_observation_schema(schema, input);
    }
    if properties.is_some_and(|properties| !properties.contains_key("observations")) {
        // Legacy/generic observation schemas do not expose semantic evidence
        // fields; preserve their existing contract.
        return Ok(schema.clone());
    }
    let blocks = input
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or_else(|| "semantic_observation_catalog_missing".to_owned())?;
    if blocks.is_empty() {
        return Err("semantic_observation_catalog_invalid".to_owned());
    }
    for (position, block) in blocks.iter().enumerate() {
        let expected = u64::try_from(position)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| "semantic_observation_catalog_invalid".to_owned())?;
        if block.get("evidence_index").and_then(Value::as_u64) != Some(expected) {
            return Err("semantic_observation_catalog_invalid".to_owned());
        }
    }
    let namespace = input
        .get("evidence_namespace")
        .and_then(Value::as_str)
        .filter(|namespace| !namespace.is_empty())
        .ok_or_else(|| "semantic_observation_namespace_missing".to_owned())?;
    let mut resolved = schema.clone();
    resolved["properties"]["evidence_namespace"]["enum"] = json!([namespace]);
    let Some(properties) = resolved.pointer_mut("/properties/observations/items/properties") else {
        return Err("semantic_observation_schema_invalid".to_owned());
    };
    let indexes = (1..=blocks.len())
        .map(u64::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "semantic_observation_catalog_invalid".to_owned())?;
    let enum_value = serde_json::to_value(&indexes)
        .map_err(|_| "semantic_observation_catalog_invalid".to_owned())?;
    for key in ["body_block_indices", "context_block_indices"] {
        properties[key]["items"]["enum"] = enum_value.clone();
    }
    properties["source_time_scope"]["properties"]["evidence_block_indices"]["items"]["enum"] =
        enum_value;
    Ok(resolved)
}

fn resolve_a80_observation_schema(schema: &Value, input: &Value) -> Result<Value, String> {
    let blocks = input
        .get("blocks")
        .and_then(Value::as_array)
        .filter(|blocks| !blocks.is_empty() && blocks.len() <= 80)
        .ok_or_else(|| "semantic_observation_catalog_invalid".to_owned())?;
    let mut indexes = BTreeSet::new();
    for block in blocks {
        let index = block
            .get("evidence_index")
            .and_then(Value::as_u64)
            .filter(|index| *index > 0)
            .ok_or_else(|| "semantic_observation_catalog_invalid".to_owned())?;
        if !indexes.insert(index) {
            return Err("semantic_observation_catalog_invalid".to_owned());
        }
    }
    let indexes = indexes.into_iter().collect::<Vec<_>>();
    let mut resolved = schema.clone();
    resolved["properties"]["claims"]["items"]["properties"]["evidence_indices"]["items"]["enum"] =
        json!(indexes.clone());
    // Historical v10 schemas deliberately remain unchanged. New schemas also
    // bind time evidence to the current batch, using the same global indices.
    if let Some(items) = resolved.pointer_mut(
        "/properties/claims/items/properties/source_time_scope/properties/evidence_indices/items",
    ) {
        items["enum"] = json!(indexes.clone());
    }
    let batch_index = input
        .get("batch_index")
        .and_then(Value::as_u64)
        .ok_or_else(|| "semantic_observation_batch_fence_invalid".to_owned())?;
    let batch_count = input
        .get("batch_count")
        .and_then(Value::as_u64)
        .filter(|count| *count > 0)
        .ok_or_else(|| "semantic_observation_batch_fence_invalid".to_owned())?;
    let first_index = batch_index
        .checked_mul(80)
        .and_then(|start| start.checked_add(1))
        .ok_or_else(|| "semantic_observation_batch_fence_invalid".to_owned())?;
    let exclusive_end = batch_index
        .checked_add(1)
        .and_then(|index| index.checked_mul(80))
        .ok_or_else(|| "semantic_observation_batch_fence_invalid".to_owned())?;
    if batch_index >= batch_count
        || indexes.first().copied() != Some(first_index)
        || indexes.last().is_none_or(|last| *last > exclusive_end)
        || indexes.iter().enumerate().any(|(offset, index)| {
            u64::try_from(offset)
                .ok()
                .and_then(|offset| first_index.checked_add(offset))
                != Some(*index)
        })
    {
        return Err("semantic_observation_batch_fence_invalid".to_owned());
    }
    Ok(resolved)
}

fn roots_overlap(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

async fn add_m6_evidence_excerpts(
    runtime: &ArmSemanticRuntime,
    pack: &mut Value,
    maximum_bytes: u32,
) -> Result<(), String> {
    let semantic = SemanticMemoryService::new(runtime.state.clone());
    let mut total_excerpt_bytes = 0usize;
    for section in ["current_context", "relevant_experiences"] {
        let Some(entries) = pack.get_mut(section).and_then(Value::as_array_mut) else {
            return Err("memory_pack_entry_shape_invalid".to_owned());
        };
        for entry in entries {
            // Several assertions can share one EvidenceRef. Resolve its full
            // evidence once without changing the entry's support references.
            let evidence_ids = entry
                .get("evidence_refs")
                .and_then(Value::as_array)
                .ok_or_else(|| "memory_pack_evidence_refs_missing".to_owned())?
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<BTreeSet<_>>>()
                .ok_or_else(|| "memory_pack_evidence_refs_invalid".to_owned())?;
            let source_references = entry
                .get("source_references")
                .and_then(Value::as_array)
                .ok_or_else(|| "memory_pack_source_reference_missing".to_owned())?;
            let allowed_sources = source_references
                .iter()
                .map(|reference| {
                    let source_id = reference
                        .get("source_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "memory_pack_source_reference_invalid".to_owned())?;
                    let revision_id = reference
                        .get("source_revision_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "memory_pack_source_reference_invalid".to_owned())?;
                    Ok((source_id.to_owned(), revision_id.to_owned()))
                })
                .collect::<Result<BTreeSet<_>, String>>()?;
            let mut excerpts = Vec::new();
            for evidence_id in evidence_ids {
                let evidence_ref_id = EvidenceRefId::parse(&evidence_id)
                    .map_err(|_| "memory_pack_evidence_ref_invalid".to_owned())?;
                let evidence = semantic
                    .read_evidence(&runtime.context, &runtime.core, evidence_ref_id)
                    .await
                    .map_err(|error| error.code().to_owned())?
                    .ok_or_else(|| "memory_pack_evidence_unavailable".to_owned())?;
                let evidence_source = (
                    evidence.source_id.to_string(),
                    evidence.source_revision_id.to_string(),
                );
                if !allowed_sources.contains(&evidence_source) {
                    return Err("memory_pack_evidence_source_mismatch".to_owned());
                }
                let mut spans = Vec::new();
                for (role, records) in [
                    ("body", evidence.body_span_records),
                    ("context", evidence.context_span_records),
                ] {
                    for span in records {
                        total_excerpt_bytes = total_excerpt_bytes
                            .checked_add(span.text.len())
                            .ok_or_else(|| "memory_pack_evidence_budget_exhausted".to_owned())?;
                        spans.push(json!({
                            "role": role,
                            "start_byte": span.start_byte,
                            "end_byte": span.end_byte,
                            "content_hash": span.content_hash,
                            "text": span.text,
                        }));
                    }
                }
                if spans.is_empty() {
                    return Err("memory_pack_evidence_unavailable".to_owned());
                }
                excerpts.push(json!({
                    "evidence_ref_id": evidence_id,
                    "source_id": evidence_source.0,
                    "source_revision_id": evidence_source.1,
                    "source_path": evidence.source_path,
                    "spans": spans,
                }));
            }
            let entry_object = entry
                .as_object_mut()
                .ok_or_else(|| "memory_pack_entry_shape_invalid".to_owned())?;
            entry_object.insert("evidence_excerpts".to_owned(), Value::Array(excerpts));
        }
    }
    if serde_json::to_vec(pack)
        .map_err(|_| "memory_pack_serialization".to_owned())?
        .len()
        > maximum_bytes as usize
    {
        return Err("memory_pack_evidence_budget_exhausted".to_owned());
    }
    if let Some(estimated) = pack.get("estimated_tokens").and_then(Value::as_u64) {
        let added = u64::try_from(total_excerpt_bytes.div_ceil(4)).unwrap_or(u64::MAX);
        pack["estimated_tokens"] = json!(estimated.saturating_add(added));
    }
    Ok(())
}

#[async_trait]
impl SemanticMemoryAppBoundary for SemanticMemoryServiceAppBoundary {
    async fn ordinary_retrieve(
        &self,
        task: &EvalTask,
        sources: &[EvalSource],
        budget: &crate::Budget,
    ) -> Result<Value, String> {
        self.ordinary_retrieve_with_profile(task, sources, budget, "index-frozen-v1")
            .await
    }

    async fn ordinary_retrieve_with_profile(
        &self,
        task: &EvalTask,
        sources: &[EvalSource],
        budget: &crate::Budget,
        index_profile_id: &str,
    ) -> Result<Value, String> {
        if !["index-frozen-v1", M6_A80_INDEX_PROFILE_ID].contains(&index_profile_id) {
            return Err("ordinary_index_profile_unavailable".to_owned());
        }
        let index = IndexService::new(self.state.clone());
        let mut selected = Vec::new();
        let mut expected_source_ids = std::collections::BTreeSet::new();
        let mut current_indexed_source_ids = std::collections::BTreeSet::new();
        let mut degradation_reasons = std::collections::BTreeSet::new();
        let mut candidate_count = 0_u32;
        let mut eligible_count = 0_u32;
        let mut available_result_count = 0_u32;
        let mut stale_hit_count = 0_u32;
        let mut total_bytes = 0_u64;
        let mut total_tokens = 0_u32;
        for source in sources {
            VaultPath::parse(&source.path).map_err(|_| "source_path_invalid".to_owned())?;
            expected_source_ids.insert(source.logical_id.clone());
            let file_id =
                FileId::parse(&source.file_id).map_err(|_| "source_file_id_invalid".to_owned())?;
            match index.current_note_projection(&self.context, file_id).await {
                Ok(Some(note))
                    if note.path.as_str() == source.path
                        && note.revision.value() == source.file_revision =>
                {
                    current_indexed_source_ids.insert(source.logical_id.clone());
                }
                Ok(_) => {
                    degradation_reasons.insert("ordinary_index_coverage_incomplete".to_owned());
                }
                Err(_) => {
                    degradation_reasons.insert("ordinary_index_unavailable".to_owned());
                }
            }
            let scope = NoteRetrievalScope {
                source_path: Some(source.path.clone()),
                ..NoteRetrievalScope::default()
            };
            let retrieval = if index_profile_id == M6_A80_INDEX_PROFILE_ID {
                // The existing recall policy uses relaxed lexical candidates
                // plus query relevance admission. None disables semantic hits;
                // this IndexService has no Provider and cannot generate.
                index
                    .retrieve_notes_for_recall_scoped(
                        &self.context,
                        &task.query,
                        None,
                        budget.max_entries.min(100),
                        &scope,
                    )
                    .await
            } else {
                index
                    .retrieve_notes(
                        &self.context,
                        &task.query,
                        NoteRetrievalMode::Lexical,
                        &scope,
                        budget.max_entries.min(100),
                        0,
                        false,
                    )
                    .await
            };
            let result = match retrieval {
                Ok(result) => result,
                Err(_) => {
                    degradation_reasons.insert("ordinary_index_unavailable".to_owned());
                    continue;
                }
            };
            let mut stale_hits = 0_u32;
            for hit in &result.hits {
                if hit.note.file_id != file_id
                    || hit.note.path.as_str() != source.path
                    || hit.note.revision.value() != source.file_revision
                {
                    stale_hits = stale_hits.saturating_add(1);
                    continue;
                }
            }
            if stale_hits > 0 {
                degradation_reasons.insert("ordinary_index_stale_hit".to_owned());
                stale_hit_count = stale_hit_count.saturating_add(stale_hits);
            }
            degradation_reasons.extend(result.degraded.iter().cloned());
            candidate_count =
                candidate_count.saturating_add(result.candidate_count.saturating_sub(stale_hits));
            eligible_count =
                eligible_count.saturating_add(result.eligible_count.saturating_sub(stale_hits));
            available_result_count = available_result_count
                .saturating_add(result.available_result_count.saturating_sub(stale_hits));
            for hit in result.hits {
                if hit.note.file_id != file_id
                    || hit.note.path.as_str() != source.path
                    || hit.note.revision.value() != source.file_revision
                {
                    continue;
                }
                if selected.len() >= budget.max_entries as usize {
                    break;
                }
                let snippet = hit.note.snippet;
                let snippet_tokens =
                    u32::try_from(snippet.chars().count().div_ceil(4)).unwrap_or(u32::MAX);
                let next_tokens = total_tokens.saturating_add(snippet_tokens);
                if next_tokens > budget.max_tokens {
                    break;
                }
                let entry = json!({
                    "source_id": source.logical_id,
                    "file_id": source.file_id,
                    "path": source.path,
                    "file_revision": source.file_revision,
                    "title": hit.note.title,
                    "snippet": snippet,
                    "matched_section": hit.matched_section.map(|section| section.heading_path),
                    "score": hit.score,
                });
                let entry_bytes = serde_json::to_vec(&entry)
                    .map_err(|_| "ordinary_retrieval_serialization".to_owned())?
                    .len() as u64;
                let next_bytes = total_bytes.saturating_add(entry_bytes);
                if next_bytes > u64::from(budget.max_bytes) {
                    break;
                }
                total_bytes = next_bytes;
                total_tokens = next_tokens;
                selected.push(entry);
            }
        }
        if current_indexed_source_ids != expected_source_ids {
            degradation_reasons.insert("ordinary_index_coverage_incomplete".to_owned());
        }
        let quality_events = if eligible_count == 0 {
            vec!["lexical_no_match"]
        } else {
            Vec::new()
        };
        let expected_source_count = u32::try_from(expected_source_ids.len()).unwrap_or(u32::MAX);
        let current_indexed_source_count =
            u32::try_from(current_indexed_source_ids.len()).unwrap_or(u32::MAX);
        let coverage_ratio = if expected_source_count == 0 {
            0.0
        } else {
            f64::from(current_indexed_source_count) / f64::from(expected_source_count)
        };
        let degradation_reasons = degradation_reasons.into_iter().collect::<Vec<_>>();
        let returned_hit_count = selected.len();
        let mut result = json!({
            "retrieval_strategy": "ordinary_note_v1",
            "task_id": task.id,
            "query": task.query,
            "index_profile_id": index_profile_id,
            "sources": selected,
            "entry_count": 0,
            "token_count": total_tokens,
            "coverage": {
                "expected_source_ids": expected_source_ids,
                "current_indexed_source_ids": current_indexed_source_ids,
                "expected_source_count": expected_source_count,
                "current_indexed_source_count": current_indexed_source_count,
                "coverage_ratio": coverage_ratio,
                "complete": expected_source_count == current_indexed_source_count,
                "candidate_count": candidate_count,
                "eligible_count": eligible_count,
                "available_result_count": available_result_count,
                "returned_hit_count": returned_hit_count,
                "stale_hit_count": stale_hit_count,
            },
            "degradation_reasons": degradation_reasons,
            "quality_events": quality_events,
        });
        loop {
            let bytes = serde_json::to_vec(&result)
                .map_err(|_| "ordinary_retrieval_serialization".to_owned())?;
            let tokens = u32::try_from(bytes.len().div_ceil(4)).unwrap_or(u32::MAX);
            if bytes.len() <= budget.max_bytes as usize && tokens <= budget.max_tokens {
                if let Some(object) = result.as_object_mut() {
                    let entry_count = object
                        .get("sources")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    object.insert("entry_count".to_owned(), json!(entry_count));
                    if let Some(coverage) =
                        object.get_mut("coverage").and_then(Value::as_object_mut)
                    {
                        coverage.insert("returned_hit_count".to_owned(), json!(entry_count));
                    }
                    object.insert("token_count".to_owned(), json!(tokens));
                }
                return Ok(result);
            }
            let Some(sources) = result.get_mut("sources").and_then(Value::as_array_mut) else {
                return Err("ordinary_retrieval_budget_exhausted".to_owned());
            };
            if sources.pop().is_none() {
                return Err("ordinary_retrieval_budget_exhausted".to_owned());
            }
        }
    }

    async fn prepare(&self, source: &EvalSource) -> Result<Value, String> {
        let _ = source;
        Err("semantic_arm_isolation_required".to_owned())
    }

    async fn build_pack(
        &self,
        task: &EvalTask,
        source_paths: &[String],
        budget: &crate::Budget,
    ) -> Result<Value, String> {
        let _ = (task, source_paths, budget);
        Err("semantic_arm_isolation_required".to_owned())
    }

    async fn build_pack_for_arm(
        &self,
        arm: crate::ComparisonArm,
        task: &EvalTask,
        sources: &[EvalSource],
        budget: &crate::Budget,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let mut source_ids = Vec::new();
        let mut source_paths = Vec::new();
        let mut source_revision_ids = Vec::new();
        for source in sources {
            let binding = Self::resolve_source_binding(runtime, source).await?;
            source_ids.push(binding.source_id.to_string());
            source_paths.push(binding.path.clone());
            source_revision_ids.push(binding.source_revision_id.to_string());
            self.source_bindings
                .lock()
                .await
                .insert((arm.clone(), source.logical_id.clone()), binding);
        }
        if sources.is_empty() {
            return Err("semantic_source_scope_empty".to_owned());
        }
        let request = MemoryPackRequest {
            task: task.query.clone(),
            query: Some(task.query.clone()),
            comparison_source_paths: source_paths.clone(),
            source_scope: Some(MemoryPackSourceScope {
                source_ids,
                source_paths,
                source_revision_ids,
            }),
            max_entries: budget.max_entries,
            max_bytes: budget.max_bytes,
            max_tokens: budget.max_tokens,
            ..MemoryPackRequest::default()
        };
        let pack = SemanticPublicFacade::new(runtime.state.clone())
            .build_pack(&runtime.context, &runtime.core, &self.access, &request)
            .await
            .map_err(|error| error.code().to_owned())?;
        for source in sources {
            Self::verify_arm_file(runtime, source).await?;
        }
        let mut value =
            serde_json::to_value(pack).map_err(|_| "memory_pack_serialization".to_owned())?;
        add_m6_evidence_excerpts(runtime, &mut value, budget.max_bytes).await?;
        Ok(value)
    }

    async fn prepare_relation_input_for_arm(
        &self,
        arm: crate::ComparisonArm,
        task: &EvalTask,
        sources: &[EvalSource],
    ) -> Result<Option<Value>, String> {
        if arm != crate::ComparisonArm::C {
            return Err("semantic_relation_requires_c_arm".to_owned());
        }
        let runtime = self.arm(arm.clone())?;
        let mut source_ids = Vec::new();
        let mut logical_sources = BTreeMap::new();
        for source in sources {
            Self::verify_arm_file(runtime, source).await?;
            let binding = Self::resolve_source_binding(runtime, source).await?;
            logical_sources.insert(binding.source_id.to_string(), source.logical_id.clone());
            source_ids.push(binding.source_id);
            self.source_bindings
                .lock()
                .await
                .insert((arm.clone(), source.logical_id.clone()), binding);
        }
        if source_ids.len() < 2 {
            self.relation_candidate_aliases
                .lock()
                .await
                .insert((arm, task.id.clone()), BTreeMap::new());
            return Ok(Some(json!({
                "dispatch_required": false,
                "status": "single_source_scope",
                "candidate_count": 0,
                "candidates": [],
            })));
        }
        let candidates = SemanticOrganizationService::new(runtime.state.clone())
            .discover_candidates(&runtime.context, &source_ids)
            .await
            .map_err(|error| error.code().to_owned())?;
        if candidates.len() > 32 {
            return Err("semantic_organization_candidate_budget_exceeded".to_owned());
        }
        let mut aliases = BTreeMap::new();
        let mut candidate_views = Vec::with_capacity(candidates.len());
        for (index, candidate) in candidates.iter().enumerate() {
            let alias = format!("C{:02}", index + 1);
            aliases.insert(alias.clone(), candidate.id.to_string());
            let left_source = logical_sources
                .get(&candidate.left.source_id.to_string())
                .ok_or_else(|| "semantic_relation_source_scope_mismatch".to_owned())?;
            let right_source = logical_sources
                .get(&candidate.right.source_id.to_string())
                .ok_or_else(|| "semantic_relation_source_scope_mismatch".to_owned())?;
            let observation = |value: &mcp_vault_state::OrganizationObservation,
                               source_id: &str| {
                json!({
                    "source_id": source_id,
                    "kind": value.kind,
                    "statement": value.statement,
                    "scope": value.scope,
                    "assertion_status": value.assertion_status,
                    "source_time_scope": value.source_time_scope,
                    "conditions": value.conditions,
                    "exceptions": value.exceptions,
                    "ordered_steps": value.ordered_steps,
                    "result": value.result,
                    "uncertainty": value.uncertainty,
                    "value_for_future_work": value.value_for_future_work,
                })
            };
            candidate_views.push(json!({
                "candidate_id": alias,
                "left": observation(&candidate.left, left_source),
                "right": observation(&candidate.right, right_source),
            }));
        }
        self.relation_candidate_aliases
            .lock()
            .await
            .insert((arm, task.id.clone()), aliases);
        Ok(Some(json!({
            "dispatch_required": !candidate_views.is_empty(),
            "status": if candidate_views.is_empty() { "no_candidates" } else { "candidates_ready" },
            "candidate_count": candidate_views.len(),
            "candidates": candidate_views,
        })))
    }

    async fn submit_relation_proposal_for_arm(
        &self,
        arm: crate::ComparisonArm,
        task: &EvalTask,
        sources: &[EvalSource],
        proposal: &Value,
    ) -> Result<Value, String> {
        if arm != crate::ComparisonArm::C {
            return Err("semantic_relation_requires_c_arm".to_owned());
        }
        let runtime = self.arm(arm.clone())?;
        let key = (arm.clone(), task.id.clone());
        let aliases = self
            .relation_candidate_aliases
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| "semantic_relation_candidates_missing".to_owned())?;
        if aliases.is_empty() {
            return Err("semantic_relation_candidates_missing".to_owned());
        }
        let mut proposal = proposal.clone();
        let actions = proposal
            .get_mut("actions")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "semantic_organization_proposal_schema".to_owned())?;
        let mut seen_aliases = std::collections::BTreeSet::new();
        for action in actions {
            let candidate_ids = action
                .get_mut("candidate_ids")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| "semantic_organization_candidate_required".to_owned())?;
            for candidate_id in candidate_ids {
                let alias = candidate_id
                    .as_str()
                    .ok_or_else(|| "semantic_organization_candidate_invalid".to_owned())?;
                if !seen_aliases.insert(alias.to_owned()) {
                    return Err("semantic_organization_candidate_reused".to_owned());
                }
                let resolved = aliases
                    .get(alias)
                    .ok_or_else(|| "semantic_organization_candidate_invalid".to_owned())?;
                *candidate_id = json!(resolved);
            }
        }
        if seen_aliases.len() != aliases.len() {
            return Err("semantic_organization_decisions_incomplete".to_owned());
        }
        let mut source_ids = Vec::with_capacity(sources.len());
        for source in sources {
            Self::verify_arm_file(runtime, source).await?;
            let binding = Self::resolve_source_binding(runtime, source).await?;
            source_ids.push(binding.source_id);
        }
        SemanticOrganizationService::new(runtime.state.clone())
            .organize_json(
                &runtime.context,
                &runtime.core,
                &source_ids,
                &serde_json::to_string(&proposal)
                    .map_err(|_| "semantic_organization_proposal_serialization".to_owned())?,
            )
            .await
            .map_err(|error| error.code().to_owned())?;
        self.relation_candidate_aliases.lock().await.remove(&key);
        Ok(json!({
            "status": "applied",
            "candidate_count": aliases.len(),
            "decision_count": seen_aliases.len(),
        }))
    }

    async fn submit_observation(
        &self,
        source: &EvalSource,
        proposal: &Value,
    ) -> Result<Value, String> {
        let _ = (source, proposal);
        Err("semantic_arm_isolation_required".to_owned())
    }

    async fn current_card_projection(&self, source: &EvalSource) -> Result<Value, String> {
        let _ = source;
        Err("semantic_arm_isolation_required".to_owned())
    }

    async fn prepare_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm.clone(), source.logical_id.clone());
        if self.active_extractions.lock().await.contains_key(&key) {
            self.abort_pending_generation_for_source(arm, &source.logical_id)
                .await?;
            return Err("semantic_prepared_generation_exists".to_owned());
        }
        let path = VaultPath::parse(&source.path).map_err(|_| "source_path_invalid".to_owned())?;
        Self::verify_arm_file(runtime, source).await?;
        let semantic = SemanticMemoryService::new(runtime.state.clone());
        let prepared = semantic
            .begin_generation(&runtime.context, &runtime.core, &path)
            .await
            .map_err(|error| error.code().to_owned())?;
        self.active_extractions
            .lock()
            .await
            .insert(key.clone(), prepared.extraction.id);
        let binding = match Self::resolve_source_binding(runtime, source).await {
            Ok(binding) => binding,
            Err(error) => {
                Self::terminalize_extraction(runtime, prepared.extraction.id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err(error);
            }
        };
        let input = match indexed_observation_model_input(&prepared) {
            Ok(input) => input,
            Err(_) => {
                Self::terminalize_extraction(runtime, prepared.extraction.id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err("semantic_input_projection_failed".to_owned());
            }
        };
        self.source_bindings
            .lock()
            .await
            .insert(key.clone(), binding);
        self.prepared_generations.lock().await.insert(key, prepared);
        Ok(input)
    }

    async fn prepare_a80_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        provider_fingerprint: &str,
        prompt_id: &str,
        schema_id: &str,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm.clone(), source.logical_id.clone());
        if self.active_extractions.lock().await.contains_key(&key) {
            self.abort_pending_generation_for_source(arm.clone(), &source.logical_id)
                .await?;
            return Err("semantic_prepared_generation_exists".to_owned());
        }
        let path = VaultPath::parse(&source.path).map_err(|_| "source_path_invalid".to_owned())?;
        Self::verify_arm_file(runtime, source).await?;
        let semantic = SemanticMemoryService::new(runtime.state.clone());
        let prepared = semantic
            .begin_generation_resumable(&runtime.context, &runtime.core, &path)
            .await
            .map_err(|error| error.code().to_owned())?;
        self.active_extractions
            .lock()
            .await
            .insert(key.clone(), prepared.extraction.id);
        let binding = match Self::resolve_source_binding(runtime, source).await {
            Ok(binding) => binding,
            Err(error) => {
                Self::terminalize_extraction(runtime, prepared.extraction.id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err(error);
            }
        };
        let batches = match prepared.observation_batches() {
            Ok(batches) => batches,
            Err(error) => {
                Self::terminalize_extraction(runtime, prepared.extraction.id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err(error.code().to_owned());
            }
        };
        let actual_block_count = u32::try_from(prepared.model_input.blocks.len())
            .map_err(|_| "semantic_source_block_count_invalid".to_owned())?;
        if actual_block_count != source.logical_block_count
            || u32::try_from(batches.len()).ok()
                != Some(mcp_vault_memory::semantic::a80_batch_count(
                    actual_block_count,
                ))
        {
            Self::terminalize_extraction(runtime, prepared.extraction.id).await?;
            self.active_extractions.lock().await.remove(&key);
            return Err("semantic_source_block_count_mismatch".to_owned());
        }
        let batch_records = match semantic
            .bind_observation_batches(
                &runtime.context,
                &prepared,
                &batches,
                provider_fingerprint,
                prompt_id,
                schema_id,
            )
            .await
        {
            Ok(records) => records,
            Err(error) => {
                Self::terminalize_extraction(runtime, prepared.extraction.id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err(error.code().to_owned());
            }
        };
        let projected = batches
            .iter()
            .map(|batch| {
                let batch_state = batch_records
                    .get(batch.index as usize)
                    .filter(|record| record.batch_index == batch.index)
                    .map(|record| record.state.as_str())
                    .ok_or_else(|| "semantic_observation_batch_catalog_invalid".to_owned())?;
                Ok::<_, String>(json!({
                    "batch_index": batch.index,
                    "batch_count": batch.count,
                    "batch_state": batch_state,
                    "attempt_count": batch_records[batch.index as usize].attempt_count,
                    "input": batch.input,
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.source_bindings
            .lock()
            .await
            .insert(key.clone(), binding);
        self.prepared_generations
            .lock()
            .await
            .insert(key.clone(), prepared);
        self.prepared_a80_batches.lock().await.insert(key, batches);
        Ok(json!({"batches":projected}))
    }

    async fn accept_a80_batch_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        batch_index: u32,
        proposal: &Value,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm.clone(), source.logical_id.clone());
        let prepared = self
            .prepared_generations
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| "semantic_prepared_generation_missing".to_owned())?;
        let batch = self
            .prepared_a80_batches
            .lock()
            .await
            .get(&key)
            .and_then(|batches| batches.get(batch_index as usize))
            .cloned()
            .filter(|batch| batch.index == batch_index)
            .ok_or_else(|| "semantic_observation_batch_unknown".to_owned())?;
        let response_json = serde_json::to_string(proposal)
            .map_err(|_| "semantic_flat_observation_serialization".to_owned())?;
        let semantic = SemanticMemoryService::new(runtime.state.clone());
        let counts = semantic
            .accept_observation_batch(
                &runtime.context,
                &runtime.core,
                &prepared,
                &batch,
                &response_json,
            )
            .await
            .map_err(|error| error.code().to_owned())?;
        let value = serde_json::to_value(&counts)
            .map_err(|_| "semantic_normalization_count_serialization".to_owned())?;
        self.a80_normalization_counts
            .lock()
            .await
            .insert(key, value.clone());
        Ok(value)
    }

    async fn reserve_a80_batch_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        batch_index: u32,
    ) -> Result<String, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm, source.logical_id.clone());
        let prepared = self
            .prepared_generations
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| "semantic_prepared_generation_missing".to_owned())?;
        let batch = self
            .prepared_a80_batches
            .lock()
            .await
            .get(&key)
            .and_then(|batches| batches.get(batch_index as usize))
            .cloned()
            .filter(|batch| batch.index == batch_index)
            .ok_or_else(|| "semantic_observation_batch_unknown".to_owned())?;
        SemanticMemoryService::new(runtime.state.clone())
            .reserve_observation_batch_attempt(&runtime.context, &prepared, &batch)
            .await
            .map_err(|error| error.code().to_owned())
    }

    async fn fail_a80_batch_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        batch_index: u32,
        safe_error_code: &str,
    ) -> Result<(), String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm, source.logical_id.clone());
        let prepared = self
            .prepared_generations
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| "semantic_prepared_generation_missing".to_owned())?;
        let batch = self
            .prepared_a80_batches
            .lock()
            .await
            .get(&key)
            .and_then(|batches| batches.get(batch_index as usize))
            .cloned()
            .filter(|batch| batch.index == batch_index)
            .ok_or_else(|| "semantic_observation_batch_unknown".to_owned())?;
        SemanticMemoryService::new(runtime.state.clone())
            .fail_observation_batch_attempt(
                &runtime.context,
                &runtime.core,
                &prepared,
                &batch,
                safe_error_code,
            )
            .await
            .map_err(|error| error.code().to_owned())
    }

    async fn reserve_a80_regen_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        batch_index: u32,
    ) -> Result<String, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm, source.logical_id.clone());
        let prepared = self
            .prepared_generations
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| "semantic_prepared_generation_missing".to_owned())?;
        let batch = self
            .prepared_a80_batches
            .lock()
            .await
            .get(&key)
            .and_then(|batches| batches.get(batch_index as usize))
            .cloned()
            .filter(|batch| batch.index == batch_index)
            .ok_or_else(|| "semantic_observation_batch_unknown".to_owned())?;
        SemanticMemoryService::new(runtime.state.clone())
            .reserve_observation_batch_regen_attempt(
                &runtime.context,
                &runtime.core,
                &prepared,
                &batch,
            )
            .await
            .map_err(|error| error.code().to_owned())
    }

    async fn finalize_a80_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm.clone(), source.logical_id.clone());
        let prepared = self
            .prepared_generations
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| "semantic_prepared_generation_missing".to_owned())?;
        let batches = self
            .prepared_a80_batches
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| "semantic_observation_batches_missing".to_owned())?;
        let path = VaultPath::parse(&source.path).map_err(|_| "source_path_invalid".to_owned())?;
        let semantic = SemanticMemoryService::new(runtime.state.clone());
        let expected_batch_count =
            u32::try_from(batches.len()).map_err(|_| "semantic_batch_count_invalid".to_owned())?;
        let submission_result = if matches!(
            prepared.extraction.state.as_str(),
            "success_nonempty" | "success_empty"
        ) {
            semantic
                .resume_published_observation_batches(
                    &runtime.context,
                    &runtime.core,
                    &path,
                    &prepared,
                    expected_batch_count,
                )
                .await
        } else {
            semantic
                .finalize_observation_batches(
                    &runtime.context,
                    &runtime.core,
                    &path,
                    &prepared,
                    expected_batch_count,
                )
                .await
        };
        let submission = match submission_result {
            Ok(value) => value,
            Err(error) => {
                let _ = Self::terminalize_extraction(runtime, prepared.extraction.id).await;
                self.active_extractions.lock().await.remove(&key);
                self.prepared_generations.lock().await.remove(&key);
                self.prepared_a80_batches.lock().await.remove(&key);
                return Err(error.code().to_owned());
            }
        };
        self.active_extractions.lock().await.remove(&key);
        self.prepared_generations.lock().await.remove(&key);
        self.prepared_a80_batches.lock().await.remove(&key);
        self.completed_extractions.lock().await.insert(key.clone());
        Ok(json!({
            "extraction_set_id":submission.extraction.id.to_string(),
            "state":submission.extraction.state,
            "observation_count":submission.extraction.observation_count,
            "card_count":submission.extraction.card_count,
        }))
    }

    async fn prepare_legacy_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let path = VaultPath::parse(&source.path).map_err(|_| "source_path_invalid".to_owned())?;
        Self::verify_arm_file(runtime, source).await?;
        let input = SemanticMemoryService::new(runtime.state.clone())
            .prepare_source(&runtime.context, &runtime.core, &path)
            .await
            .map_err(|error| error.code().to_owned())?;
        let binding = Self::resolve_source_binding(runtime, source).await?;
        self.source_bindings
            .lock()
            .await
            .insert((arm, source.logical_id.clone()), binding);
        serde_json::to_value(input).map_err(|_| "semantic_input_serialization".to_owned())
    }

    async fn submit_observation_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        proposal: &Value,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let path = VaultPath::parse(&source.path).map_err(|_| "source_path_invalid".to_owned())?;
        // Require that prepare actually established this arm's local source
        // mapping; this prevents a logical manifest ID from serving as a
        // semantic repository identifier.
        if !self
            .source_bindings
            .lock()
            .await
            .contains_key(&(arm.clone(), source.logical_id.clone()))
        {
            return Err("semantic_source_mapping_missing".to_owned());
        }
        Self::verify_arm_file(runtime, source).await?;
        let proposal_json = serde_json::to_string(proposal)
            .map_err(|_| "semantic_proposal_serialization".to_owned())?;
        let submission = SemanticMemoryService::new(runtime.state.clone())
            .submit_proposal_json(&runtime.context, &runtime.core, &path, &proposal_json)
            .await
            .map_err(|error| error.code().to_owned())?;
        if !matches!(
            submission.extraction.state.as_str(),
            "success_nonempty" | "success_empty"
        ) {
            return Err("semantic_extraction_not_published".to_owned());
        }
        let binding = Self::resolve_source_binding(runtime, source).await?;
        self.source_bindings
            .lock()
            .await
            .insert((arm.clone(), source.logical_id.clone()), binding);
        self.completed_extractions
            .lock()
            .await
            .insert((arm, source.logical_id.clone()));
        Ok(json!({
            "extraction_set_id": submission.extraction.id.to_string(),
            "state": submission.extraction.state,
            "observation_count": submission.extraction.observation_count,
            "card_count": submission.extraction.card_count,
        }))
    }

    async fn accept_observation_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        proposal: &Value,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm.clone(), source.logical_id.clone());
        let prepared = self.prepared_generations.lock().await.remove(&key);
        let Some(prepared) = prepared else {
            self.abort_pending_generation_for_source(arm, &source.logical_id)
                .await?;
            return Err("semantic_prepared_generation_missing".to_owned());
        };
        let extraction_id = prepared.extraction.id;
        let mut normalized = match restore_observation_evidence_ids(proposal, &prepared) {
            Ok(normalized) => normalized,
            Err(code) => {
                Self::terminalize_extraction(runtime, extraction_id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err(code);
            }
        };
        let overlap_removed = match normalize_v7_body_context_overlap(&mut normalized) {
            Ok(count) => count,
            Err(code) => {
                Self::terminalize_extraction(runtime, extraction_id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err(code);
            }
        };
        if overlap_removed > 0 {
            tracing::info!(
                body_context_overlap_removed_count = overlap_removed,
                "normalized duplicate M6 v7 body/context evidence roles"
            );
        }
        self.observation_context_overlap_removed
            .lock()
            .await
            .insert(key.clone(), overlap_removed);
        let proposal_json = match serde_json::to_string(&normalized) {
            Ok(value) => value,
            Err(_) => {
                Self::terminalize_extraction(runtime, extraction_id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err("semantic_proposal_serialization".to_owned());
            }
        };
        let phase = SemanticMemoryService::new(runtime.state.clone())
            .accept_observation_result(&runtime.context, &runtime.core, prepared, &proposal_json)
            .await;
        let phase = match phase {
            Ok(phase) => phase,
            Err(error) => {
                Self::terminalize_extraction(runtime, extraction_id).await?;
                self.active_extractions.lock().await.remove(&key);
                return Err(error.code().to_owned());
            }
        };
        let empty = phase.observations.is_empty();
        let composition = json!({
            "status": if empty { "success_empty" } else { "composition_required" },
            "observations": phase.observations,
            "groups": phase.groups,
        });
        self.observation_phases
            .lock()
            .await
            .insert(key.clone(), phase);
        if empty {
            self.completed_extractions.lock().await.insert(key.clone());
            self.active_extractions.lock().await.remove(&key);
        }
        Ok(composition)
    }

    async fn take_observation_context_overlap_removed_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
    ) -> u64 {
        self.observation_context_overlap_removed
            .lock()
            .await
            .remove(&(arm, source.logical_id.clone()))
            .unwrap_or_default()
    }

    async fn submit_composition_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
        proposal: &Value,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let key = (arm.clone(), source.logical_id.clone());
        let path = match VaultPath::parse(&source.path) {
            Ok(path) => path,
            Err(_) => {
                self.abort_pending_generation_for_source(arm, &source.logical_id)
                    .await?;
                return Err("source_path_invalid".to_owned());
            }
        };
        let proposal_json = match serde_json::to_string(proposal) {
            Ok(value) => value,
            Err(_) => {
                self.abort_pending_generation_for_source(arm, &source.logical_id)
                    .await?;
                return Err("semantic_composition_serialization".to_owned());
            }
        };
        let phase = self.observation_phases.lock().await.remove(&key);
        let Some(phase) = phase else {
            self.abort_pending_generation_for_source(arm, &source.logical_id)
                .await?;
            return Err("semantic_observation_phase_missing".to_owned());
        };
        let submission = SemanticMemoryService::new(runtime.state.clone())
            .submit_composition(
                &runtime.context,
                &runtime.core,
                &path,
                &phase,
                &proposal_json,
            )
            .await;
        let submission = match submission {
            Ok(value) => value,
            Err(error) => {
                if let Err(terminalization_error) =
                    Self::terminalize_extraction(runtime, phase.extraction.id).await
                {
                    self.observation_phases.lock().await.insert(key, phase);
                    return Err(terminalization_error);
                }
                self.active_extractions.lock().await.remove(&key);
                return Err(error.code().to_owned());
            }
        };
        self.completed_extractions.lock().await.insert(key.clone());
        self.active_extractions.lock().await.remove(&key);
        Ok(json!({
            "extraction_set_id": submission.extraction.id.to_string(),
            "state": submission.extraction.state,
            "observation_count": submission.extraction.observation_count,
            "card_count": submission.extraction.card_count,
        }))
    }

    async fn abort_semantic_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
    ) -> Result<(), String> {
        self.abort_pending_generation_for_source(arm, &source.logical_id)
            .await
    }

    async fn current_card_projection_for_arm(
        &self,
        arm: crate::ComparisonArm,
        source: &EvalSource,
    ) -> Result<Value, String> {
        let runtime = self.arm(arm.clone())?;
        let binding = self
            .source_bindings
            .lock()
            .await
            .get(&(arm.clone(), source.logical_id.clone()))
            .cloned()
            .ok_or_else(|| "semantic_source_mapping_missing".to_owned())?;
        if !self
            .completed_extractions
            .lock()
            .await
            .contains(&(arm.clone(), source.logical_id.clone()))
        {
            return Err("semantic_extraction_not_published".to_owned());
        }
        if binding.path != source.path || binding.file_revision != source.file_revision {
            return Err("semantic_source_fence_mismatch".to_owned());
        }
        let cards = SemanticMemoryService::new(runtime.state.clone())
            .list_cards_for_source_revision(
                &runtime.context,
                &runtime.core,
                binding.source_id,
                binding.source_revision_id,
            )
            .await
            .map_err(|error| error.code().to_owned())?;
        let cards = cards
            .into_iter()
            .filter(|card| {
                card.source_id == binding.source_id
                    && card.source_revision_id == binding.source_revision_id
            })
            .map(|card| {
                json!({
                    "card_id": card.id.to_string(),
                    "revision_id": card.revision_id.to_string(),
                    "title": card.title,
                    "kind": card.kind,
                    "scope": card.scope_ref,
                    "assertion_status": card.assertion_status,
                    "temporal_scope": card.temporal_scope,
                    "source_id": card.source_id.to_string(),
                    "source_revision_id": card.source_revision_id.to_string(),
                    "assertions": card.items.into_iter().map(|item| json!({
                        "kind": item.kind,
                        "ordinal": item.ordinal,
                        "content": item.content,
                        "observation_id": item.observation_id.to_string(),
                        "evidence_refs": item.evidence_ref_ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        if cards.is_empty() {
            return Ok(json!({
                "source_id": binding.source_id.to_string(),
                "source_revision_id": binding.source_revision_id.to_string(),
                "card_count": 0,
                "cards": [],
            }));
        }
        Ok(json!({
            "source_id": binding.source_id.to_string(),
            "source_revision_id": binding.source_revision_id.to_string(),
            "card_count": cards.len(),
            "cards": cards,
        }))
    }

    async fn verify_arm_task_sources(
        &self,
        arm: crate::ComparisonArm,
        sources: &[EvalSource],
        _boundary: &str,
    ) -> Result<(), String> {
        let runtime = self.arm(arm)?;
        for source in sources {
            Self::verify_arm_file(runtime, source).await?;
        }
        Ok(())
    }
}

/// Synchronous verifier backed by an asynchronously loaded Core/State
/// snapshot.  Loading happens before the first Provider request and validates
/// current File ID, revision, source revision, hashes, authorization,
/// generation, extraction sequence, and rules revision.
#[derive(Clone)]
pub struct VaultCoreSourceVerifier {
    snapshots: BTreeMap<String, AuthoritativeSourceSnapshot>,
    state: StateStore,
    context: VaultContext,
    core: VaultCore,
}

#[allow(clippy::too_many_arguments)]
fn source_state_is_accepted(
    eligible: bool,
    pending_rebuild: bool,
    source_generation: i64,
    extraction_commit_sequence: i64,
    invalid_reason: Option<&str>,
    authorization_revision: i64,
    policy_enabled: bool,
    policy_revision: i64,
) -> bool {
    if !policy_enabled || authorization_revision != policy_revision {
        return false;
    }
    let fresh = !eligible
        && pending_rebuild
        && source_generation == 0
        && extraction_commit_sequence == 0
        && invalid_reason == Some("source_changed");
    let published = eligible && !pending_rebuild && invalid_reason.is_none();
    fresh || published
}

impl VaultCoreSourceVerifier {
    pub async fn load(
        state: &StateStore,
        context: &VaultContext,
        core: &VaultCore,
        manifest: &EvaluationManifest,
    ) -> Result<Self, EvalError> {
        let mut snapshots = BTreeMap::new();
        let rules_revision = state
            .semantic_rules()
            .current_rules_revision(context)
            .await
            .map_err(|_| EvalError::SourceMismatch)?;
        let policy_record = state
            .settings()
            .get_vault(context, "memory.units.policy")
            .await
            .map_err(|_| EvalError::SourceMismatch)?
            .ok_or(EvalError::SourceMismatch)?;
        let policy_enabled = policy_record
            .value
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .ok_or(EvalError::SourceMismatch)?;
        let policy_revision = policy_record
            .revision
            .as_i64()
            .map_err(|_| EvalError::SourceMismatch)?;
        if !policy_enabled {
            return Err(EvalError::SourceMismatch);
        }
        for source in &manifest.sources {
            let path = VaultPath::parse(&source.path).map_err(|_| EvalError::SourceMismatch)?;
            let mut read = core
                .read(context, &path)
                .await
                .map_err(|_| EvalError::SourceMismatch)?;
            let file = read.file.clone();
            let mut content = Vec::new();
            read.reader
                .read_to_end(&mut content)
                .await
                .map_err(|_| EvalError::SourceMismatch)?;
            let source_record = state
                .semantic_memory()
                .get_source_by_file(context, file.id)
                .await
                .map_err(|_| EvalError::SourceMismatch)?
                .ok_or(EvalError::SourceMismatch)?;
            let source_revision_id = source_record
                .current_revision_id
                .ok_or(EvalError::SourceMismatch)?;
            let revision = state
                .semantic_memory()
                .get_source_revision(context, source_revision_id)
                .await
                .map_err(|_| EvalError::SourceMismatch)?
                .ok_or(EvalError::SourceMismatch)?;
            let content_hash = format!("{:x}", Sha256::digest(&content));
            let profile_id = "semantic-memory-m1-v1".to_owned();
            if source_record.vault_id != context.id()
                || source_record.file_id != file.id
                || source_record.source_path != path
                || source_record.content_hash.trim_start_matches("sha256:") != content_hash
                || revision.file_id != file.id
                || revision.file_revision != file.current_revision
                || revision.source_path != path
                || revision.content_hash.trim_start_matches("sha256:") != content_hash
                || revision.vault_id != context.id()
                || revision.source_id != source_record.source_id
                || revision.availability != "current"
                || revision.source_generation != source_record.source_generation
                || !source_state_is_accepted(
                    source_record.eligible,
                    source_record.pending_rebuild,
                    source_record.source_generation,
                    source_record.extraction_commit_sequence,
                    source_record.invalid_reason.as_deref(),
                    source_record.authorization_revision,
                    policy_enabled,
                    policy_revision,
                )
                || source.authorization_revision != source_record.authorization_revision
                || source.source_generation != source_record.source_generation
                || source.extraction_commit_sequence != source_record.extraction_commit_sequence
                || source.rules_revision != rules_revision
                || source.profile_id != profile_id
            {
                return Err(EvalError::SourceMismatch);
            }
            snapshots.insert(
                source.logical_id.clone(),
                AuthoritativeSourceSnapshot {
                    logical_id: source.logical_id.clone(),
                    vault_id: context.id().to_string(),
                    file_id: file.id.to_string(),
                    path: path.to_string(),
                    file_revision: file.current_revision.value(),
                    source_revision_id: source_revision_id.to_string(),
                    content_hash,
                    content,
                    authorized: true,
                    authorization_revision: source_record.authorization_revision,
                    profile_id,
                    rules_revision,
                    source_generation: source_record.source_generation,
                    extraction_commit_sequence: source_record.extraction_commit_sequence,
                },
            );
        }
        Ok(Self {
            snapshots,
            state: state.clone(),
            context: context.clone(),
            core: core.clone(),
        })
    }
}

#[async_trait]
impl SourceSnapshotVerifier for VaultCoreSourceVerifier {
    fn snapshot(&self, source: &EvalSource) -> Result<AuthoritativeSourceSnapshot, EvalError> {
        self.snapshots
            .get(&source.logical_id)
            .cloned()
            .ok_or(EvalError::SourceMismatch)
    }

    async fn snapshot_current(
        &self,
        source: &EvalSource,
    ) -> Result<AuthoritativeSourceSnapshot, EvalError> {
        let path = VaultPath::parse(&source.path).map_err(|_| EvalError::SourceMismatch)?;
        let mut read = self
            .core
            .read(&self.context, &path)
            .await
            .map_err(|_| EvalError::SourceMismatch)?;
        let file = read.file.clone();
        let mut content = Vec::new();
        read.reader
            .read_to_end(&mut content)
            .await
            .map_err(|_| EvalError::SourceMismatch)?;
        let source_record = self
            .state
            .semantic_memory()
            .get_source_by_file(&self.context, file.id)
            .await
            .map_err(|_| EvalError::SourceMismatch)?
            .ok_or(EvalError::SourceMismatch)?;
        let source_revision_id = source_record
            .current_revision_id
            .ok_or(EvalError::SourceMismatch)?;
        let revision = self
            .state
            .semantic_memory()
            .get_source_revision(&self.context, source_revision_id)
            .await
            .map_err(|_| EvalError::SourceMismatch)?
            .ok_or(EvalError::SourceMismatch)?;
        let rules_revision = self
            .state
            .semantic_rules()
            .current_rules_revision(&self.context)
            .await
            .map_err(|_| EvalError::SourceMismatch)?;
        let policy_record = self
            .state
            .settings()
            .get_vault(&self.context, "memory.units.policy")
            .await
            .map_err(|_| EvalError::SourceMismatch)?
            .ok_or(EvalError::SourceMismatch)?;
        let policy_enabled = policy_record
            .value
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .ok_or(EvalError::SourceMismatch)?;
        let policy_revision = policy_record
            .revision
            .as_i64()
            .map_err(|_| EvalError::SourceMismatch)?;
        if !policy_enabled {
            return Err(EvalError::SourceMismatch);
        }
        let content_hash = format!("{:x}", Sha256::digest(&content));
        let profile_id = "semantic-memory-m1-v1".to_owned();
        if source_record.vault_id != self.context.id()
            || source_record.file_id != file.id
            || source_record.source_path != path
            || source_record.content_hash.trim_start_matches("sha256:") != content_hash
            || revision.file_id != file.id
            || revision.file_revision != file.current_revision
            || revision.source_path != path
            || revision.content_hash.trim_start_matches("sha256:") != content_hash
            || revision.vault_id != self.context.id()
            || revision.source_id != source_record.source_id
            || revision.availability != "current"
            || revision.source_generation != source_record.source_generation
            || !source_state_is_accepted(
                source_record.eligible,
                source_record.pending_rebuild,
                source_record.source_generation,
                source_record.extraction_commit_sequence,
                source_record.invalid_reason.as_deref(),
                source_record.authorization_revision,
                policy_enabled,
                policy_revision,
            )
            || source.authorization_revision != source_record.authorization_revision
            || source.source_generation != source_record.source_generation
            || source.extraction_commit_sequence != source_record.extraction_commit_sequence
            || source.rules_revision != rules_revision
            || source.profile_id != profile_id
        {
            return Err(EvalError::SourceMismatch);
        }
        Ok(AuthoritativeSourceSnapshot {
            logical_id: source.logical_id.clone(),
            vault_id: self.context.id().to_string(),
            file_id: file.id.to_string(),
            path: path.to_string(),
            file_revision: file.current_revision.value(),
            source_revision_id: source_revision_id.to_string(),
            content_hash,
            content,
            authorized: true,
            authorization_revision: source_record.authorization_revision,
            profile_id,
            rules_revision,
            source_generation: source_record.source_generation,
            extraction_commit_sequence: source_record.extraction_commit_sequence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        indexed_observation_model_input_for_catalog, normalize_relation_empty_optional_fields,
        normalize_v7_body_context_overlap, remap_evidence_index_field, resolve_observation_schema,
        restore_observation_evidence_ids_for_catalog, source_state_is_accepted,
        strict_relation_wire_schema,
    };
    use crate::{semantic_a80_provider_templates, semantic_live_provider_templates};
    use mcp_vault_memory::semantic::SemanticSourceBlock;
    use serde_json::{Map, Value, json};
    use std::collections::BTreeSet;

    #[test]
    fn observation_schema_binds_each_source_catalog_without_mutating_template() {
        let template = semantic_live_provider_templates("m", 600)
            .into_iter()
            .find(|template| template.stage == "observation")
            .unwrap();
        let original = template.schema.clone();
        let first = resolve_observation_schema(
            &template.schema,
            &json!({"evidence_namespace":"h-revision-a","blocks":[{"evidence_index":1},{"evidence_index":2}]}),
        )
        .unwrap();
        let second = resolve_observation_schema(
            &template.schema,
            &json!({"evidence_namespace":"h-revision-b","blocks":[{"evidence_index":1}]}),
        )
        .unwrap();
        assert_eq!(template.schema, original);
        assert_eq!(
            first["properties"]["evidence_namespace"]["enum"],
            json!(["h-revision-a"])
        );
        assert_eq!(
            first["properties"]["observations"]["items"]["properties"]["body_block_indices"]["items"]
                ["enum"],
            json!([1, 2])
        );
        assert_eq!(
            first["properties"]["observations"]["items"]["properties"]["source_time_scope"]["properties"]
                ["evidence_block_indices"]["items"]["enum"],
            json!([1, 2])
        );
        assert_eq!(
            second["properties"]["evidence_namespace"]["enum"],
            json!(["h-revision-b"])
        );
        assert_eq!(
            second["properties"]["observations"]["items"]["properties"]["body_block_indices"]["items"]
                ["enum"],
            json!([1])
        );
        assert!(!second.to_string().contains("h-revision-a"));
    }

    #[test]
    fn observation_schema_rejects_missing_or_empty_catalog_before_provider() {
        let schema = semantic_live_provider_templates("m", 600)
            .into_iter()
            .find(|template| template.stage == "observation")
            .unwrap()
            .schema;
        assert!(resolve_observation_schema(&schema, &json!({})).is_err());
        assert!(
            resolve_observation_schema(&schema, &json!({"blocks":[{"evidence_index":1}]})).is_err()
        );
        assert!(
            resolve_observation_schema(&schema, &json!({"evidence_namespace":"h-rev","blocks":[]}))
                .is_err()
        );
        assert!(
            resolve_observation_schema(
                &schema,
                &json!({"evidence_namespace":"h-rev","blocks":[{"evidence_index":2}]})
            )
            .is_err()
        );
    }

    #[test]
    fn indexed_observation_response_maps_only_with_the_exact_revision_namespace() {
        let blocks = vec![
            SemanticSourceBlock {
                local_id: "h-source-a-1".into(),
                text: "same first line".into(),
                line_number: 1,
                kind: "paragraph".into(),
            },
            SemanticSourceBlock {
                local_id: "h-source-a-2".into(),
                text: "second line".into(),
                line_number: 2,
                kind: "paragraph".into(),
            },
        ];
        let response = json!({
            "evidence_namespace":"h-source-a",
            "outcome":"success_nonempty",
            "observations":[{
                "kind":"decision",
                "statement":"Preserve the source assertion.",
                "scope":"project",
                "assertion_status":"source_asserted",
                "admission_reason":"source evidence",
                "value_for_future_work":"retain the assertion",
                "body_block_indices":[1],
                "context_block_indices":[2],
                "source_time_scope":{"status":"source_stated","value":"2026","evidence_block_indices":[1]},
                "conditions":[],"exceptions":[],"ordered_steps":[],"result":"","uncertainty":""
            }]
        });
        let restored =
            restore_observation_evidence_ids_for_catalog(&response, "h-source-a", &blocks).unwrap();
        assert!(restored.get("evidence_namespace").is_none());
        assert_eq!(
            restored["observations"][0]["body_block_ids"],
            json!(["h-source-a-1"])
        );
        assert_eq!(
            restored["observations"][0]["context_block_ids"],
            json!(["h-source-a-2"])
        );
        assert_eq!(
            restored["observations"][0]["source_time_scope"]["evidence_block_ids"],
            json!(["h-source-a-1"])
        );
        assert_eq!(restored["observations"][0]["result"], Value::Null);
        assert_eq!(restored["observations"][0]["uncertainty"], Value::Null);

        // The same 1-based index and same first-line text in another source
        // cannot replay because the prepared source-revision namespace differs.
        let other_source_blocks = vec![SemanticSourceBlock {
            local_id: "h-source-b-1".into(),
            text: "same first line".into(),
            line_number: 1,
            kind: "paragraph".into(),
        }];
        assert_eq!(
            restore_observation_evidence_ids_for_catalog(
                &response,
                "h-source-b",
                &other_source_blocks,
            )
            .unwrap_err(),
            "semantic_evidence_namespace_mismatch"
        );
        assert_eq!(
            restore_observation_evidence_ids_for_catalog(
                &json!({"observations":[]}),
                "h-source-a",
                &blocks,
            )
            .unwrap_err(),
            "semantic_evidence_namespace_missing"
        );
        // A new revision of the same source also receives a distinct namespace.
        assert!(
            restore_observation_evidence_ids_for_catalog(&response, "h-source-a-revised", &blocks,)
                .is_err()
        );
    }

    #[test]
    fn v7_observation_normalization_removes_only_body_context_role_duplicates() {
        let blocks = vec![
            SemanticSourceBlock {
                local_id: "h-source-a-1".into(),
                text: "decision and its qualifier".into(),
                line_number: 1,
                kind: "paragraph".into(),
            },
            SemanticSourceBlock {
                local_id: "h-source-a-2".into(),
                text: "additional context".into(),
                line_number: 2,
                kind: "paragraph".into(),
            },
        ];
        let response = json!({
            "evidence_namespace":"h-source-a",
            "outcome":"success_nonempty",
            "observations":[{
                "kind":"decision","statement":"Preserve the decision and qualifier.",
                "scope":"project","assertion_status":"source_asserted",
                "admission_reason":"source evidence","value_for_future_work":"retain both",
                "body_block_indices":[1],"context_block_indices":[1,2],
                "source_time_scope":{"status":"source_stated","value":"2026","evidence_block_indices":[1]},
                "conditions":[],"exceptions":[],"ordered_steps":[],"result":"","uncertainty":""
            }]
        });
        let mut mapped =
            restore_observation_evidence_ids_for_catalog(&response, "h-source-a", &blocks).unwrap();
        let before = mapped["observations"][0].clone();
        let removed = normalize_v7_body_context_overlap(&mut mapped).unwrap();
        let observation = &mapped["observations"][0];

        assert_eq!(removed, 1);
        assert_eq!(observation["body_block_ids"], json!(["h-source-a-1"]));
        assert_eq!(observation["context_block_ids"], json!(["h-source-a-2"]));
        assert_eq!(
            observation["source_time_scope"]["evidence_block_ids"],
            json!(["h-source-a-1"])
        );
        let before_union = before["body_block_ids"]
            .as_array()
            .unwrap()
            .iter()
            .chain(before["context_block_ids"].as_array().unwrap())
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let after_union = observation["body_block_ids"]
            .as_array()
            .unwrap()
            .iter()
            .chain(observation["context_block_ids"].as_array().unwrap())
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(before_union, after_union);
    }

    #[test]
    fn v7_observation_normalization_rejects_unmapped_role_ids() {
        let mut malformed = json!({
            "observations":[{
                "body_block_ids":[1],"context_block_ids":[]
            }]
        });
        assert_eq!(
            normalize_v7_body_context_overlap(&mut malformed).unwrap_err(),
            "semantic_evidence_contract_invalid"
        );
    }

    #[test]
    fn relation_wire_requires_all_six_optional_strings_without_changing_local_schema() {
        let local = semantic_live_provider_templates("m", 30)
            .into_iter()
            .find(|template| template.stage == "relation")
            .unwrap()
            .schema;
        let wire = strict_relation_wire_schema(&local).unwrap();
        let local_item = &local["properties"]["actions"]["items"];
        let wire_item = &wire["properties"]["actions"]["items"];
        assert_eq!(local["required"], json!(["actions"]));
        assert_eq!(local_item["required"], json!(["action", "candidate_ids"]));
        assert_eq!(wire_item["required"].as_array().unwrap().len(), 8);
        assert_eq!(
            wire_item["required"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            wire_item["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        );
        assert_eq!(local_item["properties"]["reason"]["type"], json!("string"));
        assert!(local_item["required"].as_array().unwrap().len() < 8);
    }

    #[test]
    fn relation_wire_empty_sentinels_remove_only_the_six_optional_fields() {
        let mut proposal = json!({"actions":[{
            "action":"create_composed_card","candidate_ids":["candidate-1"],
            "card_ref":"","title":"new card","content":"","item_kind":"core_assertion",
            "support_operator":"","reason":"supported by both sources"
        }]});
        assert_eq!(
            normalize_relation_empty_optional_fields(&mut proposal).unwrap(),
            3
        );
        assert_eq!(proposal["actions"][0]["action"], "create_composed_card");
        assert_eq!(
            proposal["actions"][0]["candidate_ids"],
            json!(["candidate-1"])
        );
        assert!(proposal["actions"][0].get("card_ref").is_none());
        assert_eq!(proposal["actions"][0]["title"], "new card");
        assert!(proposal["actions"][0].get("content").is_none());
        assert_eq!(proposal["actions"][0]["item_kind"], "core_assertion");
        assert!(proposal["actions"][0].get("support_operator").is_none());
        assert_eq!(
            proposal["actions"][0]["reason"],
            "supported by both sources"
        );

        let mut missing =
            json!({"actions":[{"action":"no_change","candidate_ids":["candidate-1"]}]});
        assert_eq!(
            normalize_relation_empty_optional_fields(&mut missing).unwrap_err(),
            "semantic_relation_wire_result_invalid"
        );
    }

    #[test]
    fn v7_required_empty_markers_map_only_to_their_documented_optional_values() {
        let blocks = vec![SemanticSourceBlock {
            local_id: "h-v7-1".into(),
            text: "evidence".into(),
            line_number: 1,
            kind: "paragraph".into(),
        }];
        let valid = json!({
            "evidence_namespace":"h-v7",
            "outcome":"success_nonempty",
            "observations":[{
                "kind":"state","statement":"A source fact.","scope":"project",
                "assertion_status":"source_asserted","admission_reason":"source evidence",
                "value_for_future_work":"retain the fact","body_block_indices":[1],
                "source_time_scope":{"status":"unknown","value":"","evidence_block_indices":[]},
                "conditions":[],"exceptions":[],"ordered_steps":[],"result":"","uncertainty":"","context_block_indices":[]
            }]
        });
        let restored =
            restore_observation_evidence_ids_for_catalog(&valid, "h-v7", &blocks).unwrap();
        let observation = &restored["observations"][0];
        assert_eq!(observation["body_block_ids"], json!(["h-v7-1"]));
        assert_eq!(observation["context_block_ids"], json!([]));
        assert_eq!(observation["result"], Value::Null);
        assert_eq!(observation["uncertainty"], Value::Null);
        assert_eq!(observation["source_time_scope"]["status"], "unknown");
        assert_eq!(observation["source_time_scope"]["value"], Value::Null);
        assert_eq!(
            observation["source_time_scope"]["evidence_block_ids"],
            json!([])
        );

        let mut invalid_time = valid.clone();
        invalid_time["observations"][0]["source_time_scope"]["value"] = json!("2026");
        assert!(
            restore_observation_evidence_ids_for_catalog(&invalid_time, "h-v7", &blocks).is_err()
        );
        let mut invalid_outcome = valid;
        invalid_outcome["outcome"] = json!("success_empty");
        assert!(
            restore_observation_evidence_ids_for_catalog(&invalid_outcome, "h-v7", &blocks)
                .is_err()
        );
    }

    #[test]
    fn indexed_provider_input_omits_local_and_source_identity() {
        let blocks = vec![SemanticSourceBlock {
            local_id: "h-private-local-id-1".into(),
            text: "fixture body".into(),
            line_number: 7,
            kind: "paragraph".into(),
        }];
        let input =
            indexed_observation_model_input_for_catalog("h-opaque-revision-tag", &blocks).unwrap();
        assert_eq!(input["evidence_namespace"], "h-opaque-revision-tag");
        assert_eq!(input["blocks"][0]["evidence_index"], 1);
        assert_eq!(input["blocks"][0]["text"], "fixture body");
        assert_eq!(input["blocks"][0]["line_number"], 7);
        assert_eq!(input["blocks"][0]["kind"], "paragraph");
        assert!(input["blocks"][0].get("local_id").is_none());
        let model_input = input.to_string();
        assert!(!model_input.contains("h-private-local-id-1"));
        assert!(!model_input.contains("source_revision_id"));
        assert!(!model_input.contains("vault_id"));
        assert!(!model_input.contains("source_id"));
    }

    #[test]
    fn indexed_observation_evidence_rejects_unknown_duplicate_and_non_integer_indices() {
        let blocks = vec![SemanticSourceBlock {
            local_id: "h-rev-1".into(),
            text: "first".into(),
            line_number: 1,
            kind: "paragraph".into(),
        }];
        for bad_index in [json!(0), json!(2), json!(1.0), json!("1"), json!(true)] {
            let mut fields = Map::new();
            fields.insert("body_block_indices".to_owned(), json!([bad_index]));
            assert_eq!(
                remap_evidence_index_field(
                    &mut fields,
                    "body_block_indices",
                    "body_block_ids",
                    &blocks,
                    true,
                )
                .unwrap_err(),
                "semantic_evidence_index_invalid"
            );
        }
        for (index_field, id_field, required) in [
            ("body_block_indices", "body_block_ids", true),
            ("context_block_indices", "context_block_ids", false),
            ("evidence_block_indices", "evidence_block_ids", false),
        ] {
            let mut repeated = Map::new();
            repeated.insert(index_field.to_owned(), json!([1, 1]));
            assert!(
                remap_evidence_index_field(
                    &mut repeated,
                    index_field,
                    id_field,
                    &blocks,
                    required,
                )
                .is_err()
            );
        }
        let mut unknown = Map::new();
        unknown.insert("body_block_ids".to_owned(), json!(["h-rev-1"]));
        assert!(
            remap_evidence_index_field(
                &mut unknown,
                "body_block_indices",
                "body_block_ids",
                &blocks,
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn prepared_schema_binds_revision_namespace_and_one_based_index_enum() {
        let schema = semantic_live_provider_templates("m", 600)
            .into_iter()
            .find(|template| template.stage == "observation")
            .unwrap()
            .schema;
        let resolved = resolve_observation_schema(
            &schema,
            &json!({"evidence_namespace":"h0123456789ab","blocks":[{"evidence_index":1}]}),
        )
        .unwrap();
        assert_eq!(
            resolved["properties"]["evidence_namespace"]["enum"],
            json!(["h0123456789ab"])
        );
        assert_eq!(
            resolved["properties"]["observations"]["items"]["properties"]["body_block_indices"]["items"]
                ["enum"],
            json!([1])
        );
    }

    #[test]
    fn a80_prepared_schema_binds_source_local_indices_without_model_echoing_identity() {
        let template = semantic_a80_provider_templates("m", 600)
            .into_iter()
            .find(|template| template.stage == "observation")
            .unwrap();
        let input = json!({
            "batch_index": 1,
            "batch_count": 2,
            "blocks": (81..=85).map(|evidence_index| json!({"evidence_index": evidence_index})).collect::<Vec<_>>()
        });
        let resolved = resolve_observation_schema(&template.schema, &input).unwrap();
        assert_eq!(resolved["required"], json!(["claims"]));
        assert!(resolved["properties"].get("evidence_namespace").is_none());
        assert_eq!(
            resolved["properties"]["claims"]["items"]["properties"]["evidence_indices"]["items"]["enum"],
            json!([81, 82, 83, 84, 85])
        );
        let validate =
            |value: &Value| mcp_vault_providers::validate_structured_value(value, &resolved);
        let mut claim = json!({"claims":[{
            "statement":"Retain the source-stated time.","evidence_indices":[81],
            "source_time_scope":{"status":"source_stated","value":"before cutover","evidence_indices":[82]}
        }]});
        validate(&claim).unwrap();
        claim["claims"][0]["source_time_scope"]["evidence_indices"] = json!([1]);
        assert!(validate(&claim).is_err());
        claim["claims"][0]["source_time_scope"] = json!({"status":"unknown","value":"","evidence_indices":[],"synthetic_extra":"invalid"});
        assert!(validate(&claim).is_err());
        claim["claims"][0]["source_time_scope"] =
            json!({"status":"future_state","value":"later","evidence_indices":[]});
        validate(&claim).unwrap(); // Advisory values still normalize to unknown.
        claim["claims"][0]
            .as_object_mut()
            .unwrap()
            .remove("source_time_scope");
        validate(&claim).unwrap();
        let mut mismatched = input;
        mismatched["batch_index"] = json!(0);
        assert!(resolve_observation_schema(&template.schema, &mismatched).is_err());
    }

    #[test]
    fn fresh_source_is_allowed_without_an_extraction_record() {
        assert!(source_state_is_accepted(
            false,
            true,
            0,
            0,
            Some("source_changed"),
            3,
            true,
            3,
        ));
    }

    #[test]
    fn fresh_source_rejects_policy_revocation_or_revision_change() {
        for (enabled, policy_revision) in [(false, 3), (true, 4)] {
            assert!(!source_state_is_accepted(
                false,
                true,
                0,
                0,
                Some("source_changed"),
                3,
                enabled,
                policy_revision,
            ));
        }
    }

    #[test]
    fn fresh_source_rejects_generation_or_invalid_reason_drift() {
        assert!(!source_state_is_accepted(
            false,
            true,
            1,
            0,
            Some("source_changed"),
            3,
            true,
            3,
        ));
        assert!(!source_state_is_accepted(
            false,
            true,
            0,
            0,
            Some("permission_revoked"),
            3,
            true,
            3,
        ));
    }
}
