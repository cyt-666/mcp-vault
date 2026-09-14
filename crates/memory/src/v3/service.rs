//! Memory application commands, extraction, recall, and rebuild orchestration.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, LazyLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use mcp_vault_auth::AuthService;
use mcp_vault_core::{VaultCore, VaultError};
use mcp_vault_domain::{
    Actor, ActorId, FileId, MemoryId, MemorySetId, MemorySetSnapshotId, MemorySourceId, ModelId,
    Revision, SourcePlane, VaultContext, VaultPath, WritePrecondition,
};
use mcp_vault_indexer::{
    IndexService,
    relevance::{calibrated_semantic_rank_score, lexical_relevance},
};
use mcp_vault_providers::{
    EmbeddingInput, EmbeddingRequest, EmbeddingSourceRef, EmbeddingSourceResolver,
    ModelCapabilities, ProviderMode, ProviderService, StructuredGenerationRequest,
    embedding_input_hash,
};
use mcp_vault_state::{
    FileRecord, ModelBindingRecord, ModelRecord, ProviderRecord, StateStore, UnitBundle,
    UnitFilter, UnitOwnership, UnitRecord, UnitSourceRecord, UnitSourceSetRecord,
    UnitSourceSetSnapshotRecord, memory_search_terms,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

use super::canonical as current_markdown;
use super::model::*;
use crate::{MemoryError, markdown};

const MAX_CONTENT_BYTES: usize = 64 * 1024;
const MAX_RECALL_RESULTS: u32 = 100;
const MAX_RECALL_TOKENS: u32 = 32_000;
const EXTRACTION_MAX_OUTPUT_TOKENS: u32 = 8_192;
const EXTRACTION_PROMPT_VERSION: &str = "memory-source-unit-selection-v3-minimal-review-v1";
const MEMORY_EMBEDDING_MAX_INPUT_BYTES: usize = 2_048;
const MEMORY_EMBEDDING_CHUNK_OVERLAP_BYTES: usize = 256;
const MAX_MEMORY_EMBEDDING_CHUNKS: usize = 64;
const MEMORY_EMBEDDING_BATCH_SIZE: usize = 64;
const MEMORY_EMBEDDING_CHUNK_PROFILE: &str = "body-v3";
const MEMORY_ARTIFACT_PAGE_SIZE: u32 = 200;
const EXTRACTION_EVALUATION_PROFILE_VERSION: u32 = 2;
/// Current deterministic extraction/fingerprint pipeline version.
pub const EXTRACTION_PIPELINE_VERSION: u32 = 2;
/// Current memory contract generation used to reject obsolete durable jobs.
pub const MEMORY_CONTRACT_GENERATION: u32 = 4;
const EXTRACTION_POLICY_SETTING: &str = "memory.units.policy";

static OPENAI_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"sk-[A-Za-z0-9_-]{20,}").expect("valid OpenAI key regex"));
static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("valid AWS key regex"));
static BEARER_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i:\bBearer)[ \t]+[A-Za-z0-9._~+/-]{16,}=*").expect("valid bearer token regex")
});
static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(api[_-]?key|token|secret|password)\b(\s*[:=]\s*)([\"']?)[^\s\"']{8,}"#)
        .expect("valid secret assignment regex")
});
static PRIVATE_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----.*?-----END (?:[A-Z0-9 ]+ )?PRIVATE KEY-----",
    )
    .expect("valid private key regex")
});
struct ExtractionRuntime {
    policy: ExtractionPolicy,
    binding: ModelBindingRecord,
    model: ModelRecord,
    profile_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedCurrentItem {
    id: MemoryId,
    ordinal: u32,
    content: String,
    kind: Option<MemoryType>,
    tags: Vec<String>,
    content_hash: String,
    revision: Revision,
    created_at: i64,
    /// Present only for a local deletion. Older generation snapshots omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserved_memory: Option<UnitRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserved_sources: Option<Vec<UnitSourceRecord>>,
    #[serde(default)]
    source_unit: Option<crate::units::SourceUnit>,
    #[serde(default)]
    retrieval_hint: String,
}

type SourceGenerationLocks =
    Arc<Mutex<HashMap<(mcp_vault_domain::VaultId, FileId), std::sync::Weak<Mutex<()>>>>>;

/// Memory application service independent of MCP/Admin protocol adapters.
#[derive(Clone)]
pub struct MemoryService {
    pub(crate) state: StateStore,
    pub(crate) providers: ProviderService,
    vault_write_locks: Arc<Mutex<HashMap<mcp_vault_domain::VaultId, Arc<Mutex<()>>>>>,
    source_generation_locks: SourceGenerationLocks,
}

impl MemoryService {
    /// Construct memory services with the shared encrypted provider boundary.
    pub fn new(state: StateStore, auth: AuthService) -> Self {
        Self {
            providers: ProviderService::new(state.clone(), auth),
            state,
            vault_write_locks: Arc::new(Mutex::new(HashMap::new())),
            source_generation_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Construct memory services with an injected provider boundary.
    pub fn with_provider_service(state: StateStore, providers: ProviderService) -> Self {
        Self {
            state,
            providers,
            vault_write_locks: Arc::new(Mutex::new(HashMap::new())),
            source_generation_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return the underlying state boundary for worker composition only.
    pub fn state(&self) -> &StateStore {
        &self.state
    }

    async fn vault_write_lock(&self, context: &VaultContext) -> Arc<Mutex<()>> {
        let mut locks = self.vault_write_locks.lock().await;
        locks
            .entry(context.id())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn source_generation_lock(&self, context: &VaultContext, file: FileId) -> Arc<Mutex<()>> {
        let mut locks = self.source_generation_locks.lock().await;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks
            .get(&(context.id(), file))
            .and_then(std::sync::Weak::upgrade)
        {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert((context.id(), file), Arc::downgrade(&lock));
        lock
    }

    pub async fn extraction_policy(
        &self,
        context: &VaultContext,
    ) -> Result<ExtractionPolicyState, MemoryError> {
        let Some(setting) = self
            .state
            .settings()
            .get_vault(context, EXTRACTION_POLICY_SETTING)
            .await?
        else {
            return Ok(ExtractionPolicyState {
                policy: ExtractionPolicy::default(),
                revision: None,
            });
        };
        let policy: ExtractionPolicy = serde_json::from_value(setting.value)
            .map_err(|_| MemoryError::InvalidInput("memory extraction policy is invalid"))?;
        validate_extraction_policy(&policy)?;
        Ok(ExtractionPolicyState {
            policy,
            revision: Some(setting.revision),
        })
    }

    /// Persist a typed Vault extraction policy with optimistic concurrency.
    pub async fn set_extraction_policy(
        &self,
        context: &VaultContext,
        policy: ExtractionPolicy,
        expected_revision: Option<Revision>,
        updated_by: Option<&ActorId>,
    ) -> Result<ExtractionPolicyState, MemoryError> {
        validate_extraction_policy(&policy)?;
        let value = serde_json::to_value(&policy)
            .map_err(|_| MemoryError::InvalidInput("memory extraction policy is invalid"))?;
        let precondition = expected_revision.map_or(
            WritePrecondition::Unconditional,
            WritePrecondition::ExactRevision,
        );
        let setting = self
            .state
            .settings()
            .set_vault(
                context,
                EXTRACTION_POLICY_SETTING,
                &value,
                precondition,
                updated_by,
            )
            .await?;
        Ok(ExtractionPolicyState {
            policy,
            revision: Some(setting.revision),
        })
    }

    /// Return redacted readiness for the current one-call extraction path.
    pub async fn extraction_readiness(
        &self,
        context: &VaultContext,
    ) -> Result<ExtractionReadiness, MemoryError> {
        let policy = self.extraction_policy(context).await?.policy;
        let mut readiness = ExtractionReadiness::default();
        if !policy.enabled {
            readiness.blockers.push("extraction_disabled".to_owned());
        }
        if self.providers.provider_mode(context).await? == ProviderMode::Disabled {
            readiness.blockers.push("provider_mode_disabled".to_owned());
        }
        let Some(binding) = self
            .state
            .providers()
            .resolve_binding(context, "memory_extraction")
            .await?
        else {
            readiness.blockers.push("model_binding_missing".to_owned());
            readiness.ready = false;
            return Ok(readiness);
        };
        readiness.model_id = Some(binding.model_id.to_string());
        let Some(model) = self.state.providers().get_model(binding.model_id).await? else {
            readiness.blockers.push("model_missing".to_owned());
            readiness.ready = false;
            return Ok(readiness);
        };
        readiness.provider_id = Some(model.provider_id.to_string());
        readiness.external_model_id = Some(model.external_model_id.clone());
        if !model.enabled {
            readiness.blockers.push("model_disabled".to_owned());
        }
        let Some(provider) = self
            .state
            .providers()
            .get_provider(model.provider_id)
            .await?
        else {
            readiness.blockers.push("provider_missing".to_owned());
            readiness.ready = false;
            return Ok(readiness);
        };
        if !provider.enabled {
            readiness.blockers.push("provider_disabled".to_owned());
        }
        readiness.ready = readiness.blockers.is_empty();
        Ok(readiness)
    }

    /// Resolve the current one-call extraction runtime without exposing secrets.
    async fn extraction_runtime(
        &self,
        context: &VaultContext,
        policy: ExtractionPolicy,
    ) -> Result<ExtractionRuntime, MemoryError> {
        let binding = self
            .state
            .providers()
            .resolve_binding(context, "memory_extraction")
            .await?
            .ok_or(MemoryError::Configuration(
                "memory_extraction_model_unbound",
            ))?;
        let model = self
            .state
            .providers()
            .get_model(binding.model_id)
            .await?
            .ok_or(MemoryError::Configuration(
                "memory_extraction_model_missing",
            ))?;
        let provider = self
            .state
            .providers()
            .get_provider(model.provider_id)
            .await?
            .ok_or(MemoryError::Configuration(
                "memory_extraction_provider_missing",
            ))?;
        let profile_hash = extraction_profile_hash(&policy, &binding, &model, &provider);
        Ok(ExtractionRuntime {
            policy,
            binding,
            model,
            profile_hash,
        })
    }

    /// Compact byte-identical items through the source-set publication boundary.
    pub async fn embedding_status(
        &self,
        context: &VaultContext,
    ) -> Result<MemoryEmbeddingStatusView, MemoryError> {
        let inputs = self.memory_embedding_inputs(context).await?;
        let sources = inputs
            .iter()
            .map(|input| input.source.clone())
            .collect::<Vec<_>>();
        let mut status = MemoryEmbeddingStatusView {
            eligible: u64::try_from(sources.len()).unwrap_or(u64::MAX),
            provider_mode_enabled: self.providers.provider_mode(context).await?
                != ProviderMode::Disabled,
            ..MemoryEmbeddingStatusView::default()
        };
        if !status.provider_mode_enabled {
            status.blockers.push("provider_mode_disabled".to_owned());
        }
        let Some(binding) = self
            .state
            .providers()
            .resolve_binding(context, "embedding_memory")
            .await?
        else {
            status.blockers.push("model_binding_missing".to_owned());
            return Ok(status);
        };
        status.configured = true;
        status.model_id = Some(binding.model_id.to_string());
        let Some(model) = self.state.providers().get_model(binding.model_id).await? else {
            status.blockers.push("model_missing".to_owned());
            return Ok(status);
        };
        status.external_model_id = Some(model.external_model_id);
        if !model.enabled {
            status.blockers.push("model_disabled".to_owned());
        }
        match self
            .state
            .providers()
            .get_provider(model.provider_id)
            .await?
        {
            Some(provider) if provider.enabled => {}
            Some(_) => status.blockers.push("provider_disabled".to_owned()),
            None => status.blockers.push("provider_missing".to_owned()),
        }

        let profile_hash = self
            .providers
            .embeddings()
            .profile_hash(binding.model_id)
            .await?;
        status.profile_hash = Some(profile_hash.clone());
        let expected = inputs
            .iter()
            .map(|input| {
                let source = &input.source;
                (
                    (source.object_id.clone(), source.chunk_key.clone()),
                    (
                        source.content_hash.clone(),
                        embedding_input_hash(&profile_hash, source, &input.text),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut current = HashSet::new();
        for embedding in self
            .memory_embedding_metadata(context, binding.model_id)
            .await?
        {
            let key = (embedding.object_id, embedding.chunk_key);
            if expected.get(&key)
                == Some(&(embedding.content_hash.clone(), embedding.input_hash.clone()))
                && embedding.profile_hash == profile_hash
            {
                current.insert(key);
            } else {
                status.stale = status.stale.saturating_add(1);
            }
        }
        status.current = u64::try_from(current.len()).unwrap_or(u64::MAX);
        if status.current < status.eligible {
            status
                .blockers
                .push("embedding_coverage_incomplete".to_owned());
        }
        Ok(status)
    }

    /// Admit all missing/stale current-memory vectors for the effective model.
    pub async fn schedule_memory_embeddings(
        &self,
        context: &VaultContext,
    ) -> Result<MemoryEmbeddingScheduleReport, MemoryError> {
        if self.providers.provider_mode(context).await? == ProviderMode::Disabled {
            return Err(MemoryError::Configuration(
                "memory_embedding_provider_disabled",
            ));
        }
        let binding = self
            .state
            .providers()
            .resolve_binding(context, "embedding_memory")
            .await?
            .ok_or(MemoryError::Configuration(
                "memory_embedding_model_binding_missing",
            ))?;
        let model = self
            .state
            .providers()
            .get_model(binding.model_id)
            .await?
            .ok_or(MemoryError::Configuration("memory_embedding_model_missing"))?;
        if !model.enabled {
            return Err(MemoryError::Configuration(
                "memory_embedding_model_disabled",
            ));
        }
        let provider = self
            .state
            .providers()
            .get_provider(model.provider_id)
            .await?
            .ok_or(MemoryError::Configuration(
                "memory_embedding_provider_missing",
            ))?;
        if !provider.enabled {
            return Err(MemoryError::Configuration(
                "memory_embedding_provider_disabled",
            ));
        }

        let inputs = self.memory_embedding_inputs(context).await?;
        let sources = inputs
            .iter()
            .map(|input| input.source.clone())
            .collect::<Vec<_>>();
        let profile_hash = self
            .providers
            .embeddings()
            .profile_hash(binding.model_id)
            .await?;
        let expected = inputs
            .iter()
            .map(|input| {
                let source = &input.source;
                (
                    (source.object_id.clone(), source.chunk_key.clone()),
                    (
                        source.content_hash.clone(),
                        embedding_input_hash(&profile_hash, source, &input.text),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let existing = self
            .memory_embedding_metadata(context, binding.model_id)
            .await?;
        let mut retained = HashSet::new();
        let mut pruned = 0_u64;
        for embedding in existing {
            let key = (embedding.object_id.clone(), embedding.chunk_key.clone());
            if expected.get(&key)
                == Some(&(embedding.content_hash.clone(), embedding.input_hash.clone()))
                && embedding.profile_hash == profile_hash
            {
                retained.insert(key);
            } else if self
                .state
                .providers()
                .delete_embedding(context, embedding.id)
                .await?
            {
                pruned = pruned.saturating_add(1);
            }
        }
        let missing = sources
            .into_iter()
            .filter(|source| {
                !retained.contains(&(source.object_id.clone(), source.chunk_key.clone()))
            })
            .collect::<Vec<_>>();
        let mut jobs = 0_u64;
        for batch in missing.chunks(MEMORY_EMBEDDING_BATCH_SIZE) {
            self.providers
                .embeddings()
                .schedule_reembedding(context, binding.model_id, batch)
                .await?;
            jobs = jobs.saturating_add(1);
        }
        Ok(MemoryEmbeddingScheduleReport {
            eligible: u64::try_from(expected.len()).unwrap_or(u64::MAX),
            current: u64::try_from(retained.len()).unwrap_or(u64::MAX),
            queued: u64::try_from(missing.len()).unwrap_or(u64::MAX),
            pruned,
            jobs,
            model_id: Some(binding.model_id.to_string()),
            external_model_id: Some(model.external_model_id),
        })
    }

    /// Explicitly admit all uncovered existing memories for paid Admin backfill.
    pub async fn remember(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        input: RememberInput,
    ) -> Result<RememberResult, MemoryError> {
        self.remember_as(context, core, Actor::system(), SourcePlane::System, input)
            .await
    }

    /// Explicitly create or reinforce one durable memory with audit actor
    /// provenance supplied by the protocol/control-plane adapter.
    pub async fn remember_from_unit_as(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        actor: Actor,
        plane: SourcePlane,
        mut input: RememberInput,
        source: Option<MemoryCopySource>,
    ) -> Result<RememberResult, MemoryError> {
        if let Some(source) = source {
            let current = self
                .get_with_access(context, source.id, MemoryReadAccess::All)
                .await?;
            if current.revision != source.expected_revision {
                return Err(MemoryError::Conflict);
            }
            input.sources = current
                .sources
                .into_iter()
                .map(|source| MemorySourceInput {
                    source_type: source.source_type,
                    note_file_id: source.file_id,
                    note_path: source.path,
                    note_revision: source.revision,
                    heading_path: source.heading,
                    start_line: source.start_line,
                    end_line: source.end_line,
                    ..Default::default()
                })
                .collect();
        }
        self.remember_as(context, core, actor, plane, input).await
    }

    pub async fn remember_as(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        actor: Actor,
        source_plane: SourcePlane,
        mut input: RememberInput,
    ) -> Result<RememberResult, MemoryError> {
        self.ensure_initialized(context).await?;
        validate_remember_input(&input)?;
        input.sources = self
            .normalize_source_inputs(context, core, &input.sources)
            .await?;
        let request_hash = remember_request_hash(&input);
        let source_type = match input.origin {
            MemoryOrigin::ExplicitAgent => "explicit_agent",
            MemoryOrigin::ExplicitAdmin => "explicit_admin",
            MemoryOrigin::Import => "import",
        };
        let vault_write_lock = self.vault_write_lock(context).await;
        let _write_guard = vault_write_lock.lock().await;
        if let Some(key) = input.idempotency_key.as_deref()
            && let Some((existing_hash, memory_id)) = self
                .state
                .memory_units()
                .explicit_idempotency(context, key)
                .await?
        {
            if existing_hash != request_hash {
                return Err(MemoryError::InvalidInput(
                    "idempotency key was already used with another request",
                ));
            }
            let existing = self.get(context, memory_id).await?;
            return Ok(RememberResult {
                outcome: "stored_existing".to_owned(),
                memory: Some(existing),
            });
        }

        let (memory_id, now) = if let Some(key) = input.idempotency_key.as_deref() {
            let reservation = self
                .state
                .memory_units()
                .reserve_explicit(context, key, &request_hash)
                .await?;
            (reservation.memory_id, reservation.created_at)
        } else {
            (MemoryId::new(), now_millis())
        };
        let content = input.content;
        let normalized_content = markdown::normalize_content(&content);
        let canonical_path = current_markdown::explicit_path(core.managed_root(), memory_id)?;
        let sources = self
            .current_sources_from_inputs(
                context,
                memory_id,
                &input.sources,
                source_type,
                actor.actor_id().map(ActorId::as_str),
                now,
            )
            .await?;
        let mut bundle = UnitBundle {
            memory: UnitRecord {
                id: memory_id,
                vault_id: context.id(),
                ownership: UnitOwnership::Explicit,
                note_set_id: None,
                ordinal: None,
                kind: input.memory_type.map(|kind| kind.as_str().to_owned()),
                content_hash: markdown::hash_content(&content),
                content,
                normalized_content: normalized_content.clone(),
                importance: input.importance,
                confidence: input.confidence,
                origin: source_type.to_owned(),
                revision: Revision::new(1),
                canonical_file_id: None,
                canonical_path: Some(canonical_path.clone()),
                canonical_revision: None,
                valid_from: input.valid_from,
                valid_to: input.valid_to,
                tags: deduplicate_strings(input.tags),
                entities: deduplicate_strings(input.entities),
                metadata: redact_json_strings(input.extraction),
                created_at: now,
                updated_at: now,
                last_recalled_at: None,
                recall_count: 0,
            },
            sources,
            note_set: None,
        };
        let bytes = current_markdown::render_explicit(&bundle)?;
        let file = create_or_adopt_current_managed(
            core,
            context,
            &canonical_path,
            &bytes,
            actor,
            source_plane,
            input.idempotency_key.is_some(),
        )
        .await?;
        bundle.memory.canonical_file_id = Some(file.id);
        bundle.memory.canonical_revision = Some(file.current_revision);
        let published = self
            .state
            .memory_units()
            .publish_explicit(
                context,
                &bundle,
                None,
                input
                    .idempotency_key
                    .as_deref()
                    .map(|key| (key, request_hash.as_str())),
            )
            .await?;
        self.schedule_current_embedding(context, &published.memory)
            .await;
        Ok(RememberResult {
            outcome: "stored".to_owned(),
            memory: Some(self.view_from_current_bundle(&published, None, None)),
        })
    }

    /// Fetch one memory and all provenance/relations.
    pub async fn get(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<MemoryUnit, MemoryError> {
        self.get_with_access(context, memory_id, MemoryReadAccess::All)
            .await
    }

    pub async fn get_with_access(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
        access: MemoryReadAccess,
    ) -> Result<MemoryUnit, MemoryError> {
        self.ensure_initialized(context).await?;
        let bundle = self
            .state
            .memory_units()
            .get(context, memory_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        if access == MemoryReadAccess::ExplicitOnly
            && bundle.memory.ownership != UnitOwnership::Explicit
        {
            return Err(MemoryError::NotFound);
        }
        Ok(self.view_from_current_bundle(&bundle, None, None))
    }

    /// List current memory projections with bounded kind/source filters.
    #[allow(clippy::too_many_arguments)]
    pub async fn list(
        &self,
        context: &VaultContext,
        types: Vec<MemoryType>,
        tag: Option<String>,
        entity: Option<String>,
        source_path: Option<String>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MemoryUnit>, MemoryError> {
        self.list_after(
            context,
            types,
            tag,
            entity,
            source_path,
            limit,
            offset,
            None,
        )
        .await
    }

    /// Continue a list by stable identity while other formal objects change.
    #[allow(clippy::too_many_arguments)]
    pub async fn list_after(
        &self,
        context: &VaultContext,
        types: Vec<MemoryType>,
        tag: Option<String>,
        entity: Option<String>,
        source_path: Option<String>,
        limit: u32,
        offset: u32,
        after_id: Option<MemoryId>,
    ) -> Result<Vec<MemoryUnit>, MemoryError> {
        self.list_with_access(
            context,
            types,
            tag,
            entity,
            source_path,
            limit,
            offset,
            after_id,
            MemoryReadAccess::All,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_with_access(
        &self,
        context: &VaultContext,
        types: Vec<MemoryType>,
        tag: Option<String>,
        entity: Option<String>,
        source_path: Option<String>,
        limit: u32,
        offset: u32,
        after_id: Option<MemoryId>,
        access: MemoryReadAccess,
    ) -> Result<Vec<MemoryUnit>, MemoryError> {
        self.ensure_initialized(context).await?;
        let filter = UnitFilter {
            ownership: access_ownership(access),
            after_id,
            kinds: types
                .iter()
                .map(|memory_type| memory_type.as_str().to_owned())
                .collect(),
            tag,
            entity,
            source_path,
            ..UnitFilter::default()
        };
        let memories = self
            .state
            .memory_units()
            .list(context, &filter, limit, offset)
            .await?;
        let mut views = Vec::with_capacity(memories.len());
        for memory in memories {
            if let Some(bundle) = self.state.memory_units().get(context, memory.id).await? {
                views.push(self.view_from_current_bundle(&bundle, None, None));
            }
        }
        Ok(views)
    }

    /// Apply a revision-aware metadata/content update and rematerialize Markdown.
    pub async fn update(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        memory_id: MemoryId,
        expected_revision: Revision,
        patch: MemoryUpdateInput,
    ) -> Result<MemoryUnit, MemoryError> {
        self.ensure_initialized(context).await?;
        let vault_write_lock = self.vault_write_lock(context).await;
        let _write_guard = vault_write_lock.lock().await;
        let mut bundle = self
            .state
            .memory_units()
            .get_unchecked(context, memory_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        if bundle.memory.ownership != UnitOwnership::Explicit {
            return Err(MemoryError::InvalidInput(
                "note-derived memories are replaced by editing their source note",
            ));
        }
        let previous_content_hash = bundle.memory.content_hash.clone();
        if bundle.memory.revision != expected_revision {
            return Err(MemoryError::Conflict);
        }
        if let Some(content) = patch.content {
            validate_content(&content)?;
            bundle.memory.content = content;
            bundle.memory.normalized_content = markdown::normalize_content(&bundle.memory.content);
            bundle.memory.content_hash = markdown::hash_content(&bundle.memory.content);
        }
        if let Some(memory_type) = patch.memory_type {
            bundle.memory.kind = memory_type.map(|kind| kind.as_str().to_owned());
        }
        if let Some(importance) = patch.importance {
            if let Some(importance) = importance {
                validate_score(importance)?;
            }
            bundle.memory.importance = importance;
        }
        if let Some(confidence) = patch.confidence {
            if let Some(confidence) = confidence {
                validate_score(confidence)?;
            }
            bundle.memory.confidence = confidence;
        }
        if let Some(valid_from) = patch.valid_from {
            bundle.memory.valid_from = valid_from;
        }
        if let Some(valid_to) = patch.valid_to {
            bundle.memory.valid_to = valid_to;
        }
        if let Some(tags) = patch.tags {
            bundle.memory.tags = deduplicate_strings(tags);
        }
        if let Some(entities) = patch.entities {
            bundle.memory.entities = deduplicate_strings(entities);
        }
        if let (Some(from), Some(to)) = (bundle.memory.valid_from, bundle.memory.valid_to)
            && from >= to
        {
            return Err(MemoryError::InvalidInput(
                "memory validity range is invalid",
            ));
        }
        bundle.memory.revision = expected_revision
            .next()
            .map_err(|_| MemoryError::InvalidInput("memory revision overflow"))?;
        bundle.memory.updated_at = now_millis();
        if bundle.memory.content_hash != previous_content_hash {
            self.delete_current_memory_vectors(context, memory_id)
                .await?;
        }
        let path = bundle
            .memory
            .canonical_path
            .clone()
            .ok_or(MemoryError::Conflict)?;
        let canonical_revision = bundle
            .memory
            .canonical_revision
            .ok_or(MemoryError::Conflict)?;
        let bytes = current_markdown::render_explicit(&bundle)?;
        let file = replace_or_adopt_current_managed(
            core,
            context,
            &path,
            canonical_revision,
            &bytes,
            Actor::system(),
            SourcePlane::System,
        )
        .await?;
        bundle.memory.canonical_file_id = Some(file.id);
        bundle.memory.canonical_revision = Some(file.current_revision);
        let bundle = self
            .state
            .memory_units()
            .publish_explicit(context, &bundle, Some(expected_revision), None)
            .await?;
        if bundle.memory.content_hash != previous_content_hash {
            self.schedule_current_embedding(context, &bundle.memory)
                .await;
        }
        Ok(self.view_from_current_bundle(&bundle, None, None))
    }

    /// Delete the one current copy of a memory. There is no archive/history
    /// switch: successful deletion makes get/list/recall return no record.
    pub async fn forget(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        memory_id: MemoryId,
        expected_revision: Revision,
    ) -> Result<ForgetResult, MemoryError> {
        self.ensure_initialized(context).await?;
        let vault_write_lock = self.vault_write_lock(context).await;
        let write_guard = vault_write_lock.lock().await;
        let bundle = match self.state.memory_units().get(context, memory_id).await? {
            Some(bundle) => bundle,
            None => {
                let unchecked = self
                    .state
                    .memory_units()
                    .get_unchecked(context, memory_id)
                    .await?
                    .ok_or(MemoryError::NotFound)?;
                if unchecked.memory.revision != expected_revision {
                    return Err(MemoryError::Conflict);
                }
                match unchecked.memory.ownership {
                    UnitOwnership::Explicit => {
                        let path = unchecked
                            .memory
                            .canonical_path
                            .as_ref()
                            .ok_or(MemoryError::Conflict)?;
                        if !matches!(
                            core.read_managed(context, path).await,
                            Err(VaultError::NotFound)
                        ) {
                            return Err(MemoryError::NotFound);
                        }
                        self.delete_current_memory_vectors(context, memory_id)
                            .await?;
                        self.state
                            .memory_units()
                            .delete_explicit_projection(context, memory_id, expected_revision)
                            .await?;
                        return Ok(ForgetResult {
                            id: memory_id,
                            deleted: true,
                            ownership: MemoryOwnership::Explicit,
                            source_extraction_paused: false,
                        });
                    }
                    UnitOwnership::NoteDerived => {
                        let set = unchecked.note_set.ok_or(MemoryError::Conflict)?;
                        let snapshot = self
                            .state
                            .memory_units()
                            .prepared_note_set_snapshot(context, set.source_file_id)
                            .await?
                            .filter(|snapshot| {
                                snapshot.extraction_paused
                                    && snapshot.expected_set_revision == Some(set.set_revision)
                                    && serde_json::from_value::<Vec<PreparedCurrentItem>>(
                                        snapshot.items.clone(),
                                    )
                                    .is_ok_and(|items| {
                                        items.iter().all(|item| item.id != memory_id)
                                    })
                            })
                            .ok_or(MemoryError::NotFound)?;
                        drop(write_guard);
                        self.apply_prepared_note_set(context, core, snapshot, true)
                            .await?;
                        return Ok(ForgetResult {
                            id: memory_id,
                            deleted: true,
                            ownership: MemoryOwnership::NoteDerived,
                            source_extraction_paused: true,
                        });
                    }
                }
            }
        };
        if bundle.memory.revision != expected_revision {
            return Err(MemoryError::Conflict);
        }
        match bundle.memory.ownership {
            UnitOwnership::Explicit => {
                let path = bundle
                    .memory
                    .canonical_path
                    .as_ref()
                    .ok_or(MemoryError::Conflict)?;
                let revision = bundle
                    .memory
                    .canonical_revision
                    .ok_or(MemoryError::Conflict)?;
                self.delete_current_memory_vectors(context, memory_id)
                    .await?;
                core.delete_managed(
                    context,
                    path,
                    revision,
                    Actor::system(),
                    SourcePlane::System,
                    None,
                )
                .await?;
                self.state
                    .memory_units()
                    .delete_explicit_projection(context, memory_id, expected_revision)
                    .await?;
                Ok(ForgetResult {
                    id: memory_id,
                    deleted: true,
                    ownership: MemoryOwnership::Explicit,
                    source_extraction_paused: false,
                })
            }
            UnitOwnership::NoteDerived => {
                let old_set = bundle.note_set.ok_or(MemoryError::Conflict)?;
                let mut remaining = self
                    .state
                    .memory_units()
                    .list_note_set_items(context, old_set.id)
                    .await?;
                remaining.retain(|item| item.memory.id != memory_id);
                let mut updated_set = old_set.clone();
                updated_set.set_revision = old_set
                    .set_revision
                    .next()
                    .map_err(|_| MemoryError::InvalidInput("memory set revision overflow"))?;
                updated_set.extraction_paused = true;
                updated_set.updated_at = now_millis();
                let prepared_items = remaining
                    .iter()
                    .map(|item| PreparedCurrentItem {
                        id: item.memory.id,
                        ordinal: item.memory.ordinal.unwrap_or_default(),
                        content: item.memory.content.clone(),
                        kind: item
                            .memory
                            .kind
                            .as_deref()
                            .and_then(|kind| MemoryType::try_from(kind).ok()),
                        tags: item.memory.tags.clone(),
                        content_hash: item.memory.content_hash.clone(),
                        revision: item.memory.revision,
                        created_at: item.memory.created_at,
                        preserved_memory: Some(item.memory.clone()),
                        preserved_sources: Some(item.sources.clone()),
                        source_unit: None,
                        retrieval_hint: String::new(),
                    })
                    .collect::<Vec<_>>();
                let provisional = current_bundles_from_prepared(
                    context,
                    &updated_set,
                    &prepared_items,
                    updated_set.updated_at,
                );
                let bytes = current_markdown::render_note_set(&updated_set, &provisional)?;
                let provider_id = old_set.provider_id;
                let model_id = old_set.model_id;
                let snapshot = UnitSourceSetSnapshotRecord {
                    id: MemorySetSnapshotId::new(),
                    vault_id: context.id(),
                    note_set_id: old_set.id,
                    source_file_id: old_set.source_file_id,
                    source_path: old_set.source_path.clone(),
                    source_content_hash: old_set.source_content_hash.clone(),
                    source_revision: old_set.source_revision,
                    expected_set_revision: Some(old_set.set_revision),
                    proposed_set_revision: updated_set.set_revision,
                    extraction_paused: true,
                    items: serde_json::to_value(&prepared_items).map_err(|_| {
                        MemoryError::InvalidInput("memory deletion snapshot is invalid")
                    })?,
                    canonical_bytes_hash: current_markdown::hash_bytes(&bytes),
                    canonical_path: old_set.canonical_path.clone(),
                    profile_hash: old_set.profile_hash.clone(),
                    prompt_version: old_set.prompt_version.clone(),
                    provider_id,
                    model_id,
                    status: "prepared".to_owned(),
                    created_at: updated_set.updated_at,
                    applied_at: None,
                };
                self.state
                    .memory_units()
                    .prepare_note_set_snapshot(context, &snapshot)
                    .await?;
                drop(write_guard);
                self.apply_prepared_note_set(context, core, snapshot, false)
                    .await?;
                Ok(ForgetResult {
                    id: memory_id,
                    deleted: true,
                    ownership: MemoryOwnership::NoteDerived,
                    source_extraction_paused: true,
                })
            }
        }
    }

    /// Recall current relevant memory without a query-time generative call.
    pub async fn recall(
        &self,
        context: &VaultContext,
        request: RecallRequest,
    ) -> Result<RecallResult, MemoryError> {
        self.ensure_initialized(context).await?;
        let mut request = request;
        request.include_related_notes &= request.access == MemoryReadAccess::All;
        validate_recall_request(&request)?;
        let filter = UnitFilter {
            ownership: access_ownership(request.access),
            source_path: request.source_path.clone(),
            kinds: request
                .types
                .iter()
                .map(|kind| kind.as_str().to_owned())
                .collect(),
            valid_at: Some(request.valid_at.unwrap_or_else(now_millis)),
            min_importance: Some(request.min_importance),
            ..UnitFilter::default()
        };
        let fts_query = quote_fts_query(&request.query)?;
        let mut scores: HashMap<MemoryId, Score> = HashMap::new();
        let mut memory_candidates = HashSet::new();
        let mut eligible_memories = HashSet::new();
        let mut note_candidate_count = 0;
        let mut note_eligible_count = 0;
        for (rank, hit) in self
            .state
            .memory_units()
            .search_fts(context, &fts_query, &filter, 50)
            .await?
            .into_iter()
            .enumerate()
        {
            memory_candidates.insert(hit.memory.id);
            eligible_memories.insert(hit.memory.id);
            let evidence = lexical_relevance(
                &request.query,
                &hit.memory.content,
                &hit.memory.tags,
                &hit.memory.entities,
            );
            let body = hit
                .memory
                .metadata
                .pointer("/source_unit/body/text")
                .and_then(Value::as_str)
                .unwrap_or(&hit.memory.content);
            let body_evidence =
                lexical_relevance(&request.query, body, &hit.memory.tags, &hit.memory.entities);
            let hint = hit.memory.metadata["retrieval_hint"].as_str().unwrap_or("");
            let hint_evidence = lexical_relevance(&request.query, hint, &[], &[]);
            if evidence.admitted || hint_evidence.admitted {
                let score = scores.entry(hit.memory.id).or_default();
                if evidence.admitted {
                    let weighted_coverage = 0.8 * body_evidence.coverage + 0.2 * evidence.coverage;
                    let (coverage, rank_score) =
                        mcp_vault_indexer::relevance::memory_lexical_contributions(
                            weighted_coverage,
                            rank,
                        );
                    score.add(coverage, "lexical_relevance");
                    score.add(rank_score, "lexical_rrf");
                }
                if hint_evidence.admitted {
                    score.add(0.12 * hint_evidence.coverage, "retrieval_hint");
                }
                score.components.insert("lexical_bm25".into(), hit.rank);
                score.components.insert(
                    "lexical_matched_terms".into(),
                    evidence.matched_terms as f64,
                );
                score
                    .components
                    .insert("lexical_query_terms".into(), evidence.query_terms as f64);
            }
        }

        for (rank, memory) in self
            .state
            .memory_units()
            .search_terms(
                context,
                &request.context.entities,
                &request.context.recent_topics,
                &filter,
                30,
            )
            .await?
            .into_iter()
            .enumerate()
        {
            memory_candidates.insert(memory.id);
            eligible_memories.insert(memory.id);
            let evidence = lexical_relevance(
                &request.query,
                &memory.content,
                &memory.tags,
                &memory.entities,
            );
            if evidence.admitted {
                scores
                    .entry(memory.id)
                    .or_default()
                    .add(0.08 / (rank as f64 + 1.0), "context_rrf");
            }
        }

        let mut degraded = Vec::new();
        if let Some(binding) = self
            .state
            .providers()
            .resolve_binding(context, "embedding_memory")
            .await?
        {
            let model = self
                .state
                .providers()
                .get_model(binding.model_id)
                .await?
                .ok_or(MemoryError::NotFound)?;
            let profile_hash = match self
                .providers
                .embeddings()
                .profile_hash(binding.model_id)
                .await
            {
                Ok(profile_hash) => profile_hash,
                Err(_) => {
                    degraded.push("semantic_profile_unavailable".to_owned());
                    String::new()
                }
            };
            if profile_hash.is_empty() {
                // Lexical and entity retrieval remain available.
            } else {
                {
                    // ADR-0028: benchmark outcomes are diagnostics, not eligibility.
                    let min_cosine = 0.0;
                    match self
                        .providers
                        .embed(
                            context,
                            binding.model_id,
                            &EmbeddingRequest {
                                model: model.external_model_id,
                                inputs: vec![
                                    bounded_memory_embedding_text(&request.query).to_owned(),
                                ],
                            },
                        )
                        .await
                    {
                        Ok(embedding) => {
                            if let Some(query) = embedding.vectors.first() {
                                match self
                                    .providers
                                    .embeddings()
                                    .search(
                                        context,
                                        binding.model_id,
                                        "memory_unit",
                                        query,
                                        u32::try_from(50 * MAX_MEMORY_EMBEDDING_CHUNKS)
                                            .unwrap_or(3_200),
                                    )
                                    .await
                                {
                                    Ok(hits) => {
                                        let mut seen_semantic_memories = HashSet::new();
                                        for hit in hits {
                                            if hit.embedding.object_type != "memory_unit" {
                                                continue;
                                            }
                                            let Ok(memory_id) =
                                                MemoryId::parse(&hit.embedding.object_id)
                                            else {
                                                continue;
                                            };
                                            let Some(bundle) = self
                                                .state
                                                .memory_units()
                                                .get_filtered(context, memory_id, &filter)
                                                .await?
                                            else {
                                                continue;
                                            };
                                            if bundle.memory.content_hash
                                                != hit.embedding.content_hash
                                            {
                                                continue;
                                            }
                                            let Some(input) =
                                                memory_embedding_inputs_for(&bundle.memory)
                                                    .into_iter()
                                                    .find(|input| {
                                                        input.source.chunk_key
                                                            == hit.embedding.chunk_key
                                                    })
                                            else {
                                                continue;
                                            };
                                            let expected_input_hash = embedding_input_hash(
                                                &profile_hash,
                                                &input.source,
                                                &input.text,
                                            );
                                            if hit.embedding.profile_hash != profile_hash
                                                || hit.embedding.input_hash != expected_input_hash
                                            {
                                                continue;
                                            }
                                            memory_candidates.insert(memory_id);
                                            eligible_memories.insert(memory_id);
                                            let rank = seen_semantic_memories.len();
                                            if !seen_semantic_memories.insert(memory_id) {
                                                continue;
                                            }
                                            if let Some(contribution) =
                                                calibrated_semantic_rank_score(
                                                    hit.score, rank, min_cosine,
                                                )
                                            {
                                                let score = scores.entry(memory_id).or_default();
                                                score.add(contribution, "semantic_rrf");
                                                score.components.insert(
                                                    "semantic_object_rank".to_owned(),
                                                    (rank + 1) as f64,
                                                );
                                                score.components.insert(
                                                    "semantic_cosine".to_owned(),
                                                    f64::from(hit.score),
                                                );
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        degraded.push("semantic_index_unavailable".to_owned())
                                    }
                                }
                            }
                        }
                        Err(error) => degraded.push(if error.retryable() {
                            "semantic_provider_unavailable".to_owned()
                        } else {
                            "semantic_provider_not_ready".to_owned()
                        }),
                    }
                }
            }
        } else {
            degraded.push("semantic_provider_unconfigured".to_owned());
        }

        let mut ranked = Vec::new();
        for (memory_id, mut score) in scores {
            let Some(bundle) = self
                .state
                .memory_units()
                .get_filtered(context, memory_id, &filter)
                .await?
            else {
                continue;
            };
            let boost = current_memory_boost(&bundle, &request);
            score.total *= boost;
            score.components.insert("boost".to_owned(), boost);
            score
                .components
                .insert("boost_multiplier".to_owned(), boost);
            ranked.push((bundle, score));
        }
        let ranked = diversify_source_candidates(ranked);
        let available_memory_count = u32::try_from(ranked.len()).unwrap_or(u32::MAX);
        let memory_token_budget = if request.include_related_notes {
            request.max_tokens.saturating_mul(2) / 3
        } else {
            request.max_tokens
        };
        let mut selected = Vec::new();
        let mut deferred_memories = Vec::new();
        let mut seen_content = HashSet::new();
        let mut used_tokens = 0_u32;
        for (rank, (bundle, score)) in ranked.into_iter().enumerate() {
            if !seen_content.insert(presentation_identity(&bundle)) {
                continue;
            }
            let mut budget_bundle = bundle.clone();
            if !request.include_sources {
                budget_bundle.sources.clear();
            }
            let view = self.view_from_current_bundle(
                &budget_bundle,
                Some(score.total),
                request
                    .include_score_breakdown
                    .then(|| score.components.clone()),
            );
            let estimate = estimate_serialized_tokens(&view);
            if used_tokens.saturating_add(estimate) > memory_token_budget {
                deferred_memories.push((rank, bundle, score, estimate));
                continue;
            }
            used_tokens = used_tokens.saturating_add(estimate);
            selected.push((rank, bundle, score));
            if selected.len() as u32 >= request.max_results {
                break;
            }
        }

        let mut related_notes = Vec::new();
        let mut available_related_note_count = 0_u32;
        let mut note_budget_skipped = false;
        let mut used_note_tokens = 0_u32;
        if request.include_related_notes && request.max_related_notes != 0 {
            let index =
                IndexService::with_provider_service(self.state.clone(), self.providers.clone());
            let note_floor = Some(0.0);
            match index
                .retrieve_notes_for_recall_scoped(
                    context,
                    &request.query,
                    note_floor,
                    100,
                    &mcp_vault_indexer::NoteRetrievalScope {
                        source_path: request.source_path.clone(),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(result) => {
                    let scoped_count = result
                        .hits
                        .iter()
                        .filter(|hit| {
                            request
                                .source_path
                                .as_deref()
                                .is_none_or(|path| hit.note.path.as_str() == path)
                        })
                        .count() as u32;
                    note_candidate_count = if request.source_path.is_some() {
                        scoped_count
                    } else {
                        result.candidate_count
                    };
                    note_eligible_count = if request.source_path.is_some() {
                        scoped_count
                    } else {
                        result.eligible_count
                    };
                    available_related_note_count = if request.source_path.is_some() {
                        scoped_count
                    } else {
                        result.available_result_count
                    };
                    degraded.extend(result.degraded);
                    let remaining_budget = request.max_tokens.saturating_sub(used_tokens);
                    for hit in result.hits {
                        if request
                            .source_path
                            .as_deref()
                            .is_some_and(|path| hit.note.path.as_str() != path)
                        {
                            continue;
                        }
                        let view = RelatedNoteView {
                            resource_uri: format!(
                                "vault://note/{}",
                                hit.note
                                    .path
                                    .segments()
                                    .map(|segment| percent_encoding::utf8_percent_encode(
                                        segment,
                                        percent_encoding::NON_ALPHANUMERIC
                                    )
                                    .to_string())
                                    .collect::<Vec<_>>()
                                    .join("/")
                            ),
                            matched_section: hit.matched_section,
                            file_id: hit.note.file_id,
                            path: hit.note.path,
                            revision: hit.note.revision,
                            title: hit.note.title,
                            snippet: hit.note.snippet,
                            tags: hit.note.tags,
                            topic_ids: hit.note.topic_ids,
                            headings: hit.note.headings,
                            score: hit.score,
                            score_breakdown: request
                                .include_score_breakdown
                                .then_some(hit.score_breakdown)
                                .flatten(),
                        };
                        let estimate = estimate_serialized_tokens(&view);
                        if used_note_tokens.saturating_add(estimate) > remaining_budget {
                            note_budget_skipped = true;
                            continue;
                        }
                        used_note_tokens = used_note_tokens.saturating_add(estimate);
                        related_notes.push(view);
                        if related_notes.len() >= request.max_related_notes as usize {
                            break;
                        }
                    }
                }
                Err(_) => degraded.push("related_note_index_unavailable".to_owned()),
            }
        }

        // The 2/3 split is only an initial reservation. Once ordinary-note
        // results have consumed their actual share, retry higher-ranked
        // memories that did not fit the reservation so neither result class
        // strands unused space in the one response budget.
        let mut total_used_tokens = used_tokens.saturating_add(used_note_tokens);
        let mut memory_budget_skipped = false;
        let mut pointers = Vec::new();
        for (rank, bundle, score, estimate) in deferred_memories {
            if selected.len() as u32 >= request.max_results {
                memory_budget_skipped = true;
                break;
            }
            if total_used_tokens.saturating_add(estimate) > request.max_tokens {
                memory_budget_skipped = true;
                if pointers.len() < request.max_results as usize {
                    let view = self.view_from_current_bundle(&bundle, None, None);
                    let pointer = MemoryPointer {
                        id: view.id,
                        revision: view.revision,
                        resource_uri: format!("vault://memory/{}", view.id),
                        sources: view.sources,
                        reason: "complete_unit_exceeds_remaining_budget".into(),
                    };
                    let cost = estimate_serialized_tokens(&pointer);
                    if total_used_tokens.saturating_add(cost) <= request.max_tokens {
                        total_used_tokens += cost;
                        pointers.push(pointer);
                    }
                }
                continue;
            }
            total_used_tokens = total_used_tokens.saturating_add(estimate);
            selected.push((rank, bundle, score));
        }
        selected.sort_by_key(|(rank, _, _)| *rank);
        let mut memories = Vec::with_capacity(selected.len());
        for (_, mut bundle, score) in selected {
            if !request.include_sources {
                bundle.sources.clear();
            }
            memories.push(self.view_from_current_bundle(
                &bundle,
                Some(score.total),
                request.include_score_breakdown.then_some(score.components),
            ));
        }
        degraded.sort();
        degraded.dedup();
        let selected_memory_count = u32::try_from(memories.len()).unwrap_or(u32::MAX);
        let selected_note_count = u32::try_from(related_notes.len()).unwrap_or(u32::MAX);
        let memory_truncated =
            memory_budget_skipped || selected_memory_count < available_memory_count;
        let note_truncated =
            note_budget_skipped || selected_note_count < available_related_note_count;
        let mut result = RecallResult {
            memories,
            pointers,
            related_notes,
            candidate_memory_count: u32::try_from(memory_candidates.len()).unwrap_or(u32::MAX),
            relevant_memory_count: available_memory_count,
            available_result_count: available_memory_count
                .saturating_add(available_related_note_count),
            available_memory_count,
            available_related_note_count,
            truncated: memory_truncated || note_truncated,
            degraded,
            retrieval_profile_hash: markdown::hash_content("source-unit-task-recall-v2"),
            diagnostics: request.include_score_breakdown.then(|| json!({"policy":"source-unit-task-recall-v2","count_scope":"authorized_bounded_candidates","note_candidates":note_candidate_count,"note_eligible":note_eligible_count})),

        };
        let mut dropped_changed = 0_u32;
        let mut current_memories = Vec::with_capacity(result.memories.len());
        for view in std::mem::take(&mut result.memories) {
            let current = self
                .state
                .memory_units()
                .get_filtered(context, view.id, &filter)
                .await?;
            let unchanged = current.as_ref().is_some_and(|bundle| {
                let latest = self.view_from_current_bundle(bundle, None, None);
                latest.revision == view.revision
                    && latest.canonical_revision == view.canonical_revision
                    && latest.content == view.content
            });
            if unchanged {
                current_memories.push(view);
            } else {
                dropped_changed += 1;
            }
        }
        result.memories = current_memories;
        let mut current_pointers = Vec::new();
        for pointer in std::mem::take(&mut result.pointers) {
            if self
                .state
                .memory_units()
                .get_filtered(context, pointer.id, &filter)
                .await?
                .is_some_and(|bundle| bundle.memory.revision == pointer.revision)
            {
                current_pointers.push(pointer);
            } else {
                dropped_changed += 1;
            }
        }
        result.pointers = current_pointers;

        let mut current_notes = Vec::with_capacity(result.related_notes.len());
        for view in std::mem::take(&mut result.related_notes) {
            if self
                .state
                .index()
                .get_note_for_retrieval(context, view.file_id)
                .await?
                .is_some_and(|note| note.revision == view.revision && note.path == view.path)
            {
                current_notes.push(view);
            } else {
                dropped_changed += 1;
            }
        }
        result.related_notes = current_notes;
        if dropped_changed > 0 {
            result
                .degraded
                .push("candidate_changed_before_response".into());
            result.truncated = true;
        }
        if let Some(diagnostics) = result.diagnostics.as_mut() {
            diagnostics["dropped_changed_objects"] = json!(dropped_changed);
        }
        let mut final_budget_limited = memory_budget_skipped || note_budget_skipped;
        loop {
            if let Some(diagnostics) = result.diagnostics.as_mut() {
                diagnostics["returned_memories"] = json!(result.memories.len());
                diagnostics["returned_notes"] = json!(result.related_notes.len());
                diagnostics["returned_pointers"] = json!(result.pointers.len());
                diagnostics["budget_limited"] = json!(final_budget_limited);
            }
            if estimate_serialized_tokens(&result) <= request.max_tokens {
                break;
            }
            final_budget_limited = true;
            result.truncated = true;
            if result.related_notes.pop().is_none()
                && result.pointers.pop().is_none()
                && result.memories.pop().is_none()
            {
                return Err(MemoryError::InvalidInput(
                    "recall budget cannot contain response metadata",
                ));
            }
        }
        let selected_ids = result
            .memories
            .iter()
            .map(|memory| memory.id)
            .collect::<Vec<_>>();
        self.state
            .memory_units()
            .mark_recalled(context, &selected_ids)
            .await?;
        Ok(result)
    }

    #[allow(dead_code)]
    pub async fn extract_note(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
    ) -> Result<NoteExtractionResult, MemoryError> {
        self.extract_note_with_options(context, core, path, NoteExtractionOptions::default())
            .await
    }

    /// Extract one current note, optionally forcing reevaluation of an exact
    /// already-covered source. A manual derived-item deletion remains paused
    /// until [`Self::resume_note_extraction`] is called explicitly.
    pub async fn extract_note_with_options(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        path: &VaultPath,
        options: NoteExtractionOptions,
    ) -> Result<NoteExtractionResult, MemoryError> {
        self.ensure_initialized(context).await?;
        if core.is_managed_path(path) || !path.as_str().to_ascii_lowercase().ends_with(".md") {
            return Ok(NoteExtractionResult::default());
        }
        if self.state.memory_units().runtime(context).await?.paused {
            return Ok(NoteExtractionResult {
                pending_batches: 1,
                ..Default::default()
            });
        }
        let mut read = match core.read(context, path).await {
            Ok(read) => read,
            Err(VaultError::NotFound) => {
                return Err(MemoryError::SourceIngestion("memory_source_not_found"));
            }
            Err(error) => return Err(MemoryError::Core(error)),
        };
        let source_file_id = read.file.id;
        // Intentionally serialize generation I/O for this one source: an
        // incremental job and a backfill must not pay for the same batch.
        tracing::debug!(target:"mcp_vault::memory", event="memory_selection_stage", vault_id=%context.id(), source_file_id=%source_file_id, phase="source_lock_wait");
        let source_lock = self.source_generation_lock(context, source_file_id).await;
        let _source_guard = source_lock.lock().await;
        let source_revision = read.file.current_revision;
        let source_content_hash = read
            .file
            .content_hash
            .clone()
            .ok_or(MemoryError::SourceIngestion("memory_source_hash_missing"))?;
        let policy = self.extraction_policy(context).await?.policy;
        if !policy.enabled {
            return Ok(NoteExtractionResult::default());
        }
        let existing_set = self
            .state
            .memory_units()
            .get_note_set_by_source(context, source_file_id)
            .await?;
        if existing_set
            .as_ref()
            .is_some_and(|set| set.extraction_paused)
        {
            return Ok(NoteExtractionResult {
                already_evaluated: true,
                ..NoteExtractionResult::default()
            });
        }
        tracing::debug!(target:"mcp_vault::memory", event="memory_selection_stage", vault_id=%context.id(), source_file_id=%source_file_id, phase="runtime_lookup");
        let runtime = self.extraction_runtime(context, policy).await?;
        if !options.include_evaluated
            && existing_set.as_ref().is_some_and(|set| {
                set.source_content_hash == source_content_hash
                    && set.profile_hash == runtime.profile_hash
            })
        {
            let item_count = self
                .state
                .memory_units()
                .list_note_set_items(context, existing_set.as_ref().expect("checked above").id)
                .await?
                .len();
            return Ok(NoteExtractionResult {
                already_evaluated: true,
                items_published: u32::try_from(item_count).unwrap_or(u32::MAX),
                ..NoteExtractionResult::default()
            });
        }
        if let Some(prepared) = self
            .state
            .memory_units()
            .prepared_note_set_snapshot(context, source_file_id)
            .await?
        {
            if prepared.profile_hash == runtime.profile_hash
                && prepared.source_content_hash == source_content_hash
                && prepared.source_revision == source_revision
                && prepared.expected_set_revision
                    == existing_set.as_ref().map(|set| set.set_revision)
            {
                return self
                    .apply_prepared_note_set(context, core, prepared, true)
                    .await;
            }
            self.state
                .memory_units()
                .reject_note_set_snapshot(context, prepared.id)
                .await?;
        }

        tracing::debug!(target:"mcp_vault::memory", event="memory_selection_stage", vault_id=%context.id(), source_file_id=%source_file_id, phase="source_read");
        let mut source_bytes = Vec::new();
        (&mut read.reader)
            .take(512 * 1024)
            .read_to_end(&mut source_bytes)
            .await
            .map_err(|_| MemoryError::SourceIngestion("memory_source_read_failed"))?;
        if source_bytes.len() >= 512 * 1024 {
            return Err(MemoryError::SourceIngestion("memory_source_too_large"));
        }
        let source = String::from_utf8(source_bytes)
            .map_err(|_| MemoryError::SourceIngestion("memory_source_not_utf8"))?;
        let capabilities = ModelCapabilities::from_json(&runtime.model.capabilities)?;
        let max_output_tokens = capabilities
            .max_output_tokens
            .map_or(EXTRACTION_MAX_OUTPUT_TOKENS, |limit| {
                limit.min(EXTRACTION_MAX_OUTPUT_TOKENS)
            });
        tracing::debug!(target:"mcp_vault::memory", event="memory_selection_stage", vault_id=%context.id(), source_file_id=%source_file_id, phase="candidate_parse");
        let full_candidates = crate::units::source_units(&source);
        let candidates = crate::units::minimal_selection_units(&source);
        let (batches, skipped) = super::selection::batches(&candidates, path)?;
        let mut skipped_units = skipped.len() as u32;
        let mut skipped_json = json!(skipped);
        tracing::debug!(target:"mcp_vault::memory", event="memory_selection_stage", vault_id=%context.id(), source_file_id=%source_file_id, phase="batches_ready", batches=batches.len(), skipped=skipped.len());
        let mut selections = Vec::new();
        let mut completed_batches = 0_u32;
        let mut dispatched = false;
        for batch in &batches {
            let input_hash = markdown::hash_content(&format!(
                "{}\n{}\n{}\n{}",
                runtime.profile_hash,
                source_content_hash,
                existing_set
                    .as_ref()
                    .map_or(0, |set| set.set_revision.value()),
                batch.user
            ));
            let cached = self
                .state
                .memory_units()
                .selection_batch(context, source_file_id, &source_content_hash, &input_hash)
                .await?;
            let result = if let Some(cached) = cached {
                cached
            } else {
                if dispatched || self.state.memory_units().runtime(context).await?.paused {
                    return Ok(NoteExtractionResult {
                        source_admitted: true,
                        completed_batches,
                        pending_batches: (batches.len() as u32).saturating_sub(completed_batches),
                        skipped_units,
                        ..Default::default()
                    });
                }
                let request = StructuredGenerationRequest {
                    model: runtime.model.external_model_id.clone(),
                    system: super::selection::SYSTEM.to_owned(),
                    user: batch.user.clone(),
                    schema_name: "memory_unit_selection".into(),
                    schema: super::selection::schema(&batch.units),
                    allow_additional_output_properties: false,
                    missing_required_string_fallbacks: Vec::new(),
                    max_output_tokens,
                    temperature: Some(0.0),
                    timeout: Some(Duration::from_secs(runtime.policy.request_timeout_seconds)),
                };
                tracing::debug!(target:"mcp_vault::memory", event="memory_selection_stage", vault_id=%context.id(), source_file_id=%source_file_id, phase="provider_wait", completed_batches);
                let generated = self
                    .providers
                    .generate_structured(context, runtime.binding.model_id, &request)
                    .await?;
                let parsed: crate::units::SelectionOutput =
                    serde_json::from_value(generated.value.clone()).map_err(|_| {
                        MemoryError::GeneratedOutput("memory_selection_output_invalid")
                    })?;
                crate::units::resolve_selection(&batch.units, parsed)?;
                self.state
                    .memory_units()
                    .save_selection_batch(
                        context,
                        source_file_id,
                        &source_content_hash,
                        &input_hash,
                        &generated.value,
                    )
                    .await?;
                dispatched = true;
                generated.value
            };
            let parsed: crate::units::SelectionOutput = serde_json::from_value(result)
                .map_err(|_| MemoryError::GeneratedOutput("memory_selection_checkpoint_invalid"))?;
            crate::units::resolve_selection(&batch.units, parsed.clone())?;
            selections.extend(parsed.selections);
            completed_batches += 1;
            self.state
                .memory_units()
                .save_selection_progress(
                    context,
                    source_file_id,
                    &source_content_hash,
                    &runtime.profile_hash,
                    completed_batches,
                    batches.len() as u32,
                    &skipped_json,
                )
                .await?;
        }
        let first_output = crate::units::SelectionOutput { selections };
        let first_selection_hash =
            markdown::hash_content(&serde_json::to_string(&first_output).map_err(|_| {
                MemoryError::GeneratedOutput("memory_selection_checkpoint_invalid")
            })?);
        let review_plan =
            super::review::build_plan(&full_candidates, &candidates, &first_output, path);
        skipped_units = skipped_units
            .saturating_add(u32::try_from(review_plan.omissions.len()).unwrap_or(u32::MAX));
        if !review_plan.omissions.is_empty()
            && let Some(items) = skipped_json.as_array_mut()
        {
            items.extend(review_plan.omissions.iter().map(
                |item| json!({"unit_id": item.unit_id, "heading": [], "reason": item.reason}),
            ));
        }
        let total_work = u32::try_from(batches.len().saturating_add(review_plan.scopes.len()))
            .unwrap_or(u32::MAX);
        self.state
            .memory_units()
            .save_selection_progress(
                context,
                source_file_id,
                &source_content_hash,
                &runtime.profile_hash,
                completed_batches,
                total_work,
                &skipped_json,
            )
            .await?;
        let mut reviews = Vec::with_capacity(review_plan.scopes.len());
        let mut review_completed = 0_u32;
        for scope in &review_plan.scopes {
            let review_input = super::review::input(scope, path)?;
            let input_hash = markdown::hash_content(&format!(
                "stage=review\nschema={}\nprompt={}\nprofile={}\nsource={}\nfirst={}\nset_revision={}\nscope={}\n{}",
                super::review::REVIEW_SCHEMA_VERSION,
                super::review::SYSTEM,
                runtime.profile_hash,
                source_content_hash,
                first_selection_hash,
                existing_set
                    .as_ref()
                    .map_or(0, |set| set.set_revision.value()),
                scope.id,
                review_input,
            ));
            let cached = self
                .state
                .memory_units()
                .selection_batch(context, source_file_id, &source_content_hash, &input_hash)
                .await?;
            let had_cached = cached.is_some();
            let result = if let Some(cached) = cached {
                cached
            } else {
                if dispatched || self.state.memory_units().runtime(context).await?.paused {
                    return Ok(NoteExtractionResult {
                        source_admitted: true,
                        completed_batches: completed_batches.saturating_add(review_completed),
                        pending_batches: total_work
                            .saturating_sub(completed_batches.saturating_add(review_completed)),
                        skipped_units,
                        ..Default::default()
                    });
                }
                let request = StructuredGenerationRequest {
                    model: runtime.model.external_model_id.clone(),
                    system: super::review::SYSTEM.to_owned(),
                    user: review_input,
                    schema_name: super::review::REVIEW_SCHEMA_VERSION.to_owned(),
                    schema: super::review::schema(scope),
                    allow_additional_output_properties: false,
                    missing_required_string_fallbacks: Vec::new(),
                    max_output_tokens,
                    temperature: Some(0.0),
                    timeout: Some(Duration::from_secs(runtime.policy.request_timeout_seconds)),
                };
                let generated = self
                    .providers
                    .generate_structured(context, runtime.binding.model_id, &request)
                    .await?;
                let parsed: super::review::ReviewOutput =
                    serde_json::from_value(generated.value.clone()).map_err(|_| {
                        MemoryError::GeneratedOutput("memory_selection_review_output_invalid")
                    })?;
                super::review::validate(scope, parsed)?;
                self.state
                    .memory_units()
                    .save_selection_batch(
                        context,
                        source_file_id,
                        &source_content_hash,
                        &input_hash,
                        &generated.value,
                    )
                    .await?;
                dispatched = true;
                generated.value
            };
            let parsed: super::review::ReviewOutput =
                serde_json::from_value(result).map_err(|_| {
                    if had_cached {
                        MemoryError::GeneratedOutput("memory_selection_review_checkpoint_invalid")
                    } else {
                        MemoryError::GeneratedOutput("memory_selection_review_output_invalid")
                    }
                })?;
            reviews.push(super::review::validate(scope, parsed)?);
            review_completed = review_completed.saturating_add(1);
            self.state
                .memory_units()
                .save_selection_progress(
                    context,
                    source_file_id,
                    &source_content_hash,
                    &runtime.profile_hash,
                    completed_batches.saturating_add(review_completed),
                    total_work,
                    &skipped_json,
                )
                .await?;
        }
        let output = super::review::reduce_plan(
            &full_candidates,
            &candidates,
            &first_output,
            &review_plan,
            &reviews,
        )?;
        if self.state.memory_units().runtime(context).await?.paused {
            return Ok(NoteExtractionResult {
                source_admitted: true,
                completed_batches: completed_batches.saturating_add(review_completed),
                pending_batches: 1,
                skipped_units,
                ..Default::default()
            });
        }

        let now = now_millis();
        let existing_items = if let Some(set) = existing_set.as_ref() {
            self.state
                .memory_units()
                .list_note_set_items(context, set.id)
                .await?
        } else {
            Vec::new()
        };
        let mut reusable = existing_items
            .into_iter()
            .map(|bundle| (bundle.memory.content_hash.clone(), bundle.memory))
            .collect::<HashMap<_, _>>();
        let mut prepared_items = Vec::with_capacity(output.len());
        for (index, (unit, item)) in output.into_iter().enumerate() {
            let content = unit.complete_text();
            let content_hash = markdown::hash_content(&content);
            let existing = reusable.remove(&content_hash);
            prepared_items.push(PreparedCurrentItem {
                id: existing.as_ref().map_or_else(MemoryId::new, |item| item.id),
                ordinal: u32::try_from(index)
                    .map_err(|_| MemoryError::GeneratedOutput("memory_set_too_many_items"))?,
                content,
                kind: item
                    .kind
                    .as_deref()
                    .and_then(|kind| MemoryType::try_from(kind).ok()),
                tags: Vec::new(),
                content_hash,
                revision: existing
                    .as_ref()
                    .map(|item| item.revision.next())
                    .transpose()
                    .map_err(|_| MemoryError::InvalidInput("memory revision overflow"))?
                    .unwrap_or(Revision::new(1)),
                created_at: existing.as_ref().map_or(now, |item| item.created_at),
                preserved_memory: None,
                preserved_sources: None,
                source_unit: Some(unit),
                retrieval_hint: item.retrieval_hint,
            });
        }
        let note_set_id = existing_set
            .as_ref()
            .map_or_else(MemorySetId::new, |set| set.id);
        let proposed_set_revision = existing_set
            .as_ref()
            .map(|set| set.set_revision.next())
            .transpose()
            .map_err(|_| MemoryError::InvalidInput("memory set revision overflow"))?
            .unwrap_or(Revision::new(1));
        let canonical_path = current_markdown::note_set_path(core.managed_root(), source_file_id)?;
        let provisional_set = UnitSourceSetRecord {
            id: note_set_id,
            vault_id: context.id(),
            source_file_id,
            source_path: path.clone(),
            source_content_hash: source_content_hash.clone(),
            source_revision,
            set_revision: proposed_set_revision,
            extraction_paused: false,
            canonical_file_id: existing_set
                .as_ref()
                .map_or(source_file_id, |set| set.canonical_file_id),
            canonical_path: canonical_path.clone(),
            canonical_revision: existing_set
                .as_ref()
                .map_or(Revision::new(1), |set| set.canonical_revision),
            profile_hash: runtime.profile_hash.clone(),
            prompt_version: EXTRACTION_PROMPT_VERSION.to_owned(),
            provider_id: Some(runtime.model.provider_id),
            model_id: Some(runtime.model.id),
            created_at: existing_set.as_ref().map_or(now, |set| set.created_at),
            updated_at: now,
        };
        let provisional_bundles =
            current_bundles_from_prepared(context, &provisional_set, &prepared_items, now);
        let canonical_bytes =
            current_markdown::render_note_set(&provisional_set, &provisional_bundles)?;
        let snapshot = UnitSourceSetSnapshotRecord {
            id: MemorySetSnapshotId::new(),
            vault_id: context.id(),
            note_set_id,
            source_file_id,
            source_path: path.clone(),
            source_content_hash,
            source_revision,
            expected_set_revision: existing_set.as_ref().map(|set| set.set_revision),
            proposed_set_revision,
            extraction_paused: false,
            items: serde_json::to_value(&prepared_items)
                .map_err(|_| MemoryError::GeneratedOutput("memory_set_output_invalid"))?,
            canonical_bytes_hash: current_markdown::hash_bytes(&canonical_bytes),
            canonical_path,
            profile_hash: runtime.profile_hash,
            prompt_version: EXTRACTION_PROMPT_VERSION.to_owned(),
            provider_id: Some(runtime.model.provider_id),
            model_id: Some(runtime.model.id),
            status: "prepared".to_owned(),
            created_at: now,
            applied_at: None,
        };
        let lock = self.vault_write_lock(context).await;
        let guard = lock.lock().await;
        let current_file = self
            .state
            .files()
            .get_by_id(context, source_file_id)
            .await?
            .filter(FileRecord::is_active)
            .ok_or(MemoryError::Conflict)?;
        if current_file.path != *path
            || current_file.current_revision != source_revision
            || current_file.content_hash.as_deref() != Some(snapshot.source_content_hash.as_str())
            || self
                .state
                .memory_units()
                .get_note_set_by_source(context, source_file_id)
                .await?
                .as_ref()
                .map(|set| set.set_revision)
                != snapshot.expected_set_revision
        {
            return Err(MemoryError::Conflict);
        }
        self.state
            .memory_units()
            .prepare_note_set_snapshot(context, &snapshot)
            .await?;
        drop(guard);
        let mut result = self
            .apply_prepared_note_set(context, core, snapshot, false)
            .await?;
        result.completed_batches = completed_batches.saturating_add(review_completed);
        result.skipped_units = skipped_units;
        result.empty_set_published &= skipped_units == 0;
        result.source_admitted = !batches.is_empty();
        Ok(result)
    }

    /// Explicitly resume automatic extraction after a manual derived-item
    /// deletion. Resumption is revision-aware and does not itself call a model.
    pub async fn resume_note_extraction(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        source_file_id: FileId,
        expected_set_revision: Revision,
        actor: Actor,
    ) -> Result<mcp_vault_domain::JobId, MemoryError> {
        self.ensure_initialized(context).await?;
        self.state
            .memory_units()
            .get_note_set_by_source(context, source_file_id)
            .await?
            .filter(|set| set.extraction_paused && set.set_revision == expected_set_revision)
            .ok_or(MemoryError::Conflict)?;
        let requested_at = now_millis();
        let intent = self.state.jobs().enqueue(context, "memory.source_resume",
            &format!("source-resume-intent:{}:{}", source_file_id, expected_set_revision.value()),
            &json!({"memory_contract_generation":MEMORY_CONTRACT_GENERATION,"source_file_id":source_file_id,"expected_set_revision":expected_set_revision.value(),"requested_at":requested_at,"actor":actor}),4,5,requested_at).await?;
        // The durable intent precedes canonical mutation. A process/DB failure
        // after rename is completed by the registered worker on restart.
        match self
            .complete_note_extraction_resume(context, core, &intent.payload)
            .await
        {
            Ok(job) => Ok(job),
            Err(error) if error.retryable() => Ok(intent.id),
            Err(error) => Err(error),
        }
    }

    /// Replay an explicitly authorized source resume after a crash. This never
    /// arises from background compensation and never resumes a newer paused set.
    pub async fn complete_note_extraction_resume(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        payload: &Value,
    ) -> Result<mcp_vault_domain::JobId, MemoryError> {
        self.ensure_initialized(context).await?;
        let source_file_id = payload["source_file_id"]
            .as_str()
            .and_then(|value| FileId::parse(value).ok())
            .ok_or(MemoryError::InvalidInput("resume source invalid"))?;
        let expected_set_revision = payload["expected_set_revision"]
            .as_u64()
            .map(Revision::new)
            .ok_or(MemoryError::InvalidInput("resume revision invalid"))?;
        let requested_at = payload["requested_at"]
            .as_i64()
            .ok_or(MemoryError::InvalidInput("resume time invalid"))?;
        let actor: Actor = serde_json::from_value(payload["actor"].clone())
            .map_err(|_| MemoryError::InvalidInput("resume actor invalid"))?;
        let target = expected_set_revision
            .next()
            .map_err(|_| MemoryError::InvalidInput("resume revision overflow"))?;
        if self
            .state
            .memory_units()
            .get_note_set_by_source(context, source_file_id)
            .await?
            .is_some_and(|set| set.set_revision == target && !set.extraction_paused)
        {
            return self
                .state
                .jobs()
                .find_by_dedup(
                    context,
                    &format!(
                        "vault:{}:source-resume:{}:{}",
                        context.id(),
                        source_file_id,
                        target.value()
                    ),
                )
                .await?
                .map(|job| job.id)
                .ok_or(MemoryError::Conflict);
        }
        let lock = self.vault_write_lock(context).await;
        let _guard = lock.lock().await;
        let source = self
            .state
            .files()
            .get_by_id(context, source_file_id)
            .await?
            .filter(|file| file.is_active() && file.path.as_str().to_lowercase().ends_with(".md"))
            .ok_or(MemoryError::Conflict)?;
        let old_set = self
            .state
            .memory_units()
            .get_note_set_by_source(context, source_file_id)
            .await?
            .filter(|set| set.extraction_paused && set.set_revision == expected_set_revision)
            .ok_or(MemoryError::Conflict)?;
        let items = self
            .state
            .memory_units()
            .list_note_set_items(context, old_set.id)
            .await?;
        let mut updated_set = old_set.clone();
        updated_set.extraction_paused = false;
        updated_set.set_revision = expected_set_revision
            .next()
            .map_err(|_| MemoryError::InvalidInput("memory set revision overflow"))?;
        updated_set.updated_at = requested_at;
        let bytes = current_markdown::render_note_set(&updated_set, &items)?;
        let file = replace_or_adopt_current_managed(
            core,
            context,
            &old_set.canonical_path,
            old_set.canonical_revision,
            &bytes,
            actor,
            SourcePlane::Admin,
        )
        .await?;
        updated_set.canonical_file_id = file.id;
        updated_set.canonical_revision = file.current_revision;
        let job=self.state.memory_units().resume_note_extraction(context, &updated_set, expected_set_revision,
            &json!({"memory_contract_generation":MEMORY_CONTRACT_GENERATION,"pipeline_version":EXTRACTION_PIPELINE_VERSION,
                "path":source.path.as_str(),"reason":"admin_source_resume","include_evaluated":true})).await?;
        Ok(job)
    }

    /// Reconcile one current note set from authoritative file metadata without
    /// scanning note bodies or invoking a Provider. Content changes/deletion
    /// fail closed through repository joins; same-ID/same-hash moves only
    /// update navigation metadata.
    pub async fn reconcile_current_source_event(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        source_file_id: FileId,
    ) -> Result<CurrentSourceReconcileReport, MemoryError> {
        self.ensure_initialized(context).await?;
        let Some(set) = self
            .state
            .memory_units()
            .get_note_set_by_source(context, source_file_id)
            .await?
        else {
            return Ok(CurrentSourceReconcileReport::default());
        };
        let mut report = CurrentSourceReconcileReport {
            sources_checked: 1,
            ..CurrentSourceReconcileReport::default()
        };
        let file = self
            .state
            .files()
            .get_by_id(context, source_file_id)
            .await?;
        let Some(file) = file.filter(FileRecord::is_active) else {
            let lock = self.vault_write_lock(context).await;
            let _guard = lock.lock().await;

            if self
                .state
                .memory_units()
                .get_note_set_by_source(context, source_file_id)
                .await?
                .as_ref()
                .map(|s| s.set_revision)
                != Some(set.set_revision)
            {
                return Err(MemoryError::Conflict);
            }
            let set = self
                .state
                .memory_units()
                .get_note_set_by_source(context, source_file_id)
                .await?
                .filter(|current| current.set_revision == set.set_revision)
                .ok_or(MemoryError::Conflict)?;
            let removed_items = self
                .state
                .memory_units()
                .list_note_set_items(context, set.id)
                .await?;
            let removed = removed_items.len();
            for item in &removed_items {
                self.delete_current_memory_vectors(context, item.memory.id)
                    .await?;
            }
            match core.read_managed(context, &set.canonical_path).await {
                Ok(read) if read.file.current_revision == set.canonical_revision => {
                    core.delete_managed(
                        context,
                        &set.canonical_path,
                        set.canonical_revision,
                        Actor::system(),
                        SourcePlane::System,
                        None,
                    )
                    .await?;
                }
                Ok(_) => return Err(MemoryError::Conflict),
                Err(VaultError::NotFound) => {}
                Err(error) => return Err(MemoryError::Core(error)),
            }
            self.state
                .memory_units()
                .delete_note_set_projection(context, source_file_id, set.set_revision)
                .await?;
            report.deleted = 1;
            report.memories_removed = u64::try_from(removed).unwrap_or(u64::MAX);
            return Ok(report);
        };
        if file.content_hash.as_deref() != Some(set.source_content_hash.as_str()) {
            report.changed = 1;
            report.memories_hidden = u64::try_from(
                self.state
                    .memory_units()
                    .list_note_set_items(context, set.id)
                    .await?
                    .len(),
            )
            .unwrap_or(u64::MAX);
            return Ok(report);
        }
        let moved = file.path != set.source_path || file.current_revision != set.source_revision;
        if moved {
            let lock = self.vault_write_lock(context).await;
            let _guard = lock.lock().await;

            if self
                .state
                .memory_units()
                .get_note_set_by_source(context, source_file_id)
                .await?
                .as_ref()
                .map(|s| s.set_revision)
                != Some(set.set_revision)
            {
                return Err(MemoryError::Conflict);
            }
            let items = self
                .state
                .memory_units()
                .list_note_set_items(context, set.id)
                .await?;
            let mut updated_set = set.clone();
            updated_set.source_path = file.path.clone();
            updated_set.source_revision = file.current_revision;
            updated_set.set_revision = set
                .set_revision
                .next()
                .map_err(|_| MemoryError::InvalidInput("memory set revision overflow"))?;
            updated_set.updated_at = now_millis();
            let bytes = current_markdown::render_note_set(&updated_set, &items)?;
            let canonical = replace_or_adopt_current_managed(
                core,
                context,
                &set.canonical_path,
                set.canonical_revision,
                &bytes,
                Actor::system(),
                SourcePlane::System,
            )
            .await?;
            updated_set.canonical_file_id = canonical.id;
            updated_set.canonical_revision = canonical.current_revision;
            self.state
                .memory_units()
                .move_note_set_source(context, &updated_set, set.set_revision)
                .await?;
            report.moved = 1;
        } else {
            report.current = 1;
        }
        Ok(report)
    }

    async fn apply_prepared_note_set(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        snapshot: UnitSourceSetSnapshotRecord,
        reused: bool,
    ) -> Result<NoteExtractionResult, MemoryError> {
        let prepared_items: Vec<PreparedCurrentItem> =
            serde_json::from_value(snapshot.items.clone())
                .map_err(|_| MemoryError::GeneratedOutput("memory_set_snapshot_invalid"))?;
        let existing_set = self
            .state
            .memory_units()
            .get_note_set_by_source(context, snapshot.source_file_id)
            .await?;
        if existing_set.as_ref().map(|set| set.set_revision) != snapshot.expected_set_revision {
            return Err(MemoryError::Conflict);
        }
        let now = snapshot.created_at;
        let mut set = UnitSourceSetRecord {
            id: snapshot.note_set_id,
            vault_id: context.id(),
            source_file_id: snapshot.source_file_id,
            source_path: snapshot.source_path.clone(),
            source_content_hash: snapshot.source_content_hash.clone(),
            source_revision: snapshot.source_revision,
            set_revision: snapshot.proposed_set_revision,
            extraction_paused: snapshot.extraction_paused,
            canonical_file_id: existing_set
                .as_ref()
                .map_or(snapshot.source_file_id, |set| set.canonical_file_id),
            canonical_path: snapshot.canonical_path.clone(),
            canonical_revision: existing_set
                .as_ref()
                .map_or(Revision::new(1), |set| set.canonical_revision),
            profile_hash: snapshot.profile_hash.clone(),
            prompt_version: snapshot.prompt_version.clone(),
            provider_id: snapshot.provider_id,
            model_id: snapshot.model_id,
            created_at: existing_set
                .as_ref()
                .map_or(snapshot.created_at, |set| set.created_at),
            updated_at: now,
        };
        let mut bundles =
            current_bundles_from_prepared(context, &set, &prepared_items, snapshot.created_at);
        let canonical_bytes = current_markdown::render_note_set(&set, &bundles)?;
        if current_markdown::hash_bytes(&canonical_bytes) != snapshot.canonical_bytes_hash {
            return Err(MemoryError::GeneratedOutput(
                "memory_set_snapshot_hash_mismatch",
            ));
        }
        let lock = self.vault_write_lock(context).await;
        let _guard = lock.lock().await;

        if self
            .state
            .memory_units()
            .get_note_set_by_source(context, snapshot.source_file_id)
            .await?
            .as_ref()
            .map(|set| set.set_revision)
            != snapshot.expected_set_revision
        {
            return Err(MemoryError::Conflict);
        }
        let current_source = self
            .state
            .files()
            .get_by_id(context, snapshot.source_file_id)
            .await?
            .filter(FileRecord::is_active)
            .ok_or(MemoryError::Conflict)?;
        if current_source.path != snapshot.source_path
            || current_source.current_revision != snapshot.source_revision
            || current_source.content_hash.as_deref() != Some(snapshot.source_content_hash.as_str())
        {
            return Err(MemoryError::Conflict);
        }
        if let Some(existing_set) = existing_set.as_ref() {
            for item in self
                .state
                .memory_units()
                .list_note_set_items(context, existing_set.id)
                .await?
            {
                if !prepared_items.iter().any(|next| {
                    next.id == item.memory.id && next.content_hash == item.memory.content_hash
                }) {
                    self.delete_current_memory_vectors(context, item.memory.id)
                        .await?;
                }
            }
        }
        let canonical_file = match core.read_managed(context, &snapshot.canonical_path).await {
            Ok(mut read) => {
                let mut current_bytes = Vec::new();
                read.reader
                    .read_to_end(&mut current_bytes)
                    .await
                    .map_err(|_| {
                        MemoryError::SourceIngestion("memory_set_canonical_read_failed")
                    })?;
                if current_bytes == canonical_bytes {
                    read.file
                } else {
                    let expected = existing_set
                        .as_ref()
                        .map(|set| set.canonical_revision)
                        .ok_or(MemoryError::Conflict)?;
                    if read.file.current_revision != expected {
                        return Err(MemoryError::Conflict);
                    }
                    core.replace_managed_bytes(
                        context,
                        &snapshot.canonical_path,
                        expected,
                        &canonical_bytes,
                        Actor::system(),
                        SourcePlane::System,
                        None,
                    )
                    .await?
                    .file
                }
            }
            Err(VaultError::NotFound) if existing_set.is_none() => {
                core.create_managed_bytes(
                    context,
                    &snapshot.canonical_path,
                    &canonical_bytes,
                    Actor::system(),
                    SourcePlane::System,
                    None,
                )
                .await?
                .file
            }
            Err(VaultError::NotFound) => return Err(MemoryError::Conflict),
            Err(error) => return Err(MemoryError::Core(error)),
        };
        set.canonical_file_id = canonical_file.id;
        set.canonical_revision = canonical_file.current_revision;
        for bundle in &mut bundles {
            bundle.note_set = Some(set.clone());
        }
        let published = self
            .state
            .memory_units()
            .publish_note_set(context, snapshot.id, &set, &bundles)
            .await?;
        for bundle in &published {
            self.schedule_current_embedding(context, &bundle.memory)
                .await;
        }
        Ok(NoteExtractionResult {
            source_admitted: true,
            empty_set_published: published.is_empty(),
            already_evaluated: false,
            items_published: u32::try_from(published.len()).unwrap_or(u32::MAX),
            reused_prepared_snapshot: reused,
            ..Default::default()
        })
    }

    #[allow(dead_code)]
    pub async fn rebuild(
        &self,
        context: &VaultContext,
        core: &VaultCore,
    ) -> Result<MemoryRebuildReport, MemoryError> {
        self.ensure_initialized(context).await?;
        let vault_write_lock = self.vault_write_lock(context).await;
        let _write_guard = vault_write_lock.lock().await;
        let files = core.list_managed_files(context).await?;
        let mut report = MemoryRebuildReport::default();
        for metadata in files {
            let Some(path) = metadata.path.clone() else {
                continue;
            };
            let explicit = is_current_explicit_path(core, &path);
            let note_set = is_current_note_set_path(core, &path);
            if !explicit && !note_set {
                continue;
            }
            let Some(file) = self.state.files().get_active(context, &path).await? else {
                report.quarantined = report.quarantined.saturating_add(1);
                *report
                    .quarantine_reasons
                    .entry("missing_canonical_file".into())
                    .or_default() += 1;
                continue;
            };
            let mut read = match core.read_managed(context, &path).await {
                Ok(read) => read,
                Err(_) => {
                    report.quarantined = report.quarantined.saturating_add(1);
                    *report
                        .quarantine_reasons
                        .entry("canonical_read_failed".into())
                        .or_default() += 1;
                    continue;
                }
            };
            let mut bytes = Vec::new();
            if (&mut read.reader)
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await
                .is_err()
            {
                report.quarantined = report.quarantined.saturating_add(1);
                *report
                    .quarantine_reasons
                    .entry("canonical_read_failed".into())
                    .or_default() += 1;
                continue;
            }
            if explicit {
                let mut bundle = match current_markdown::parse_explicit(
                    &bytes,
                    &path,
                    context.id(),
                    file.id,
                    file.current_revision,
                ) {
                    Ok(bundle) => bundle,
                    Err(_) => {
                        report.quarantined = report.quarantined.saturating_add(1);
                        *report
                            .quarantine_reasons
                            .entry("explicit_invalid".into())
                            .or_default() += 1;
                        continue;
                    }
                };
                if !self
                    .current_source_identities_belong_to_vault(context, &bundle.sources)
                    .await?
                {
                    report.quarantined = report.quarantined.saturating_add(1);
                    *report
                        .quarantine_reasons
                        .entry("explicit_provenance_invalid".into())
                        .or_default() += 1;
                    continue;
                }
                let previous = self
                    .state
                    .memory_units()
                    .get_unchecked(context, bundle.memory.id)
                    .await?;
                if let Some(previous) = previous.as_ref() {
                    bundle.memory.last_recalled_at = previous.memory.last_recalled_at;
                    bundle.memory.recall_count = previous.memory.recall_count;
                    if previous.memory.content_hash != bundle.memory.content_hash {
                        self.delete_current_memory_vectors(context, bundle.memory.id)
                            .await?;
                    }
                }
                if previous.as_ref().is_some_and(|previous| {
                    previous.memory == bundle.memory
                        && provenance_identity(&previous.sources)
                            == provenance_identity(&bundle.sources)
                }) {
                    self.state
                        .memory_units()
                        .rebuild_search_row(context, &bundle.memory)
                        .await?;
                    report.projected += 1;
                    continue;
                }
                let restored = self
                    .state
                    .memory_units()
                    .restore_explicit_projection(context, &bundle)
                    .await?;
                if previous.as_ref().is_none_or(|previous| {
                    previous.memory.content_hash != restored.memory.content_hash
                }) {
                    self.schedule_current_embedding(context, &restored.memory)
                        .await;
                }
                report.projected = report.projected.saturating_add(1);
                continue;
            }

            let (mut set, mut bundles) = match current_markdown::parse_note_set(
                &bytes,
                &path,
                context.id(),
                file.id,
                file.current_revision,
                now_millis(),
            ) {
                Ok(parsed) => parsed,
                Err(_) => {
                    report.quarantined = report.quarantined.saturating_add(1);
                    *report
                        .quarantine_reasons
                        .entry("source_set_invalid".into())
                        .or_default() += 1;
                    continue;
                }
            };
            // Pausing a source is durable control state. After its body changes,
            // restore that control without making any old unit current.
            if self
                .state
                .files()
                .get_by_id(context, set.source_file_id)
                .await?
                .filter(FileRecord::is_active)
                .is_some_and(|file| file.content_hash.as_deref() != Some(&set.source_content_hash))
            {
                bundles.clear();
            }
            if !self
                .validate_rebuilt_source_units(context, core, &set, &bundles)
                .await?
            {
                report.quarantined += 1;
                *report
                    .quarantine_reasons
                    .entry("source_units_do_not_match_original".into())
                    .or_default() += 1;
                continue;
            }
            if let Some(provider_id) = set.provider_id
                && self
                    .state
                    .providers()
                    .get_provider(provider_id)
                    .await?
                    .is_none()
            {
                set.provider_id = None;
            }
            if let Some(model_id) = set.model_id {
                match self.state.providers().get_model(model_id).await? {
                    Some(model) if set.provider_id.is_none_or(|id| id == model.provider_id) => {
                        set.provider_id = Some(model.provider_id);
                    }
                    _ => set.model_id = None,
                }
            }
            for bundle in &mut bundles {
                bundle.note_set = Some(set.clone());
                if let Some(previous) = self
                    .state
                    .memory_units()
                    .get_unchecked(context, bundle.memory.id)
                    .await?
                {
                    bundle.memory.last_recalled_at = previous.memory.last_recalled_at;
                    bundle.memory.recall_count = previous.memory.recall_count;
                }
            }
            if let Some(previous_set) = self
                .state
                .memory_units()
                .get_note_set_by_source(context, set.source_file_id)
                .await?
            {
                let current_items = self
                    .state
                    .memory_units()
                    .list_note_set_items(context, previous_set.id)
                    .await?;
                if previous_set == set
                    && current_items.len() == bundles.len()
                    && current_items.iter().zip(&bundles).all(|(old, new)| {
                        old.memory == new.memory
                            && provenance_identity(&old.sources)
                                == provenance_identity(&new.sources)
                    })
                {
                    for bundle in &bundles {
                        self.state
                            .memory_units()
                            .rebuild_search_row(context, &bundle.memory)
                            .await?;
                    }
                    report.projected += 1;
                    continue;
                }
                for previous in self
                    .state
                    .memory_units()
                    .list_note_set_items(context, previous_set.id)
                    .await?
                {
                    self.delete_current_memory_vectors(context, previous.memory.id)
                        .await?;
                }
            }
            let restored = match self
                .state
                .memory_units()
                .restore_note_set_projection(context, &set, &bundles)
                .await
            {
                Ok(restored) => restored,
                Err(mcp_vault_state::StateError::Conflict) => {
                    report.quarantined = report.quarantined.saturating_add(1);
                    *report
                        .quarantine_reasons
                        .entry("source_set_conflict".into())
                        .or_default() += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            for bundle in &restored {
                self.schedule_current_embedding(context, &bundle.memory)
                    .await;
            }
            report.projected = report.projected.saturating_add(1);
        }
        Ok(report)
    }

    async fn validate_rebuilt_source_units(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        set: &UnitSourceSetRecord,
        bundles: &[UnitBundle],
    ) -> Result<bool, MemoryError> {
        let Some(file) = self
            .state
            .files()
            .get_by_id(context, set.source_file_id)
            .await?
            .filter(FileRecord::is_active)
        else {
            return Ok(false);
        };
        if core.is_managed_path(&file.path) {
            return Ok(false);
        }
        if file.content_hash.as_deref() != Some(&set.source_content_hash) {
            return Ok(bundles.is_empty());
        }
        let mut read = core.read(context, &file.path).await?;
        let mut bytes = Vec::new();
        (&mut read.reader)
            .take(512 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| MemoryError::SourceIngestion("memory_source_read_failed"))?;
        if bytes.len() > 512 * 1024 {
            return Ok(false);
        }
        let Ok(source) = String::from_utf8(bytes) else {
            return Ok(false);
        };
        if markdown::hash_content(&source).strip_prefix("sha256:")
            != Some(
                set.source_content_hash
                    .strip_prefix("sha256:")
                    .unwrap_or(&set.source_content_hash),
            )
        {
            return Ok(false);
        }
        let mut candidates = crate::units::source_units(&source);
        for unit in crate::units::minimal_selection_units(&source) {
            if !candidates.contains(&unit) {
                candidates.push(unit);
            }
        }
        Ok(bundles.iter().all(|bundle| {
            serde_json::from_value::<crate::units::SourceUnit>(
                bundle.memory.metadata["source_unit"].clone(),
            )
            .ok()
            .is_some_and(|unit| {
                candidates.contains(&unit) && unit.complete_text() == bundle.memory.content
            })
        }))
    }

    async fn current_source_identities_belong_to_vault(
        &self,
        context: &VaultContext,
        sources: &[UnitSourceRecord],
    ) -> Result<bool, MemoryError> {
        for file_id in sources.iter().filter_map(|source| source.note_file_id) {
            if self
                .state
                .files()
                .get_by_id(context, file_id)
                .await?
                .is_none()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Load current memory bodies for rebuildable vector projection.
    async fn memory_embedding_inputs(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<EmbeddingInput>, MemoryError> {
        let filter = UnitFilter::default();
        let mut inputs = Vec::new();
        let mut offset = 0_u32;
        loop {
            let page = self
                .state
                .memory_units()
                .list(context, &filter, MEMORY_ARTIFACT_PAGE_SIZE, offset)
                .await?;
            if page.is_empty() {
                break;
            }
            let page_len = u32::try_from(page.len()).unwrap_or(MEMORY_ARTIFACT_PAGE_SIZE);
            inputs.extend(page.iter().flat_map(memory_embedding_inputs_for));
            offset = offset.saturating_add(page_len);
            if page_len < MEMORY_ARTIFACT_PAGE_SIZE {
                break;
            }
        }
        Ok(inputs)
    }

    async fn memory_embedding_metadata(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<Vec<mcp_vault_state::EmbeddingRecord>, MemoryError> {
        let mut records = Vec::new();
        let mut offset = 0_u32;
        loop {
            let page = self
                .state
                .providers()
                .list_embeddings(context, model_id, "memory_unit", 1_000, offset)
                .await?;
            if page.is_empty() {
                break;
            }
            let page_len = u32::try_from(page.len()).unwrap_or(1_000);
            records.extend(page);
            offset = offset.saturating_add(page_len);
            if page_len < 1_000 {
                break;
            }
        }
        Ok(records)
    }

    /// Execute one durable embedding.rebuild job for memory sources.
    pub async fn reembed_sources(
        &self,
        context: &VaultContext,
        model_id: mcp_vault_domain::ModelId,
        sources: &[EmbeddingSourceRef],
    ) -> Result<u64, MemoryError> {
        self.ensure_initialized(context).await?;
        let resolver = MemoryEmbeddingResolver {
            state: self.state.clone(),
        };
        let records = self
            .providers
            .embeddings()
            .reembed_with_resolver(context, model_id, sources, &resolver)
            .await?;
        Ok(records.len() as u64)
    }

    async fn normalize_source_inputs(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        sources: &[MemorySourceInput],
    ) -> Result<Vec<MemorySourceInput>, MemoryError> {
        let mut normalized = Vec::with_capacity(sources.len());
        for source in sources {
            if source.source_type != "note" {
                normalized.push(source.clone());
                continue;
            }
            if source
                .note_path
                .as_ref()
                .is_some_and(|path| core.is_managed_path(path))
            {
                return Err(MemoryError::InvalidInput(
                    "memory provenance cannot reference managed files",
                ));
            }
            let file = if let Some(file_id) = source.note_file_id {
                self.state.files().get_by_id(context, file_id).await?
            } else if let Some(path) = source.note_path.as_ref() {
                self.state.files().get_active(context, path).await?
            } else {
                return Err(MemoryError::InvalidInput("note provenance has no source"));
            };
            let Some(file) = file else {
                return Err(MemoryError::Conflict);
            };
            if source.note_file_id.is_some_and(|id| id != file.id)
                || source
                    .note_path
                    .as_ref()
                    .is_some_and(|path| path != &file.path)
                || source
                    .note_revision
                    .is_some_and(|revision| revision != file.current_revision)
            {
                return Err(MemoryError::Conflict);
            }
            let mut source = source.clone();
            source.note_file_id = Some(file.id);
            source.note_path = Some(file.path);
            source.note_revision = Some(file.current_revision);
            normalized.push(source);
        }
        Ok(normalized)
    }

    async fn current_sources_from_inputs(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
        inputs: &[MemorySourceInput],
        default_source_type: &str,
        default_actor_id: Option<&str>,
        created_at: i64,
    ) -> Result<Vec<UnitSourceRecord>, MemoryError> {
        if inputs.is_empty() {
            return Ok(vec![UnitSourceRecord {
                id: MemorySourceId::new(),
                vault_id: context.id(),
                memory_id,
                source_type: default_source_type.to_owned(),
                note_file_id: None,
                note_path: None,
                note_revision: None,
                source_content_hash: None,
                heading_path: Vec::new(),
                start_line: None,
                end_line: None,
                excerpt_hash: None,
                actor_id: default_actor_id.map(str::to_owned),
                created_at,
            }]);
        }
        let mut sources = Vec::with_capacity(inputs.len());
        for input in inputs {
            let source_type = if input.source_type == "note" {
                "note"
            } else {
                default_source_type
            };
            let source_file = match input.note_file_id {
                Some(file_id) => self
                    .state
                    .files()
                    .get_by_id(context, file_id)
                    .await?
                    .filter(FileRecord::is_active),
                None => None,
            };
            if source_type == "note" && source_file.is_none() {
                return Err(MemoryError::Conflict);
            }
            sources.push(UnitSourceRecord {
                id: MemorySourceId::new(),
                vault_id: context.id(),
                memory_id,
                source_type: source_type.to_owned(),
                note_file_id: source_file.as_ref().map(|file| file.id),
                note_path: source_file
                    .as_ref()
                    .map(|file| file.path.clone())
                    .or_else(|| input.note_path.clone()),
                note_revision: source_file
                    .as_ref()
                    .map(|file| file.current_revision)
                    .or(input.note_revision),
                source_content_hash: source_file.and_then(|file| file.content_hash),
                heading_path: input.heading_path.clone(),
                start_line: input.start_line,
                end_line: input.end_line,
                excerpt_hash: input.excerpt_hash.clone(),
                actor_id: input
                    .actor_id
                    .clone()
                    .or_else(|| default_actor_id.map(str::to_owned)),
                created_at,
            });
        }
        Ok(sources)
    }

    async fn schedule_current_embedding(&self, context: &VaultContext, memory: &UnitRecord) {
        let Ok(Some(binding)) = self
            .state
            .providers()
            .resolve_binding(context, "embedding_memory")
            .await
        else {
            return;
        };
        let sources = memory_embedding_inputs_for(memory)
            .into_iter()
            .map(|input| input.source)
            .collect::<Vec<_>>();
        let _ = self
            .providers
            .embeddings()
            .schedule_reembedding(context, binding.model_id, &sources)
            .await;
    }

    async fn delete_current_memory_vectors(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<u64, MemoryError> {
        Ok(self
            .providers
            .embeddings()
            .delete_object_vectors(context, "memory_unit", &memory_id.to_string())
            .await?)
    }

    fn view_from_current_bundle(
        &self,
        bundle: &UnitBundle,
        score: Option<f64>,
        breakdown: Option<BTreeMap<String, f64>>,
    ) -> MemoryUnit {
        let ownership = match bundle.memory.ownership {
            UnitOwnership::Explicit => MemoryOwnership::Explicit,
            UnitOwnership::NoteDerived => MemoryOwnership::NoteDerived,
        };
        let sources = bundle
            .sources
            .iter()
            .map(|source| MemorySourceView {
                source_type: source.source_type.clone(),
                path: source.note_path.clone(),
                file_id: source.note_file_id,
                revision: source.note_revision,
                heading: source.heading_path.clone(),
                start_line: source.start_line,
                end_line: source.end_line,
            })
            .collect();
        MemoryUnit {
            id: bundle.memory.id,
            memory_type: bundle
                .memory
                .kind
                .as_deref()
                .and_then(|kind| MemoryType::try_from(kind).ok()),
            ownership,
            note_set_id: bundle.memory.note_set_id,
            revision: bundle.memory.revision,
            content: bundle.memory.content.clone(),
            importance: bundle.memory.importance,
            confidence: bundle.memory.confidence,
            valid_from: bundle.memory.valid_from,
            valid_to: bundle.memory.valid_to,
            canonical_path: bundle.memory.canonical_path.clone().or_else(|| {
                bundle
                    .note_set
                    .as_ref()
                    .map(|set| set.canonical_path.clone())
            }),
            canonical_revision: bundle
                .memory
                .canonical_revision
                .or_else(|| bundle.note_set.as_ref().map(|set| set.canonical_revision)),
            tags: bundle.memory.tags.clone(),
            entities: bundle.memory.entities.clone(),
            source_count: bundle.sources.len(),
            sources,
            score,
            score_breakdown: breakdown,
        }
    }
}

/// Result of a managed-memory projection rebuild.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryRebuildReport {
    /// Valid managed records projected.
    pub projected: u64,
    /// Invalid records quarantined/diagnosed.
    pub quarantined: u64,
    /// Bounded content-free diagnostic counts; no source paths or bodies.
    pub quarantine_reasons: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default)]
struct Score {
    total: f64,
    components: BTreeMap<String, f64>,
}

fn current_bundles_from_prepared(
    context: &VaultContext,
    set: &UnitSourceSetRecord,
    items: &[PreparedCurrentItem],
    updated_at: i64,
) -> Vec<UnitBundle> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            if let Some(memory) = item.preserved_memory.as_ref() {
                let mut memory = memory.clone();
                memory.ordinal = Some(index as u32);
                return UnitBundle {
                    memory,
                    sources: item.preserved_sources.clone().unwrap_or_default(),
                    note_set: Some(set.clone()),
                };
            }
            let source = UnitSourceRecord {
                id: MemorySourceId::new(),
                vault_id: context.id(),
                memory_id: item.id,
                source_type: "note".to_owned(),
                note_file_id: Some(set.source_file_id),
                note_path: Some(set.source_path.clone()),
                note_revision: Some(set.source_revision),
                source_content_hash: Some(set.source_content_hash.clone()),
                heading_path: item
                    .source_unit
                    .as_ref()
                    .map(|unit| unit.headings.clone())
                    .unwrap_or_default(),
                start_line: item.source_unit.as_ref().map(|unit| unit.body.start_line),
                end_line: item.source_unit.as_ref().map(|unit| unit.body.end_line),
                excerpt_hash: Some(item.content_hash.clone()),
                actor_id: None,
                created_at: updated_at,
            };
            UnitBundle {
                memory: UnitRecord {
                    id: item.id,
                    vault_id: context.id(),
                    ownership: UnitOwnership::NoteDerived,
                    note_set_id: Some(set.id),
                    ordinal: Some(item.ordinal),
                    kind: item.kind.map(|kind| kind.as_str().to_owned()),
                    content: item.content.clone(),
                    normalized_content: markdown::normalize_content(&item.content),
                    content_hash: item.content_hash.clone(),
                    importance: None,
                    confidence: None,
                    origin: "note_extracted".to_owned(),
                    revision: item.revision,
                    canonical_file_id: None,
                    canonical_path: None,
                    canonical_revision: None,
                    valid_from: None,
                    valid_to: None,
                    tags: item.tags.clone(),
                    entities: Vec::new(),
                    metadata: json!({
                        "pipeline_version": EXTRACTION_PIPELINE_VERSION,
                        "prompt_version": set.prompt_version,
                        "source_content_hash": set.source_content_hash,
                        "source_unit": item.source_unit,
                        "retrieval_hint": item.retrieval_hint,
                    }),
                    created_at: item.created_at,
                    updated_at,
                    last_recalled_at: None,
                    recall_count: 0,
                },
                sources: vec![source],
                note_set: Some(set.clone()),
            }
        })
        .collect()
}

fn access_ownership(access: MemoryReadAccess) -> Vec<UnitOwnership> {
    match access {
        MemoryReadAccess::ExplicitOnly => vec![UnitOwnership::Explicit],
        MemoryReadAccess::All => Vec::new(),
    }
}

fn provenance_identity(sources: &[UnitSourceRecord]) -> String {
    json!(sources.iter().map(|source|json!({"type":source.source_type,"file":source.note_file_id,"path":source.note_path,"revision":source.note_revision,"hash":source.source_content_hash,"heading":source.heading_path,"from":source.start_line,"to":source.end_line,"excerpt":source.excerpt_hash,"actor":source.actor_id})).collect::<Vec<_>>()).to_string()
}

fn presentation_identity(bundle: &UnitBundle) -> String {
    // No normalization: case, whitespace, ownership, scope and provenance matter.
    json!({"body": bundle.memory.content, "kind": bundle.memory.kind, "ownership": bundle.memory.ownership,
        "sources": bundle.sources.iter().map(|s|json!({"file":s.note_file_id,"path":s.note_path,"heading":s.heading_path,"from":s.start_line,"to":s.end_line})).collect::<Vec<_>>(),
        "tags":bundle.memory.tags,"entities":bundle.memory.entities,"from":bundle.memory.valid_from,"to":bundle.memory.valid_to
    }).to_string()
}

/// Greedy soft source diversification. It reorders already admitted units,
/// never removes canonical data or forces an unrelated source into the set.
fn diversify_source_candidates(
    mut remaining: Vec<(UnitBundle, Score)>,
) -> Vec<(UnitBundle, Score)> {
    let mut selected = Vec::with_capacity(remaining.len());
    let mut seen = HashMap::<FileId, u32>::new();
    while !remaining.is_empty() {
        let penalty = |bundle: &UnitBundle| {
            let count = bundle
                .sources
                .iter()
                .filter_map(|source| source.note_file_id)
                .filter_map(|id| seen.get(&id))
                .copied()
                .max()
                .unwrap_or(0);
            1.0 / (1.0 + 0.2 * f64::from(count.min(4)))
        };
        let index = (0..remaining.len())
            .max_by(|left, right| {
                (remaining[*left].1.total * penalty(&remaining[*left].0))
                    .total_cmp(&(remaining[*right].1.total * penalty(&remaining[*right].0)))
                    .then_with(|| {
                        remaining[*right]
                            .0
                            .memory
                            .id
                            .cmp(&remaining[*left].0.memory.id)
                    })
            })
            .expect("remaining is nonempty");
        let (bundle, mut score) = remaining.remove(index);
        let multiplier = penalty(&bundle);
        score.total *= multiplier;
        score
            .components
            .insert("source_diversity_multiplier".into(), multiplier);
        for id in bundle
            .sources
            .iter()
            .filter_map(|source| source.note_file_id)
            .collect::<HashSet<_>>()
        {
            *seen.entry(id).or_default() += 1;
        }
        selected.push((bundle, score));
    }
    selected
}

fn current_memory_boost(bundle: &UnitBundle, request: &RecallRequest) -> f64 {
    let path_match = request.context.paths.iter().any(|path| {
        bundle.sources.iter().any(|source| {
            source.note_path.as_ref().is_some_and(|source| {
                let path = path.trim_end_matches('/');
                source.as_str() == path
                    || source
                        .as_str()
                        .strip_prefix(path)
                        .is_some_and(|tail| tail.starts_with('/'))
            })
        })
    });
    let topic_match = request
        .context
        .entities
        .iter()
        .chain(&request.context.recent_topics)
        .any(|term| {
            !term.is_empty()
                && bundle
                    .memory
                    .normalized_content
                    .contains(&term.to_lowercase())
        });
    1.0 + if path_match { 0.15 } else { 0.0 } + if topic_match { 0.1 } else { 0.0 }
}

/// Conservative deterministic envelope: complete JSON UTF-8 bytes / 4,
/// rounded up. This is an estimate, not a tokenizer-specific token count.
fn estimate_serialized_tokens(value: &impl Serialize) -> u32 {
    serde_json::to_vec(value)
        .ok()
        .and_then(|bytes| u32::try_from(bytes.len().div_ceil(4)).ok())
        .unwrap_or(u32::MAX)
}

impl Score {
    fn add(&mut self, value: f64, name: &str) {
        self.total += value;
        *self.components.entry(name.to_owned()).or_default() += value;
        *self
            .components
            .entry(format!("{name}_contribution"))
            .or_default() += value;
    }
}

fn validate_remember_input(input: &RememberInput) -> Result<(), MemoryError> {
    validate_content(&input.content)?;
    if let Some(importance) = input.importance {
        validate_score(importance)?;
    }
    if let Some(confidence) = input.confidence {
        validate_score(confidence)?;
    }
    if let (Some(from), Some(to)) = (input.valid_from, input.valid_to)
        && from >= to
    {
        return Err(MemoryError::InvalidInput(
            "memory validity range is invalid",
        ));
    }
    if input.tags.len() > 64 || input.entities.len() > 64 || input.sources.len() > 32 {
        return Err(MemoryError::InvalidInput("memory metadata is too large"));
    }
    for value in input.tags.iter().chain(input.entities.iter()) {
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(MemoryError::InvalidInput("memory tag/entity is invalid"));
        }
    }
    if let Some(key) = input.idempotency_key.as_deref()
        && (key.is_empty() || key.len() > 256 || key.chars().any(char::is_control))
    {
        return Err(MemoryError::InvalidInput(
            "memory idempotency key is invalid",
        ));
    }
    Ok(())
}

fn validate_content(content: &str) -> Result<(), MemoryError> {
    if content.trim().is_empty()
        || content.len() > MAX_CONTENT_BYTES
        || content.contains('\0')
        || content
            .chars()
            .any(|value| value.is_control() && !matches!(value, '\n' | '\r' | '\t'))
    {
        return Err(MemoryError::InvalidInput("memory content is invalid"));
    }
    Ok(())
}

fn validate_score(value: f64) -> Result<(), MemoryError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(MemoryError::InvalidInput("memory score is invalid"));
    }
    Ok(())
}

fn validate_recall_request(request: &RecallRequest) -> Result<(), MemoryError> {
    if request.query.trim().is_empty()
        || request.query.len() > 8192
        || request.max_results == 0
        || request.max_results > MAX_RECALL_RESULTS
        || request.max_related_notes > MAX_RECALL_RESULTS
        || request.max_tokens < 128
        || request.max_tokens > MAX_RECALL_TOKENS
    {
        return Err(MemoryError::InvalidInput("recall request is invalid"));
    }
    validate_score(request.min_importance)
}

pub(crate) fn quote_fts_query(query: &str) -> Result<String, MemoryError> {
    let normalized = memory_search_terms([query], 64);
    let terms = normalized.split_whitespace().collect::<Vec<_>>();
    if terms.is_empty() {
        return Err(MemoryError::InvalidInput(
            "recall query has no searchable terms",
        ));
    }
    Ok(terms
        .into_iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR "))
}

fn deduplicate_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.to_lowercase()))
        .collect()
}

fn remember_request_hash(input: &RememberInput) -> String {
    let value = json!({
        "content": input.content,
        "sources": input.sources,
        "type": input.memory_type.map(MemoryType::as_str),
        "importance": input.importance,
        "confidence": input.confidence,
        "valid_from": input.valid_from,
        "valid_to": input.valid_to,
        "tags": input.tags,
        "entities": input.entities,
        "origin": input.origin.as_str(),
        "extraction": input.extraction
    });
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn extraction_profile_hash(
    policy: &ExtractionPolicy,
    binding: &ModelBindingRecord,
    model: &ModelRecord,
    provider: &ProviderRecord,
) -> String {
    let value = json!({
        "evaluation_profile_version": EXTRACTION_EVALUATION_PROFILE_VERSION,
        "pipeline_version": EXTRACTION_PIPELINE_VERSION,
        "prompt_version": EXTRACTION_PROMPT_VERSION,
        "request_timeout_seconds": policy.request_timeout_seconds,
        "system": super::selection::SYSTEM,
        "selection_schema": super::selection::schema(&[]),
        "review_system": super::review::SYSTEM,
        "review_schema_version": super::review::REVIEW_SCHEMA_VERSION,
        "review_contract": "explicit-keep-add-replace-omit-v1",
        "batch_contract": "32-units-60KiB-json-64KiB-unit-v2",
        "binding_id": &binding.id,
        "binding_settings": &binding.settings,
        "model_id": model.id,
        "external_model_id": &model.external_model_id,
        "model_capabilities": &model.capabilities,
        "model_settings": &model.settings,
        "model_enabled": model.enabled,
        "provider_id": provider.id,
        "provider_type": &provider.provider_type,
        "provider_base_url": &provider.base_url,
        "provider_settings": &provider.settings,
        "provider_enabled": provider.enabled,
    });
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

async fn create_or_adopt_current_managed(
    core: &VaultCore,
    context: &VaultContext,
    path: &VaultPath,
    bytes: &[u8],
    actor: Actor,
    source_plane: SourcePlane,
    allow_adopt: bool,
) -> Result<FileRecord, MemoryError> {
    match core
        .create_managed_bytes(context, path, bytes, actor, source_plane, None)
        .await
    {
        Ok(result) => Ok(result.file),
        Err(VaultError::AlreadyExists) if allow_adopt => {
            exact_managed_file(core, context, path, bytes)
                .await?
                .ok_or(MemoryError::Conflict)
        }
        Err(error) => Err(MemoryError::Core(error)),
    }
}

async fn replace_or_adopt_current_managed(
    core: &VaultCore,
    context: &VaultContext,
    path: &VaultPath,
    expected_revision: Revision,
    bytes: &[u8],
    actor: Actor,
    source_plane: SourcePlane,
) -> Result<FileRecord, MemoryError> {
    match core
        .replace_managed_bytes(
            context,
            path,
            expected_revision,
            bytes,
            actor,
            source_plane,
            None,
        )
        .await
    {
        Ok(result) => Ok(result.file),
        Err(VaultError::RevisionConflict { .. }) => exact_managed_file(core, context, path, bytes)
            .await?
            .ok_or(MemoryError::Conflict),
        Err(error) => Err(MemoryError::Core(error)),
    }
}

async fn exact_managed_file(
    core: &VaultCore,
    context: &VaultContext,
    path: &VaultPath,
    expected: &[u8],
) -> Result<Option<FileRecord>, MemoryError> {
    let mut read = match core.read_managed(context, path).await {
        Ok(read) => read,
        Err(VaultError::NotFound) => return Ok(None),
        Err(error) => return Err(MemoryError::Core(error)),
    };
    let limit = u64::try_from(expected.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut actual = Vec::with_capacity(expected.len().saturating_add(1));
    (&mut read.reader)
        .take(limit)
        .read_to_end(&mut actual)
        .await
        .map_err(|_| MemoryError::InvalidInput("managed current memory cannot be read"))?;
    Ok((actual == expected).then_some(read.file))
}

pub(super) fn redact_generated_text(input: String) -> String {
    let redacted = PRIVATE_KEY_REGEX.replace_all(&input, "[REDACTED_SECRET]");
    let redacted = BEARER_TOKEN_REGEX.replace_all(&redacted, "Bearer [REDACTED_SECRET]");
    let redacted = OPENAI_KEY_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = AWS_ACCESS_KEY_ID_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    SECRET_ASSIGNMENT_REGEX
        .replace_all(&redacted, "$1$2$3[REDACTED_SECRET]")
        .into_owned()
}

fn redact_json_strings(mut value: Value) -> Value {
    match &mut value {
        Value::String(text) => *text = redact_generated_text(std::mem::take(text)),
        Value::Array(items) => {
            for item in items {
                *item = redact_json_strings(std::mem::take(item));
            }
        }
        Value::Object(properties) => {
            for item in properties.values_mut() {
                *item = redact_json_strings(std::mem::take(item));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    value
}

fn validate_extraction_policy(policy: &ExtractionPolicy) -> Result<(), MemoryError> {
    if !(30..=1_800).contains(&policy.request_timeout_seconds) {
        return Err(MemoryError::InvalidInput(
            "memory extraction timeout must be between 30 and 1800 seconds",
        ));
    }
    Ok(())
}

fn is_current_explicit_path(core: &VaultCore, path: &VaultPath) -> bool {
    is_direct_child_markdown(
        path,
        &format!("{}/memory-v3/explicit/", core.managed_root().as_str()),
    )
}

fn is_current_note_set_path(core: &VaultCore, path: &VaultPath) -> bool {
    is_direct_child_markdown(
        path,
        &format!("{}/memory-v3/sources/", core.managed_root().as_str()),
    )
}

fn is_direct_child_markdown(path: &VaultPath, prefix: &str) -> bool {
    path.as_str()
        .strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.contains('/') && suffix.ends_with(".md"))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

pub(crate) fn bounded_memory_embedding_text(value: &str) -> &str {
    if value.len() <= MEMORY_EMBEDDING_MAX_INPUT_BYTES {
        return value;
    }
    let mut end = MEMORY_EMBEDDING_MAX_INPUT_BYTES;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn memory_embedding_inputs_for(memory: &UnitRecord) -> Vec<EmbeddingInput> {
    memory_embedding_inputs_for_current_fields(
        memory.id,
        &memory.content_hash,
        &memory.normalized_content,
    )
}

pub(crate) fn memory_embedding_inputs_for_current_fields(
    memory_id: MemoryId,
    content_hash: &str,
    normalized_content: &str,
) -> Vec<EmbeddingInput> {
    let body = normalized_content.trim();
    if body.is_empty() {
        return Vec::new();
    }
    let mut inputs = Vec::new();
    let mut start = 0_usize;
    while start < body.len() {
        let end = floor_utf8_boundary(
            body,
            start
                .saturating_add(MEMORY_EMBEDDING_MAX_INPUT_BYTES)
                .min(body.len()),
        );
        let text = body[start..end].to_owned();
        let ordinal = inputs.len();
        inputs.push(EmbeddingInput {
            source: EmbeddingSourceRef {
                object_type: "memory_unit".to_owned(),
                object_id: memory_id.to_string(),
                chunk_key: format!("{MEMORY_EMBEDDING_CHUNK_PROFILE}:{ordinal:04}"),
                content_hash: content_hash.to_owned(),
            },
            text,
        });
        if end == body.len() {
            break;
        }
        start = ceil_utf8_boundary(
            body,
            end.saturating_sub(MEMORY_EMBEDDING_CHUNK_OVERLAP_BYTES),
        );
        debug_assert!(inputs.len() < MAX_MEMORY_EMBEDDING_CHUNKS);
    }
    inputs
}

fn floor_utf8_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_utf8_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[derive(Clone)]
struct MemoryEmbeddingResolver {
    state: StateStore,
}

#[async_trait]
impl EmbeddingSourceResolver for MemoryEmbeddingResolver {
    async fn resolve_source(
        &self,
        context: &VaultContext,
        source: &EmbeddingSourceRef,
    ) -> Result<Option<String>, mcp_vault_providers::ProviderError> {
        if source.object_type != "memory_unit"
            || (!source
                .chunk_key
                .starts_with(&format!("{MEMORY_EMBEDDING_CHUNK_PROFILE}:"))
                && source.chunk_key != "body")
        {
            return Ok(None);
        }
        let memory_id = MemoryId::parse(&source.object_id).map_err(|_| {
            mcp_vault_providers::ProviderError::InvalidConfiguration("memory source id is invalid")
        })?;
        let memory = self
            .state
            .memory_units()
            .get(context, memory_id)
            .await
            .map_err(mcp_vault_providers::ProviderError::State)?;
        let Some(bundle) = memory else {
            return Ok(None);
        };
        if bundle.memory.content_hash != source.content_hash {
            return Ok(None);
        }
        Ok(memory_embedding_inputs_for(&bundle.memory)
            .into_iter()
            .find(|input| input.source.chunk_key == source.chunk_key)
            .map(|input| input.text))
    }
}
