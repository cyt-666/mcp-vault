//! Pluggable LLM, embedding, reranking, and vector-index adapters.
//!
//! Providers are optional enrichment and cannot become a dependency of core
//! file operations or normal lexical recall.

mod adapter;
mod error;
mod fastembed;
mod policy;
mod service;
mod transport;
mod vector;

pub use adapter::{
    AnthropicMessagesAdapter, DiscoveredModel, EmbeddingRequest, EmbeddingResult,
    GenerationOptions, HttpEmbeddingAdapter, MissingRequiredStringFallback,
    OpenAiCompatibleAdapter, OpenAiResponsesAdapter, ProviderAdapter, StructuredGenerationRequest,
    StructuredGenerationResult,
};
pub use error::ProviderError;
pub use fastembed::FastEmbedAdapter;
pub use policy::{
    DEFAULT_REASONING_GENERATION_TOKENS, ModelCapabilities, ModelSettings,
    OpenAiCompatibilityPreset, OpenAiStructuredOutputMode, OpenAiThinkingMode,
    OpenAiTokenLimitField, ProviderKind, ProviderMode, ProviderSettings, endpoint_ip_allowed,
};
pub use service::{
    EmbeddingInput, EmbeddingService, EmbeddingSourceResolver, ModelInput, ProviderInput,
    ProviderModeState, ProviderService,
};
pub use transport::{
    AuthStyle, JsonResponse, ProviderTransport, RequestOptions, endpoint_url, retryable_status,
    validate_endpoint,
};
pub use vector::{EmbeddingSourceRef, SqliteVectorIndex, VectorHit, VectorIndex, new_embedding_id};
