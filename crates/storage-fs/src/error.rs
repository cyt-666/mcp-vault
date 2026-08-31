//! Redacted errors for the filesystem storage boundary.

use std::io::ErrorKind;

use mcp_vault_domain::{DomainError, FilesystemEntryKind};
use thiserror::Error;

/// Errors returned by safe Vault filesystem and history operations.
///
/// The error intentionally stores an [`ErrorKind`] instead of the original
/// `std::io::Error`, because an I/O error may include an absolute path or
/// other host-specific detail that must not escape a lower-level boundary.
#[derive(Debug, Error)]
pub enum StorageError {
    /// A domain path, policy, or Vault context invariant failed.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// A host filesystem operation failed without retaining its raw path.
    #[error("filesystem operation {operation} failed ({kind:?})")]
    Io {
        /// Stable operation label for internal diagnostics.
        operation: &'static str,
        /// Redacted standard-library error category.
        kind: ErrorKind,
    },
    /// The entry kind is denied by the storage safety policy.
    #[error("unsafe filesystem entry: {kind}")]
    UnsafeEntry {
        /// Stable entry-kind label.
        kind: &'static str,
    },
    /// The requested target already exists under a no-replace operation.
    #[error("destination already exists")]
    DestinationExists,
    /// The filesystem cannot provide an atomic no-replace file commit using
    /// either exclusive rename or the safe same-filesystem link fallback.
    #[error("filesystem does not support safe atomic no-replace file creation")]
    AtomicCreateUnsupported,
    /// The source or history blob was not found.
    #[error("source does not exist")]
    SourceNotFound,
    /// The supplied operation cannot be performed by this primitive.
    #[error("invalid filesystem operation: {0}")]
    InvalidOperation(&'static str),
    /// The configured free-space safety margin would be violated.
    #[error("insufficient free disk space: {available} bytes available, {required} required")]
    InsufficientDiskSpace {
        /// Bytes reported free by the filesystem.
        available: u64,
        /// Minimum bytes required by the configured policy.
        required: u64,
    },
    /// A history address was not a 64-character hexadecimal SHA-256 value.
    #[error("invalid content hash")]
    InvalidContentHash,
    /// The requested history blob does not exist.
    #[error("history blob does not exist")]
    HistoryNotFound,
    /// The storage root could not be treated as a directory.
    #[error("storage root is not a directory")]
    RootNotDirectory,
    /// A configured root itself is a symlink and therefore is not accepted.
    #[error("storage root is a symbolic link")]
    RootSymlink,
    /// A blocking filesystem task was cancelled before its result was known.
    #[error("filesystem task was cancelled")]
    TaskCancelled,
}

impl StorageError {
    pub(crate) fn io(operation: &'static str, kind: ErrorKind) -> Self {
        Self::Io { operation, kind }
    }

    pub(crate) fn unsafe_entry(kind: FilesystemEntryKind) -> Self {
        Self::UnsafeEntry {
            kind: kind.as_str(),
        }
    }
}
