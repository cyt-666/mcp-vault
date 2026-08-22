//! Redacted memory application errors.

use thiserror::Error;

/// Errors at the durable memory application boundary.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Caller input or an untrusted Markdown record is invalid.
    #[error("memory input is invalid: {0}")]
    InvalidInput(&'static str),
    /// A memory, candidate, source, or Vault was not found.
    #[error("memory resource was not found")]
    NotFound,
    /// The requested memory operation conflicts with current state.
    #[error("memory operation requires review")]
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
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "memory_invalid_input",
            Self::NotFound => "memory_not_found",
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
        matches!(self, Self::State(_) | Self::Core(_) | Self::Index(_))
            || matches!(self, Self::Provider(error) if error.retryable())
    }
}
