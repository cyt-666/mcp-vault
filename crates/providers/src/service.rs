//! Provider configuration, secret resolution, model binding, and embedding
//! orchestration services.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use mcp_vault_auth::{AuthService, SecretString};
use mcp_vault_domain::{
    DomainError, ModelId, ProviderId, Revision, VaultContext, WritePrecondition,
};
use mcp_vault_state::{
    EmbeddingCoverage, EmbeddingRecord, ModelRecord, ProviderDeletionSummary, ProviderHealthRecord,
    ProviderRecord, StateError, StateStore,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

use crate::{
    AnthropicMessagesAdapter, EmbeddingRequest, EmbeddingResult, EmbeddingSourceRef,
    FastEmbedAdapter, GenerationOptions, HttpEmbeddingAdapter, ModelCapabilities, ModelSettings,
    OpenAiCompatibleAdapter, OpenAiResponsesAdapter, ProviderAdapter, ProviderError, ProviderKind,
    ProviderMode, ProviderSettings, ProviderTransport, SqliteVectorIndex,
    StructuredGenerationRequest, StructuredGenerationResult, VectorHit, VectorIndex,
    new_embedding_id,
};

/// Current reference-only embedding job/projection contract.
///
/// Bump this when source chunk identity or resolution changes incompatibly so
/// a corrected rebuild does not reuse a terminal job created by an older
/// projection.
pub const EMBEDDING_PROJECTION_VERSION: u32 = 3;

const PROVIDER_SECRET_PURPOSE: &str = "provider-api-key";
const PROVIDER_SECRET_OWNER: &str = "provider";
const PROVIDER_MODE_SETTING: &str = "provider.mode";

/// Source-resolution boundary owned by the indexer or memory application
/// service. Provider jobs carry references, never source bodies.
#[async_trait]
pub trait EmbeddingSourceResolver: Send + Sync {
    /// Resolve one current source body for a Vault-scoped reference.
    async fn resolve_source(
        &self,
        context: &VaultContext,
        source: &EmbeddingSourceRef,
    ) -> Result<Option<String>, ProviderError>;
}

/// Input used to create or replace a provider configuration.
#[derive(Clone, Debug)]
pub struct ProviderInput {
    /// Admin-visible name.
    pub name: String,
    /// Adapter family.
    pub kind: ProviderKind,
    /// Base URL controlled by Admin configuration.
    pub base_url: Url,
    /// Typed transport/privacy settings.
    pub settings: ProviderSettings,
    /// Whether the provider is enabled.
    pub enabled: bool,
    /// New secret, shown only at the caller's one-time boundary.
    pub secret: Option<SecretString>,
}

/// Typed model registration input.
#[derive(Clone, Debug)]
pub struct ModelInput {
    /// Provider identity.
    pub provider_id: ProviderId,
    /// External model name.
    pub external_model_id: String,
    /// Capability metadata.
    pub capabilities: ModelCapabilities,
    /// Typed model settings.
    pub settings: ModelSettings,
    /// Whether this model is enabled.
    pub enabled: bool,
}

/// Embedding source plus untrusted source text supplied by an owning service.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingInput {
    /// Stable source reference.
    pub source: EmbeddingSourceRef,
    /// Text to send to the embedding provider.
    pub text: String,
}

/// Effective per-Vault provider privacy mode and optimistic revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderModeState {
    /// Effective privacy mode; absent settings resolve to `disabled`.
    pub mode: ProviderMode,
    /// Persisted optimistic revision, or `None` for the implicit default.
    pub revision: Option<Revision>,
}

/// Provider/model application service independent of Admin HTTP.
#[derive(Clone)]
pub struct ProviderService {
    state: StateStore,
    auth: AuthService,
    vector: Arc<dyn VectorIndex>,
    transports: Arc<Mutex<HashMap<ProviderId, CachedTransport>>>,
    /// Provider configuration mutations are rare Admin operations. Holding
    /// this process-wide lock across their state/secret I/O prevents a delete
    /// from racing a secret-bearing update into an orphaned secret row.
    lifecycle: Arc<AsyncMutex<()>>,
    generation_budget: Option<Arc<dyn crate::RequestBudget>>,
}

#[derive(Clone)]
struct CachedTransport {
    revision: Revision,
    transport: ProviderTransport,
}

impl ProviderService {
    /// Construct provider services with the mandatory exact SQLite vector
    /// backend.
    pub fn new(state: StateStore, auth: AuthService) -> Self {
        let vector = Arc::new(SqliteVectorIndex::new(state.clone()));
        Self {
            state,
            auth,
            vector,
            transports: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: Arc::new(AsyncMutex::new(())),
            generation_budget: None,
        }
    }

    /// Construct with a substitute vector backend at a real test/extension
    /// boundary.
    pub fn with_vector_index(
        state: StateStore,
        auth: AuthService,
        vector: Arc<dyn VectorIndex>,
    ) -> Self {
        Self {
            state,
            auth,
            vector,
            transports: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: Arc::new(AsyncMutex::new(())),
            generation_budget: None,
        }
    }

    /// Limit every generation HTTP attempt, including transport retries, for
    /// this service instance. Existing Provider configuration is unchanged.
    pub fn with_generation_budget(mut self, budget: Arc<dyn crate::RequestBudget>) -> Self {
        self.generation_budget = Some(budget);
        self
    }

    /// Create a provider and optionally save its encrypted API secret.
    pub async fn create_provider(
        &self,
        input: ProviderInput,
    ) -> Result<ProviderRecord, ProviderError> {
        input.settings.validate()?;
        validate_provider_url(&input.base_url)?;
        let id = ProviderId::new();
        let now = now_millis();
        let mut record = ProviderRecord {
            id,
            name: input.name,
            provider_type: input.kind.as_str().to_owned(),
            base_url: input.base_url.to_string(),
            secret_id: None,
            settings: serde_json::to_value(&input.settings).map_err(|_| {
                ProviderError::InvalidConfiguration("provider settings are invalid")
            })?,
            enabled: input.enabled,
            revision: Revision::new(1),
            embedding_revision: Revision::new(1),
            created_at: now,
            updated_at: now,
        };
        self.state.providers().insert_provider(&record).await?;
        if let Some(secret) = input.secret {
            let metadata = self
                .auth
                .put_installation_secret(
                    PROVIDER_SECRET_PURPOSE,
                    PROVIDER_SECRET_OWNER,
                    Some(&id.to_string()),
                    &secret,
                )
                .await?;
            record.secret_id = Some(metadata.id);
            record = self.state.providers().update_provider(&record).await?;
        }
        self.state
            .providers()
            .upsert_health(&ProviderHealthRecord {
                provider_id: id,
                status: "unknown".to_owned(),
                checked_at: None,
                latency_ms: None,
                model_count: 0,
                last_success_at: None,
                last_error: None,
                updated_at: now,
            })
            .await?;
        Ok(record)
    }

    /// Update provider configuration under its optimistic revision.
    pub async fn update_provider(
        &self,
        mut record: ProviderRecord,
        input: ProviderInput,
    ) -> Result<ProviderRecord, ProviderError> {
        input.settings.validate()?;
        validate_provider_url(&input.base_url)?;
        let _lifecycle = self.lifecycle.lock().await;
        let current = self
            .state
            .providers()
            .get_provider(record.id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        if current.revision != record.revision {
            return Err(StateError::InvalidDomain(DomainError::RevisionConflict {
                expected: record.revision,
                current: current.revision,
            })
            .into());
        }
        record = current;
        record.name = input.name;
        record.provider_type = input.kind.as_str().to_owned();
        record.base_url = input.base_url.to_string();
        record.settings = serde_json::to_value(&input.settings)
            .map_err(|_| ProviderError::InvalidConfiguration("provider settings are invalid"))?;
        record.enabled = input.enabled;
        if let Some(secret) = input.secret {
            let metadata = self
                .auth
                .put_installation_secret(
                    PROVIDER_SECRET_PURPOSE,
                    PROVIDER_SECRET_OWNER,
                    Some(&record.id.to_string()),
                    &secret,
                )
                .await?;
            record.secret_id = Some(metadata.id);
        }
        Ok(self.state.providers().update_provider(&record).await?)
    }

    /// List provider configurations without secret plaintext.
    pub async fn list_providers(&self) -> Result<Vec<ProviderRecord>, ProviderError> {
        Ok(self.state.providers().list_providers(1000).await?)
    }

    /// Fetch one provider configuration.
    pub async fn get_provider(
        &self,
        provider_id: ProviderId,
    ) -> Result<ProviderRecord, ProviderError> {
        self.state
            .providers()
            .get_provider(provider_id)
            .await?
            .ok_or(ProviderError::NotFound)
    }

    /// Delete one provider and its dependent operational/derived state through
    /// the application boundary.
    pub async fn delete_provider(
        &self,
        provider_id: ProviderId,
        expected_revision: Option<Revision>,
    ) -> Result<ProviderDeletionSummary, ProviderError> {
        let _lifecycle = self.lifecycle.lock().await;
        let summary = self
            .state
            .providers()
            .delete_provider(provider_id, expected_revision)
            .await?
            .ok_or(ProviderError::NotFound)?;
        self.transports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&provider_id);
        Ok(summary)
    }

    /// List registered models for one provider or all providers.
    pub async fn list_models(
        &self,
        provider_id: Option<ProviderId>,
    ) -> Result<Vec<ModelRecord>, ProviderError> {
        Ok(self
            .state
            .providers()
            .list_models(provider_id, 1000)
            .await?)
    }

    /// Read one exact model binding through the application boundary.
    pub async fn get_binding(
        &self,
        context: Option<&VaultContext>,
        role: &str,
    ) -> Result<Option<mcp_vault_state::ModelBindingRecord>, ProviderError> {
        Ok(self.state.providers().get_binding(context, role).await?)
    }

    /// Register a model returned by discovery or entered by Admin.
    pub async fn register_model(&self, input: ModelInput) -> Result<ModelRecord, ProviderError> {
        let provider = self.get_provider(input.provider_id).await?;
        if !provider.enabled {
            return Err(ProviderError::Disabled);
        }
        input.capabilities.validate()?;
        if input.settings.generation_token_limit.is_some() && !input.capabilities.structured_output
        {
            return Err(ProviderError::InvalidConfiguration(
                "generation-token limit requires structured generation capability",
            ));
        }
        let provider_kind = ProviderKind::try_from(provider.provider_type.as_str())?;
        let provider_url = Url::parse(&provider.base_url)?;
        input.settings.validate_for_model(
            provider_kind,
            provider_url.host_str(),
            &input.external_model_id,
        )?;
        let record = ModelRecord {
            id: ModelId::new(),
            provider_id: input.provider_id,
            external_model_id: input.external_model_id,
            capabilities: serde_json::to_value(input.capabilities).map_err(|_| {
                ProviderError::InvalidConfiguration("model capabilities are invalid")
            })?,
            settings: serde_json::to_value(input.settings)
                .map_err(|_| ProviderError::InvalidConfiguration("model settings are invalid"))?,
            enabled: input.enabled,
            revision: Revision::new(1),
            embedding_revision: Revision::new(1),
            created_at: now_millis(),
            updated_at: now_millis(),
        };
        Ok(self.state.providers().insert_model(&record).await?)
    }

    /// Bind one model to a global role or Vault-specific override.
    pub async fn bind_model(
        &self,
        context: Option<&VaultContext>,
        role: &str,
        model_id: ModelId,
        settings: Value,
        expected_revision: Option<Revision>,
    ) -> Result<mcp_vault_state::ModelBindingRecord, ProviderError> {
        self.state
            .providers()
            .upsert_binding(context, role, model_id, &settings, expected_revision)
            .await
            .map_err(ProviderError::State)
    }

    /// Set a Vault's provider privacy mode through typed settings.
    pub async fn set_provider_mode(
        &self,
        context: &VaultContext,
        mode: ProviderMode,
        expected_revision: Option<Revision>,
    ) -> Result<mcp_vault_state::SettingRecord, ProviderError> {
        let precondition = expected_revision.map_or(
            WritePrecondition::Unconditional,
            WritePrecondition::ExactRevision,
        );
        Ok(self
            .state
            .settings()
            .set_vault(
                context,
                PROVIDER_MODE_SETTING,
                &json!(mode),
                precondition,
                None,
            )
            .await?)
    }

    /// Resolve the effective Vault provider privacy mode.
    pub async fn provider_mode(
        &self,
        context: &VaultContext,
    ) -> Result<ProviderMode, ProviderError> {
        Ok(self.provider_mode_state(context).await?.mode)
    }

    /// Resolve the mode together with the setting revision exposed to Admin.
    pub async fn provider_mode_state(
        &self,
        context: &VaultContext,
    ) -> Result<ProviderModeState, ProviderError> {
        let Some(setting) = self
            .state
            .settings()
            .get_vault(context, PROVIDER_MODE_SETTING)
            .await?
        else {
            return Ok(ProviderModeState {
                mode: ProviderMode::Disabled,
                revision: None,
            });
        };
        let mode = serde_json::from_value(setting.value)
            .map_err(|_| ProviderError::InvalidConfiguration("provider mode is invalid"))?;
        Ok(ProviderModeState {
            mode,
            revision: Some(setting.revision),
        })
    }

    /// Generate structured JSON using an explicitly selected model.
    pub async fn generate_structured(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        let runtime = self.runtime(context, model_id).await?;
        if request.model != runtime.model.external_model_id {
            return Err(ProviderError::InvalidConfiguration(
                "request model does not match registered model",
            ));
        }
        let generation_token_limit = runtime.model_settings.effective_generation_token_limit(
            runtime.provider_kind,
            runtime.base_url.host_str(),
            &runtime.capabilities,
            request.max_output_tokens,
        );
        let transport = self.generation_budget.as_ref().map_or_else(
            || runtime.transport.clone(),
            |budget| runtime.transport.clone().with_budget(budget.clone()),
        );
        let started = std::time::Instant::now();
        let result = runtime
            .adapter
            .generate_structured(
                &transport,
                &runtime.base_url,
                runtime.mode,
                GenerationOptions::new(
                    &runtime.model_settings,
                    generation_token_limit,
                    runtime.secret.as_ref(),
                ),
                request,
            )
            .await;
        if let Ok(output) = &result {
            let usage = output.usage.as_ref();
            let purpose = match request.schema_name.as_str() {
                "memory_unit_selection" => "memory_unit_selection",
                "memory_overview" => "memory_overview",
                _ => "structured_generation",
            };
            tracing::info!(target:"mcp_vault::providers",event="provider_generation_completed",vault_id=%context.id(),model_id=%model_id,purpose,
                input_tokens=?usage.and_then(|usage|usage.get("prompt_tokens").or_else(||usage.get("input_tokens"))).and_then(serde_json::Value::as_u64),
                output_tokens=?usage.and_then(|usage|usage.get("completion_tokens").or_else(||usage.get("output_tokens"))).and_then(serde_json::Value::as_u64),
                cached_tokens=?usage.and_then(|usage|usage.pointer("/prompt_tokens_details/cached_tokens")).and_then(serde_json::Value::as_u64),
                elapsed_ms=started.elapsed().as_millis() as u64,"structured generation completed");
        }
        result
    }

    /// Generate structured JSON using the effective role binding.
    pub async fn generate_for_role(
        &self,
        context: &VaultContext,
        role: &str,
        request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        let binding = self
            .state
            .providers()
            .resolve_binding(context, role)
            .await?
            .ok_or(ProviderError::NotFound)?;
        self.generate_structured(context, binding.model_id, request)
            .await
    }

    /// Generate embeddings with the selected model and dimension checks.
    pub async fn embed(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError> {
        let runtime = self.runtime(context, model_id).await?;
        validate_embedding_inputs(&runtime.model, request)?;
        let result = runtime
            .adapter
            .embed(
                &runtime.transport,
                &runtime.base_url,
                runtime.mode,
                &runtime.settings,
                runtime.secret.as_ref(),
                request,
            )
            .await?;
        validate_model_dimensions(&runtime.model, &result)?;
        tracing::info!(target:"mcp_vault::providers",event="provider_embedding_completed",vault_id=%context.id(),model_id=%model_id,inputs=request.inputs.len(),
            input_tokens=?result.usage.as_ref().and_then(|usage|usage.get("prompt_tokens").or_else(||usage.get("total_tokens"))).and_then(serde_json::Value::as_u64),"embedding completed");
        Ok(result)
    }

    /// Execute real embedding adapters with persistent per-attempt accounting.
    /// All ordinary Provider authorization and model checks still apply.
    pub async fn embed_with_budget(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        request: &EmbeddingRequest,
        budget: Arc<dyn crate::RequestBudget>,
    ) -> Result<EmbeddingResult, ProviderError> {
        let runtime = self.runtime(context, model_id).await?;
        validate_embedding_inputs(&runtime.model, request)?;
        let transport = runtime.transport.with_budget(budget);
        let result = runtime
            .adapter
            .embed(
                &transport,
                &runtime.base_url,
                runtime.mode,
                &runtime.settings,
                runtime.secret.as_ref(),
                request,
            )
            .await?;
        validate_model_dimensions(&runtime.model, &result)?;
        Ok(result)
    }

    /// Test one provider and update its redacted health row.
    pub async fn test_provider(
        &self,
        context: &VaultContext,
        provider_id: ProviderId,
    ) -> Result<Vec<mcp_vault_state::ModelRecord>, ProviderError> {
        let provider = self.get_provider(provider_id).await?;
        let kind = ProviderKind::try_from(provider.provider_type.as_str())?;
        let settings = ProviderSettings::from_json(&provider.settings)?;
        let base_url = Url::parse(&provider.base_url)?;
        let mode = self.provider_mode(context).await?;
        let secret = self.read_secret(&provider).await?;
        let transport = self.transport_for(&provider, &settings)?;
        let started = std::time::Instant::now();
        let adapter = adapter_for(kind);
        let result = adapter
            .list_models(&transport, &base_url, mode, secret.as_ref())
            .await;
        match result {
            Ok(models) => {
                let mut records = Vec::with_capacity(models.len());
                for discovered in models {
                    let record = self
                        .upsert_discovered_model(provider_id, discovered)
                        .await?;
                    records.push(record);
                }
                self.record_health(
                    provider_id,
                    "healthy",
                    Some(started.elapsed().as_millis() as u64),
                    records.len() as u64,
                    None,
                )
                .await?;
                Ok(records)
            }
            Err(error) => {
                let status = if error.retryable() {
                    "degraded"
                } else {
                    "unavailable"
                };
                self.record_health(
                    provider_id,
                    status,
                    Some(started.elapsed().as_millis() as u64),
                    0,
                    Some(error.code().to_owned()),
                )
                .await?;
                Err(error)
            }
        }
    }

    fn transport_for(
        &self,
        provider: &ProviderRecord,
        settings: &ProviderSettings,
    ) -> Result<ProviderTransport, ProviderError> {
        let mut transports = self.transports.lock().map_err(|_| {
            ProviderError::InvalidConfiguration("provider transport cache is unavailable")
        })?;
        if let Some(cached) = transports.get(&provider.id)
            && cached.revision == provider.revision
        {
            return Ok(cached.transport.clone());
        }
        let transport = ProviderTransport::new(settings.clone())?;
        transports.insert(
            provider.id,
            CachedTransport {
                revision: provider.revision,
                transport: transport.clone(),
            },
        );
        Ok(transport)
    }

    /// Return a redacted provider health row.
    pub async fn health(
        &self,
        provider_id: ProviderId,
    ) -> Result<Option<ProviderHealthRecord>, ProviderError> {
        Ok(self.state.providers().get_health(provider_id).await?)
    }

    /// Return the vector application service using this provider service.
    pub fn embeddings(&self) -> EmbeddingService {
        EmbeddingService {
            provider: self.clone(),
            vector: self.vector.clone(),
        }
    }

    async fn runtime(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<Runtime, ProviderError> {
        let model = self
            .state
            .providers()
            .get_model(model_id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        if !model.enabled {
            return Err(ProviderError::Disabled);
        }
        let provider = self.get_provider(model.provider_id).await?;
        if !provider.enabled {
            return Err(ProviderError::Disabled);
        }
        let kind = ProviderKind::try_from(provider.provider_type.as_str())?;
        let settings = ProviderSettings::from_json(&provider.settings)?;
        let capabilities = ModelCapabilities::from_json(&model.capabilities)?;
        let model_settings = ModelSettings::from_json(&model.settings)?;
        let base_url = Url::parse(&provider.base_url)?;
        model_settings.validate_for_model(kind, base_url.host_str(), &model.external_model_id)?;
        let mode = self.provider_mode(context).await?;
        let secret = self.read_secret(&provider).await?;
        let transport = self.transport_for(&provider, &settings)?;
        Ok(Runtime {
            model,
            capabilities,
            base_url,
            settings,
            model_settings,
            provider_kind: kind,
            mode,
            secret,
            transport,
            adapter: adapter_for(kind),
        })
    }

    async fn read_secret(
        &self,
        provider: &ProviderRecord,
    ) -> Result<Option<SecretString>, ProviderError> {
        let Some(secret_id) = provider.secret_id else {
            return Ok(None);
        };
        Ok(Some(
            self.auth
                .read_installation_secret(
                    secret_id,
                    PROVIDER_SECRET_PURPOSE,
                    PROVIDER_SECRET_OWNER,
                    Some(&provider.id.to_string()),
                )
                .await
                .map_err(ProviderError::Auth)?,
        ))
    }

    async fn upsert_discovered_model(
        &self,
        provider_id: ProviderId,
        discovered: crate::DiscoveredModel,
    ) -> Result<ModelRecord, ProviderError> {
        let existing = self
            .state
            .providers()
            .list_models(Some(provider_id), 1000)
            .await?
            .into_iter()
            .find(|model| model.external_model_id == discovered.id);
        if let Some(mut existing) = existing {
            existing.capabilities = discovered.capabilities;
            existing.enabled = true;
            return Ok(self.state.providers().update_model(&existing).await?);
        }
        self.register_model(ModelInput {
            provider_id,
            external_model_id: discovered.id,
            capabilities: ModelCapabilities::from_json(&discovered.capabilities)?,
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
    }

    async fn record_health(
        &self,
        provider_id: ProviderId,
        status: &str,
        latency_ms: Option<u64>,
        model_count: u64,
        last_error: Option<String>,
    ) -> Result<(), ProviderError> {
        let now = now_millis();
        self.state
            .providers()
            .upsert_health(&ProviderHealthRecord {
                provider_id,
                status: status.to_owned(),
                checked_at: Some(now),
                latency_ms,
                model_count,
                last_success_at: (status == "healthy").then_some(now),
                last_error,
                updated_at: now,
            })
            .await?;
        Ok(())
    }
}

struct Runtime {
    model: ModelRecord,
    capabilities: ModelCapabilities,
    base_url: Url,
    settings: ProviderSettings,
    model_settings: ModelSettings,
    provider_kind: ProviderKind,
    mode: ProviderMode,
    secret: Option<SecretString>,
    transport: ProviderTransport,
    adapter: Box<dyn ProviderAdapter>,
}

/// Application service for embedding persistence, vector search, and
/// re-embedding job admission.
#[derive(Clone)]
pub struct EmbeddingService {
    provider: ProviderService,
    vector: Arc<dyn VectorIndex>,
}

impl EmbeddingService {
    /// Return the stable fingerprint of the complete configuration that can
    /// affect vectors produced by one model. Secrets are deliberately absent.
    pub async fn profile_hash(&self, model_id: ModelId) -> Result<String, ProviderError> {
        let model = self
            .provider
            .state
            .providers()
            .get_model(model_id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        let provider = self
            .provider
            .state
            .providers()
            .get_provider(model.provider_id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        Ok(embedding_profile_hash(&provider, &model))
    }

    /// Generate and persist a batch of source embeddings.
    pub async fn embed_and_store(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        inputs: &[EmbeddingInput],
    ) -> Result<Vec<EmbeddingRecord>, ProviderError> {
        let prepared = self.prepare_embeddings(context, model_id, inputs).await?;
        let mut records = Vec::with_capacity(prepared.len());
        for (record, vector) in prepared {
            self.vector.upsert(context, &record, &vector).await?;
            records.push(record);
        }
        Ok(records)
    }

    async fn prepare_embeddings(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        inputs: &[EmbeddingInput],
    ) -> Result<Vec<(EmbeddingRecord, Vec<f32>)>, ProviderError> {
        if inputs.is_empty() || inputs.len() > 128 {
            return Err(ProviderError::InvalidConfiguration(
                "embedding input batch is invalid",
            ));
        }
        if inputs.iter().any(|input| input.text.len() > 1_000_000) {
            return Err(ProviderError::InvalidConfiguration(
                "embedding input is too large",
            ));
        }
        let model = self
            .provider
            .state
            .providers()
            .get_model(model_id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        let provider = self
            .provider
            .state
            .providers()
            .get_provider(model.provider_id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        let profile_hash = embedding_profile_hash(&provider, &model);
        let result = self
            .provider
            .embed(
                context,
                model_id,
                &EmbeddingRequest {
                    model: model.external_model_id.clone(),
                    inputs: inputs.iter().map(|input| input.text.clone()).collect(),
                },
            )
            .await?;
        if result.vectors.len() != inputs.len() {
            return Err(ProviderError::InvalidResponse(
                "embedding result count does not match input count",
            ));
        }
        let provider_id = model.provider_id;
        let dimension = result
            .vectors
            .first()
            .map(|vector| vector.len() as u32)
            .ok_or(ProviderError::InvalidResponse("embedding result is empty"))?;
        if dimension == 0
            || result.vectors.iter().any(|vector| {
                let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
                vector.len() != dimension as usize
                    || vector.iter().any(|value| !value.is_finite())
                    || !norm.is_finite()
                    || norm <= f32::EPSILON
            })
        {
            return Err(ProviderError::InvalidResponse(
                "embedding result contains an invalid vector",
            ));
        }
        let mut records = Vec::with_capacity(inputs.len());
        for (input, vector) in inputs.iter().zip(result.vectors) {
            let now = now_millis();
            let record = EmbeddingRecord {
                id: new_embedding_id(),
                vault_id: context.id(),
                object_type: input.source.object_type.clone(),
                object_id: input.source.object_id.clone(),
                chunk_key: input.source.chunk_key.clone(),
                provider_id,
                model_id,
                dimension,
                content_hash: input.source.content_hash.clone(),
                profile_hash: profile_hash.clone(),
                input_hash: embedding_input_hash(&profile_hash, &input.source, &input.text),
                vector_backend_key: format!(
                    "{}:{}:{}:{}",
                    context.id(),
                    input.source.object_type,
                    input.source.object_id,
                    input.source.chunk_key
                ),
                created_at: now,
                updated_at: now,
            };
            records.push((record, vector));
        }
        Ok(records)
    }

    /// Resolve current source bodies through the owning application service
    /// and rebuild their vectors. Missing/stale sources are skipped without
    /// deleting canonical data.
    pub async fn reembed_with_resolver<R: EmbeddingSourceResolver + ?Sized>(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        sources: &[EmbeddingSourceRef],
        resolver: &R,
    ) -> Result<Vec<EmbeddingRecord>, ProviderError> {
        let profile_hash = self.profile_hash(model_id).await?;
        let mut records = Vec::new();
        let mut inputs = Vec::new();
        for source in sources {
            if let Some(text) = resolver.resolve_source(context, source).await? {
                let input_hash = embedding_input_hash(&profile_hash, source, &text);
                if let Some(existing) = self
                    .provider
                    .state
                    .providers()
                    .find_valid_embedding_for_object(
                        context,
                        model_id,
                        &profile_hash,
                        &input_hash,
                        &source.object_id,
                    )
                    .await?
                {
                    records.push(existing);
                    continue;
                }
                if let Some(cached) = self
                    .provider
                    .state
                    .providers()
                    .find_valid_vector_by_input(context, model_id, &profile_hash, &input_hash)
                    .await?
                    && cached.embedding.object_type == source.object_type
                    && cached.embedding.chunk_key == source.chunk_key
                    && cached.embedding.content_hash == source.content_hash
                {
                    let still_current = resolver
                        .resolve_source(context, source)
                        .await?
                        .is_some_and(|current| current == text);
                    if !still_current {
                        continue;
                    }
                    let mut record = cached.embedding;
                    record.id = new_embedding_id();
                    record.object_id = source.object_id.clone();
                    record.vector_backend_key = format!(
                        "{}:{}:{}:{}",
                        context.id(),
                        source.object_type,
                        source.object_id,
                        source.chunk_key
                    );
                    record.created_at = now_millis();
                    record.updated_at = record.created_at;
                    self.vector.upsert(context, &record, &cached.vector).await?;
                    records.push(record);
                    continue;
                }
                inputs.push(EmbeddingInput {
                    source: source.clone(),
                    text,
                });
            }
        }
        if inputs.is_empty() {
            return Ok(records);
        }
        let prepared = self.prepare_embeddings(context, model_id, &inputs).await?;
        records.reserve(prepared.len());
        for ((record, vector), input) in prepared.into_iter().zip(&inputs) {
            let still_current = resolver
                .resolve_source(context, &input.source)
                .await?
                .is_some_and(|text| text == input.text);
            if !still_current {
                continue;
            }
            self.vector.upsert(context, &record, &vector).await?;
            records.push(record);
        }
        Ok(records)
    }

    /// Search one Vault/model/object-type/dimension partition for raw candidates.
    pub async fn search(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        object_type: &str,
        query: &[f32],
        limit: u32,
    ) -> Result<Vec<VectorHit>, ProviderError> {
        let model = self
            .provider
            .state
            .providers()
            .get_model(model_id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        if let Some(expected) = ModelCapabilities::from_json(&model.capabilities)?.dimension
            && query.len() as u32 != expected
        {
            return Err(ProviderError::DimensionMismatch);
        }
        self.vector
            .search(
                context,
                model_id,
                object_type,
                query.len() as u32,
                query,
                limit,
            )
            .await
    }

    /// Return derived embedding coverage for one Vault/model partition.
    pub async fn coverage(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<EmbeddingCoverage, ProviderError> {
        Ok(self
            .provider
            .state
            .providers()
            .embedding_coverage(context, model_id)
            .await?)
    }

    /// Schedule a durable reference-only re-embedding job.
    pub async fn schedule_reembedding(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        sources: &[EmbeddingSourceRef],
    ) -> Result<mcp_vault_state::JobRecord, ProviderError> {
        if sources.is_empty() || sources.len() > 10_000 {
            return Err(ProviderError::InvalidConfiguration(
                "re-embedding source batch is invalid",
            ));
        }
        let payload = json!({
            "projection_version": EMBEDDING_PROJECTION_VERSION,
            // Repair previously completed cache hits that returned another
            // object's metadata. Vector inputs/profile stay compatible.
            "reference_version": 2,
            "model_id": model_id,
            "sources": sources,
        });
        let dedup_key = format!(
            "vault:{}:embedding:v{}:{}:{}",
            context.id(),
            EMBEDDING_PROJECTION_VERSION,
            model_id,
            hash_json(&payload)
        );
        Ok(self
            .provider
            .state
            .jobs()
            .enqueue(
                context,
                "embedding.rebuild",
                &dedup_key,
                &payload,
                0,
                10,
                now_millis(),
            )
            .await?)
    }

    /// Delete one model's derived vectors while leaving source state intact.
    pub async fn delete_model_vectors(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<u64, ProviderError> {
        self.vector.delete_model(context, model_id).await
    }

    /// Remove every derived embedding for one exact current source object.
    /// This is model-agnostic so deleting/replacing canonical content cannot
    /// leave an older model/profile partition online.
    pub async fn delete_object_vectors(
        &self,
        context: &VaultContext,
        object_type: &str,
        object_id: &str,
    ) -> Result<u64, ProviderError> {
        self.vector
            .delete_object(context, object_type, object_id)
            .await
    }
}

/// Hash the exact input and preparation fields under one embedding profile.
/// Object IDs are deliberately omitted so values can be reused; callers must
/// separately bind the stored record to the actual object identity.
pub fn embedding_input_hash(profile_hash: &str, source: &EmbeddingSourceRef, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mcp-vault-embedding-input-v1\0");
    hasher.update(profile_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(source.object_type.as_bytes());
    hasher.update(b"\0");
    hasher.update(source.chunk_key.as_bytes());
    hasher.update(b"\0");
    hasher.update(source.content_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(text.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn embedding_profile_hash(provider: &ProviderRecord, model: &ModelRecord) -> String {
    let value = json!({
        "contract": "mcp-vault-embedding-profile-v1",
        "projection_version": EMBEDDING_PROJECTION_VERSION,
        "provider": {
            "id": provider.id,
            "type": provider.provider_type,
            "base_url": provider.base_url,
            "settings": provider.settings,
            "revision": provider.embedding_revision.value(),
        },
        "model": {
            "id": model.id,
            "external_model_id": model.external_model_id,
            "capabilities": model.capabilities,
            "settings": model.settings,
            "revision": model.embedding_revision.value(),
        }
    });
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn adapter_for(kind: ProviderKind) -> Box<dyn ProviderAdapter> {
    match kind {
        ProviderKind::OpenAiResponses => Box::new(OpenAiResponsesAdapter),
        ProviderKind::OpenAiCompatible
        | ProviderKind::DeepSeek
        | ProviderKind::XiaomiMimo
        | ProviderKind::ZhipuGlm
        | ProviderKind::MoonshotKimi
        | ProviderKind::GoogleGemini
        | ProviderKind::AlibabaQwen => Box::new(OpenAiCompatibleAdapter::new(kind)),
        ProviderKind::AnthropicMessages => Box::new(AnthropicMessagesAdapter),
        ProviderKind::EmbeddingHttp => Box::new(HttpEmbeddingAdapter),
        ProviderKind::FastEmbedLocal => Box::new(FastEmbedAdapter),
    }
}

fn validate_provider_url(url: &Url) -> Result<(), ProviderError> {
    if url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderError::InvalidConfiguration(
            "provider base URL must be an origin/path without credentials or query",
        ));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ProviderError::InvalidConfiguration(
            "provider base URL must use HTTP or HTTPS",
        ));
    }
    Ok(())
}

// Defaults are resource ceilings, never requests to use a model's maximum.
const DEFAULT_EMBEDDING_INPUT_BYTES: usize = 8_192;
const DEFAULT_EMBEDDING_DIMENSION_LIMIT: usize = 8_192;

fn validate_embedding_inputs(
    model: &ModelRecord,
    request: &EmbeddingRequest,
) -> Result<(), ProviderError> {
    let cap = ModelCapabilities::from_json(&model.capabilities)?;
    // Conservative bytes-as-token bound when a tokenizer is unavailable.
    let limit = cap
        .context_window
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_EMBEDDING_INPUT_BYTES);
    if request.inputs.iter().any(|input| input.len() > limit) {
        return Err(ProviderError::InvalidConfiguration(
            "embedding input exceeds configured or default bound",
        ));
    }
    Ok(())
}

fn validate_model_dimensions(
    model: &ModelRecord,
    result: &EmbeddingResult,
) -> Result<(), ProviderError> {
    let expected = ModelCapabilities::from_json(&model.capabilities)?.dimension;
    let limit = expected
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_EMBEDDING_DIMENSION_LIMIT);
    if result.vectors.iter().any(|vector| {
        vector.is_empty()
            || vector.len() > limit
            || expected.is_some_and(|n| vector.len() != n as usize)
    }) {
        return Err(ProviderError::DimensionMismatch);
    }
    Ok(())
}

fn hash_json(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use mcp_vault_auth::{AuthService, MasterKeyRing};
    use mcp_vault_domain::{ProviderId, Revision};
    use mcp_vault_state::{ProviderRecord, StateStore};

    use super::ProviderService;
    use crate::ProviderSettings;

    #[test]
    fn unset_embedding_capabilities_still_enforce_resource_bounds() {
        let mut model = mcp_vault_state::ModelRecord {
            id: mcp_vault_domain::ModelId::new(),
            provider_id: ProviderId::new(),
            external_model_id: "default".into(),
            capabilities: serde_json::json!({"embeddings":true}),
            settings: serde_json::json!({}),
            enabled: true,
            revision: Revision::new(1),
            embedding_revision: Revision::new(1),
            created_at: 0,
            updated_at: 0,
        };
        let mut request = crate::EmbeddingRequest {
            model: "default".into(),
            inputs: vec!["x".repeat(8192)],
        };
        assert!(super::validate_embedding_inputs(&model, &request).is_ok());
        request.inputs[0].push('x');
        assert!(super::validate_embedding_inputs(&model, &request).is_err());
        let result = crate::EmbeddingResult {
            vectors: vec![vec![1.0; 8193]],
            model: None,
            usage: None,
        };
        assert!(super::validate_model_dimensions(&model, &result).is_err());
        model.capabilities =
            serde_json::json!({"embeddings":true,"dimension":256,"context_window":1024});
        assert!(super::validate_embedding_inputs(&model, &request).is_err());
        assert!(
            super::validate_model_dimensions(
                &model,
                &crate::EmbeddingResult {
                    vectors: vec![vec![1.0; 256]],
                    model: None,
                    usage: None,
                }
            )
            .is_ok()
        );
        assert!(
            super::validate_model_dimensions(
                &model,
                &crate::EmbeddingResult {
                    vectors: vec![vec![1.0; 2048]],
                    model: None,
                    usage: None,
                }
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn provider_transport_cache_is_shared_and_revision_aware() {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
        );
        let service = ProviderService::new(state, auth);
        let mut provider = ProviderRecord {
            id: ProviderId::new(),
            name: "test".to_owned(),
            provider_type: "openai_compatible".to_owned(),
            base_url: "https://provider.example.test/v1/".to_owned(),
            secret_id: None,
            settings: serde_json::json!({}),
            enabled: true,
            revision: Revision::new(1),
            embedding_revision: Revision::new(1),
            created_at: 1,
            updated_at: 1,
        };
        service
            .transport_for(&provider, &ProviderSettings::default())
            .unwrap();
        service
            .transport_for(&provider, &ProviderSettings::default())
            .unwrap();
        let cache = service.transports.lock().unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&provider.id).unwrap().revision, Revision::new(1));
        drop(cache);

        provider.revision = Revision::new(2);
        service
            .transport_for(&provider, &ProviderSettings::default())
            .unwrap();
        let cache = service.transports.lock().unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&provider.id).unwrap().revision, Revision::new(2));
    }
}
