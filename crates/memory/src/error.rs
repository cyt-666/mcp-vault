//! Redacted memory application errors.

use thiserror::Error;

/// Errors at the durable memory application boundary.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Caller input or an untrusted Markdown record is invalid.
    #[error("memory input is invalid: {0}")]
    InvalidInput(&'static str),
    /// A source note could not be ingested before any Provider call.
    #[error("memory source ingestion failed: {0}")]
    SourceIngestion(&'static str),
    /// A Provider result passed the wire contract but failed Phase 1 validation.
    #[error("memory generated output is invalid: {0}")]
    GeneratedOutput(&'static str),
    /// A memory, staged source, or Vault was not found.
    #[error("memory resource was not found")]
    NotFound,
    /// Required extraction/provider policy is incomplete.
    #[error("memory configuration is incomplete: {0}")]
    Configuration(&'static str),
    /// The requested memory operation conflicts with current state.
    #[error("memory operation conflicts with current state")]
    Conflict,
    /// A durable memory file is invalid and has been quarantined.
    #[error("managed memory file is invalid")]
    Quarantined,
    /// State repository failure.
    #[error("memory state is unavailable")]
    State(#[from] mcp_vault_state::StateError),
    /// Canonical Vault Core failure.
    #[error("canonical memory file operation failed")]
    Core(#[from] mcp_vault_core::VaultError),
    /// Markdown/frontmatter parsing failed.
    #[error("memory Markdown is invalid")]
    Markdown,
    /// Existing index projection failure.
    #[error("memory index projection is unavailable")]
    Index(#[from] mcp_vault_indexer::IndexError),
    /// Optional provider failure. The inner error is already redacted.
    #[error("memory provider operation failed")]
    Provider(#[from] mcp_vault_providers::ProviderError),
}

impl MemoryError {
    /// Stable redacted error code for protocol adapters and jobs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "memory_invalid_input",
            Self::SourceIngestion(code) | Self::GeneratedOutput(code) => code,
            Self::NotFound => "memory_not_found",
            Self::Configuration(code) => code,
            Self::Conflict => "memory_conflict",
            Self::Quarantined => "memory_quarantined",
            Self::State(_) => "memory_state_error",
            Self::Core(_) => "memory_core_error",
            Self::Markdown => "memory_markdown_invalid",
            Self::Index(_) => "memory_index_error",
            Self::Provider(error) => error.code(),
        }
    }

    /// Whether a durable worker may retry this error.
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Conflict | Self::State(_) | Self::Core(_) | Self::Index(_)
        ) || matches!(self, Self::Provider(error) if error.retryable())
    }
}
