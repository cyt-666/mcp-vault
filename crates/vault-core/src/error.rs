//! Stable application errors for Vault Core.

use mcp_vault_domain::{DomainError, Revision};
use mcp_vault_state::StateError;
use mcp_vault_storage_fs::StorageError;
use thiserror::Error;

/// Errors that protocol adapters can map without seeing SQL, paths, or stack
/// traces.
#[derive(Debug, Error)]
pub enum VaultError {
    /// A domain value or precondition failed.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// State repository failure.
    #[error("Vault operational state error")]
    State(#[from] StateError),
    /// Safe filesystem failure.
    #[error("Vault storage error")]
    Storage(#[from] StorageError),
    /// The context is not registered or does not match the registry.
    #[error("Vault context is not registered")]
    VaultNotRegistered,
    /// The Vault context identity/root differs from registered state.
    #[error("Vault context does not match registered Vault")]
    ContextMismatch,
    /// A live file was expected to be absent.
    #[error("file already exists")]
    AlreadyExists,
    /// A live file was expected to exist.
    #[error("file not found")]
    NotFound,
    /// A write observed content that does not match SQLite state.
    #[error("filesystem and operational state do not match")]
    ExternalMismatch,
    /// A supplied idempotency key is currently unresolved.
    #[error("idempotent operation is still in progress")]
    InFlight,
    /// A key was reused for a different operation payload.
    #[error("idempotency key was reused with a different operation")]
    IdempotencyConflict,
    /// A text patch did not apply exactly.
    #[error("invalid exact patch: {0}")]
    InvalidPatch(&'static str),
    /// A binary file was passed to a text-only operation.
    #[error("text operation requires UTF-8 content")]
    BinaryTextOperation,
    /// Recovery could not prove old or new canonical state.
    #[error("operation requires maintenance review")]
    NeedsReview,
    /// A deterministic test fault was injected at a commit phase.
    #[error("injected failure at commit phase: {0}")]
    InjectedFailure(&'static str),
    /// The Vault is not accepting writes while in maintenance/error state.
    #[error("Vault is not accepting writes")]
    Maintenance,
    /// A revision conflict includes the current safe state.
    #[error("revision conflict")]
    RevisionConflict {
        /// Revision supplied by the caller.
        expected: Revision,
        /// Current revision observed by Core.
        current: Revision,
        /// Current content hash, when available.
        current_hash: Option<String>,
    },
}
