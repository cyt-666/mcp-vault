//! Optional local FastEmbed adapter.

use async_trait::async_trait;
use mcp_vault_auth::SecretString;
use url::Url;

use crate::{
    EmbeddingRequest, EmbeddingResult, ProviderAdapter, ProviderError, ProviderKind, ProviderMode,
    ProviderSettings, ProviderTransport, StructuredGenerationRequest, StructuredGenerationResult,
};

/// Local FastEmbed adapter marker.
#[derive(Clone, Copy, Debug, Default)]
pub struct FastEmbedAdapter;

#[cfg(feature = "fastembed")]
#[async_trait]
impl ProviderAdapter for FastEmbedAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::FastEmbedLocal
    }

    async fn generate_structured(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _settings: &ProviderSettings,
        _secret: Option<&SecretString>,
        _request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }

    async fn embed(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        mode: ProviderMode,
        settings: &ProviderSettings,
        _secret: Option<&SecretString>,
        request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError> {
        if mode != ProviderMode::LocalOnly {
            return Err(ProviderError::PrivacyDenied);
        }
        let inputs = request.inputs.clone();
        let cache_dir = settings.model_cache_dir.clone();
        let vectors = tokio::task::spawn_blocking(move || {
            use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

            let mut options = TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
                .with_show_download_progress(false);
            if let Some(cache_dir) = cache_dir {
                options = options.with_cache_dir(cache_dir.into());
            }
            let mut model = TextEmbedding::try_new(options)
                .map_err(|_| ProviderError::TemporarilyUnavailable)?;
            model
                .embed(inputs, None)
                .map_err(|_| ProviderError::TemporarilyUnavailable)
        })
        .await
        .map_err(|_| ProviderError::TemporarilyUnavailable)??;
        Ok(EmbeddingResult {
            vectors,
            model: Some("fastembed:all-MiniLM-L6-v2".to_owned()),
            usage: None,
        })
    }

    async fn list_models(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _secret: Option<&SecretString>,
    ) -> Result<Vec<crate::DiscoveredModel>, ProviderError> {
        Ok(vec![crate::DiscoveredModel {
            id: "fastembed:all-MiniLM-L6-v2".to_owned(),
            capabilities: serde_json::json!({
                "embeddings": true,
                "dimension": 384
            }),
        }])
    }
}

#[cfg(not(feature = "fastembed"))]
#[async_trait]
impl ProviderAdapter for FastEmbedAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::FastEmbedLocal
    }

    async fn generate_structured(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _settings: &ProviderSettings,
        _secret: Option<&SecretString>,
        _request: &StructuredGenerationRequest,
    ) -> Result<StructuredGenerationResult, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }

    async fn embed(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _settings: &ProviderSettings,
        _secret: Option<&SecretString>,
        _request: &EmbeddingRequest,
    ) -> Result<EmbeddingResult, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }

    async fn list_models(
        &self,
        _transport: &ProviderTransport,
        _base_url: &Url,
        _mode: ProviderMode,
        _secret: Option<&SecretString>,
    ) -> Result<Vec<crate::DiscoveredModel>, ProviderError> {
        Err(ProviderError::CapabilityUnavailable)
    }
}
