//! Redacted memory application errors.

use mcp_vault_domain::VaultPath;
use std::io::ErrorKind;
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
    /// A Provider result passed the wire contract but failed memory-pipeline
    /// validation. The stable code identifies a trusted local validation rule.
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
    /// An offline initialization failure with safe stage/path context.
    #[error("memory initialization failed during {stage}")]
    InitializationFailure {
        stage: &'static str,
        path: Option<VaultPath>,
        completed_files: usize,
        source: Box<MemoryError>,
    },
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
    /// More specific operational diagnostics for workers; never expose inner
    /// SQL/filesystem error messages or memory contents.
    pub fn diagnostic_code(&self) -> &'static str {
        use mcp_vault_core::VaultError;
        use mcp_vault_domain::DomainError;
        match self {
            Self::InitializationFailure { source, .. } => source.diagnostic_code(),
            Self::Core(VaultError::Domain(DomainError::PreconditionFailed { .. })) => {
                "memory_core_precondition_failed"
            }
            Self::Core(VaultError::Storage(mcp_vault_storage_fs::StorageError::Io {
                kind,
                ..
            })) => storage_io_code(*kind),
            Self::State(error) | Self::Core(VaultError::State(error)) => error.diagnostic_code(),
            Self::Core(VaultError::ExternalMismatch) => "memory_core_external_mismatch",
            Self::Core(VaultError::RevisionConflict { .. }) => "memory_core_revision_conflict",
            Self::Core(VaultError::NeedsReview) => "memory_core_needs_review",
            Self::Core(VaultError::InFlight) => "memory_core_in_flight",
            Self::Core(VaultError::NotFound) => "memory_core_not_found",
            Self::Core(VaultError::AlreadyExists) => "memory_core_already_exists",
            Self::Core(VaultError::Storage(_)) => "memory_core_storage_error",
            _ => self.code(),
        }
    }

    /// Stable redacted error code for protocol adapters and jobs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InitializationFailure { source, .. } => source.code(),
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

    pub fn initialization_failure_details(
        &self,
    ) -> Option<(&'static str, Option<&VaultPath>, usize, &'static str)> {
        match self {
            Self::InitializationFailure {
                stage,
                path,
                completed_files,
                source,
            } => Some((
                *stage,
                path.as_ref(),
                *completed_files,
                source.diagnostic_code(),
            )),
            _ => None,
        }
    }

    /// Whether a durable worker may retry this error.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Conflict | Self::State(_) | Self::Core(_) | Self::Index(_)
        ) || matches!(self, Self::Provider(error) if error.retryable())
            || matches!(self, Self::InitializationFailure { source, .. } if source.retryable())
    }
}

fn storage_io_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::NotFound => "memory_core_storage_not_found",
        ErrorKind::PermissionDenied => "memory_core_storage_permission_denied",
        ErrorKind::AlreadyExists => "memory_core_storage_already_exists",
        ErrorKind::InvalidInput | ErrorKind::InvalidData => "memory_core_storage_invalid_input",
        ErrorKind::WouldBlock | ErrorKind::TimedOut => "memory_core_storage_busy",
        ErrorKind::WriteZero | ErrorKind::UnexpectedEof => "memory_core_storage_io_incomplete",
        _ => "memory_core_storage_io",
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryError;
    use mcp_vault_core::VaultError;

    #[test]
    fn initialization_diagnostics_preserve_core_categories() {
        assert_eq!(
            MemoryError::Core(VaultError::NeedsReview).diagnostic_code(),
            "memory_core_needs_review"
        );
        assert_eq!(
            MemoryError::Core(VaultError::NotFound).diagnostic_code(),
            "memory_core_not_found"
        );
        assert_eq!(
            MemoryError::Core(VaultError::Domain(
                mcp_vault_domain::DomainError::PreconditionFailed { reason: "opaque" },
            ))
            .diagnostic_code(),
            "memory_core_precondition_failed"
        );
    }
}
