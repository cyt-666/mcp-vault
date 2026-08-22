//! SQL repositories for provider configuration, model bindings, health, and
//! Vault-scoped embedding/vector projections.

use serde_json::Value;
use sqlx::{FromRow, SqlitePool};

use mcp_vault_domain::{
    EmbeddingId, ModelId, ProviderId, Revision, SecretId, VaultContext, VaultId,
};

use crate::{StateError, now_millis};

const MAX_PROVIDER_LIMIT: u32 = 1000;
const MAX_VECTOR_LIMIT: u32 = 10_000;

/// Global provider configuration without secret plaintext.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRecord {
    /// Stable provider identity.
    pub id: ProviderId,
    /// Admin-visible provider name.
    pub name: String,
    /// Stable adapter kind owned by the providers crate.
    pub provider_type: String,
    /// Configured endpoint; secrets are never embedded here.
    pub base_url: String,
    /// Installation-encrypted secret metadata reference.
    pub secret_id: Option<SecretId>,
    /// Typed provider settings as validated JSON.
    pub settings: Value,
    /// Whether this provider may receive requests.
    pub enabled: bool,
    /// Optimistic configuration revision.
    pub revision: Revision,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last configuration update timestamp.
    pub updated_at: i64,
}

/// A model advertised or manually registered for one provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelRecord {
    /// Stable model identity.
    pub id: ModelId,
    /// Owning provider.
    pub provider_id: ProviderId,
    /// Provider-specific model identifier.
    pub external_model_id: String,
    /// Capability metadata such as embedding dimension.
    pub capabilities: Value,
    /// Model-specific validated settings.
    pub settings: Value,
    /// Whether this model can be selected.
    pub enabled: bool,
    /// Optimistic configuration revision.
    pub revision: Revision,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last update timestamp.
    pub updated_at: i64,
}

/// Global or Vault-specific role binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelBindingRecord {
    /// Binding identity.
    pub id: String,
    /// None means global default; Some means a Vault override.
    pub vault_id: Option<VaultId>,
    /// Stable role such as embedding_note or memory_extraction.
    pub role: String,
    /// Selected model.
    pub model_id: ModelId,
    /// Role-specific validated settings.
    pub settings: Value,
    /// Optimistic configuration revision.
    pub revision: Revision,
    /// Last update timestamp.
    pub updated_at: i64,
}

/// Redacted provider health snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderHealthRecord {
    /// Provider identity.
    pub provider_id: ProviderId,
    /// unknown, healthy, degraded, or unavailable.
    pub status: String,
    /// Last test/check timestamp.
    pub checked_at: Option<i64>,
    /// Last observed request latency.
    pub latency_ms: Option<u64>,
    /// Number of models observed during discovery.
    pub model_count: u64,
    /// Last successful check.
    pub last_success_at: Option<i64>,
    /// Redacted stable failure code.
    pub last_error: Option<String>,
    /// Health row update timestamp.
    pub updated_at: i64,
}

/// Metadata for one stored embedding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingRecord {
    /// Stable embedding row identity.
    pub id: EmbeddingId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// note, memory, or a future derived object category.
    pub object_type: String,
    /// Stable source object identity.
    pub object_id: String,
    /// Section/chunk identity within the source object.
    pub chunk_key: String,
    /// Provider that generated the vector.
    pub provider_id: ProviderId,
    /// Model that generated the vector.
    pub model_id: ModelId,
    /// Vector dimension.
    pub dimension: u32,
    /// Hash of the exact source chunk.
    pub content_hash: String,
    /// Backend-local stable key.
    pub vector_backend_key: String,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last replacement timestamp.
    pub updated_at: i64,
}

/// One vector candidate loaded from the rebuildable SQLite fallback.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorCandidate {
    /// Embedding metadata.
    pub embedding: EmbeddingRecord,
    /// Little-endian decoded vector values.
    pub vector: Vec<f32>,
}

/// Coarse embedding coverage for one Vault/model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingCoverage {
    /// Number of stored vectors.
    pub total: u64,
    /// Distinct object count.
    pub objects: u64,
    /// Distinct dimensions represented.
    pub dimensions: Vec<u32>,
}

#[derive(Debug, FromRow)]
struct ProviderRow {
    id: String,
    name: String,
    provider_type: String,
    base_url: String,
    secret_id: Option<String>,
    settings_json: String,
    enabled: i64,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct ModelRow {
    id: String,
    provider_id: String,
    external_model_id: String,
    capability_json: String,
    settings_json: String,
    enabled: i64,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct BindingRow {
    id: String,
    vault_id: Option<String>,
    role: String,
    model_id: String,
    settings_json: String,
    revision: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct HealthRow {
    provider_id: String,
    status: String,
    checked_at: Option<i64>,
    latency_ms: Option<i64>,
    model_count: i64,
    last_success_at: Option<i64>,
    last_error: Option<String>,
    updated_at: i64,
}

#[derive(Clone, Debug, FromRow)]
struct EmbeddingRow {
    id: String,
    vault_id: String,
    object_type: String,
    object_id: String,
    chunk_key: String,
    provider_id: String,
    model_id: String,
    dimension: i64,
    content_hash: String,
    vector_backend_key: String,
    created_at: i64,
    updated_at: i64,
    vector_blob: Vec<u8>,
}

/// SQL boundary for provider and vector state.
#[derive(Clone)]
pub struct ProviderRepository {
    pool: SqlitePool,
}

impl ProviderRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a global provider configuration.
    pub async fn insert_provider(
        &self,
        record: &ProviderRecord,
    ) -> Result<ProviderRecord, StateError> {
        validate_provider_record(record)?;
        sqlx::query(
            "INSERT INTO providers
             (id, name, provider_type, base_url, secret_id, settings_json,
              enabled, revision, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.id.to_string())
        .bind(&record.name)
        .bind(&record.provider_type)
        .bind(&record.base_url)
        .bind(record.secret_id.map(|id| id.to_string()))
        .bind(serde_json::to_string(&record.settings)?)
        .bind(if record.enabled { 1_i64 } else { 0_i64 })
        .bind(record.revision.as_i64()?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&self.pool)
        .await?;
        self.get_provider(record.id)
            .await?
            .ok_or(StateError::InvalidInput("inserted provider was not found"))
    }

    /// Fetch one provider configuration.
    pub async fn get_provider(
        &self,
        provider_id: ProviderId,
    ) -> Result<Option<ProviderRecord>, StateError> {
        let row = sqlx::query_as::<_, ProviderRow>(
            "SELECT id, name, provider_type, base_url, secret_id, settings_json,
                    enabled, revision, created_at, updated_at
             FROM providers WHERE id = ?",
        )
        .bind(provider_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_provider).transpose()
    }

    /// List providers in deterministic ID order.
    pub async fn list_providers(&self, limit: u32) -> Result<Vec<ProviderRecord>, StateError> {
        if limit == 0 || limit > MAX_PROVIDER_LIMIT {
            return Err(StateError::InvalidInput("provider page is invalid"));
        }
        let rows = sqlx::query_as::<_, ProviderRow>(
            "SELECT id, name, provider_type, base_url, secret_id, settings_json,
                    enabled, revision, created_at, updated_at
             FROM providers ORDER BY id ASC LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_provider).collect()
    }

    /// Replace a provider configuration under an expected revision.
    pub async fn update_provider(
        &self,
        record: &ProviderRecord,
    ) -> Result<ProviderRecord, StateError> {
        validate_provider_record(record)?;
        let updated_at = now_millis()?;
        let next_revision = record.revision.next()?;
        let result = sqlx::query(
            "UPDATE providers
             SET name = ?, provider_type = ?, base_url = ?, secret_id = ?,
                 settings_json = ?, enabled = ?, revision = ?, updated_at = ?
             WHERE id = ? AND revision = ?",
        )
        .bind(&record.name)
        .bind(&record.provider_type)
        .bind(&record.base_url)
        .bind(record.secret_id.map(|id| id.to_string()))
        .bind(serde_json::to_string(&record.settings)?)
        .bind(if record.enabled { 1_i64 } else { 0_i64 })
        .bind(next_revision.as_i64()?)
        .bind(updated_at)
        .bind(record.id.to_string())
        .bind(record.revision.as_i64()?)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("provider revision conflict"));
        }
        self.get_provider(record.id)
            .await?
            .ok_or(StateError::InvalidInput("updated provider was not found"))
    }

    /// Delete a provider configuration. Encrypted secrets remain recoverable
    /// and are not deleted implicitly by this repository.
    pub async fn delete_provider(&self, provider_id: ProviderId) -> Result<(), StateError> {
        let result = sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(provider_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("provider does not exist"));
        }
        Ok(())
    }

    /// Insert one model record.
    pub async fn insert_model(&self, record: &ModelRecord) -> Result<ModelRecord, StateError> {
        validate_model_record(record)?;
        sqlx::query(
            "INSERT INTO models
             (id, provider_id, external_model_id, capability_json, settings_json,
              enabled, revision, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.id.to_string())
        .bind(record.provider_id.to_string())
        .bind(&record.external_model_id)
        .bind(serde_json::to_string(&record.capabilities)?)
        .bind(serde_json::to_string(&record.settings)?)
        .bind(if record.enabled { 1_i64 } else { 0_i64 })
        .bind(record.revision.as_i64()?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&self.pool)
        .await?;
        self.get_model(record.id)
            .await?
            .ok_or(StateError::InvalidInput("inserted model was not found"))
    }

    /// Fetch one model.
    pub async fn get_model(&self, model_id: ModelId) -> Result<Option<ModelRecord>, StateError> {
        let row = sqlx::query_as::<_, ModelRow>(
            "SELECT id, provider_id, external_model_id, capability_json,
                    settings_json, enabled, revision, created_at, updated_at
             FROM models WHERE id = ?",
        )
        .bind(model_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_model).transpose()
    }

    /// List models for one provider or all providers.
    pub async fn list_models(
        &self,
        provider_id: Option<ProviderId>,
        limit: u32,
    ) -> Result<Vec<ModelRecord>, StateError> {
        if limit == 0 || limit > MAX_PROVIDER_LIMIT {
            return Err(StateError::InvalidInput("model page is invalid"));
        }
        let rows = if let Some(provider_id) = provider_id {
            sqlx::query_as::<_, ModelRow>(
                "SELECT id, provider_id, external_model_id, capability_json,
                        settings_json, enabled, revision, created_at, updated_at
                 FROM models WHERE provider_id = ? ORDER BY id ASC LIMIT ?",
            )
            .bind(provider_id.to_string())
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, ModelRow>(
                "SELECT id, provider_id, external_model_id, capability_json,
                        settings_json, enabled, revision, created_at, updated_at
                 FROM models ORDER BY id ASC LIMIT ?",
            )
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter().map(row_to_model).collect()
    }

    /// Replace a model record under an expected revision.
    pub async fn update_model(&self, record: &ModelRecord) -> Result<ModelRecord, StateError> {
        validate_model_record(record)?;
        let next_revision = record.revision.next()?;
        let result = sqlx::query(
            "UPDATE models
             SET provider_id = ?, external_model_id = ?, capability_json = ?,
                 settings_json = ?, enabled = ?, revision = ?, updated_at = ?
             WHERE id = ? AND revision = ?",
        )
        .bind(record.provider_id.to_string())
        .bind(&record.external_model_id)
        .bind(serde_json::to_string(&record.capabilities)?)
        .bind(serde_json::to_string(&record.settings)?)
        .bind(if record.enabled { 1_i64 } else { 0_i64 })
        .bind(next_revision.as_i64()?)
        .bind(now_millis()?)
        .bind(record.id.to_string())
        .bind(record.revision.as_i64()?)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("model revision conflict"));
        }
        self.get_model(record.id)
            .await?
            .ok_or(StateError::InvalidInput("updated model was not found"))
    }

    /// Upsert a global or Vault-specific role binding.
    pub async fn upsert_binding(
        &self,
        context: Option<&VaultContext>,
        role: &str,
        model_id: ModelId,
        settings: &Value,
        expected_revision: Option<Revision>,
    ) -> Result<ModelBindingRecord, StateError> {
        validate_label(role, "model role")?;
        let vault_id = context.map(|context| context.id());
        if let Some(context) = context {
            self.ensure_vault_context(context).await?;
        }
        let current = self.get_binding(context, role).await?;
        if let Some(expected_revision) = expected_revision
            && current.as_ref().map(|binding| binding.revision) != Some(expected_revision)
        {
            return Err(StateError::InvalidInput("model binding revision conflict"));
        }
        let now = now_millis()?;
        let id = current
            .as_ref()
            .map(|binding| binding.id.clone())
            .unwrap_or_else(|| {
                format!(
                    "binding:{}:{}",
                    vault_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "global".to_owned()),
                    role
                )
            });
        let revision = current
            .as_ref()
            .map(|binding| binding.revision.next())
            .transpose()?
            .unwrap_or_else(|| Revision::new(1));
        let vault_id_string = vault_id.map(|id| id.to_string());
        if current.is_some() {
            if let Some(vault_id_string) = &vault_id_string {
                sqlx::query(
                    "UPDATE model_bindings
                     SET model_id = ?, settings_json = ?, revision = ?, updated_at = ?
                     WHERE vault_id = ? AND role = ?",
                )
                .bind(model_id.to_string())
                .bind(serde_json::to_string(settings)?)
                .bind(revision.as_i64()?)
                .bind(now)
                .bind(vault_id_string)
                .bind(role)
                .execute(&self.pool)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE model_bindings
                     SET model_id = ?, settings_json = ?, revision = ?, updated_at = ?
                     WHERE vault_id IS NULL AND role = ?",
                )
                .bind(model_id.to_string())
                .bind(serde_json::to_string(settings)?)
                .bind(revision.as_i64()?)
                .bind(now)
                .bind(role)
                .execute(&self.pool)
                .await?;
            }
        } else {
            sqlx::query(
                "INSERT INTO model_bindings
                 (id, vault_id, role, model_id, settings_json, revision, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(vault_id_string)
            .bind(role)
            .bind(model_id.to_string())
            .bind(serde_json::to_string(settings)?)
            .bind(revision.as_i64()?)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        self.get_binding(context, role)
            .await?
            .ok_or(StateError::InvalidInput("model binding was not saved"))
    }

    /// Return a Vault-specific binding, falling back to the global role.
    pub async fn resolve_binding(
        &self,
        context: &VaultContext,
        role: &str,
    ) -> Result<Option<ModelBindingRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        self.get_binding(Some(context), role)
            .await?
            .or(self.get_binding(None, role).await?)
            .map_or(Ok(None), |binding| Ok(Some(binding)))
    }

    /// Return one exact binding scope.
    pub async fn get_binding(
        &self,
        context: Option<&VaultContext>,
        role: &str,
    ) -> Result<Option<ModelBindingRecord>, StateError> {
        validate_label(role, "model role")?;
        let row = if let Some(context) = context {
            sqlx::query_as::<_, BindingRow>(
                "SELECT id, vault_id, role, model_id, settings_json, revision,
                        updated_at
                 FROM model_bindings WHERE vault_id = ? AND role = ?",
            )
            .bind(context.id().to_string())
            .bind(role)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, BindingRow>(
                "SELECT id, vault_id, role, model_id, settings_json, revision,
                        updated_at
                 FROM model_bindings WHERE vault_id IS NULL AND role = ?",
            )
            .bind(role)
            .fetch_optional(&self.pool)
            .await?
        };
        row.map(row_to_binding).transpose()
    }

    /// Persist a redacted provider health result.
    pub async fn upsert_health(
        &self,
        health: &ProviderHealthRecord,
    ) -> Result<ProviderHealthRecord, StateError> {
        validate_label(&health.status, "provider health status")?;
        sqlx::query(
            "INSERT INTO provider_health
             (provider_id, status, checked_at, latency_ms, model_count,
              last_success_at, last_error, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(provider_id) DO UPDATE SET
               status = excluded.status,
               checked_at = excluded.checked_at,
               latency_ms = excluded.latency_ms,
               model_count = excluded.model_count,
               last_success_at = excluded.last_success_at,
               last_error = excluded.last_error,
               updated_at = excluded.updated_at",
        )
        .bind(health.provider_id.to_string())
        .bind(&health.status)
        .bind(health.checked_at)
        .bind(
            health
                .latency_ms
                .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
        )
        .bind(i64::try_from(health.model_count).unwrap_or(i64::MAX))
        .bind(health.last_success_at)
        .bind(health.last_error.as_deref())
        .bind(health.updated_at)
        .execute(&self.pool)
        .await?;
        self.get_health(health.provider_id)
            .await?
            .ok_or(StateError::InvalidInput("provider health was not saved"))
    }

    /// Fetch one redacted health result.
    pub async fn get_health(
        &self,
        provider_id: ProviderId,
    ) -> Result<Option<ProviderHealthRecord>, StateError> {
        let row = sqlx::query_as::<_, HealthRow>(
            "SELECT provider_id, status, checked_at, latency_ms, model_count,
                    last_success_at, last_error, updated_at
             FROM provider_health WHERE provider_id = ?",
        )
        .bind(provider_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_health).transpose()
    }

    /// List redacted provider health rows in deterministic provider order.
    pub async fn list_health(&self, limit: u32) -> Result<Vec<ProviderHealthRecord>, StateError> {
        if limit == 0 || limit > MAX_PROVIDER_LIMIT {
            return Err(StateError::InvalidInput("provider health page is invalid"));
        }
        let rows = sqlx::query_as::<_, HealthRow>(
            "SELECT provider_id, status, checked_at, latency_ms, model_count,
                    last_success_at, last_error, updated_at
             FROM provider_health
             ORDER BY provider_id ASC
             LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_health).collect()
    }

    /// Replace one Vault-scoped vector and its metadata atomically.
    pub async fn upsert_embedding(
        &self,
        context: &VaultContext,
        embedding: &EmbeddingRecord,
        vector: &[f32],
    ) -> Result<EmbeddingRecord, StateError> {
        if embedding.vault_id != context.id()
            || embedding.dimension == 0
            || embedding.dimension as usize != vector.len()
            || vector.iter().any(|value| !value.is_finite())
        {
            return Err(StateError::InvalidInput("embedding vector is invalid"));
        }
        let vector_blob = encode_vector(vector);
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM embedding_records
             WHERE vault_id = ? AND object_type = ? AND object_id = ?
               AND chunk_key = ? AND model_id = ?",
        )
        .bind(context.id().to_string())
        .bind(&embedding.object_type)
        .bind(&embedding.object_id)
        .bind(&embedding.chunk_key)
        .bind(embedding.model_id.to_string())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO embedding_records
             (id, vault_id, object_type, object_id, chunk_key, provider_id,
              model_id, dimension, content_hash, vector_backend_key,
              created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(embedding.id.to_string())
        .bind(context.id().to_string())
        .bind(&embedding.object_type)
        .bind(&embedding.object_id)
        .bind(&embedding.chunk_key)
        .bind(embedding.provider_id.to_string())
        .bind(embedding.model_id.to_string())
        .bind(i64::from(embedding.dimension))
        .bind(&embedding.content_hash)
        .bind(&embedding.vector_backend_key)
        .bind(embedding.created_at)
        .bind(embedding.updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO embedding_vectors
             (vault_id, embedding_id, dimension, vector_blob, norm, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(context.id().to_string())
        .bind(embedding.id.to_string())
        .bind(i64::from(embedding.dimension))
        .bind(vector_blob)
        .bind(f64::from(norm))
        .bind(embedding.updated_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.get_embedding(context, embedding.id)
            .await?
            .ok_or(StateError::InvalidInput("embedding was not saved"))
    }

    /// Fetch one Vault-scoped embedding metadata row.
    pub async fn get_embedding(
        &self,
        context: &VaultContext,
        embedding_id: EmbeddingId,
    ) -> Result<Option<EmbeddingRecord>, StateError> {
        let row = sqlx::query_as::<_, EmbeddingRow>(
            "SELECT e.id, e.vault_id, e.object_type, e.object_id, e.chunk_key,
                    e.provider_id, e.model_id, e.dimension, e.content_hash,
                    e.vector_backend_key, e.created_at, e.updated_at,
                    v.vector_blob
             FROM embedding_records e
             JOIN embedding_vectors v
               ON v.vault_id = e.vault_id AND v.embedding_id = e.id
             WHERE e.vault_id = ? AND e.id = ?",
        )
        .bind(context.id().to_string())
        .bind(embedding_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_embedding).transpose()
    }

    /// Load bounded vector candidates for one Vault/model/dimension.
    pub async fn list_vectors(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        dimension: u32,
        limit: u32,
    ) -> Result<Vec<VectorCandidate>, StateError> {
        if limit == 0 || limit > MAX_VECTOR_LIMIT || dimension == 0 {
            return Err(StateError::InvalidInput("vector query page is invalid"));
        }
        let rows = sqlx::query_as::<_, EmbeddingRow>(
            "SELECT e.id, e.vault_id, e.object_type, e.object_id, e.chunk_key,
                    e.provider_id, e.model_id, e.dimension, e.content_hash,
                    e.vector_backend_key, e.created_at, e.updated_at,
                    v.vector_blob
             FROM embedding_records e
             JOIN embedding_vectors v
               ON v.vault_id = e.vault_id AND v.embedding_id = e.id
             WHERE e.vault_id = ? AND e.model_id = ? AND e.dimension = ?
             ORDER BY e.id ASC LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(model_id.to_string())
        .bind(i64::from(dimension))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_candidate).collect()
    }

    /// Delete all vectors for one Vault/model, preserving canonical content.
    pub async fn delete_embeddings_for_model(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<u64, StateError> {
        let result =
            sqlx::query("DELETE FROM embedding_records WHERE vault_id = ? AND model_id = ?")
                .bind(context.id().to_string())
                .bind(model_id.to_string())
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected())
    }

    /// Return vector coverage for one Vault/model.
    pub async fn embedding_coverage(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<EmbeddingCoverage, StateError> {
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM embedding_records
             WHERE vault_id = ? AND model_id = ?",
        )
        .bind(context.id().to_string())
        .bind(model_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        let objects: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT object_type || ':' || object_id)
             FROM embedding_records WHERE vault_id = ? AND model_id = ?",
        )
        .bind(context.id().to_string())
        .bind(model_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        let dimensions = sqlx::query_scalar::<_, i64>(
            "SELECT DISTINCT dimension FROM embedding_records
             WHERE vault_id = ? AND model_id = ? ORDER BY dimension ASC",
        )
        .bind(context.id().to_string())
        .bind(model_id.to_string())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|value| {
            u32::try_from(value)
                .map_err(|_| StateError::InvalidInput("vector dimension is invalid"))
        })
        .collect::<Result<Vec<_>, _>>()?;
        Ok(EmbeddingCoverage {
            total: u64::try_from(total)
                .map_err(|_| StateError::InvalidInput("embedding count is invalid"))?,
            objects: u64::try_from(objects)
                .map_err(|_| StateError::InvalidInput("embedding object count is invalid"))?,
            dimensions,
        })
    }

    async fn ensure_vault_context(&self, context: &VaultContext) -> Result<(), StateError> {
        let exists: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vaults WHERE id = ?)")
            .bind(context.id().to_string())
            .fetch_one(&self.pool)
            .await?;
        if exists != 1 {
            return Err(StateError::InvalidInput("Vault is not registered"));
        }
        Ok(())
    }
}

fn validate_provider_record(record: &ProviderRecord) -> Result<(), StateError> {
    validate_label(&record.name, "provider name")?;
    validate_label(&record.provider_type, "provider type")?;
    if record.base_url.is_empty() || record.base_url.len() > 4096 {
        return Err(StateError::InvalidInput("provider base URL is invalid"));
    }
    Ok(())
}

fn validate_model_record(record: &ModelRecord) -> Result<(), StateError> {
    validate_label(&record.external_model_id, "external model ID")?;
    Ok(())
}

fn validate_label(value: &str, kind: &'static str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(StateError::InvalidInput(kind));
    }
    Ok(())
}

fn row_to_provider(row: ProviderRow) -> Result<ProviderRecord, StateError> {
    Ok(ProviderRecord {
        id: ProviderId::parse(&row.id)?,
        name: row.name,
        provider_type: row.provider_type,
        base_url: row.base_url,
        secret_id: row.secret_id.as_deref().map(SecretId::parse).transpose()?,
        settings: serde_json::from_str(&row.settings_json)?,
        enabled: row.enabled != 0,
        revision: Revision::try_from(row.revision)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_model(row: ModelRow) -> Result<ModelRecord, StateError> {
    Ok(ModelRecord {
        id: ModelId::parse(&row.id)?,
        provider_id: ProviderId::parse(&row.provider_id)?,
        external_model_id: row.external_model_id,
        capabilities: serde_json::from_str(&row.capability_json)?,
        settings: serde_json::from_str(&row.settings_json)?,
        enabled: row.enabled != 0,
        revision: Revision::try_from(row.revision)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_binding(row: BindingRow) -> Result<ModelBindingRecord, StateError> {
    Ok(ModelBindingRecord {
        id: row.id,
        vault_id: row.vault_id.as_deref().map(VaultId::parse).transpose()?,
        role: row.role,
        model_id: ModelId::parse(&row.model_id)?,
        settings: serde_json::from_str(&row.settings_json)?,
        revision: Revision::try_from(row.revision)?,
        updated_at: row.updated_at,
    })
}

fn row_to_health(row: HealthRow) -> Result<ProviderHealthRecord, StateError> {
    Ok(ProviderHealthRecord {
        provider_id: ProviderId::parse(&row.provider_id)?,
        status: row.status,
        checked_at: row.checked_at,
        latency_ms: row
            .latency_ms
            .map(|value| {
                u64::try_from(value)
                    .map_err(|_| StateError::InvalidInput("health latency is invalid"))
            })
            .transpose()?,
        model_count: u64::try_from(row.model_count)
            .map_err(|_| StateError::InvalidInput("health model count is invalid"))?,
        last_success_at: row.last_success_at,
        last_error: row.last_error,
        updated_at: row.updated_at,
    })
}

fn row_to_embedding(row: EmbeddingRow) -> Result<EmbeddingRecord, StateError> {
    Ok(EmbeddingRecord {
        id: EmbeddingId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        object_type: row.object_type,
        object_id: row.object_id,
        chunk_key: row.chunk_key,
        provider_id: ProviderId::parse(&row.provider_id)?,
        model_id: ModelId::parse(&row.model_id)?,
        dimension: u32::try_from(row.dimension)
            .map_err(|_| StateError::InvalidInput("embedding dimension is invalid"))?,
        content_hash: row.content_hash,
        vector_backend_key: row.vector_backend_key,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_candidate(row: EmbeddingRow) -> Result<VectorCandidate, StateError> {
    let embedding = row_to_embedding(row.clone())?;
    let vector = decode_vector(&row.vector_blob, embedding.dimension)?;
    Ok(VectorCandidate { embedding, vector })
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(blob: &[u8], dimension: u32) -> Result<Vec<f32>, StateError> {
    if blob.len() != dimension as usize * std::mem::size_of::<f32>() {
        return Err(StateError::InvalidInput(
            "stored vector dimension is invalid",
        ));
    }
    Ok(blob
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("f32 chunk has four bytes")))
        .collect())
}
