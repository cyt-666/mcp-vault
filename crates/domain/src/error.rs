//! Stable errors returned by domain value construction and invariant checks.

use thiserror::Error;

use crate::{Revision, VaultId, VaultPath};

/// Errors that protocol and infrastructure boundaries can map without
/// exposing raw parser, filesystem, or database errors.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DomainError {
    /// A value failed a domain-level validation rule.
    #[error("invalid {kind}: {reason}")]
    InvalidValue {
        /// Stable value category used by callers and diagnostics.
        kind: &'static str,
        /// Non-sensitive reason for rejection.
        reason: &'static str,
    },
    /// A logical Vault path failed normalization or safety validation.
    #[error("invalid Vault path: {0}")]
    InvalidPath(#[from] PathError),
    /// A user-facing operation attempted to enter the managed namespace.
    #[error("path is reserved for MCP Vault management: {0}")]
    ReservedPath(VaultPath),
    /// A managed operation attempted to use a path outside its namespace.
    #[error("path is outside the managed MCP Vault namespace")]
    PathNotManaged,
    /// Two normalized paths would address the same entry under a selected
    /// case-sensitivity policy.
    #[error("normalized path collision")]
    PathCollision {
        /// First path encountered by the collision detector.
        first: VaultPath,
        /// Later path that collides with the first one.
        second: VaultPath,
    },
    /// The configured content root is not an absolute, stable root path.
    #[error("Vault content root must be an absolute path without parent traversal")]
    InvalidContentRoot,
    /// Two operations were attempted with different Vault identities.
    #[error("Vault contexts do not match")]
    VaultMismatch {
        /// Vault identity expected by the receiving operation.
        expected: VaultId,
        /// Vault identity supplied by the caller.
        actual: VaultId,
    },
    /// An SQLite-compatible signed revision was negative.
    #[error("revision cannot be negative")]
    NegativeRevision,
    /// A revision increment would overflow the domain representation.
    #[error("revision overflow")]
    RevisionOverflow,
    /// A write precondition did not match the current entry state.
    #[error("write precondition failed: {reason}")]
    PreconditionFailed {
        /// Stable reason suitable for protocol mapping.
        reason: &'static str,
    },
    /// The caller expected a revision that differs from the current one.
    #[error("revision conflict")]
    RevisionConflict {
        /// Revision supplied by the caller.
        expected: Revision,
        /// Revision observed by the application service.
        current: Revision,
    },
    /// The filesystem entry kind is disallowed by the active safety policy.
    #[error("unsafe filesystem entry kind: {kind}")]
    UnsafeFilesystemEntry {
        /// Stable entry-kind label.
        kind: &'static str,
    },
}

/// Reasons a raw logical path is rejected.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PathError {
    /// A path begins with a logical separator.
    #[error("absolute paths are not allowed")]
    Absolute,
    /// A path contains `.` or `..` traversal segments.
    #[error("parent or current-directory traversal is not allowed")]
    Traversal,
    /// A path contains two separators or an empty segment.
    #[error("empty path segments are not allowed")]
    EmptySegment,
    /// Backslash is never a logical path separator in the Vault model.
    #[error("backslash is not a logical Vault separator")]
    Backslash,
    /// NUL is not valid in a Vault path.
    #[error("NUL is not allowed in a Vault path")]
    Nul,
    /// ASCII or Unicode control characters are not valid path content.
    #[error("control characters are not allowed in a Vault path")]
    ControlCharacter,
    /// A percent-encoded separator, dot, NUL, or nested percent escape was
    /// found after the decode boundary.
    #[error("encoded traversal or separator must be decoded exactly once")]
    EncodedUnsafeComponent,
    /// A URL path contained malformed percent encoding.
    #[error("invalid percent encoding")]
    InvalidPercentEncoding,
    /// Percent decoding did not produce UTF-8.
    #[error("percent-decoded path is not valid UTF-8")]
    InvalidUtf8,
    /// A path exceeds the hard maximum number of segments.
    #[error("path depth exceeds the domain limit")]
    TooDeep,
    /// A path exceeds the hard maximum encoded byte length.
    #[error("path length exceeds the domain limit")]
    TooLong,
    /// A segment exceeds the hard platform-portable byte length.
    #[error("path segment length exceeds the domain limit")]
    SegmentTooLong,
    /// A character is rejected by the conservative cross-platform policy.
    #[error("path contains a platform-invalid character")]
    InvalidPlatformCharacter,
    /// A segment is a Windows device name such as `CON` or `NUL`.
    #[error("path contains a reserved platform name")]
    ReservedPlatformName,
    /// Windows treats trailing spaces and periods as non-addressable.
    #[error("path segments cannot end with a space or period")]
    TrailingSpaceOrPeriod,
}
