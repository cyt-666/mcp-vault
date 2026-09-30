//! Pluggable LLM, embedding, reranking, and vector-index adapters.
//!
//! Providers are optional enrichment and cannot become a dependency of core
//! file operations or normal lexical recall.

mod adapter;
mod error;
mod fastembed;
mod native_stream;
mod policy;
mod service;
mod sse;
mod stream_response;
mod transport;
mod vector;

pub use adapter::{
    AnthropicMessagesAdapter, DiscoveredModel, EmbeddingRequest, EmbeddingResult,
    GenerationOptions, HttpEmbeddingAdapter, MissingRequiredStringFallback,
    OpenAiCompatibleAdapter, OpenAiResponsesAdapter, ProviderAdapter, StructuredGenerationRequest,
    StructuredGenerationResult, validate_structured_value,
};
pub use error::{
    ProviderError, StrictFunctionCallIssue, StructuredJsonDiagnostic, StructuredJsonFinishReason,
    StructuredJsonParseIssue, StructuredJsonParserCategory,
};
pub use fastembed::FastEmbedAdapter;
pub use policy::{
    DEFAULT_REASONING_GENERATION_TOKENS, ModelCapabilities, ModelSettings,
    OpenAiCompatibilityPreset, OpenAiStructuredOutputMode, OpenAiThinkingMode,
    OpenAiTokenLimitField, ProviderKind, ProviderMode, ProviderSettings, endpoint_ip_allowed,
};
pub use service::{
    EMBEDDING_PROJECTION_VERSION, EmbeddingInput, EmbeddingService, EmbeddingSourceResolver,
    ModelInput, PROVIDER_SECRET_OWNER, PROVIDER_SECRET_PURPOSE, ProviderInput, ProviderModeState,
    ProviderRuntimeSnapshot, ProviderService, SafeProviderSettings, embedding_input_hash,
};
pub use sse::{SseDecoder, SseEvent};
pub use transport::{
    AuthStyle, JsonResponse, ProviderTransport, RequestBudget, RequestOptions, SseEventAction,
    SseResponse, endpoint_url, retryable_status, validate_endpoint,
};
pub use vector::{
    EmbeddingSourceRef, SqliteVectorIndex, VectorHit, VectorIndex, exact_cosine_similarity,
    new_embedding_id,
};
