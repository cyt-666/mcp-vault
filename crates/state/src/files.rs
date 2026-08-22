//! Vault-scoped file, revision, journal, audit, and outbox repositories.

use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};
use tokio::sync::Semaphore;

use mcp_vault_domain::{
    Actor, ActorId, ActorType, DomainError, FileId, OperationId, Revision, RevisionId, SourcePlane,
    VaultContext, VaultPath,
};

use crate::{StateError, now_millis};

/// Current file entry kind stored in SQLite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryType {
    /// Regular file containing canonical bytes.
    File,
    /// Directory collection.
    Directory,
}

impl EntryType {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "file" => Ok(Self::File),
            "directory" => Ok(Self::Directory),
            _ => Err(StateError::InvalidInput("stored entry type is invalid")),
        }
    }
}

/// Canonical mutation operation recorded in revision history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileOperation {
    /// First materialization of a path.
    Create,
    /// Full content replacement.
    Replace,
    /// Exact text patch.
    Patch,
    /// Exact append.
    Append,
    /// Rename preserving File ID.
    Move,
    /// Copy creating a new File ID.
    Copy,
    /// Tombstone a current entry.
    Delete,
    /// Materialize a previous history revision as a new revision.
    Restore,
    /// Import a change observed outside the service.
    ExternalChange,
}

impl FileOperation {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Replace => "replace",
            Self::Patch => "patch",
            Self::Append => "append",
            Self::Move => "move",
            Self::Copy => "copy",
            Self::Delete => "delete",
            Self::Restore => "restore",
            Self::ExternalChange => "external_change",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "create" => Ok(Self::Create),
            "replace" => Ok(Self::Replace),
            "patch" => Ok(Self::Patch),
            "append" => Ok(Self::Append),
            "move" => Ok(Self::Move),
            "copy" => Ok(Self::Copy),
            "delete" => Ok(Self::Delete),
            "restore" => Ok(Self::Restore),
            "external_change" => Ok(Self::ExternalChange),
            _ => Err(StateError::InvalidInput("stored file operation is invalid")),
        }
    }
}

/// Durable operation-journal lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalState {
    /// Intent is durable; no canonical rename is known yet.
    Prepared,
    /// Canonical filesystem mutation is known to have completed.
    FileCommitted,
    /// SQLite metadata, audit, and outbox are committed.
    MetadataCommitted,
    /// The operation was safely rolled back.
    RolledBack,
    /// Recovery could not prove old or new state.
    NeedsReview,
}

impl JournalState {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::FileCommitted => "file_committed",
            Self::MetadataCommitted => "metadata_committed",
            Self::RolledBack => "rolled_back",
            Self::NeedsReview => "needs_review",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "file_committed" => Ok(Self::FileCommitted),
            "metadata_committed" => Ok(Self::MetadataCommitted),
            "rolled_back" => Ok(Self::RolledBack),
            "needs_review" => Ok(Self::NeedsReview),
            _ => Err(StateError::InvalidInput("stored journal state is invalid")),
        }
    }
}

/// Current authoritative file identity and content metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileRecord {
    /// Stable identity independent from path.
    pub id: FileId,
    /// Owning Vault.
    pub vault_id: mcp_vault_domain::VaultId,
    /// Canonical normalized path.
    pub path: VaultPath,
    /// File or directory.
    pub entry_type: EntryType,
    /// Monotonically increasing current revision.
    pub current_revision: Revision,
    /// SHA-256 content address for regular files.
    pub content_hash: Option<String>,
    /// Size in bytes.
    pub size: u64,
    /// Filesystem modification timestamp in UTC milliseconds.
    pub modified_at: i64,
    /// Best-effort stable filesystem identity string.
    pub filesystem_identity: Option<String>,
    /// Tombstone timestamp when deleted.
    pub deleted_at: Option<i64>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last metadata update timestamp.
    pub updated_at: i64,
}

impl FileRecord {
    /// Return whether this row is a live canonical entry.
    pub const fn is_active(&self) -> bool {
        self.deleted_at.is_none()
    }
}

/// Immutable revision metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileRevisionRecord {
    /// Revision row identity.
    pub id: RevisionId,
    /// Owning Vault.
    pub vault_id: mcp_vault_domain::VaultId,
    /// Stable file identity.
    pub file_id: FileId,
    /// Monotonic revision number.
    pub revision: Revision,
    /// Operation that produced this revision.
    pub operation: FileOperation,
    /// Prior path for moves/deletes/restores.
    pub path_before: Option<VaultPath>,
    /// Resulting path.
    pub path_after: Option<VaultPath>,
    /// Current content hash, when content exists.
    pub content_hash: Option<String>,
    /// History blob containing the prior/current payload, when retained.
    pub history_blob_hash: Option<String>,
    /// Content size when known.
    pub size: Option<u64>,
    /// Actor category.
    pub actor_type: String,
    /// Non-secret actor identifier.
    pub actor_id: Option<ActorId>,
    /// Origin plane.
    pub source_plane: SourcePlane,
    /// Client idempotency key.
    pub idempotency_key: Option<String>,
    /// Creation timestamp.
    pub created_at: i64,
}

/// Durable write intent used by recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalRecord {
    /// Operation identity.
    pub id: OperationId,
    /// Owning Vault.
    pub vault_id: mcp_vault_domain::VaultId,
    /// Intended operation.
    pub operation: FileOperation,
    /// Current lifecycle state.
    pub state: JournalState,
    /// Source path, when applicable.
    pub source_path: Option<VaultPath>,
    /// Destination path, when applicable.
    pub destination_path: Option<VaultPath>,
    /// File ID affected by the intent.
    pub prior_file_id: Option<FileId>,
    /// Caller expected revision.
    pub expected_revision: Option<Revision>,
    /// Hash observed before the operation.
    pub prior_hash: Option<String>,
    /// Hash proposed by the operation.
    pub proposed_hash: Option<String>,
    /// Typed Vault-relative temporary path.
    pub temp_path: Option<VaultPath>,
    /// Durable operation description without note body.
    pub payload: Value,
    /// Durable client idempotency key.
    pub idempotency_key: Option<String>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last state update timestamp.
    pub updated_at: i64,
    /// Redacted recovery error, if any.
    pub error: Option<String>,
}

/// Outbox payload inserted atomically with a file revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxEventInput {
    /// Stable event name.
    pub event_type: String,
    /// Aggregate category.
    pub aggregate_type: String,
    /// Aggregate ID, never a secret.
    pub aggregate_id: String,
    /// JSON event payload without note body.
    pub payload: Value,
}

/// Inputs for creating a durable journal intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrepareOperationInput {
    /// Operation identity.
    pub id: OperationId,
    /// Operation type.
    pub operation: FileOperation,
    /// Source path.
    pub source_path: Option<VaultPath>,
    /// Destination path.
    pub destination_path: Option<VaultPath>,
    /// Existing file identity affected by the operation.
    pub prior_file_id: Option<FileId>,
    /// Expected current revision.
    pub expected_revision: Option<Revision>,
    /// Current content hash.
    pub prior_hash: Option<String>,
    /// Proposed content hash, if already known.
    pub proposed_hash: Option<String>,
    /// Same-Vault temporary path.
    pub temp_path: Option<VaultPath>,
    /// Bounded operation metadata.
    pub payload: Value,
    /// Optional client idempotency key.
    pub idempotency_key: Option<String>,
}

/// Inputs for one conditional metadata/revision commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitMutationInput {
    /// Journal operation being finalized.
    pub operation_id: OperationId,
    /// Stable file identity to create/update.
    pub file_id: FileId,
    /// Resulting entry kind.
    pub entry_type: EntryType,
    /// Resulting canonical path.
    pub path: VaultPath,
    /// Previous path, when applicable.
    pub path_before: Option<VaultPath>,
    /// Resulting path for revision metadata.
    pub path_after: Option<VaultPath>,
    /// Exact prior revision required for an update.
    pub expected_revision: Option<Revision>,
    /// Require no active destination at the resulting path.
    pub require_absent: bool,
    /// Move a source into a path whose prior tombstone must be archived
    /// inside the reserved operational namespace in the same transaction.
    pub tombstone_archive_path: Option<VaultPath>,
    /// Current content hash.
    pub content_hash: Option<String>,
    /// History blob hash for the prior/current payload.
    pub history_blob_hash: Option<String>,
    /// Resulting content size.
    pub size: u64,
    /// Observed filesystem modification timestamp.
    pub modified_at: i64,
    /// Filesystem identity hint serialized by Core.
    pub filesystem_identity: Option<String>,
    /// Tombstone timestamp for delete.
    pub deleted_at: Option<i64>,
    /// Revision operation.
    pub operation: FileOperation,
    /// Actor provenance.
    pub actor: Actor,
    /// Source plane.
    pub source_plane: SourcePlane,
    /// Client idempotency key.
    pub idempotency_key: Option<String>,
    /// Audit action label.
    pub audit_action: String,
    /// Redacted audit metadata.
    pub audit_metadata: Value,
    /// Optional request correlation identifier.
    pub request_id: Option<String>,
    /// Outbox events to insert in this transaction.
    pub outbox_events: Vec<OutboxEventInput>,
}

/// Result returned after a conditional metadata commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationCommitResult {
    /// New current file record.
    pub file: FileRecord,
    /// Immutable revision row.
    pub revision: FileRevisionRecord,
}

/// Durable result of an idempotency lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyLookup {
    /// The operation was committed previously.
    Committed {
        /// Stored operation payload for equivalence checking.
        payload: Value,
        /// Current file record.
        file: FileRecord,
        /// Original revision result.
        revision: Box<FileRevisionRecord>,
    },
    /// An operation is durable but has not completed metadata commit.
    InFlight {
        /// Original operation identity.
        operation_id: OperationId,
        /// Current journal state.
        state: JournalState,
        /// Stored operation payload.
        payload: Value,
    },
}

/// Fault-injection phase inside a state metadata commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitHookPhase {
    /// SQL transaction has started.
    MetadataTransactionStarted,
    /// All outbox rows have been inserted but transaction is not committed.
    OutboxInserted,
}

/// Test/observability seam that cannot execute SQL.
pub trait CommitHook: Send + Sync {
    /// Allow or reject the next commit phase.
    fn on_phase(&self, phase: CommitHookPhase) -> Result<(), StateError>;
}

/// No-op production commit hook.
pub struct NoopCommitHook;

impl CommitHook for NoopCommitHook {
    fn on_phase(&self, _phase: CommitHookPhase) -> Result<(), StateError> {
        Ok(())
    }
}

/// Repository for Vault file identities and mutation metadata.
#[derive(Clone)]
pub struct FileStateRepository {
    pool: SqlitePool,
    write_gate: Arc<Semaphore>,
}

impl FileStateRepository {
    pub(crate) fn new(pool: SqlitePool, write_gate: Arc<Semaphore>) -> Self {
        Self { pool, write_gate }
    }

    async fn acquire_write_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, StateError> {
        self.write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("file write gate is closed".to_owned()))
    }

    /// Read the live entry at a Vault-relative path.
    pub async fn get_active(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<Option<FileRecord>, StateError> {
        self.get_by_path(context, path, false).await
    }

    /// Read any entry at a path, including a tombstone.
    pub async fn get_any_by_path(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<Option<FileRecord>, StateError> {
        self.get_by_path(context, path, true).await
    }

    /// Read an entry by Vault-scoped stable File ID.
    pub async fn get_by_id(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<Option<FileRecord>, StateError> {
        let row = sqlx::query_as::<_, FileRow>(
            "SELECT id, vault_id, path, entry_type, current_revision,
                    content_hash, size, modified_at, filesystem_identity,
                    deleted_at, created_at, updated_at
             FROM file_entries
             WHERE vault_id = ? AND id = ?",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_file).transpose()
    }

    /// List all live entries for one Vault in deterministic path order.
    pub async fn list_active_entries(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<FileRecord>, StateError> {
        let rows = sqlx::query_as::<_, FileRow>(
            "SELECT id, vault_id, path, entry_type, current_revision,
                    content_hash, size, modified_at, filesystem_identity,
                    deleted_at, created_at, updated_at
             FROM file_entries
             WHERE vault_id = ? AND deleted_at IS NULL
             ORDER BY path ASC",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_file).collect()
    }

    /// Read one immutable revision for a Vault-scoped file.
    pub async fn get_revision(
        &self,
        context: &VaultContext,
        file_id: FileId,
        revision: Revision,
    ) -> Result<Option<FileRevisionRecord>, StateError> {
        let row = sqlx::query_as::<_, FileRevisionRow>(
            "SELECT id, vault_id, file_id, revision, operation,
                    path_before, path_after, content_hash, history_blob_hash,
                    size, actor_type, actor_id, source_plane,
                    idempotency_key, created_at
             FROM file_revisions
             WHERE vault_id = ? AND file_id = ? AND revision = ?",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .bind(revision.as_i64()?)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_revision).transpose()
    }

    /// List immutable revisions in ascending order.
    pub async fn list_revisions(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<Vec<FileRevisionRecord>, StateError> {
        let rows = sqlx::query_as::<_, FileRevisionRow>(
            "SELECT id, vault_id, file_id, revision, operation,
                    path_before, path_after, content_hash, history_blob_hash,
                    size, actor_type, actor_id, source_plane,
                    idempotency_key, created_at
             FROM file_revisions
             WHERE vault_id = ? AND file_id = ?
             ORDER BY revision ASC",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_revision).collect()
    }

    /// List the newest revision facts for one Vault in deterministic order.
    pub async fn list_recent_revisions(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<FileRevisionRecord>, StateError> {
        if limit == 0 || limit > 200 {
            return Err(StateError::InvalidInput(
                "recent revision limit must be between 1 and 200",
            ));
        }
        let rows = sqlx::query_as::<_, FileRevisionRow>(
            "SELECT id, vault_id, file_id, revision, operation,
                    path_before, path_after, content_hash, history_blob_hash,
                    size, actor_type, actor_id, source_plane,
                    idempotency_key, created_at
             FROM file_revisions
             WHERE vault_id = ?
             ORDER BY created_at DESC, id DESC
             LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_revision).collect()
    }

    /// Count durable outbox events for one Vault aggregate.
    pub async fn count_outbox_events(
        &self,
        context: &VaultContext,
        aggregate_id: &str,
    ) -> Result<u64, StateError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outbox_events
             WHERE vault_id = ? AND aggregate_id = ?",
        )
        .bind(context.id().to_string())
        .bind(aggregate_id)
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(count).map_err(|_| StateError::InvalidInput("outbox count is invalid"))
    }

    /// Count redacted audit entries for one Vault aggregate.
    pub async fn count_audit_entries(
        &self,
        context: &VaultContext,
        target_id: &str,
    ) -> Result<u64, StateError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log
             WHERE vault_id = ? AND target_id = ?",
        )
        .bind(context.id().to_string())
        .bind(target_id)
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(count).map_err(|_| StateError::InvalidInput("audit count is invalid"))
    }

    /// Find an already committed or in-flight idempotent operation.
    pub async fn find_idempotency(
        &self,
        context: &VaultContext,
        key: &str,
    ) -> Result<Option<IdempotencyLookup>, StateError> {
        validate_idempotency_key(key)?;
        let journal = sqlx::query_as::<_, JournalRow>(
            "SELECT id, vault_id, operation, state, source_path,
                    destination_path, prior_file_id, expected_revision,
                    prior_hash, proposed_hash, temp_path, payload_json,
                    idempotency_key, created_at, updated_at, error
             FROM operation_journal
             WHERE vault_id = ? AND idempotency_key = ?",
        )
        .bind(context.id().to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        let Some(journal) = journal else {
            return Ok(None);
        };
        let journal = row_to_journal(journal)?;
        let revision = sqlx::query_as::<_, FileRevisionRow>(
            "SELECT id, vault_id, file_id, revision, operation,
                    path_before, path_after, content_hash, history_blob_hash,
                    size, actor_type, actor_id, source_plane,
                    idempotency_key, created_at
             FROM file_revisions
             WHERE vault_id = ? AND idempotency_key = ?",
        )
        .bind(context.id().to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(revision) = revision {
            let revision = row_to_revision(revision)?;
            let file = self
                .get_by_id(context, revision.file_id)
                .await?
                .ok_or(StateError::InvalidInput("revision file entry is missing"))?;
            return Ok(Some(IdempotencyLookup::Committed {
                payload: journal.payload,
                file,
                revision: Box::new(revision),
            }));
        }

        Ok(Some(IdempotencyLookup::InFlight {
            operation_id: journal.id,
            state: journal.state,
            payload: journal.payload,
        }))
    }

    /// Insert the durable journal intent before touching canonical content.
    pub async fn prepare_operation(
        &self,
        context: &VaultContext,
        input: PrepareOperationInput,
    ) -> Result<JournalRecord, StateError> {
        let _write_permit = self.acquire_write_permit().await?;
        if let Some(key) = input.idempotency_key.as_deref() {
            validate_idempotency_key(key)?;
        }
        let now = now_millis()?;
        let payload_json = serde_json::to_string(&input.payload)?;
        sqlx::query(
            "INSERT INTO operation_journal
             (id, vault_id, operation, state, source_path, destination_path,
              prior_file_id, expected_revision, prior_hash, proposed_hash,
              temp_path, idempotency_key, payload_json, created_at, updated_at)
             VALUES (?, ?, ?, 'prepared', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.id.to_string())
        .bind(context.id().to_string())
        .bind(input.operation.as_str())
        .bind(input.source_path.as_ref().map(VaultPath::as_str))
        .bind(input.destination_path.as_ref().map(VaultPath::as_str))
        .bind(input.prior_file_id.map(|id| id.to_string()))
        .bind(input.expected_revision.map(Revision::as_i64).transpose()?)
        .bind(input.prior_hash.as_deref())
        .bind(input.proposed_hash.as_deref())
        .bind(input.temp_path.as_ref().map(VaultPath::as_str))
        .bind(input.idempotency_key.as_deref())
        .bind(payload_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(JournalRecord {
            id: input.id,
            vault_id: context.id(),
            operation: input.operation,
            state: JournalState::Prepared,
            source_path: input.source_path,
            destination_path: input.destination_path,
            prior_file_id: input.prior_file_id,
            expected_revision: input.expected_revision,
            prior_hash: input.prior_hash,
            proposed_hash: input.proposed_hash,
            temp_path: input.temp_path,
            payload: input.payload,
            idempotency_key: input.idempotency_key,
            created_at: now,
            updated_at: now,
            error: None,
        })
    }

    /// Mark the physical filesystem phase complete.
    pub async fn mark_file_committed(
        &self,
        context: &VaultContext,
        operation_id: OperationId,
        proposed_hash: Option<&str>,
    ) -> Result<(), StateError> {
        let _write_permit = self.acquire_write_permit().await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE operation_journal
             SET state = 'file_committed', proposed_hash = COALESCE(?, proposed_hash),
                 updated_at = ?
             WHERE vault_id = ? AND id = ? AND state = 'prepared'",
        )
        .bind(proposed_hash)
        .bind(now)
        .bind(context.id().to_string())
        .bind(operation_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("journal is not prepared"));
        }
        Ok(())
    }

    /// Persist a proposed content hash while the journal is still prepared.
    pub async fn set_proposed_hash(
        &self,
        context: &VaultContext,
        operation_id: OperationId,
        proposed_hash: &str,
    ) -> Result<(), StateError> {
        let _write_permit = self.acquire_write_permit().await?;
        if proposed_hash.is_empty() || proposed_hash.len() > 128 {
            return Err(StateError::InvalidInput("proposed hash is invalid"));
        }
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE operation_journal
             SET proposed_hash = ?, updated_at = ?
             WHERE vault_id = ? AND id = ? AND state = 'prepared'",
        )
        .bind(proposed_hash)
        .bind(now)
        .bind(context.id().to_string())
        .bind(operation_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("journal is not prepared"));
        }
        Ok(())
    }

    /// Update bounded journal metadata while the physical phase is prepared.
    pub async fn update_operation_payload(
        &self,
        context: &VaultContext,
        operation_id: OperationId,
        payload: &Value,
        proposed_hash: Option<&str>,
    ) -> Result<(), StateError> {
        let _write_permit = self.acquire_write_permit().await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE operation_journal
             SET payload_json = ?, proposed_hash = COALESCE(?, proposed_hash),
                 updated_at = ?
             WHERE vault_id = ? AND id = ? AND state = 'prepared'",
        )
        .bind(serde_json::to_string(payload)?)
        .bind(proposed_hash)
        .bind(now)
        .bind(context.id().to_string())
        .bind(operation_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("journal is not prepared"));
        }
        Ok(())
    }

    /// Atomically update file identity/revision and insert audit/outbox facts.
    pub async fn commit_mutation(
        &self,
        context: &VaultContext,
        input: CommitMutationInput,
        hook: &dyn CommitHook,
    ) -> Result<MutationCommitResult, StateError> {
        // SQLite has one writer even in WAL mode. Queue this short metadata
        // phase inside the process instead of making every concurrent PUT
        // race the database busy timeout. Canonical upload streaming, fsync,
        // history materialization, and atomic rename all happen before this
        // gate, so they retain their normal concurrency.
        let _write_permit = self.acquire_write_permit().await?;
        // This transaction reads the current file/destination rows before it
        // writes the revision, audit, outbox, and terminal journal state. A
        // deferred SQLite transaction can therefore acquire a stale WAL
        // snapshot and fail immediately with SQLITE_BUSY_SNAPSHOT when
        // another file commit wins the writer lock between those phases.
        // Acquire the write reservation before reading so concurrent Vault
        // commits wait under the configured busy timeout instead of leaving
        // canonical files stranded in `file_committed`.
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        hook.on_phase(CommitHookPhase::MetadataTransactionStarted)?;
        let now = now_millis()?;
        let vault_id = context.id().to_string();
        let file_id = input.file_id.to_string();
        let current = sqlx::query_as::<_, FileRow>(
            "SELECT id, vault_id, path, entry_type, current_revision,
                    content_hash, size, modified_at, filesystem_identity,
                    deleted_at, created_at, updated_at
             FROM file_entries
             WHERE vault_id = ? AND id = ?",
        )
        .bind(&vault_id)
        .bind(&file_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let destination = sqlx::query_as::<_, FileRow>(
            "SELECT id, vault_id, path, entry_type, current_revision,
                    content_hash, size, modified_at, filesystem_identity,
                    deleted_at, created_at, updated_at
             FROM file_entries
             WHERE vault_id = ? AND path = ?",
        )
        .bind(&vault_id)
        .bind(input.path.as_str())
        .fetch_optional(&mut *transaction)
        .await?;

        if input.require_absent
            && let Some(destination) = destination.as_ref()
            && destination.deleted_at.is_none()
            && destination.id != input.file_id.to_string()
        {
            return Err(StateError::InvalidDomain(DomainError::PreconditionFailed {
                reason: "destination already exists",
            }));
        }

        if let Some(archive_path) = input.tombstone_archive_path.as_ref() {
            let Some(destination) = destination.as_ref() else {
                return Err(StateError::InvalidInput(
                    "tombstone archive was requested without a destination",
                ));
            };
            if destination.deleted_at.is_none() || destination.id == file_id {
                return Err(StateError::InvalidInput(
                    "tombstone archive target is not a prior destination",
                ));
            }
            let archived = sqlx::query_scalar::<_, String>(
                "SELECT id FROM file_entries WHERE vault_id = ? AND path = ? LIMIT 1",
            )
            .bind(&vault_id)
            .bind(archive_path.as_str())
            .fetch_optional(&mut *transaction)
            .await?;
            if archived.is_some() {
                return Err(StateError::InvalidInput(
                    "tombstone archive path is already occupied",
                ));
            }
            let result = sqlx::query(
                "UPDATE file_entries
                 SET path = ?, updated_at = ?
                 WHERE vault_id = ? AND id = ? AND deleted_at IS NOT NULL",
            )
            .bind(archive_path.as_str())
            .bind(now)
            .bind(&vault_id)
            .bind(&destination.id)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() != 1 {
                return Err(StateError::InvalidInput(
                    "tombstone archive update did not match one row",
                ));
            }
        }

        let current_record = current.map(row_to_file).transpose()?;
        if let Some(expected) = input.expected_revision {
            let Some(current) = current_record.as_ref() else {
                return Err(StateError::InvalidDomain(DomainError::PreconditionFailed {
                    reason: "entry does not exist",
                }));
            };
            if !current.is_active() && input.operation != FileOperation::Restore {
                return Err(StateError::InvalidDomain(DomainError::PreconditionFailed {
                    reason: "entry is deleted",
                }));
            }
            if current.current_revision != expected {
                return Err(StateError::InvalidDomain(DomainError::RevisionConflict {
                    expected,
                    current: current.current_revision,
                }));
            }
        } else if let Some(current) = current_record.as_ref()
            && current.is_active()
            && input.require_absent
        {
            return Err(StateError::InvalidDomain(DomainError::PreconditionFailed {
                reason: "entry already exists",
            }));
        }

        if input.operation == FileOperation::Move && current_record.is_none() {
            return Err(StateError::InvalidInput("move file entry is missing"));
        }
        if let Some(destination) = destination.as_ref()
            && destination.deleted_at.is_some()
            && destination.id != file_id
            && input.tombstone_archive_path.is_none()
        {
            return Err(StateError::InvalidInput(
                "destination tombstone must be reused explicitly",
            ));
        }

        let next_revision = match current_record.as_ref() {
            Some(record) => record.current_revision.next()?,
            None => Revision::new(1),
        };
        let created_at = current_record
            .as_ref()
            .map_or(now, |record| record.created_at);
        if let Some(current) = current_record.as_ref() {
            let result = sqlx::query(
                "UPDATE file_entries
                 SET path = ?, entry_type = ?, current_revision = ?,
                     content_hash = ?, size = ?, modified_at = ?,
                     filesystem_identity = ?, deleted_at = ?, updated_at = ?
                 WHERE vault_id = ? AND id = ? AND current_revision = ?",
            )
            .bind(input.path.as_str())
            .bind(input.entry_type.as_str())
            .bind(next_revision.as_i64()?)
            .bind(input.content_hash.as_deref())
            .bind(
                i64::try_from(input.size)
                    .map_err(|_| StateError::InvalidInput("file size is too large"))?,
            )
            .bind(input.modified_at)
            .bind(input.filesystem_identity.as_deref())
            .bind(input.deleted_at)
            .bind(now)
            .bind(&vault_id)
            .bind(&file_id)
            .bind(current.current_revision.as_i64()?)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() != 1 {
                return Err(StateError::InvalidDomain(DomainError::RevisionConflict {
                    expected: current.current_revision,
                    current: current.current_revision.next()?,
                }));
            }
        } else {
            sqlx::query(
                "INSERT INTO file_entries
                 (id, vault_id, path, entry_type, current_revision,
                  content_hash, size, modified_at, filesystem_identity,
                  deleted_at, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&file_id)
            .bind(&vault_id)
            .bind(input.path.as_str())
            .bind(input.entry_type.as_str())
            .bind(next_revision.as_i64()?)
            .bind(input.content_hash.as_deref())
            .bind(
                i64::try_from(input.size)
                    .map_err(|_| StateError::InvalidInput("file size is too large"))?,
            )
            .bind(input.modified_at)
            .bind(input.filesystem_identity.as_deref())
            .bind(input.deleted_at)
            .bind(created_at)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        }

        let revision_id = RevisionId::new();
        let actor_type = actor_type_label(input.actor.actor_type());
        let actor_id = input.actor.actor_id().map(ActorId::as_str);
        let path_before = input
            .path_before
            .as_ref()
            .or_else(|| current_record.as_ref().map(|record| &record.path));
        let path_after = input.path_after.as_ref().or(Some(&input.path));
        sqlx::query(
            "INSERT INTO file_revisions
             (id, vault_id, file_id, revision, operation, path_before,
              path_after, content_hash, history_blob_hash, size, actor_type,
              actor_id, source_plane, idempotency_key, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(revision_id.to_string())
        .bind(&vault_id)
        .bind(&file_id)
        .bind(next_revision.as_i64()?)
        .bind(input.operation.as_str())
        .bind(path_before.map(VaultPath::as_str))
        .bind(path_after.map(VaultPath::as_str))
        .bind(input.content_hash.as_deref())
        .bind(input.history_blob_hash.as_deref())
        .bind(
            i64::try_from(input.size)
                .map_err(|_| StateError::InvalidInput("file size is too large"))?,
        )
        .bind(actor_type)
        .bind(actor_id)
        .bind(input.source_plane.as_str())
        .bind(input.idempotency_key.as_deref())
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        let audit_id = mcp_vault_domain::EventId::new();
        sqlx::query(
            "INSERT INTO audit_log
             (id, occurred_at, request_id, vault_id, plane, actor_type,
              actor_id, action, target_type, target_id, target_path_hash,
              result, metadata_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'file', ?, ?, 'success', ?)",
        )
        .bind(audit_id.to_string())
        .bind(now)
        .bind(input.request_id.as_deref())
        .bind(&vault_id)
        .bind(input.source_plane.as_str())
        .bind(actor_type_label(input.actor.actor_type()))
        .bind(input.actor.actor_id().map(ActorId::as_str))
        .bind(&input.audit_action)
        .bind(&file_id)
        .bind(path_hash(&input.path))
        .bind(serde_json::to_string(&input.audit_metadata)?)
        .execute(&mut *transaction)
        .await?;

        for event in &input.outbox_events {
            validate_event(event)?;
            sqlx::query(
                "INSERT INTO outbox_events
                 (id, vault_id, event_type, aggregate_type, aggregate_id,
                  payload_json, created_at, available_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(mcp_vault_domain::EventId::new().to_string())
            .bind(&vault_id)
            .bind(&event.event_type)
            .bind(&event.aggregate_type)
            .bind(&event.aggregate_id)
            .bind(serde_json::to_string(&event.payload)?)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        }
        hook.on_phase(CommitHookPhase::OutboxInserted)?;

        let journal = sqlx::query(
            "UPDATE operation_journal
             SET state = 'metadata_committed', updated_at = ?, error = NULL
             WHERE vault_id = ? AND id = ? AND state = 'file_committed'",
        )
        .bind(now)
        .bind(&vault_id)
        .bind(input.operation_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if journal.rows_affected() != 1 {
            return Err(StateError::InvalidInput("journal is not file-committed"));
        }
        transaction.commit().await?;

        let file = FileRecord {
            id: input.file_id,
            vault_id: context.id(),
            path: input.path.clone(),
            entry_type: input.entry_type,
            current_revision: next_revision,
            content_hash: input.content_hash.clone(),
            size: input.size,
            modified_at: input.modified_at,
            filesystem_identity: input.filesystem_identity.clone(),
            deleted_at: input.deleted_at,
            created_at,
            updated_at: now,
        };
        let revision = FileRevisionRecord {
            id: revision_id,
            vault_id: context.id(),
            file_id: input.file_id,
            revision: next_revision,
            operation: input.operation,
            path_before: path_before.cloned(),
            path_after: path_after.cloned(),
            content_hash: input.content_hash,
            history_blob_hash: input.history_blob_hash,
            size: Some(input.size),
            actor_type: actor_type_label(input.actor.actor_type()),
            actor_id: input.actor.actor_id().cloned(),
            source_plane: input.source_plane,
            idempotency_key: input.idempotency_key,
            created_at: now,
        };
        Ok(MutationCommitResult { file, revision })
    }

    /// List incomplete journal rows for startup/recovery.
    pub async fn list_incomplete(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<JournalRecord>, StateError> {
        let rows = sqlx::query_as::<_, JournalRow>(
            "SELECT id, vault_id, operation, state, source_path,
                    destination_path, prior_file_id, expected_revision,
                    prior_hash, proposed_hash, temp_path, payload_json,
                    idempotency_key, created_at, updated_at, error
             FROM operation_journal
             WHERE vault_id = ? AND state IN ('prepared', 'file_committed')
             ORDER BY created_at ASC",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_journal).collect()
    }

    /// Mark a journal row safely rolled back.
    pub async fn mark_rolled_back(
        &self,
        context: &VaultContext,
        operation_id: OperationId,
        error: Option<&str>,
    ) -> Result<(), StateError> {
        self.mark_terminal(context, operation_id, JournalState::RolledBack, error)
            .await
    }

    /// Mark a journal row for operator review.
    pub async fn mark_needs_review(
        &self,
        context: &VaultContext,
        operation_id: OperationId,
        error: &str,
    ) -> Result<(), StateError> {
        self.mark_terminal(
            context,
            operation_id,
            JournalState::NeedsReview,
            Some(error),
        )
        .await
    }

    async fn mark_terminal(
        &self,
        context: &VaultContext,
        operation_id: OperationId,
        state: JournalState,
        error: Option<&str>,
    ) -> Result<(), StateError> {
        let _write_permit = self.acquire_write_permit().await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE operation_journal
             SET state = ?, error = ?, updated_at = ?
             WHERE vault_id = ? AND id = ? AND state IN ('prepared', 'file_committed')",
        )
        .bind(state.as_str())
        .bind(error)
        .bind(now)
        .bind(context.id().to_string())
        .bind(operation_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("journal is already terminal"));
        }
        Ok(())
    }

    async fn get_by_path(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        include_deleted: bool,
    ) -> Result<Option<FileRecord>, StateError> {
        let query = if include_deleted {
            "SELECT id, vault_id, path, entry_type, current_revision,
                    content_hash, size, modified_at, filesystem_identity,
                    deleted_at, created_at, updated_at
             FROM file_entries WHERE vault_id = ? AND path = ?"
        } else {
            "SELECT id, vault_id, path, entry_type, current_revision,
                    content_hash, size, modified_at, filesystem_identity,
                    deleted_at, created_at, updated_at
             FROM file_entries WHERE vault_id = ? AND path = ? AND deleted_at IS NULL"
        };
        let row = sqlx::query_as::<_, FileRow>(query)
            .bind(context.id().to_string())
            .bind(path.as_str())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_file).transpose()
    }
}

#[derive(Debug, FromRow)]
struct FileRow {
    id: String,
    vault_id: String,
    path: String,
    entry_type: String,
    current_revision: i64,
    content_hash: Option<String>,
    size: i64,
    modified_at: i64,
    filesystem_identity: Option<String>,
    deleted_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct FileRevisionRow {
    id: String,
    vault_id: String,
    file_id: String,
    revision: i64,
    operation: String,
    path_before: Option<String>,
    path_after: Option<String>,
    content_hash: Option<String>,
    history_blob_hash: Option<String>,
    size: Option<i64>,
    actor_type: String,
    actor_id: Option<String>,
    source_plane: String,
    idempotency_key: Option<String>,
    created_at: i64,
}

#[derive(Debug, FromRow)]
struct JournalRow {
    id: String,
    vault_id: String,
    operation: String,
    state: String,
    source_path: Option<String>,
    destination_path: Option<String>,
    prior_file_id: Option<String>,
    expected_revision: Option<i64>,
    prior_hash: Option<String>,
    proposed_hash: Option<String>,
    temp_path: Option<String>,
    payload_json: String,
    idempotency_key: Option<String>,
    created_at: i64,
    updated_at: i64,
    error: Option<String>,
}

fn row_to_file(row: FileRow) -> Result<FileRecord, StateError> {
    Ok(FileRecord {
        id: FileId::parse(&row.id)?,
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        path: VaultPath::parse(&row.path)?,
        entry_type: EntryType::parse(&row.entry_type)?,
        current_revision: Revision::try_from(row.current_revision)?,
        content_hash: row.content_hash,
        size: u64::try_from(row.size)
            .map_err(|_| StateError::InvalidInput("stored file size is invalid"))?,
        modified_at: row.modified_at,
        filesystem_identity: row.filesystem_identity,
        deleted_at: row.deleted_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_revision(row: FileRevisionRow) -> Result<FileRevisionRecord, StateError> {
    Ok(FileRevisionRecord {
        id: RevisionId::parse(&row.id)?,
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        file_id: FileId::parse(&row.file_id)?,
        revision: Revision::try_from(row.revision)?,
        operation: FileOperation::parse(&row.operation)?,
        path_before: row
            .path_before
            .as_deref()
            .map(VaultPath::parse)
            .transpose()?,
        path_after: row
            .path_after
            .as_deref()
            .map(VaultPath::parse)
            .transpose()?,
        content_hash: row.content_hash,
        history_blob_hash: row.history_blob_hash,
        size: row
            .size
            .map(|size| {
                u64::try_from(size)
                    .map_err(|_| StateError::InvalidInput("stored revision size is invalid"))
            })
            .transpose()?,
        actor_type: row.actor_type,
        actor_id: row.actor_id.as_deref().map(ActorId::new).transpose()?,
        source_plane: row.source_plane.parse()?,
        idempotency_key: row.idempotency_key,
        created_at: row.created_at,
    })
}

fn row_to_journal(row: JournalRow) -> Result<JournalRecord, StateError> {
    Ok(JournalRecord {
        id: OperationId::parse(&row.id)?,
        vault_id: mcp_vault_domain::VaultId::parse(&row.vault_id)?,
        operation: FileOperation::parse(&row.operation)?,
        state: JournalState::parse(&row.state)?,
        source_path: row
            .source_path
            .as_deref()
            .map(VaultPath::parse)
            .transpose()?,
        destination_path: row
            .destination_path
            .as_deref()
            .map(VaultPath::parse)
            .transpose()?,
        prior_file_id: row
            .prior_file_id
            .as_deref()
            .map(FileId::parse)
            .transpose()?,
        expected_revision: row.expected_revision.map(Revision::try_from).transpose()?,
        prior_hash: row.prior_hash,
        proposed_hash: row.proposed_hash,
        temp_path: row.temp_path.as_deref().map(VaultPath::parse).transpose()?,
        payload: serde_json::from_str(&row.payload_json)?,
        idempotency_key: row.idempotency_key,
        created_at: row.created_at,
        updated_at: row.updated_at,
        error: row.error,
    })
}

fn actor_type_label(actor_type: ActorType) -> String {
    serde_json::to_string(&actor_type)
        .expect("ActorType serialization is infallible")
        .trim_matches('"')
        .to_owned()
}

fn validate_idempotency_key(key: &str) -> Result<(), StateError> {
    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(StateError::InvalidInput("idempotency key is invalid"));
    }
    Ok(())
}

fn validate_event(event: &OutboxEventInput) -> Result<(), StateError> {
    if event.event_type.is_empty()
        || event.aggregate_type.is_empty()
        || event.aggregate_id.is_empty()
    {
        return Err(StateError::InvalidInput("outbox event identity is invalid"));
    }
    Ok(())
}

fn path_hash(path: &VaultPath) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_str().as_bytes());
    let digest = hasher.finalize();
    let mut value = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}
