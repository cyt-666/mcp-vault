//! SQLite operational state and repository implementations.
//!
//! SQL is confined to this crate. Callers receive typed records/repositories,
//! while every Vault-owned operation keeps its `VaultContext`/`VaultId` scope.

mod audit;
mod auth;
mod background;
mod backups;
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
    McpTokenRecord, OAuthGrantRecord, OAuthIssuerRecord, WebDavCredentialRecord,
};
pub use background::{
    JobRecord, JobRepository, JobStatus, OutboxEventRecord, OutboxRepository, ScanCheckpointRecord,
    ScanCheckpointRepository, ScanStatus,
};
pub use backups::{BackupRecord, BackupRepository, BackupStatus};
pub use error::{IntegrityReport, StateError};
pub use files::{
    CommitHook, CommitHookPhase, CommitMutationInput, EntryType, FileOperation, FileRecord,
    FileRevisionRecord, FileStateRepository, IdempotencyLookup, JournalRecord, JournalState,
    MutationCommitResult, NoopCommitHook, OutboxEventInput, PrepareOperationInput,
};
pub use index::{
    HeadingProjectionInput, IndexMembershipProjectionInput, IndexNodeProjectionInput,
    IndexNodeRecord, IndexRepository, IndexStatusRecord, LinkProjectionInput, NoteLinkRecord,
    NoteProjectionInput, NoteSearchRecord, TagProjectionInput,
};
pub use memory::{
    MemoryBundle, MemoryCandidateRecord, MemoryCounts, MemoryDiagnosticRecord, MemoryFilter,
    MemoryIdempotencyRecord, MemoryRecord, MemoryRelationRecord, MemoryRepository, MemorySearchHit,
    MemorySourceRecord,
};
pub use pool::{StateStore, StateTransaction};
pub use providers::{
    EmbeddingCoverage, EmbeddingRecord, ModelBindingRecord, ModelRecord, ProviderHealthRecord,
    ProviderRecord, ProviderRepository, VectorCandidate,
};
pub use settings::{SettingRecord, SettingsRepository};
pub use vaults::{VaultRecord, VaultRepository, VaultStatus};

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_millis() -> Result<i64, StateError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StateError::InvalidInput("system clock is before Unix epoch"))?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| StateError::InvalidInput("system clock exceeds SQLite timestamp range"))
}
