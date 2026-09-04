//! Rebuildable Vault-scoped vector index abstractions.

use async_trait::async_trait;
use mcp_vault_domain::{EmbeddingId, ModelId, VaultContext};
use mcp_vault_state::{EmbeddingRecord, StateStore};
use serde::{Deserialize, Serialize};

use crate::ProviderError;

/// One vector similarity hit.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorHit {
    /// Embedding metadata/provenance.
    pub embedding: EmbeddingRecord,
    /// Exact cosine similarity in the range [-1, 1].
    pub score: f32,
}

/// Internal vector storage contract.
#[async_trait]
pub trait VectorIndex: Send + Sync {
    /// Upsert one derived vector.
    async fn upsert(
        &self,
        context: &VaultContext,
        embedding: &EmbeddingRecord,
        vector: &[f32],
    ) -> Result<(), ProviderError>;

    /// Search one Vault/model/object-type/dimension partition.
    ///
    /// Results are cosine-descending chunk candidates. Equal scores use
    /// object ID, chunk key, and embedding ID as deterministic tie breakers;
    /// the owning application validates current content and performs any
    /// object-level aggregation.
    async fn search(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        object_type: &str,
        dimension: u32,
        query: &[f32],
        limit: u32,
    ) -> Result<Vec<VectorHit>, ProviderError>;

    /// Delete one model partition for a Vault.
    async fn delete_model(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<u64, ProviderError>;
}

/// Mandatory SQLite exact-cosine backend.
#[derive(Clone)]
pub struct SqliteVectorIndex {
    state: StateStore,
}

impl SqliteVectorIndex {
    /// Bind the backend to operational state.
    pub fn new(state: StateStore) -> Self {
        Self { state }
    }

    /// Return the state store used by this derived backend.
    pub fn state(&self) -> &StateStore {
        &self.state
    }
}

#[async_trait]
impl VectorIndex for SqliteVectorIndex {
    async fn upsert(
        &self,
        context: &VaultContext,
        embedding: &EmbeddingRecord,
        vector: &[f32],
    ) -> Result<(), ProviderError> {
        self.state
            .providers()
            .upsert_embedding(context, embedding, vector)
            .await?;
        Ok(())
    }

    async fn search(
        &self,
        context: &VaultContext,
        model_id: ModelId,
        object_type: &str,
        dimension: u32,
        query: &[f32],
        limit: u32,
    ) -> Result<Vec<VectorHit>, ProviderError> {
        if object_type.is_empty()
            || dimension == 0
            || query.len() != dimension as usize
            || limit == 0
        {
            return Err(ProviderError::DimensionMismatch);
        }
        if query.iter().any(|value| !value.is_finite()) {
            return Err(ProviderError::InvalidConfiguration(
                "query vector contains a non-finite value",
            ));
        }
        let query_norm = query.iter().map(|value| value * value).sum::<f32>().sqrt();
        let candidates = self
            .state
            .providers()
            .list_vectors(context, model_id, object_type, dimension, 10_000)
            .await?;
        let mut hits = candidates
            .into_iter()
            .map(|candidate| {
                let candidate_norm = candidate
                    .vector
                    .iter()
                    .map(|value| value * value)
                    .sum::<f32>()
                    .sqrt();
                let dot = candidate
                    .vector
                    .iter()
                    .zip(query)
                    .map(|(left, right)| left * right)
                    .sum::<f32>();
                let score = if query_norm == 0.0 || candidate_norm == 0.0 {
                    0.0
                } else {
                    (dot / (query_norm * candidate_norm)).clamp(-1.0, 1.0)
                };
                VectorHit {
                    embedding: candidate.embedding,
                    score,
                }
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.embedding.object_id.cmp(&right.embedding.object_id))
                .then_with(|| left.embedding.chunk_key.cmp(&right.embedding.chunk_key))
                .then_with(|| left.embedding.id.cmp(&right.embedding.id))
        });
        hits.truncate(limit as usize);
        Ok(hits)
    }

    async fn delete_model(
        &self,
        context: &VaultContext,
        model_id: ModelId,
    ) -> Result<u64, ProviderError> {
        Ok(self
            .state
            .providers()
            .delete_embeddings_for_model(context, model_id)
            .await?)
    }
}

/// Stable source reference used by re-embedding job admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmbeddingSourceRef {
    /// Source object category.
    pub object_type: String,
    /// Source object ID.
    pub object_id: String,
    /// Chunk/section key.
    pub chunk_key: String,
    /// Content hash used for stale detection.
    pub content_hash: String,
}

/// Allocate one embedding row identity. The stable source/model uniqueness is
/// enforced by the Vault-scoped repository key, not by exposing a UUID as an
/// authorization or source identifier.
pub fn new_embedding_id() -> EmbeddingId {
    EmbeddingId::new()
}
