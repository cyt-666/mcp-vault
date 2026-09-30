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
    AdminSessionId, AdminUserId, BackupId, CardItemId, CardRevisionId, ComposedCardAliasId,
    ComposedCardId, ComposedCardItemId, ComposedCardRevisionId, CorrectionId, CredentialId,
    EmbeddingId, EventId, EvidenceRefId, ExtractionSetId, FileId, IdentityId, JobId,
    MemoryCandidateId, MemoryCardId, MemoryConsolidationId, MemoryId, MemoryRawId,
    MemoryRelationId, MemoryRetrievalProposalId, MemorySetId, MemorySetSnapshotId, MemorySourceId,
    ModelId, OAuthAccessTokenId, OAuthAuthorizationCodeId, OAuthAuthorizationRequestId,
    OAuthClientId, OAuthGrantId, OAuthIssuerId, OAuthLocalUserId, OAuthRefreshTokenId,
    OAuthTokenFamilyId, ObservationId, OperationId, OrganizationJobId, OrganizationSnapshotId,
    PreparedSnapshotId, ProviderId, RelationCandidateId, RelationDecisionId, RevisionId, ScanId,
    SecretId, SemanticSourceId, SourceRevisionId, SupportGroupId, SupportMemberId, SuppressionId,
    VaultId,
};
pub use maintenance::{
    MaintenanceGate, MaintenanceLease, MaintenanceMode, MaintenanceOperationGuard,
    MaintenancePermitToken,
};
pub use path::{
    FilesystemEntryKind, FilesystemPolicy, PathCaseSensitivity, PathComparisonKey, VaultPath,
    VaultPathPolicy, detect_path_collisions,
};
pub use permission::{Permission, PermissionSet, Scope, ScopeSet};
pub use revision::{Revision, WritePrecondition};
pub use vault::{VaultContext, VaultSlug};
