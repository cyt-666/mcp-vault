//! Protocol-neutral public semantic facade.
//!
//! Adapters in later M5 work call this module; it intentionally knows nothing
//! about MCP, HTTP, UI, or provider transport.

use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, FileId, MemoryId, Permission, PermissionSet, Revision, SourcePlane, VaultContext,
    VaultPath,
};
use mcp_vault_state::{ComposedCardRecord, SemanticCardRecord, StateStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    MemoryError, MemoryOrigin, MemoryPack, MemoryPackRequest, MemoryPackService, MemoryReadAccess,
    MemoryService, MemorySourceInput, MemoryType, MemoryUpdateInput, RememberInput,
    SemanticMemoryService,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticAccess {
    permissions: PermissionSet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticActor(String);

impl SemanticActor {
    pub fn trusted(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl SemanticAccess {
    pub fn new(permissions: PermissionSet) -> Self {
        Self { permissions }
    }

    pub fn permissions(&self) -> &PermissionSet {
        &self.permissions
    }

    fn can_read(&self) -> bool {
        self.permissions.contains(Permission::ReadMemory)
            && self.permissions.contains(Permission::ReadVault)
    }

    fn can_manage(&self) -> bool {
        self.permissions.contains(Permission::ManageMemory)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticEvidenceAccess {
    pub source_id: String,
    pub source_revision_id: String,
    pub parent_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCardRequest {
    /// Stable card reference.
    pub card_id: String,
    /// Optional typed `card` or `composed_card` selector.
    pub card_kind: Option<SemanticCardKind>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticCardKind {
    Card,
    ComposedCard,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticEvidenceRequest {
    /// Stable evidence reference.
    pub evidence_ref_id: String,
    /// Source identity bound by the caller.
    pub source_id: String,
    /// Source revision identity bound by the caller.
    pub source_revision_id: String,
    /// Optional explicit current card/composed-card parent reference.
    pub parent_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticExplicitMemoryType {
    Preference,
    Constraint,
    Decision,
    Experience,
    Procedure,
    State,
}

impl SemanticExplicitMemoryType {
    fn to_memory_type(&self) -> MemoryType {
        match self {
            Self::Preference => MemoryType::Preference,
            Self::Constraint => MemoryType::Constraint,
            Self::Decision => MemoryType::Decision,
            Self::Experience => MemoryType::Experience,
            Self::Procedure => MemoryType::Procedure,
            Self::State => MemoryType::State,
        }
    }

    fn from_memory_type(value: MemoryType) -> Self {
        match value {
            MemoryType::Preference => Self::Preference,
            MemoryType::Constraint => Self::Constraint,
            MemoryType::Decision => Self::Decision,
            MemoryType::Experience => Self::Experience,
            MemoryType::Procedure => Self::Procedure,
            MemoryType::State => Self::State,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticExplicitSourceRequest {
    #[schemars(description = "Source-note path used for navigation and validation.")]
    pub path: String,
    #[schemars(description = "Stable file ID returned by a current source read.")]
    pub file_id: String,
    #[schemars(description = "Current source file revision used as the write precondition.")]
    pub revision: u64,
    #[serde(default)]
    #[schemars(description = "Optional heading coordinates identifying the source location.")]
    pub heading: Vec<String>,
    #[schemars(description = "Optional one-based source span start line.")]
    pub start_line: Option<u32>,
    #[schemars(description = "Optional one-based source span end line.")]
    pub end_line: Option<u32>,
    #[schemars(description = "Optional hash of the cited source excerpt.")]
    pub excerpt_hash: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticRememberExplicitRequest {
    #[schemars(description = "Exact durable memory text to save.")]
    pub content: String,
    #[schemars(description = "Optional explicit memory type.")]
    pub memory_type: Option<SemanticExplicitMemoryType>,
    #[schemars(
        description = "Optional importance score in the supported 0..=1 range.",
        range(min = 0, max = 1)
    )]
    pub importance: Option<f64>,
    #[schemars(
        description = "Optional caller-provided confidence score in the supported 0..=1 range.",
        range(min = 0, max = 1)
    )]
    pub confidence: Option<f64>,
    #[schemars(description = "Optional validity start as a Unix timestamp in seconds.")]
    pub valid_from: Option<i64>,
    #[schemars(description = "Optional validity end as a Unix timestamp in seconds.")]
    pub valid_to: Option<i64>,
    #[serde(default)]
    #[schemars(
        description = "Optional tags for later memory filtering.",
        length(max = 64)
    )]
    pub tags: Vec<String>,
    #[serde(default)]
    #[schemars(
        description = "Optional entities associated with the memory.",
        length(max = 64)
    )]
    pub entities: Vec<String>,
    #[serde(default)]
    #[schemars(
        description = "Optional source bindings obtained from a current source read.",
        length(max = 32)
    )]
    pub sources: Vec<SemanticExplicitSourceRequest>,
    #[schemars(
        description = "Required non-empty key for idempotent retries of the same logical save."
    )]
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticExplicitSourceBindingDto {
    pub source_type: String,
    pub path: Option<String>,
    pub file_id: Option<String>,
    pub revision: Option<u64>,
    pub heading: Vec<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
pub struct SemanticExplicitMemoryDto {
    pub memory_id: String,
    pub ownership: String,
    pub revision: u64,
    pub content: String,
    pub memory_type: Option<SemanticExplicitMemoryType>,
    pub importance: Option<f64>,
    pub confidence: Option<f64>,
    pub valid_from: Option<i64>,
    pub valid_to: Option<i64>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub canonical_file_id: Option<String>,
    pub canonical_path: Option<String>,
    pub canonical_revision: Option<u64>,
    pub source_bindings: Vec<SemanticExplicitSourceBindingDto>,
    pub embedding_eligible: bool,
    pub embedding_binding_present: bool,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
pub struct SemanticRememberExplicitResult {
    pub outcome: String,
    pub memory: SemanticExplicitMemoryDto,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticExplicitListRequest {
    /// Maximum number of current explicit memories. Range 1..=200.
    #[serde(default)]
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<u32>,
    /// Stable memory ID returned as the previous page's continuation cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticExplicitUpdatePatch {
    /// Replacement body. Omission preserves the current body.
    #[serde(default)]
    pub content: Option<String>,
    /// Replacement type. Omission preserves it; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    pub memory_type: Option<Option<SemanticExplicitMemoryType>>,
    /// Replacement importance. Omission preserves it; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    #[schemars(range(min = 0, max = 1))]
    pub importance: Option<Option<f64>>,
    /// Replacement confidence. Omission preserves it; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    #[schemars(range(min = 0, max = 1))]
    pub confidence: Option<Option<f64>>,
    /// Replacement validity start. Omission preserves it; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    pub valid_from: Option<Option<i64>>,
    /// Replacement validity end. Omission preserves it; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    pub valid_to: Option<Option<i64>>,
    /// Complete replacement tag set. Omission preserves it; [] clears it.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub tags: Option<Vec<String>>,
    /// Complete replacement entity set. Omission preserves it; [] clears it.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub entities: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticExplicitUpdateRequest {
    pub memory_id: String,
    pub expected_revision: u64,
    pub patch: SemanticExplicitUpdatePatch,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticExplicitDeleteRequest {
    pub memory_id: String,
    pub expected_revision: u64,
    /// Required stable retry key for the logical deletion request.
    pub idempotency_key: String,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
pub struct SemanticExplicitListResult {
    pub memories: Vec<SemanticExplicitMemoryDto>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticExplicitDeleteResult {
    pub memory_id: String,
    pub deleted: bool,
    pub ownership: String,
    pub source_extraction_paused: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticListCardsRequest {
    /// Maximum number of cards.
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticProcessingStatusRequest {
    /// Maximum organization jobs to return.
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticSourceBindingDto {
    pub source_id: String,
    pub source_revision_id: String,
    pub observation_id: Option<String>,
    pub evidence_ref_id: Option<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticCardDto {
    pub card_id: String,
    pub card_revision_id: String,
    pub title: String,
    pub kind: String,
    pub scope_ref: String,
    pub assertion_status: String,
    pub temporal_scope: Value,
    pub revision_number: u32,
    pub items: Vec<SemanticCardItemDto>,
    pub source_bindings: Vec<SemanticSourceBindingDto>,
    pub canonical_revision: u64,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticCardItemDto {
    pub kind: String,
    pub ordinal: u32,
    pub content: String,
    pub observation_id: String,
    pub evidence_ref_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticComposedCardDto {
    pub composed_card_id: String,
    pub composed_card_revision_id: String,
    pub title: String,
    pub kind: String,
    pub scope_ref: String,
    pub assertion_status: String,
    pub temporal_scope: Value,
    pub revision_number: u32,
    pub items: Vec<SemanticComposedItemDto>,
    pub source_bindings: Vec<SemanticSourceBindingDto>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticComposedItemDto {
    pub kind: String,
    pub ordinal: u32,
    pub content: String,
    pub support_operator: Vec<String>,
    pub source_bindings: Vec<SemanticSourceBindingDto>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticEvidenceDto {
    pub evidence_ref_id: String,
    pub source_id: String,
    pub source_revision_id: String,
    pub body_spans: Vec<SemanticSpanDto>,
    pub context_spans: Vec<SemanticSpanDto>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticSpanDto {
    pub start_byte: u64,
    pub end_byte: u64,
    pub text: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticProcessingJobDto {
    pub job_id: String,
    pub state: String,
    pub lifecycle_state: Option<String>,
    pub safe_error_code: Option<String>,
    pub rules_revision: i64,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticProcessingStatusDto {
    pub extraction_pending_count: u32,
    pub organization_jobs: Vec<SemanticProcessingJobDto>,
    pub rules_revision: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticMutation {
    Correction,
    SuppressRead,
    ForgetCurrent,
    SuppressRegeneration,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticRuleCommand {
    /// Stable semantic target reference.
    pub target_ref: String,
    /// Optional explicit evidence parent reference.
    pub parent_ref: Option<String>,
    /// Optional source-scoped binding.
    pub source_id: Option<String>,
    /// Optional source revision for source-scoped binding.
    pub source_revision_id: Option<String>,
    /// Typed mutation operation.
    pub mutation: SemanticMutation,
    /// Correction or suppression payload.
    pub payload: Option<Value>,
    /// Expected current parent revision.
    pub expected_parent_revision: Option<u32>,
    /// Expected current rules revision.
    pub expected_rules_revision: Option<i64>,
    /// Idempotency key.
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SemanticRuleDto {
    pub rule_kind: String,
    pub id: String,
    pub target_ref: String,
    pub action: String,
    pub scope_ref: String,
    pub active: bool,
    pub revision: i64,
    pub rules_revision: i64,
}

#[derive(Clone)]
pub struct SemanticPublicFacade {
    state: StateStore,
}

impl SemanticPublicFacade {
    pub fn new(state: StateStore) -> Self {
        Self { state }
    }

    pub async fn build_pack(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        access: &SemanticAccess,
        request: &MemoryPackRequest,
    ) -> Result<MemoryPack, MemoryError> {
        ensure_read(access)?;
        MemoryPackService::new(self.state.clone())
            .build(context, core, request)
            .await
    }

    pub async fn list_cards(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        access: &SemanticAccess,
        limit: u32,
    ) -> Result<(Vec<SemanticCardDto>, Vec<SemanticComposedCardDto>), MemoryError> {
        ensure_read(access)?;
        let cards = SemanticMemoryService::new(self.state.clone())
            .list_cards(context, core, limit)
            .await?;
        let composed =
            crate::semantic::organize::SemanticOrganizationService::new(self.state.clone())
                .list_composed_cards(context, core, limit)
                .await?;
        Ok((
            cards.into_iter().map(card_dto).collect(),
            composed.into_iter().map(composed_dto).collect(),
        ))
    }

    pub async fn get_card(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        access: &SemanticAccess,
        card_id: &str,
    ) -> Result<Option<SemanticCardDto>, MemoryError> {
        ensure_read(access)?;
        let id = card_id
            .parse()
            .map_err(|_| MemoryError::InvalidInput("card reference is invalid"))?;
        let Some(card) = SemanticMemoryService::new(self.state.clone())
            .get_card(context, core, id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(card_dto(card)))
    }

    pub async fn get_composed_card(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        access: &SemanticAccess,
        card_id: &str,
    ) -> Result<Option<SemanticComposedCardDto>, MemoryError> {
        ensure_read(access)?;
        let id = card_id
            .parse()
            .map_err(|_| MemoryError::InvalidInput("composed card reference is invalid"))?;
        let Some(card) =
            crate::semantic::organize::SemanticOrganizationService::new(self.state.clone())
                .get_composed_card(context, core, id)
                .await?
        else {
            return Ok(None);
        };
        Ok(Some(composed_dto(card)))
    }

    pub async fn read_evidence(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        access: &SemanticAccess,
        evidence_access: &SemanticEvidenceAccess,
        evidence_ref_id: &str,
    ) -> Result<Option<SemanticEvidenceDto>, MemoryError> {
        ensure_read(access)?;
        let id = evidence_ref_id
            .parse()
            .map_err(|_| MemoryError::InvalidInput("evidence reference is invalid"))?;
        let resolved = self
            .state
            .resolve_semantic_target(
                context,
                &format!("evidence:{evidence_ref_id}"),
                evidence_access.parent_ref.as_deref(),
            )
            .await
            .map_err(map_state_error)?;
        if resolved.is_none() {
            return Ok(None);
        }
        let Some(record) = self
            .state
            .semantic_memory()
            .get_evidence(context, id)
            .await?
        else {
            return Ok(None);
        };
        if record.source_id.to_string() != evidence_access.source_id
            || record.source_revision_id.to_string() != evidence_access.source_revision_id
        {
            return Err(MemoryError::AccessDenied);
        }
        let Some(evidence) = SemanticMemoryService::new(self.state.clone())
            .read_evidence(context, core, id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(evidence_dto(
            evidence_ref_id,
            evidence_access,
            evidence,
        )))
    }

    pub async fn processing_status(
        &self,
        context: &VaultContext,
        access: &SemanticAccess,
        limit: u32,
    ) -> Result<SemanticProcessingStatusDto, MemoryError> {
        ensure_read(access)?;
        if limit == 0 || limit > 200 {
            return Err(MemoryError::InvalidInput(
                "semantic processing limit is invalid",
            ));
        }
        let pending = self
            .state
            .semantic_memory()
            .pending_extractions(context)
            .await
            .map_err(map_state_error)?;
        let jobs = self
            .state
            .semantic_organization()
            .list_jobs(context, limit)
            .await
            .map_err(map_state_error)?;
        Ok(SemanticProcessingStatusDto {
            extraction_pending_count: pending.len() as u32,
            organization_jobs: jobs
                .into_iter()
                .map(|job| SemanticProcessingJobDto {
                    job_id: job.id.to_string(),
                    state: job.state,
                    lifecycle_state: Some(job.lifecycle_state),
                    safe_error_code: job.safe_error_code,
                    rules_revision: job.rules_revision,
                })
                .collect(),
            rules_revision: self
                .state
                .semantic_rules()
                .current_rules_revision(context)
                .await
                .map_err(map_state_error)?,
        })
    }

    pub async fn apply_rule(
        &self,
        context: &VaultContext,
        access: &SemanticAccess,
        actor: &SemanticActor,
        command: &SemanticRuleCommand,
    ) -> Result<SemanticRuleDto, MemoryError> {
        if !access.can_manage() {
            return Err(MemoryError::AccessDenied);
        }
        if command.idempotency_key.trim().is_empty() {
            return Err(MemoryError::InvalidInput("idempotency key is required"));
        }
        let resolved = if let Some(parent_ref) = command.parent_ref.as_deref() {
            self.state
                .resolve_semantic_target(context, &command.target_ref, Some(parent_ref))
                .await
        } else {
            self.state
                .resolve_semantic_target(context, &command.target_ref, None)
                .await
        }
        .map_err(map_state_error)?;
        let Some(resolved) = resolved else {
            return Err(MemoryError::NotFound);
        };
        if let Some(expected) = command.expected_parent_revision
            && resolved.parent_revision != i64::from(expected)
        {
            return Err(MemoryError::Conflict);
        }
        let binding = command.source_id.as_ref().map(|source_id| {
            resolved.source_bindings.iter().find(|value| {
                &value.source_id == source_id
                    && command
                        .source_revision_id
                        .as_deref()
                        .is_none_or(|revision| revision == value.source_revision_id)
            })
        });
        if command.source_id.is_some() && binding.flatten().is_none() {
            return Err(MemoryError::Conflict);
        }
        let source_id = binding.flatten().map(|value| value.source_id.as_str());
        let source_revision_id = binding
            .flatten()
            .map(|value| value.source_revision_id.as_str());
        let observation_id = binding
            .flatten()
            .and_then(|value| value.observation_id.as_deref());
        let payload = command.payload.clone().unwrap_or_else(|| json!({}));
        let result = match command.mutation {
            SemanticMutation::Correction => {
                if command.payload.is_none() {
                    return Err(MemoryError::InvalidInput("correction payload is required"));
                }
                self.state
                    .semantic_rules()
                    .apply_correction(
                        context,
                        &resolved.target_kind,
                        &resolved.scope_ref,
                        resolved.fingerprint_version,
                        &resolved.fingerprint,
                        &payload,
                        source_id,
                        source_revision_id,
                        observation_id,
                        resolved.card_id.as_deref(),
                        resolved.composed_card_id.as_deref(),
                        command.expected_rules_revision,
                        &actor.0,
                        &command.idempotency_key,
                    )
                    .await
                    .map_err(map_state_error)?
            }
            SemanticMutation::SuppressRead
            | SemanticMutation::ForgetCurrent
            | SemanticMutation::SuppressRegeneration => self
                .state
                .semantic_rules()
                .apply_suppression(
                    context,
                    &resolved.target_kind,
                    &resolved.scope_ref,
                    resolved.fingerprint_version,
                    &resolved.fingerprint,
                    &payload,
                    mutation_action(&command.mutation),
                    source_id,
                    source_revision_id,
                    observation_id,
                    resolved.card_id.as_deref(),
                    resolved.composed_card_id.as_deref(),
                    command.expected_rules_revision,
                    &actor.0,
                    &command.idempotency_key,
                )
                .await
                .map_err(map_state_error)?,
        };
        Ok(SemanticRuleDto {
            rule_kind: result.rule_kind,
            id: result.id,
            target_ref: command.target_ref.clone(),
            action: result.action,
            scope_ref: result.scope_ref,
            active: result.active,
            revision: result.revision,
            rules_revision: result.rules_revision,
        })
    }
}

/// Protocol-neutral adapter for the existing v3 explicit-memory ownership and
/// canonical-Markdown write path. Its result is intentionally not a semantic
/// card, evidence record, or MemoryPack entry.
#[derive(Clone)]
pub struct SemanticExplicitFacade {
    memory: MemoryService,
}

impl SemanticExplicitFacade {
    pub fn new(memory: MemoryService) -> Self {
        Self { memory }
    }

    async fn embedding_binding_present(&self, context: &VaultContext) -> Result<bool, MemoryError> {
        let Some(binding) = self
            .memory
            .state()
            .providers()
            .resolve_binding(context, "embedding_memory")
            .await?
        else {
            return Ok(false);
        };
        match self
            .memory
            .providers
            .embeddings()
            .profile_hash(binding.model_id)
            .await
        {
            Ok(_) => Ok(true),
            Err(mcp_vault_providers::ProviderError::ModelCapabilityMismatch {
                capability: "embeddings",
            }) => Ok(false),
            Err(error) => Err(MemoryError::Provider(error)),
        }
    }

    /// List only current, user-owned explicit memories in stable ID order.
    ///
    /// The cursor is deliberately just the last returned memory ID. The v3
    /// repository applies the Vault predicate and explicit-ownership filter;
    /// note-derived and foreign-Vault IDs therefore cannot be observed here.
    pub async fn list(
        &self,
        context: &VaultContext,
        request: &SemanticExplicitListRequest,
    ) -> Result<SemanticExplicitListResult, MemoryError> {
        let limit = request.limit.unwrap_or(50);
        if limit == 0 || limit > 200 {
            return Err(MemoryError::InvalidInput(
                "explicit memory list limit is invalid",
            ));
        }
        let after_id = request
            .cursor
            .as_deref()
            .map(|value| {
                value
                    .parse::<MemoryId>()
                    .map_err(|_| MemoryError::InvalidInput("explicit memory cursor is invalid"))
            })
            .transpose()?;
        let memories = self
            .memory
            .list_with_access(
                context,
                Vec::new(),
                None,
                None,
                None,
                limit,
                0,
                after_id,
                MemoryReadAccess::ExplicitOnly,
            )
            .await?;
        let embedding_binding_present = self.embedding_binding_present(context).await?;
        let truncated = memories.len() == limit as usize;
        let next_cursor = truncated
            .then(|| memories.last().map(|memory| memory.id.to_string()))
            .flatten();
        Ok(SemanticExplicitListResult {
            memories: memories
                .into_iter()
                .map(|memory| explicit_memory_dto(memory, embedding_binding_present))
                .collect(),
            next_cursor,
            truncated,
        })
    }

    /// Fetch one current explicit memory. Missing, foreign-Vault, and
    /// note-derived identities are intentionally all represented as `None`.
    pub async fn get(
        &self,
        context: &VaultContext,
        memory_id: &str,
    ) -> Result<Option<SemanticExplicitMemoryDto>, MemoryError> {
        let id = memory_id
            .parse::<MemoryId>()
            .map_err(|_| MemoryError::InvalidInput("explicit memory id is invalid"))?;
        let memory = match self
            .memory
            .get_with_access(context, id, MemoryReadAccess::ExplicitOnly)
            .await
        {
            Ok(memory) => memory,
            Err(MemoryError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        let embedding_binding_present = self.embedding_binding_present(context).await?;
        Ok(Some(explicit_memory_dto(memory, embedding_binding_present)))
    }

    /// Apply a revision-fenced explicit-memory patch through the v3 service.
    /// Actor and source-plane are supplied by the protocol adapter so the
    /// boundary does not invent caller identity; the existing v3 update path
    /// remains the sole canonical write transaction.
    pub async fn update(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        actor: Actor,
        source_plane: SourcePlane,
        request: &SemanticExplicitUpdateRequest,
    ) -> Result<SemanticExplicitMemoryDto, MemoryError> {
        let id = request
            .memory_id
            .parse::<MemoryId>()
            .map_err(|_| MemoryError::InvalidInput("explicit memory id is invalid"))?;
        // This preflight keeps note-derived, foreign-Vault, and missing IDs on
        // the same not-found boundary before the v3 CAS write is attempted.
        self.memory
            .get_with_access(context, id, MemoryReadAccess::ExplicitOnly)
            .await
            .map_err(|error| match error {
                MemoryError::NotFound => MemoryError::NotFound,
                other => other,
            })?;
        let patch = MemoryUpdateInput {
            content: request.patch.content.clone(),
            memory_type: request.patch.memory_type.as_ref().map(|value| {
                value
                    .as_ref()
                    .map(SemanticExplicitMemoryType::to_memory_type)
            }),
            importance: request.patch.importance,
            confidence: request.patch.confidence,
            valid_from: request.patch.valid_from,
            valid_to: request.patch.valid_to,
            tags: request.patch.tags.clone(),
            entities: request.patch.entities.clone(),
        };
        let memory = self
            .memory
            .update_as(
                context,
                core,
                id,
                Revision::new(request.expected_revision),
                patch,
                actor,
                source_plane,
            )
            .await?;
        let embedding_binding_present = self.embedding_binding_present(context).await?;
        Ok(explicit_memory_dto(memory, embedding_binding_present))
    }

    /// Delete one current explicit memory with an adapter-provided actor,
    /// source plane, and expected revision. The idempotency key is validated at
    /// this protocol-neutral boundary; the v3 delete transaction remains the
    /// source of truth for CAS and canonical-file removal.
    pub async fn delete(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        actor: Actor,
        source_plane: SourcePlane,
        request: &SemanticExplicitDeleteRequest,
    ) -> Result<SemanticExplicitDeleteResult, MemoryError> {
        validate_explicit_idempotency_key(&request.idempotency_key)?;
        let id = request
            .memory_id
            .parse::<MemoryId>()
            .map_err(|_| MemoryError::InvalidInput("explicit memory id is invalid"))?;
        let result = self
            .memory
            .forget_explicit_as(
                context,
                core,
                id,
                Revision::new(request.expected_revision),
                actor,
                source_plane,
                Some(&request.idempotency_key),
            )
            .await?;
        Ok(SemanticExplicitDeleteResult {
            memory_id: result.id.to_string(),
            deleted: result.deleted,
            ownership: "explicit".to_owned(),
            source_extraction_paused: result.source_extraction_paused,
        })
    }

    pub async fn remember(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        actor: Actor,
        source_plane: SourcePlane,
        origin: MemoryOrigin,
        request: SemanticRememberExplicitRequest,
    ) -> Result<SemanticRememberExplicitResult, MemoryError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(MemoryError::InvalidInput(
                "explicit idempotency_key must not be empty",
            ));
        }
        let embedding_binding_present = self.embedding_binding_present(context).await?;
        let sources = request
            .sources
            .into_iter()
            .map(source_input)
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .memory
            .remember_as(
                context,
                core,
                actor,
                source_plane,
                RememberInput {
                    content: request.content,
                    memory_type: request
                        .memory_type
                        .as_ref()
                        .map(SemanticExplicitMemoryType::to_memory_type),
                    importance: request.importance,
                    confidence: request.confidence,
                    valid_from: request.valid_from,
                    valid_to: request.valid_to,
                    tags: request.tags,
                    entities: request.entities,
                    sources,
                    idempotency_key: Some(request.idempotency_key),
                    origin,
                    extraction: Value::Object(Default::default()),
                },
            )
            .await?;
        let memory = result.memory.ok_or(MemoryError::Conflict)?;
        let (canonical_file_id, canonical_revision) =
            if let Some(path) = memory.canonical_path.as_ref() {
                let file = core
                    .read_managed(context, path)
                    .await
                    .map_err(MemoryError::Core)?
                    .file;
                (
                    Some(file.id.to_string()),
                    Some(file.current_revision.value()),
                )
            } else {
                (None, memory.canonical_revision.map(Revision::value))
            };
        let source_bindings = memory
            .sources
            .into_iter()
            .map(|source| SemanticExplicitSourceBindingDto {
                source_type: source.source_type,
                path: source.path.map(|path| path.to_string()),
                file_id: source.file_id.map(|id| id.to_string()),
                revision: source.revision.map(Revision::value),
                heading: source.heading,
                start_line: source.start_line,
                end_line: source.end_line,
            })
            .collect();
        Ok(SemanticRememberExplicitResult {
            outcome: result.outcome,
            memory: SemanticExplicitMemoryDto {
                memory_id: memory.id.to_string(),
                ownership: "explicit".to_owned(),
                revision: memory.revision.value(),
                content: memory.content,
                memory_type: memory
                    .memory_type
                    .map(SemanticExplicitMemoryType::from_memory_type),
                importance: memory.importance,
                confidence: memory.confidence,
                valid_from: memory.valid_from,
                valid_to: memory.valid_to,
                tags: memory.tags,
                entities: memory.entities,
                canonical_file_id,
                canonical_path: memory.canonical_path.map(|path| path.to_string()),
                canonical_revision,
                source_bindings,
                embedding_eligible: true,
                embedding_binding_present,
            },
        })
    }
}

fn deserialize_patch_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value = Option::<T>::deserialize(deserializer)?;
    Ok(Some(value))
}

fn source_input(source: SemanticExplicitSourceRequest) -> Result<MemorySourceInput, MemoryError> {
    let path = VaultPath::parse(&source.path)
        .map_err(|_| MemoryError::InvalidInput("explicit source path is invalid"))?;
    let file_id = source
        .file_id
        .parse::<FileId>()
        .map_err(|_| MemoryError::InvalidInput("explicit source file id is invalid"))?;
    Ok(MemorySourceInput {
        source_type: "note".to_owned(),
        note_file_id: Some(file_id),
        note_path: Some(path),
        note_revision: Some(Revision::new(source.revision)),
        heading_path: source.heading,
        start_line: source.start_line,
        end_line: source.end_line,
        excerpt_hash: source.excerpt_hash,
        actor_id: None,
    })
}

fn validate_explicit_idempotency_key(key: &str) -> Result<(), MemoryError> {
    if key.trim().is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(MemoryError::InvalidInput(
            "explicit idempotency_key must not be empty",
        ));
    }
    Ok(())
}

fn explicit_memory_dto(
    memory: crate::MemoryUnit,
    embedding_binding_present: bool,
) -> SemanticExplicitMemoryDto {
    let source_bindings = memory
        .sources
        .into_iter()
        .map(|source| SemanticExplicitSourceBindingDto {
            source_type: source.source_type,
            path: source.path.map(|path| path.to_string()),
            file_id: source.file_id.map(|id| id.to_string()),
            revision: source.revision.map(Revision::value),
            heading: source.heading,
            start_line: source.start_line,
            end_line: source.end_line,
        })
        .collect();
    SemanticExplicitMemoryDto {
        memory_id: memory.id.to_string(),
        ownership: "explicit".to_owned(),
        revision: memory.revision.value(),
        content: memory.content,
        memory_type: memory
            .memory_type
            .map(SemanticExplicitMemoryType::from_memory_type),
        importance: memory.importance,
        confidence: memory.confidence,
        valid_from: memory.valid_from,
        valid_to: memory.valid_to,
        tags: memory.tags,
        entities: memory.entities,
        // `MemoryUnit` intentionally carries the canonical path/revision but
        // not a duplicated FileRecord. Adapters that need the stable File ID
        // already have Vault Core and can resolve this navigation field.
        canonical_file_id: None,
        canonical_path: memory.canonical_path.map(|path| path.to_string()),
        canonical_revision: memory.canonical_revision.map(Revision::value),
        source_bindings,
        embedding_eligible: true,
        embedding_binding_present,
    }
}

fn ensure_read(access: &SemanticAccess) -> Result<(), MemoryError> {
    if access.can_read() {
        Ok(())
    } else {
        Err(MemoryError::AccessDenied)
    }
}

fn map_state_error(error: mcp_vault_state::StateError) -> MemoryError {
    match error {
        mcp_vault_state::StateError::Conflict => MemoryError::Conflict,
        mcp_vault_state::StateError::InvalidInput(message) => MemoryError::InvalidInput(message),
        other => MemoryError::State(other),
    }
}

fn mutation_action(mutation: &SemanticMutation) -> &'static str {
    match mutation {
        SemanticMutation::SuppressRead => "suppress_read",
        SemanticMutation::ForgetCurrent => "forget_current",
        SemanticMutation::SuppressRegeneration => "suppress_regeneration",
        SemanticMutation::Correction => "replace",
    }
}

fn card_dto(card: SemanticCardRecord) -> SemanticCardDto {
    let mut bindings = Vec::new();
    let items = card
        .items
        .into_iter()
        .map(|item| {
            bindings.push(SemanticSourceBindingDto {
                source_id: card.source_id.to_string(),
                source_revision_id: card.source_revision_id.to_string(),
                observation_id: Some(item.observation_id.to_string()),
                evidence_ref_id: item.evidence_ref_ids.first().map(ToString::to_string),
            });
            SemanticCardItemDto {
                kind: item.kind,
                ordinal: item.ordinal,
                content: item.content,
                observation_id: item.observation_id.to_string(),
                evidence_ref_ids: item
                    .evidence_ref_ids
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
            }
        })
        .collect();
    SemanticCardDto {
        card_id: card.id.to_string(),
        card_revision_id: card.revision_id.to_string(),
        title: card.title,
        kind: card.kind,
        scope_ref: card.scope_ref,
        assertion_status: card.assertion_status,
        temporal_scope: card.temporal_scope,
        revision_number: card.revision_number,
        items,
        source_bindings: bindings,
        canonical_revision: card.canonical_file_revision.value(),
    }
}

fn composed_dto(card: ComposedCardRecord) -> SemanticComposedCardDto {
    let mut all_bindings = Vec::new();
    let items = card
        .items
        .into_iter()
        .map(|item| {
            let mut bindings = Vec::new();
            let mut operators = Vec::new();
            for group in item.support_groups {
                operators.push(group.operator);
                for member in group.members {
                    let binding = SemanticSourceBindingDto {
                        source_id: member.source_id.to_string(),
                        source_revision_id: member.source_revision_id.to_string(),
                        observation_id: Some(member.observation_id.to_string()),
                        evidence_ref_id: Some(member.evidence_ref_id.to_string()),
                    };
                    bindings.push(binding.clone());
                    all_bindings.push(binding);
                }
            }
            SemanticComposedItemDto {
                kind: item.kind,
                ordinal: item.ordinal,
                content: item.content,
                support_operator: operators,
                source_bindings: bindings,
            }
        })
        .collect();
    SemanticComposedCardDto {
        composed_card_id: card.id.to_string(),
        composed_card_revision_id: card.revision_id.to_string(),
        title: card.title,
        kind: card.kind,
        scope_ref: card.scope_ref,
        assertion_status: card.assertion_status,
        temporal_scope: card.temporal_scope,
        revision_number: card.revision_number,
        items,
        source_bindings: all_bindings,
    }
}

fn evidence_dto(
    evidence_ref_id: &str,
    access: &SemanticEvidenceAccess,
    evidence: crate::semantic::SemanticEvidenceText,
) -> SemanticEvidenceDto {
    let body_spans = evidence
        .body_span_records
        .iter()
        .map(|span| SemanticSpanDto {
            start_byte: span.start_byte,
            end_byte: span.end_byte,
            text: span.text.clone(),
        })
        .collect();
    let context_spans = evidence
        .context_span_records
        .iter()
        .map(|span| SemanticSpanDto {
            start_byte: span.start_byte,
            end_byte: span.end_byte,
            text: span.text.clone(),
        })
        .collect();
    SemanticEvidenceDto {
        evidence_ref_id: evidence_ref_id.to_owned(),
        source_id: access.source_id.clone(),
        source_revision_id: access.source_revision_id.clone(),
        body_spans,
        context_spans,
    }
}
