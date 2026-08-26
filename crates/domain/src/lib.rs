//! Stable domain types and invariants for MCP Vault.
//!
//! This crate deliberately depends only on value/serialization libraries and
//! the standard library. It does not know about Axum, RMCP, WebDAV, SQLx,
//! providers, or the filesystem implementation.

mod actor;
mod error;
mod id;
mod maintenance;
mod path;
mod permission;
mod revision;
mod vault;

pub use actor::{Actor, ActorId, ActorType, SourcePlane};
pub use error::{DomainError, PathError};
pub use id::{
    AdminSessionId, AdminUserId, BackupId, CredentialId, EmbeddingId, EventId, FileId, IdentityId,
    JobId, MemoryCandidateId, MemoryConsolidationId, MemoryId, MemoryRawId, MemoryRelationId,
    MemorySourceId, ModelId, OAuthGrantId, OAuthIssuerId, OperationId, ProviderId, RevisionId,
    ScanId, SecretId, VaultId,
};
pub use maintenance::{MaintenanceGate, MaintenanceMode, MaintenanceOperationGuard};
pub use path::{
    FilesystemEntryKind, FilesystemPolicy, PathCaseSensitivity, PathComparisonKey, VaultPath,
    VaultPathPolicy, detect_path_collisions,
};
pub use permission::{Permission, PermissionSet, Scope, ScopeSet};
pub use revision::{Revision, WritePrecondition};
pub use vault::{VaultContext, VaultSlug};
