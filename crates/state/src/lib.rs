//! SQLite operational state and repository implementations.
//!
//! SQL is confined to this crate. Callers receive typed records/repositories,
//! while every Vault-owned operation keeps its `VaultContext`/`VaultId` scope.

mod audit;
mod auth;
mod background;
mod backups;
mod current_memory;
mod error;
mod files;
mod index;
mod memory;
mod migrations;
mod pool;
mod providers;
mod settings;
mod vaults;

pub use audit::{AuditRecord, AuditRepository};
pub use auth::{
    AdminSessionRecord, AdminUserRecord, AuthStateRepository, EncryptedSecretRecord,
    McpTokenRecord, NewOAuthAccessToken, NewOAuthAuthorizationCode, NewOAuthRefreshToken,
    OAuthAccessTokenRecord, OAuthAuthorizationCodeRecord, OAuthAuthorizationRequestRecord,
    OAuthClientRecord, OAuthGrantRecord, OAuthIssuerRecord, OAuthLocalUserRecord,
    OAuthRefreshTokenRecord, WebDavCredentialRecord,
};
pub use background::{
    JobRecord, JobRepository, JobStatus, JobStatusCounts, OutboxEventRecord, OutboxRepository,
    ScanCheckpointRecord, ScanCheckpointRepository, ScanStatus,
};
pub use backups::{BackupRecord, BackupRepository, BackupStatus};
pub use current_memory::{
    CurrentExplicitReservation, CurrentMemoryBundle, CurrentMemoryCounts, CurrentMemoryFilter,
    CurrentMemoryOwnership, CurrentMemoryRecord, CurrentMemoryRepository, CurrentMemorySearchHit,
    CurrentMemorySourceRecord, MemoryNoteSetRecord, MemoryNoteSetSnapshotRecord,
    MemoryV2MigrationPreflight,
};
pub use error::{IntegrityReport, StateError};
pub use files::{
    CommitHook, CommitHookPhase, CommitMutationInput, EntryType, FileOperation, FileRecord,
    FileRevisionRecord, FileStateRepository, IdempotencyLookup, JournalRecord, JournalState,
    MutationCommitResult, NoopCommitHook, OutboxEventInput, PrepareOperationInput,
};
pub use index::{
    HeadingProjectionInput, IndexMembershipProjectionInput, IndexNodeProjectionInput,
    IndexNodeRecord, IndexRepository, IndexStatusRecord, LinkProjectionInput,
    NoteEmbeddingSourceRecord, NoteLinkRecord, NoteProjectionInput, NoteSearchRecord,
    TagProjectionInput,
};
pub use memory::{
    MemoryBundle, MemoryCandidateRecord, MemoryConsolidationProposalRecord,
    MemoryConsolidationStateRecord, MemoryCounts, MemoryDiagnosticRecord, MemoryFilter,
    MemoryIdempotencyRecord, MemoryPipelinePurgeReport, MemoryRecord, MemoryRelationRecord,
    MemoryRepository, MemoryRetrievalCoverage, MemoryRetrievalMetadataRecord,
    MemoryRetrievalProposalRecord, MemorySearchHit, MemorySourceAuditStateRecord,
    MemorySourceHealthCounts, MemorySourceHealthDetailRecord, MemorySourceHealthRecord,
    MemorySourceHealthState, MemorySourceRecord, MemoryStage1Counts, MemoryStage1OutputRecord,
    memory_search_terms,
};
pub use pool::{StateStore, StateTransaction};
pub use providers::{
    EmbeddingCoverage, EmbeddingRecord, ModelBindingRecord, ModelRecord, ProviderDeletionSummary,
    ProviderHealthRecord, ProviderRecord, ProviderRepository, VectorCandidate,
};
pub use settings::{SettingRecord, SettingsRepository};
pub use vaults::{
    LEGACY_DEFAULT_VAULT_SETTING, VaultAvailability, VaultRecord, VaultRepository, VaultStatus,
};

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_millis() -> Result<i64, StateError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StateError::InvalidInput("system clock is before Unix epoch"))?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| StateError::InvalidInput("system clock exceeds SQLite timestamp range"))
}
