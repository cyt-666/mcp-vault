//! Canonical Vault application services.
//!
//! Every later protocol adapter calls this boundary for file reads and
//! mutations. SQL remains inside `mcp-vault-state`; physical I/O remains
//! inside `mcp-vault-storage-fs`.

mod error;
mod lock;
mod patch;

use std::{
    collections::HashSet,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

use mcp_vault_domain::{
    Actor, DomainError, FileId, FilesystemEntryKind, MaintenanceGate, MaintenanceOperationGuard,
    Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug,
};
use mcp_vault_state::{
    CommitHook, CommitHookPhase, CommitMutationInput, EntryType, FileOperation, FileRecord,
    FileRevisionRecord, FileStateRepository, IdempotencyLookup, JobRecord, JournalRecord,
    JournalState, OutboxEventInput, PrepareOperationInput, StateError, StateStore, VaultRecord,
};
use mcp_vault_storage_fs::{
    AtomicWrite, ContentHash, DestinationPolicy, FileMetadata, HistoryStore, ReadFile,
    StorageError, StorageOptions, TemporaryPath, VaultStorage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
};

use crate::lock::PathLockManager;

pub use error::VaultError;

/// Commit phases used by crash-recovery tests and structured diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitPhase {
    /// Journal intent is durable.
    JournalPrepared,
    /// Temporary payload has been fully streamed and hashed.
    TempFileWritten,
    /// Temporary payload has been synced.
    FileFsynced,
    /// Canonical rename/delete/move has completed.
    RenameCommitted,
    /// SQLite metadata transaction has started.
    MetadataTransactionStarted,
    /// Audit/outbox rows are inserted but transaction is uncommitted.
    OutboxInserted,
    /// SQLite metadata transaction has committed.
    MetadataCommitted,
}

impl CommitPhase {
    /// Stable phase label for tests and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JournalPrepared => "journal_prepared",
            Self::TempFileWritten => "temp_file_written",
            Self::FileFsynced => "file_fsynced",
            Self::RenameCommitted => "rename_committed",
            Self::MetadataTransactionStarted => "metadata_transaction_started",
            Self::OutboxInserted => "outbox_inserted",
            Self::MetadataCommitted => "metadata_committed",
        }
    }
}

/// Failure-injection boundary for deterministic crash/recovery tests.
pub trait FailureInjector: Send + Sync {
    /// Return an error to stop immediately after a phase.
    fn fail(&self, phase: CommitPhase) -> Result<(), &'static str>;
}

/// Production no-op failure injector.
pub struct NoopFailureInjector;

impl FailureInjector for NoopFailureInjector {
    fn fail(&self, _phase: CommitPhase) -> Result<(), &'static str> {
        Ok(())
    }
}

/// Result of a committed canonical mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationResult {
    /// Current file identity and metadata.
    pub file: FileRecord,
    /// New immutable revision.
    pub revision: FileRevisionRecord,
    /// Strong ETag-compatible representation.
    pub etag: String,
}

/// A query result containing state metadata and a streaming reader.
pub struct ReadResult {
    /// Current file state.
    pub file: FileRecord,
    /// Filesystem metadata captured at open.
    pub metadata: FileMetadata,
    /// Streaming canonical bytes.
    pub reader: ReadFile,
    _operation: MaintenanceOperationGuard,
}

/// A stream for one explicitly service-managed file.
///
/// Managed files such as `_mcp-vault/index.yaml` are intentionally absent from
/// ordinary user file metadata and are never returned by `read`. Callers must
/// use the explicit managed Core method so the reserved-namespace policy stays
/// visible at the application boundary.
pub struct ManagedReadResult {
    /// Managed file identity and current optimistic revision.
    pub file: FileRecord,
    /// Filesystem metadata captured when the stream was opened.
    pub metadata: FileMetadata,
    /// Streaming managed bytes.
    pub reader: ReadFile,
    _operation: MaintenanceOperationGuard,
}

/// A historical content stream selected by an immutable revision.
pub struct RevisionReadResult {
    /// File identity associated with the selected revision.
    pub file: FileRecord,
    /// Immutable revision metadata.
    pub revision: FileRevisionRecord,
    /// Historical content bytes from the Vault-scoped history store.
    pub reader: ReadFile,
    _operation: MaintenanceOperationGuard,
}

/// A stat result without opening a content stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatResult {
    /// Current state record.
    pub file: FileRecord,
    /// Current filesystem metadata.
    pub metadata: FileMetadata,
}

/// Protocol-neutral metadata plus the stable DAV/application ETag value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoreMetadata {
    /// Safe storage metadata.
    pub metadata: FileMetadata,
    /// Unquoted ETag value suitable for HTTP/DAV header serialization.
    pub etag: String,
}

/// A Core-owned, journal-backed streamed content write.
pub struct StagedWrite {
    core: VaultCore,
    context: VaultContext,
    _operation: MaintenanceOperationGuard,
    _locks: Vec<tokio::sync::OwnedMutexGuard<()>>,
    storage: VaultStorage,
    history: HistoryStore,
    atomic: Option<AtomicWrite>,
    payload: CorePayload,
}

impl StagedWrite {
    /// Append one bounded upload chunk to the same-directory temporary file.
    pub async fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), VaultError> {
        self.atomic
            .as_mut()
            .ok_or(VaultError::InFlight)?
            .write_chunk(bytes)
            .await
            .map_err(VaultError::Storage)
    }

    /// Return the current staged byte/hash progress when finalized.
    pub fn progress(&self) -> Option<&mcp_vault_storage_fs::WriteProgress> {
        self.atomic.as_ref().and_then(AtomicWrite::progress)
    }

    /// Abort the staged upload and mark its journal safely rolled back.
    pub async fn abort(&mut self) {
        if let Some(mut atomic) = self.atomic.take() {
            atomic.abort().await;
        }
        let _ = self
            .core
            .state
            .files()
            .mark_rolled_back(
                &self.context,
                self.payload.operation_id,
                Some("staged write aborted"),
            )
            .await;
    }

    /// Finish the staged upload through the normal Core commit sequence.
    pub async fn commit(mut self) -> Result<MutationResult, VaultError> {
        let progress = self
            .atomic
            .as_mut()
            .ok_or(VaultError::InFlight)?
            .finish()
            .map_err(VaultError::Storage)?;
        self.payload.content_hash = Some(progress.content_hash.to_string());
        self.payload.history_blob_hash = self.payload.content_hash.clone();
        self.payload.size = progress.size;
        self.core
            .update_payload(&self.context, self.payload.operation_id, &self.payload)
            .await?;
        self.core.inject(CommitPhase::TempFileWritten)?;

        if let Err(error) = self
            .atomic
            .as_mut()
            .ok_or(VaultError::InFlight)?
            .sync()
            .await
            .map_err(VaultError::Storage)
        {
            self.abort().await;
            return Err(error);
        }
        self.core.inject(CommitPhase::FileFsynced)?;

        let atomic = self.atomic.take().ok_or(VaultError::InFlight)?;
        let receipt = match atomic.commit().await.map_err(VaultError::Storage) {
            Ok(receipt) => receipt,
            Err(error) => {
                let _ = self
                    .core
                    .state
                    .files()
                    .mark_rolled_back(
                        &self.context,
                        self.payload.operation_id,
                        Some("staged canonical commit failed"),
                    )
                    .await;
                return Err(error);
            }
        };
        self.payload.modified_at = receipt.metadata.modified_at;
        self.payload.filesystem_identity = identity_string(&receipt.metadata);
        self.core
            .update_payload(&self.context, self.payload.operation_id, &self.payload)
            .await?;
        self.core.inject(CommitPhase::RenameCommitted)?;
        self.core
            .state
            .files()
            .mark_file_committed(
                &self.context,
                self.payload.operation_id,
                self.payload.content_hash.as_deref(),
            )
            .await
            .map_err(VaultError::State)?;
        self.core
            .ensure_history_blob(
                &self.storage,
                &self.history,
                &self.payload.path,
                self.payload.content_hash.as_deref(),
            )
            .await?;
        let result = self
            .core
            .commit_payload(&self.context, self.payload.clone())
            .await?;
        self.core.inject(CommitPhase::MetadataCommitted)?;
        Ok(result)
    }
}

impl Drop for StagedWrite {
    fn drop(&mut self) {
        let Some(mut atomic) = self.atomic.take() else {
            return;
        };
        let state = self.core.state.clone();
        let context = self.context.clone();
        let operation_id = self.payload.operation_id;
        let cleanup = async move {
            atomic.abort().await;
            let _ = state
                .files()
                .mark_rolled_back(&context, operation_id, Some("staged write dropped"))
                .await;
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(cleanup);
        }
        // Without a live runtime, the journal remains recoverable on the next
        // startup; the temporary descriptor is still closed by drop.
    }
}

/// Recovery counters for one Vault.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Journal rows safely rolled back.
    pub rolled_back: usize,
    /// Journal rows whose metadata was finalized.
    pub finalized: usize,
    /// Journal rows requiring maintenance review.
    pub needs_review: usize,
}

/// Result of one bounded initial/reconciliation pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationReport {
    /// Safe entries observed by the scanner.
    pub entries_seen: u64,
    /// Regular files observed.
    pub files_seen: u64,
    /// Directories observed.
    pub directories_seen: u64,
    /// Entries whose content/identity was already represented correctly.
    pub unchanged: u64,
    /// New or modified files imported as external-change revisions.
    pub imported: u64,
    /// Files whose externally observed rename preserved identity.
    pub moved: u64,
    /// Directly deleted files imported as tombstones.
    pub deleted: u64,
    /// Unsafe/reserved/invalid entries skipped by storage-fs.
    pub unsafe_entries_skipped: u64,
    /// Missing-entry deletion was skipped because scan evidence was incomplete.
    pub missing_deletes_skipped: bool,
}

/// Process runtime shared by every Core instance and protocol plane.
#[derive(Clone)]
pub struct VaultCoreRuntime {
    locks: PathLockManager,
    maintenance: MaintenanceGate,
}

/// Opaque capability for recovery while the process is deliberately offline.
///
/// Only the composition-owned shared runtime can mint this value. Ordinary
/// protocol and worker calls continue through maintenance admission.
#[derive(Clone, Debug)]
pub struct MaintenanceRecoveryPermit {
    maintenance: MaintenanceGate,
}

impl std::fmt::Debug for VaultCoreRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultCoreRuntime")
            .field("maintenance", &self.maintenance)
            .finish_non_exhaustive()
    }
}

impl VaultCoreRuntime {
    /// Build a shared Core runtime around the process maintenance gate.
    pub fn new(maintenance: MaintenanceGate) -> Self {
        Self {
            locks: PathLockManager::default(),
            maintenance,
        }
    }

    /// Return the shared process maintenance coordinator.
    pub fn maintenance(&self) -> MaintenanceGate {
        self.maintenance.clone()
    }

    /// Mint an explicit permit for backup/restore recovery while offline.
    pub fn maintenance_recovery_permit(&self) -> MaintenanceRecoveryPermit {
        MaintenanceRecoveryPermit {
            maintenance: self.maintenance.clone(),
        }
    }
}

impl Default for VaultCoreRuntime {
    fn default() -> Self {
        Self::new(MaintenanceGate::new())
    }
}

/// Result of admitting one new service-managed Vault.
#[derive(Clone, Debug)]
pub struct ManagedVaultCreation {
    /// Registered Vault row.
    pub vault: VaultRecord,
    /// Durable initialization job scoped to the new Vault.
    pub initialization_job: JobRecord,
}

/// Application service for safe, service-managed Vault admission.
///
/// Admin handlers pass validated human input to this boundary. Filesystem
/// inspection remains in storage-fs and SQL remains in state.
#[derive(Clone)]
pub struct ManagedVaultService {
    state: StateStore,
    managed_root: PathBuf,
    storage_options: StorageOptions,
}

impl ManagedVaultService {
    /// Construct the service around the process data directory.
    pub fn new(state: StateStore, data_root: PathBuf, storage_options: StorageOptions) -> Self {
        Self {
            state,
            managed_root: data_root.join("vaults"),
            storage_options,
        }
    }

    /// Create and register one empty managed Vault, then enqueue its initial
    /// reconciliation. Slug and root bindings are immutable after admission.
    pub async fn create(
        &self,
        slug: VaultSlug,
        name: &str,
    ) -> Result<ManagedVaultCreation, VaultError> {
        // Upgrade compatibility: persist the existing sole/default Vault
        // before a second row can make legacy selection ambiguous.
        let _ = self.state.vaults().legacy_default().await?;
        if self.state.vaults().find_by_slug(&slug).await?.is_some() {
            return Err(VaultError::AlreadyExists);
        }
        let content_root = self.managed_root.join(slug.as_str());
        let context = VaultContext::new(VaultId::new(), slug, content_root, Revision::ZERO)?;
        let storage = VaultStorage::new(&context, VaultPathPolicy::default(), self.storage_options);
        storage.ensure_root().await?;
        if !storage.is_root_empty().await? {
            return Err(StorageError::InvalidOperation("managed Vault root is not empty").into());
        }

        let (vault, initialization_job) = match self
            .state
            .vaults()
            .insert_managed_with_initialization(&context, name)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if self
                    .state
                    .vaults()
                    .find_by_slug(context.slug())
                    .await?
                    .is_some()
                {
                    return Err(VaultError::AlreadyExists);
                }
                return Err(error.into());
            }
        };

        Ok(ManagedVaultCreation {
            vault,
            initialization_job,
        })
    }
}

/// Core service bound to operational state, storage roots, and lock policy.
#[derive(Clone)]
pub struct VaultCore {
    state: StateStore,
    history_root: PathBuf,
    path_policy: VaultPathPolicy,
    storage_options: StorageOptions,
    runtime: VaultCoreRuntime,
    failure: Arc<dyn FailureInjector>,
}

impl VaultCore {
    /// Construct a Core service with production no-op fault injection.
    pub fn new(
        state: StateStore,
        history_root: PathBuf,
        path_policy: VaultPathPolicy,
        storage_options: StorageOptions,
        runtime: VaultCoreRuntime,
    ) -> Self {
        Self {
            state,
            history_root,
            path_policy,
            storage_options,
            runtime,
            failure: Arc::new(NoopFailureInjector),
        }
    }

    /// Replace the failure injector, primarily for recovery tests.
    pub fn with_failure_injector(mut self, failure: Arc<dyn FailureInjector>) -> Self {
        self.failure = failure;
        self
    }

    /// Read a live regular file after checking state/filesystem agreement.
    pub async fn read(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<ReadResult, VaultError> {
        let operation = self.validate_context(context, false).await?;
        let _guards = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        let file = self.require_active_file(context, path).await?;
        let metadata = self.verify_current(&storage, &file).await?;
        let reader = storage.open_read(path).await.map_err(map_storage)?;
        Ok(ReadResult {
            file,
            metadata,
            reader,
            _operation: operation,
        })
    }

    /// Read one service-managed file through the explicit reserved-namespace
    /// boundary. This method does not create ordinary user file metadata.
    pub async fn read_managed(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<ManagedReadResult, VaultError> {
        let operation = self.validate_context(context, false).await?;
        self.path_policy
            .validate_managed_path(path)
            .map_err(VaultError::Domain)?;
        let _guards = self.acquire_locks(context, &[path]).await;
        let file = self
            .state
            .files()
            .get_active(context, path)
            .await?
            .ok_or(VaultError::NotFound)?;
        let reader = self
            .storage(context)
            .open_read_managed(path)
            .await
            .map_err(map_storage)?;
        let metadata = reader.metadata().clone();
        Ok(ManagedReadResult {
            file,
            metadata,
            reader,
            _operation: operation,
        })
    }

    /// Read content retained for one immutable revision through the Core
    /// boundary. The caller remains responsible for applying a response-size
    /// limit while consuming the returned stream.
    pub async fn read_revision(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        revision: Revision,
    ) -> Result<RevisionReadResult, VaultError> {
        let operation = self.validate_context(context, false).await?;
        let _guards = self.acquire_locks(context, &[path]).await;
        let file = self
            .state
            .files()
            .get_any_by_path(context, path)
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::NotFound)?;
        let revision_record = self
            .state
            .files()
            .get_revision(context, file.id, revision)
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::NotFound)?;
        let hash = revision_record
            .history_blob_hash
            .clone()
            .or(revision_record.content_hash.clone())
            .ok_or(VaultError::InvalidPatch("revision has no content blob"))?;
        let content_hash = ContentHash::from_hex(hash.strip_prefix("sha256:").unwrap_or(&hash))
            .map_err(|_| VaultError::InvalidPatch("revision content hash is invalid"))?;
        let reader = self
            .history_store(context)?
            .open(content_hash)
            .await
            .map_err(map_storage)?;
        Ok(RevisionReadResult {
            file,
            revision: revision_record,
            reader,
            _operation: operation,
        })
    }

    /// Read current metadata after checking the content hash and identity.
    pub async fn stat(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<StatResult, VaultError> {
        let _operation = self.validate_context(context, false).await?;
        let _guards = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        let file = self.require_active_file(context, path).await?;
        let metadata = self.verify_current(&storage, &file).await?;
        Ok(StatResult { file, metadata })
    }

    /// Read safe metadata for a Vault path, including the Vault root.
    pub async fn metadata(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<CoreMetadata, VaultError> {
        let _operation = self.validate_context(context, false).await?;
        let metadata = self
            .storage(context)
            .stat(path)
            .await
            .map_err(map_storage)?;
        let file = if path.is_root() {
            None
        } else {
            self.state
                .files()
                .get_active(context, path)
                .await
                .map_err(VaultError::State)?
        };
        Ok(CoreMetadata {
            etag: file
                .as_ref()
                .map_or_else(|| filesystem_etag(&metadata), file_etag_tag),
            metadata,
        })
    }

    /// List one directory level through the storage/Core boundary.
    pub async fn list_directory(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<Vec<CoreMetadata>, VaultError> {
        let _operation = self.validate_context(context, false).await?;
        let storage = self.storage(context);
        let entries = storage.list_directory(path).await.map_err(map_storage)?;
        let files = self.state.files();
        let mut result = Vec::with_capacity(entries.len());
        for metadata in entries {
            let file = match metadata.path.as_ref() {
                Some(path) => files
                    .get_active(context, path)
                    .await
                    .map_err(VaultError::State)?,
                None => None,
            };
            result.push(CoreMetadata {
                etag: file
                    .as_ref()
                    .map_or_else(|| filesystem_etag(&metadata), file_etag_tag),
                metadata,
            });
        }
        Ok(result)
    }

    /// Begin a journal-backed streamed PUT/replace operation.
    pub async fn begin_put(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        create: bool,
        create_new: bool,
        actor: Actor,
        source_plane: SourcePlane,
    ) -> Result<StagedWrite, VaultError> {
        let maintenance_operation = self.validate_context(context, true).await?;
        let locks = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        let history = self.history_store(context)?;
        let files = self.state.files();
        let target = files
            .get_any_by_path(context, path)
            .await
            .map_err(VaultError::State)?;
        let (operation, expected_revision, require_absent, prior_hash) =
            if let Some(target) = target.as_ref().filter(|file| file.is_active()) {
                if create_new {
                    return Err(VaultError::AlreadyExists);
                }
                if target.entry_type != EntryType::File {
                    return Err(VaultError::InvalidPatch("target is not a regular file"));
                }
                self.verify_current(&storage, target).await?;
                (
                    FileOperation::Replace,
                    Some(target.current_revision),
                    false,
                    target.content_hash.clone(),
                )
            } else {
                if !create {
                    return Err(VaultError::NotFound);
                }
                if !storage_absent(&storage, path).await? {
                    return Err(VaultError::ExternalMismatch);
                }
                (FileOperation::Create, None, true, None)
            };
        let operation_id = mcp_vault_domain::OperationId::new();
        if let Some(parent) = path.parent() {
            storage.create_dir_all(&parent).await.map_err(map_storage)?;
        }
        let temporary = storage
            .temporary_path_for(path)
            .map_err(VaultError::Storage)?;
        let file_id = target.as_ref().map_or_else(FileId::new, |file| file.id);
        let payload = CorePayload::new(
            operation_id,
            file_id,
            EntryType::File,
            operation,
            path.clone(),
            target.as_ref().map(|file| file.path.clone()),
            Some(path.clone()),
            expected_revision,
            require_absent,
            None,
            None,
            0,
            now_millis(),
            None,
            None,
            actor,
            source_plane,
            None,
            match operation {
                FileOperation::Create => "file.create",
                _ => "file.replace",
            },
            self.event_for(event_type_for(operation), file_id, path, operation),
        );
        files
            .prepare_operation(
                context,
                payload.prepare_input(
                    operation_id,
                    Some(path.clone()),
                    Some(temporary.as_path().clone()),
                    prior_hash,
                    None,
                )?,
            )
            .await
            .map_err(VaultError::State)?;
        self.inject(CommitPhase::JournalPrepared)?;
        let atomic = match storage
            .begin_atomic_write_at(
                path,
                if require_absent {
                    DestinationPolicy::MustNotExist
                } else {
                    DestinationPolicy::ReplaceExisting
                },
                &temporary,
            )
            .await
        {
            Ok(atomic) => atomic,
            Err(error) => {
                let _ = files
                    .mark_rolled_back(context, operation_id, Some("staged write could not start"))
                    .await;
                return Err(map_storage(error));
            }
        };
        Ok(StagedWrite {
            core: self.clone(),
            context: context.clone(),
            _operation: maintenance_operation,
            _locks: locks,
            storage,
            history,
            atomic: Some(atomic),
            payload,
        })
    }

    /// Create and journal one empty directory collection.
    pub async fn create_directory(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        actor: Actor,
        source_plane: SourcePlane,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        if path.is_root() {
            return Err(VaultError::AlreadyExists);
        }
        let _guards = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        let files = self.state.files();
        let existing = files
            .get_any_by_path(context, path)
            .await
            .map_err(VaultError::State)?;
        if existing.as_ref().is_some_and(FileRecord::is_active) {
            return Err(VaultError::AlreadyExists);
        }
        match storage.stat(path).await {
            Ok(_) => return Err(VaultError::AlreadyExists),
            Err(StorageError::SourceNotFound) => {}
            Err(error) => return Err(map_storage(error)),
        }
        if let Some(parent) = path.parent() {
            storage.create_dir_all(&parent).await.map_err(map_storage)?;
        }
        let operation_id = mcp_vault_domain::OperationId::new();
        let file_id = existing.as_ref().map_or_else(FileId::new, |file| file.id);
        let payload = CorePayload::new(
            operation_id,
            file_id,
            EntryType::Directory,
            FileOperation::Create,
            path.clone(),
            existing.as_ref().map(|file| file.path.clone()),
            Some(path.clone()),
            None,
            true,
            None,
            None,
            0,
            now_millis(),
            None,
            None,
            actor,
            source_plane,
            None,
            "directory.create",
            self.event_for("FileCreated", file_id, path, FileOperation::Create),
        );
        files
            .prepare_operation(
                context,
                payload.prepare_input(operation_id, Some(path.clone()), None, None, None)?,
            )
            .await
            .map_err(VaultError::State)?;
        self.inject(CommitPhase::JournalPrepared)?;
        if let Err(error) = storage.create_dir_all(path).await.map_err(map_storage) {
            let _ = files
                .mark_rolled_back(context, operation_id, Some("directory create failed"))
                .await;
            return Err(error);
        }
        self.inject(CommitPhase::RenameCommitted)?;
        files
            .mark_file_committed(context, operation_id, None)
            .await
            .map_err(VaultError::State)?;
        let result = self.commit_payload(context, payload).await?;
        self.inject(CommitPhase::MetadataCommitted)?;
        Ok(result)
    }

    /// Create a regular file with an absent-path precondition.
    pub async fn create_bytes(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        bytes: &[u8],
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        self.create(
            context,
            path,
            ByteReader::new(bytes.to_owned()),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Create one canonical service-managed file inside the reserved
    /// namespace. It receives the same journal/history/audit/outbox behavior
    /// as an ordinary Core file, but remains hidden from ordinary user paths.
    pub async fn create_managed_bytes(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        bytes: &[u8],
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        self.content_mutation_with_policy(
            context,
            path,
            FileOperation::Create,
            None,
            true,
            true,
            ByteReader::new(bytes.to_owned()),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Replace one canonical service-managed file at an exact revision.
    #[allow(clippy::too_many_arguments)]
    pub async fn replace_managed_bytes(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        bytes: &[u8],
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        self.content_mutation_with_policy(
            context,
            path,
            FileOperation::Replace,
            Some(expected_revision),
            false,
            true,
            ByteReader::new(bytes.to_owned()),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// List safe files inside the explicit managed namespace.
    pub async fn list_managed_files(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<FileMetadata>, VaultError> {
        let _operation = self.validate_context(context, false).await?;
        let storage = self.storage(context);
        let (sender, mut receiver) = mpsc::channel(64);
        let scan = tokio::spawn(async move { storage.walk_managed_entries(sender).await });
        let mut entries = Vec::new();
        while let Some(entry) = receiver.recv().await {
            entries.push(entry);
        }
        scan.await.map_err(|_| VaultError::Maintenance)??;
        Ok(entries)
    }

    /// Return whether a path is inside this Core's managed namespace.
    pub fn is_managed_path(&self, path: &VaultPath) -> bool {
        self.path_policy.is_reserved(path)
    }

    /// Return the configured reserved namespace root for managed services.
    pub fn managed_root(&self) -> &VaultPath {
        self.path_policy.reserved_root()
    }

    /// Stream-create a regular file.
    pub async fn create<R>(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        reader: R,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError>
    where
        R: AsyncRead + Unpin,
    {
        self.content_mutation(
            context,
            path,
            FileOperation::Create,
            None,
            true,
            reader,
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Replace content at an exact current revision.
    #[allow(clippy::too_many_arguments)]
    pub async fn replace_bytes(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        bytes: &[u8],
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        self.content_mutation(
            context,
            path,
            FileOperation::Replace,
            Some(expected_revision),
            false,
            ByteReader::new(bytes.to_owned()),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Append bytes at an exact current revision.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_bytes(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        bytes: &[u8],
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        let storage = self.storage(context);
        let current = self.require_active_file(context, path).await?;
        if current.entry_type != EntryType::File {
            return Err(VaultError::InvalidPatch("append target is not a file"));
        }
        self.require_revision(&current, expected_revision)?;
        let mut reader = storage.open_read(path).await.map_err(map_storage)?;
        let mut original = Vec::new();
        reader.read_to_end(&mut original).await.map_err(|error| {
            VaultError::Storage(StorageError::Io {
                operation: "read append source",
                kind: error.kind(),
            })
        })?;
        original.extend_from_slice(bytes);
        self.content_mutation(
            context,
            path,
            FileOperation::Append,
            Some(expected_revision),
            false,
            ByteReader::new(original),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Apply an exact unified diff at an exact current revision.
    #[allow(clippy::too_many_arguments)]
    pub async fn patch_unified_diff(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        unified_diff: &str,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        let storage = self.storage(context);
        let current = self.require_active_file(context, path).await?;
        self.require_revision(&current, expected_revision)?;
        let mut reader = storage.open_read(path).await.map_err(map_storage)?;
        let mut original = String::new();
        reader
            .read_to_string(&mut original)
            .await
            .map_err(|_| VaultError::BinaryTextOperation)?;
        let patched = patch::apply_unified_diff(&original, unified_diff)?;
        self.content_mutation(
            context,
            path,
            FileOperation::Patch,
            Some(expected_revision),
            false,
            ByteReader::new(patched.into_bytes()),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Insert exact UTF-8 content after one exact heading line.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_after_heading(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        heading: &str,
        insertion: &str,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        self.heading_edit(
            context,
            path,
            expected_revision,
            actor,
            source_plane,
            idempotency_key,
            |content| patch::insert_after_heading(content, heading, insertion),
            FileOperation::Patch,
        )
        .await
    }

    /// Replace the body of one exact Markdown heading section.
    #[allow(clippy::too_many_arguments)]
    pub async fn replace_heading_section(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        heading: &str,
        replacement: &str,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        self.heading_edit(
            context,
            path,
            expected_revision,
            actor,
            source_plane,
            idempotency_key,
            |content| patch::replace_heading_section(content, heading, replacement),
            FileOperation::Patch,
        )
        .await
    }

    /// Copy a regular file to a new path. The source is read from a stable
    /// descriptor before the destination mutation is admitted.
    pub async fn copy(
        &self,
        context: &VaultContext,
        source: &VaultPath,
        destination: &VaultPath,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        if source == destination {
            return Err(VaultError::Domain(DomainError::PreconditionFailed {
                reason: "copy source and destination match",
            }));
        }
        let _guards = self.acquire_locks(context, &[source, destination]).await;
        let storage = self.storage(context);
        let source_file = self.require_active_file(context, source).await?;
        self.verify_current(&storage, &source_file).await?;
        let reader = storage.open_read(source).await.map_err(map_storage)?;
        drop(_guards);
        self.content_mutation(
            context,
            destination,
            FileOperation::Copy,
            None,
            true,
            reader,
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Move/rename a regular file or directory while preserving File ID.
    #[allow(clippy::too_many_arguments)]
    pub async fn move_entry(
        &self,
        context: &VaultContext,
        source: &VaultPath,
        destination: &VaultPath,
        expected_revision: Revision,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        if source == destination {
            return Err(VaultError::Domain(DomainError::PreconditionFailed {
                reason: "move source and destination match",
            }));
        }
        let storage = self.storage(context);
        let files = self.state.files();
        let initial = files
            .get_active(context, source)
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::NotFound)?;
        if initial.current_revision != expected_revision {
            return Err(VaultError::RevisionConflict {
                expected: expected_revision,
                current: initial.current_revision,
                current_hash: initial.content_hash.clone(),
            });
        }
        if initial.entry_type == EntryType::Directory {
            return self
                .move_directory_entry(
                    context,
                    source,
                    destination,
                    actor,
                    source_plane,
                    idempotency_key,
                )
                .await;
        }
        let _guards = self.acquire_locks(context, &[source, destination]).await;
        if let Some(parent) = destination.parent() {
            storage.create_dir_all(&parent).await.map_err(map_storage)?;
        }
        let destination_record = files
            .get_any_by_path(context, destination)
            .await
            .map_err(VaultError::State)?;
        let current = self.require_active_file(context, source).await?;
        self.require_revision(&current, expected_revision)?;
        self.verify_current(&storage, &current).await?;
        self.ensure_destination_absent(context, &storage, destination)
            .await?;

        let operation_id = mcp_vault_domain::OperationId::new();
        let mut payload = CorePayload::new(
            operation_id,
            current.id,
            entry_type_of(&current),
            FileOperation::Move,
            destination.clone(),
            Some(source.clone()),
            Some(destination.clone()),
            Some(expected_revision),
            true,
            current.content_hash.clone(),
            current.content_hash.clone(),
            current.size,
            current.modified_at,
            current.filesystem_identity.clone(),
            None,
            actor.clone(),
            source_plane,
            idempotency_key.map(str::to_owned),
            "file.move",
            self.event_for("FileMoved", current.id, destination, FileOperation::Move),
        );
        if let Some(destination_record) = destination_record.as_ref()
            && !destination_record.is_active()
        {
            payload.tombstone_archive_path =
                Some(self.tombstone_archive_path(destination_record.id)?);
        }
        if let Some(result) = self
            .lookup_idempotency(context, idempotency_key, &payload)
            .await?
        {
            return Ok(result);
        }
        let temp_path = None;
        files
            .prepare_operation(
                context,
                PrepareOperationInput {
                    id: operation_id,
                    operation: FileOperation::Move,
                    source_path: Some(source.clone()),
                    destination_path: Some(destination.clone()),
                    prior_file_id: Some(current.id),
                    expected_revision: Some(expected_revision),
                    prior_hash: current.content_hash.clone(),
                    proposed_hash: current.content_hash.clone(),
                    temp_path,
                    payload: serde_json::to_value(&payload)
                        .map_err(|_| VaultError::InvalidPatch("operation payload is invalid"))?,
                    idempotency_key: idempotency_key.map(str::to_owned),
                },
            )
            .await
            .map_err(VaultError::State)?;
        self.inject(CommitPhase::JournalPrepared)?;
        storage
            .move_entry(source, destination, DestinationPolicy::MustNotExist)
            .await
            .map_err(map_storage)?;
        let metadata = storage.stat(destination).await.map_err(map_storage)?;
        payload.modified_at = metadata.modified_at.or(Some(now_millis()));
        payload.filesystem_identity = identity_string(&metadata);
        self.update_payload(context, operation_id, &payload).await?;
        self.inject(CommitPhase::RenameCommitted)?;
        files
            .mark_file_committed(context, operation_id, current.content_hash.as_deref())
            .await
            .map_err(VaultError::State)?;
        let result = self.commit_payload(context, payload).await?;
        self.inject(CommitPhase::MetadataCommitted)?;
        Ok(result)
    }

    async fn move_directory_entry(
        &self,
        context: &VaultContext,
        source: &VaultPath,
        destination: &VaultPath,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        if destination.starts_with(source) {
            return Err(VaultError::Domain(DomainError::PreconditionFailed {
                reason: "directory cannot move into its own subtree",
            }));
        }
        let storage = self.storage(context);
        let files = self.state.files();
        let entries = files
            .list_active_entries(context)
            .await
            .map_err(VaultError::State)?
            .into_iter()
            .filter(|entry| entry.path == *source || entry.path.starts_with(source))
            .collect::<Vec<_>>();
        let root = entries
            .iter()
            .find(|entry| entry.path == *source)
            .cloned()
            .ok_or(VaultError::NotFound)?;
        if root.entry_type != EntryType::Directory {
            return Err(VaultError::InvalidPatch(
                "directory move source is not a directory",
            ));
        }

        let mut mapped = Vec::with_capacity(entries.len());
        let mut lock_paths = vec![source.clone(), destination.clone()];
        for entry in entries {
            let relative = entry
                .path
                .as_str()
                .strip_prefix(source.as_str())
                .unwrap_or_default()
                .trim_start_matches('/');
            let target = if relative.is_empty() {
                destination.clone()
            } else {
                destination
                    .join(&VaultPath::parse(relative).map_err(VaultError::Domain)?)
                    .map_err(VaultError::Domain)?
            };
            lock_paths.push(entry.path.clone());
            lock_paths.push(target.clone());
            mapped.push((entry, target));
        }
        let lock_refs = lock_paths.iter().collect::<Vec<_>>();
        let _guards = self.acquire_locks(context, &lock_refs).await;

        let latest = files
            .list_active_entries(context)
            .await
            .map_err(VaultError::State)?;
        for (entry, _) in &mapped {
            let Some(current) = latest.iter().find(|candidate| candidate.id == entry.id) else {
                return Err(VaultError::RevisionConflict {
                    expected: entry.current_revision,
                    current: Revision::ZERO,
                    current_hash: None,
                });
            };
            if current.current_revision != entry.current_revision
                || current.path != entry.path
                || !current.is_active()
            {
                return Err(VaultError::RevisionConflict {
                    expected: entry.current_revision,
                    current: current.current_revision,
                    current_hash: current.content_hash.clone(),
                });
            }
        }
        if let Some(parent) = destination.parent() {
            storage.create_dir_all(&parent).await.map_err(map_storage)?;
        }
        if !storage_absent(&storage, destination).await? {
            return Err(VaultError::AlreadyExists);
        }

        let mut operations = Vec::with_capacity(mapped.len());
        for (entry, target) in mapped {
            let destination_record = files
                .get_any_by_path(context, &target)
                .await
                .map_err(VaultError::State)?;
            if destination_record
                .as_ref()
                .is_some_and(FileRecord::is_active)
            {
                return Err(VaultError::AlreadyExists);
            }
            let operation_id = mcp_vault_domain::OperationId::new();
            let mut payload = CorePayload::new(
                operation_id,
                entry.id,
                entry.entry_type,
                FileOperation::Move,
                target.clone(),
                Some(entry.path.clone()),
                Some(target.clone()),
                Some(entry.current_revision),
                true,
                entry.content_hash.clone(),
                entry.content_hash.clone(),
                entry.size,
                entry.modified_at,
                entry.filesystem_identity.clone(),
                None,
                actor.clone(),
                source_plane,
                if entry.path == *source {
                    idempotency_key.map(str::to_owned)
                } else {
                    None
                },
                "file.move",
                self.event_for("FileMoved", entry.id, &target, FileOperation::Move),
            );
            if let Some(destination_record) = destination_record.as_ref()
                && !destination_record.is_active()
            {
                payload.tombstone_archive_path =
                    Some(self.tombstone_archive_path(destination_record.id)?);
            }
            operations.push((entry, payload));
        }

        let mut prepared = Vec::with_capacity(operations.len());
        for (entry, payload) in &operations {
            let prepare = payload.prepare_input(
                payload.operation_id,
                Some(entry.path.clone()),
                None,
                entry.content_hash.clone(),
                entry.content_hash.clone(),
            )?;
            if let Err(error) = files.prepare_operation(context, prepare).await {
                for operation_id in prepared {
                    let _ = files
                        .mark_rolled_back(
                            context,
                            operation_id,
                            Some("directory move prepare failed"),
                        )
                        .await;
                }
                return Err(VaultError::State(error));
            }
            prepared.push(payload.operation_id);
        }
        if let Err(error) = self.inject(CommitPhase::JournalPrepared) {
            for operation_id in prepared {
                let _ = files
                    .mark_rolled_back(
                        context,
                        operation_id,
                        Some("directory move was not admitted"),
                    )
                    .await;
            }
            return Err(error);
        }
        if let Err(error) = storage
            .move_entry(source, destination, DestinationPolicy::MustNotExist)
            .await
            .map_err(map_storage)
        {
            for operation_id in prepared {
                let _ = files
                    .mark_rolled_back(context, operation_id, Some("directory move failed"))
                    .await;
            }
            return Err(error);
        }
        self.inject(CommitPhase::RenameCommitted)?;

        let mut root_result = None;
        for (entry, mut payload) in operations {
            let metadata = storage.stat(&payload.path).await.map_err(map_storage)?;
            payload.modified_at = metadata.modified_at.or(Some(now_millis()));
            payload.filesystem_identity = identity_string(&metadata);
            self.update_payload(context, payload.operation_id, &payload)
                .await?;
            files
                .mark_file_committed(context, payload.operation_id, entry.content_hash.as_deref())
                .await
                .map_err(VaultError::State)?;
            let result = self.commit_payload(context, payload).await?;
            if entry.path == *source {
                root_result = Some(result);
            }
        }
        self.inject(CommitPhase::MetadataCommitted)?;
        root_result.ok_or(VaultError::NotFound)
    }

    /// Tombstone a file while retaining its revision/history rows.
    pub async fn delete(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        let _guards = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        let history = self.history_store(context)?;
        let files = self.state.files();
        let current = self.require_active_file(context, path).await?;
        self.require_revision(&current, expected_revision)?;
        self.verify_current(&storage, &current).await?;
        let history_hash = self.capture_history(&storage, &history, &current).await?;
        let operation_id = mcp_vault_domain::OperationId::new();
        let deleted_at = now_millis();
        let payload = CorePayload::new(
            operation_id,
            current.id,
            entry_type_of(&current),
            FileOperation::Delete,
            path.clone(),
            Some(path.clone()),
            Some(path.clone()),
            Some(expected_revision),
            false,
            current.content_hash.clone(),
            history_hash.or(current.content_hash.clone()),
            current.size,
            current.modified_at,
            current.filesystem_identity.clone(),
            Some(deleted_at),
            actor,
            source_plane,
            idempotency_key.map(str::to_owned),
            "file.delete",
            self.event_for("FileDeleted", current.id, path, FileOperation::Delete),
        );
        if let Some(result) = self
            .lookup_idempotency(context, idempotency_key, &payload)
            .await?
        {
            return Ok(result);
        }
        files
            .prepare_operation(
                context,
                payload.prepare_input(
                    operation_id,
                    Some(path.clone()),
                    None,
                    current.content_hash.clone(),
                    None,
                )?,
            )
            .await
            .map_err(VaultError::State)?;
        self.inject(CommitPhase::JournalPrepared)?;
        storage.delete(path).await.map_err(map_storage)?;
        self.inject(CommitPhase::RenameCommitted)?;
        files
            .mark_file_committed(context, operation_id, current.content_hash.as_deref())
            .await
            .map_err(VaultError::State)?;
        let result = self.commit_payload(context, payload).await?;
        self.inject(CommitPhase::MetadataCommitted)?;
        Ok(result)
    }

    /// Tombstone one service-managed canonical file while retaining its
    /// revision/history rows. Ordinary user paths cannot call this method.
    pub async fn delete_managed(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        self.path_policy
            .validate_managed_path(path)
            .map_err(VaultError::Domain)?;
        let _guards = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        let history = self.history_store(context)?;
        let files = self.state.files();
        let current = self.require_active_file(context, path).await?;
        self.require_revision(&current, expected_revision)?;
        self.verify_current_with_policy(&storage, &current, true)
            .await?;
        let history_hash = self
            .capture_history_with_policy(&storage, &history, &current, true)
            .await?;
        let operation_id = mcp_vault_domain::OperationId::new();
        let payload = CorePayload::new(
            operation_id,
            current.id,
            entry_type_of(&current),
            FileOperation::Delete,
            path.clone(),
            Some(path.clone()),
            Some(path.clone()),
            Some(expected_revision),
            false,
            current.content_hash.clone(),
            history_hash.or(current.content_hash.clone()),
            current.size,
            current.modified_at,
            current.filesystem_identity.clone(),
            Some(now_millis()),
            actor,
            source_plane,
            idempotency_key.map(str::to_owned),
            "managed.file.delete",
            self.event_for("FileDeleted", current.id, path, FileOperation::Delete),
        );
        if let Some(result) = self
            .lookup_idempotency(context, idempotency_key, &payload)
            .await?
        {
            return Ok(result);
        }
        files
            .prepare_operation(
                context,
                payload.prepare_input(
                    operation_id,
                    Some(path.clone()),
                    None,
                    current.content_hash.clone(),
                    None,
                )?,
            )
            .await
            .map_err(VaultError::State)?;
        self.inject(CommitPhase::JournalPrepared)?;
        storage.delete_managed(path).await.map_err(map_storage)?;
        self.inject(CommitPhase::RenameCommitted)?;
        files
            .mark_file_committed(context, operation_id, current.content_hash.as_deref())
            .await
            .map_err(VaultError::State)?;
        let result = self.commit_payload(context, payload).await?;
        self.inject(CommitPhase::MetadataCommitted)?;
        Ok(result)
    }

    /// List immutable revision metadata for one live or tombstoned file.
    pub async fn history(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<Vec<FileRevisionRecord>, VaultError> {
        let _operation = self.validate_context(context, false).await?;
        let file = self
            .state
            .files()
            .get_any_by_path(context, path)
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::NotFound)?;
        self.state
            .files()
            .list_revisions(context, file.id)
            .await
            .map_err(VaultError::State)
    }

    /// Restore a history blob as a new current revision.
    #[allow(clippy::too_many_arguments)]
    pub async fn restore(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        revision: Revision,
        expected_current_revision: Revision,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        let file = self
            .state
            .files()
            .get_any_by_path(context, path)
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::NotFound)?;
        self.require_revision(&file, expected_current_revision)?;
        let revision_record = self
            .state
            .files()
            .get_revision(context, file.id, revision)
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::NotFound)?;
        let hash = revision_record
            .history_blob_hash
            .clone()
            .or(revision_record.content_hash.clone())
            .ok_or(VaultError::InvalidPatch("revision has no content blob"))?;
        let content_hash = ContentHash::from_hex(hash.strip_prefix("sha256:").unwrap_or(&hash))
            .map_err(|_| VaultError::InvalidPatch("revision content hash is invalid"))?;
        let history = self.history_store(context)?;
        let mut reader = history.open(content_hash).await.map_err(map_storage)?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.map_err(|error| {
            VaultError::Storage(StorageError::Io {
                operation: "read history blob",
                kind: error.kind(),
            })
        })?;
        self.content_mutation(
            context,
            path,
            FileOperation::Restore,
            Some(expected_current_revision),
            false,
            ByteReader::new(bytes),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    /// Reconcile safe filesystem observations into Vault-scoped external
    /// change revisions. The scan never writes canonical bytes.
    pub async fn reconcile(
        &self,
        context: &VaultContext,
        actor: Actor,
    ) -> Result<ReconciliationReport, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        let storage = self.storage(context);
        let files = self.state.files();
        let known = files
            .list_active_entries(context)
            .await
            .map_err(VaultError::State)?
            .into_iter()
            .filter(|file| !self.path_policy.is_reserved(&file.path))
            .collect::<Vec<_>>();
        let (sender, mut receiver) = mpsc::channel(64);
        let scan_storage = storage.clone();
        let scan_task = tokio::spawn(async move { scan_storage.walk_entries(sender).await });
        let mut report = ReconciliationReport::default();
        let mut seen_keys = HashSet::new();

        while let Some(metadata) = receiver.recv().await {
            report.entries_seen = report.entries_seen.saturating_add(1);
            let Some(path) = metadata.path.clone() else {
                continue;
            };
            let key = path
                .comparison_key(self.path_policy.case_sensitivity())
                .as_str()
                .to_owned();
            if !seen_keys.insert(key) {
                report.unsafe_entries_skipped = report.unsafe_entries_skipped.saturating_add(1);
                continue;
            }
            if metadata.kind == FilesystemEntryKind::Directory {
                report.directories_seen = report.directories_seen.saturating_add(1);
                continue;
            }
            if metadata.kind != FilesystemEntryKind::RegularFile {
                report.unsafe_entries_skipped = report.unsafe_entries_skipped.saturating_add(1);
                continue;
            }
            report.files_seen = report.files_seen.saturating_add(1);
            let (_, content_hash) = storage.hash_file(&path).await.map_err(map_storage)?;
            let content_hash_text = content_hash.to_string();
            let current = files
                .get_active(context, &path)
                .await
                .map_err(VaultError::State)?;
            let identity = identity_string(&metadata);
            if current.is_none()
                && let Some(candidate) = known.iter().find(|file| {
                    file.entry_type == EntryType::File
                        && file.content_hash.as_deref() == Some(content_hash_text.as_str())
                        && file.filesystem_identity == identity
                        && file.path != path
                        && !seen_keys.contains(
                            file.path
                                .comparison_key(self.path_policy.case_sensitivity())
                                .as_str(),
                        )
                })
                && files
                    .get_any_by_path(context, &path)
                    .await
                    .map_err(VaultError::State)?
                    .is_none()
                && storage_absent(&storage, &candidate.path).await?
            {
                self.import_external_move(
                    context,
                    candidate,
                    &path,
                    &metadata,
                    content_hash,
                    actor.clone(),
                )
                .await?;
                report.moved = report.moved.saturating_add(1);
                continue;
            }
            if current.as_ref().is_some_and(|file| {
                file.entry_type == EntryType::File
                    && file.size == metadata.size
                    && file.content_hash.as_deref() == Some(content_hash_text.as_str())
                    && file.filesystem_identity == identity
            }) {
                report.unchanged = report.unchanged.saturating_add(1);
                continue;
            }
            self.import_external_file(context, &path, &metadata, content_hash, actor.clone())
                .await?;
            report.imported = report.imported.saturating_add(1);
        }
        let scan_summary = scan_task
            .await
            .map_err(|_| VaultError::Maintenance)?
            .map_err(map_storage)?;
        report.unsafe_entries_skipped = report
            .unsafe_entries_skipped
            .saturating_add(scan_summary.unsafe_entries_skipped);

        if scan_summary.unsafe_entries_skipped == 0 {
            for file in known {
                let key = file
                    .path
                    .comparison_key(self.path_policy.case_sensitivity())
                    .as_str()
                    .to_owned();
                if !seen_keys.contains(&key)
                    && self
                        .import_external_delete(context, &file.path, actor.clone())
                        .await?
                {
                    report.deleted = report.deleted.saturating_add(1);
                }
            }
        } else {
            report.missing_deletes_skipped = true;
        }
        Ok(report)
    }

    async fn import_external_file(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        metadata: &FileMetadata,
        content_hash: ContentHash,
        actor: Actor,
    ) -> Result<(), VaultError> {
        let _guards = self.acquire_locks(context, &[path]).await;
        let files = self.state.files();
        let current = files
            .get_any_by_path(context, path)
            .await
            .map_err(VaultError::State)?;
        let current_active = current.as_ref().filter(|file| file.is_active());
        let event_type = match current.as_ref() {
            None => "FileCreated",
            Some(file) if file.is_active() => "FileUpdated",
            Some(_) => "FileRestored",
        };
        let expected_revision = current_active.map(|file| file.current_revision);
        let file_id = current.as_ref().map_or_else(FileId::new, |file| file.id);
        let hash = content_hash.to_string();
        if current_active.is_some_and(|file| {
            file.entry_type == EntryType::File
                && file.size == metadata.size
                && file.content_hash.as_deref() == Some(hash.as_str())
                && file.filesystem_identity == identity_string(metadata)
        }) {
            return Ok(());
        }
        let now = now_millis();
        let operation_id = mcp_vault_domain::OperationId::new();
        let payload = CorePayload::new(
            operation_id,
            file_id,
            EntryType::File,
            FileOperation::ExternalChange,
            path.clone(),
            current.as_ref().map(|file| file.path.clone()),
            Some(path.clone()),
            expected_revision,
            false,
            Some(hash.clone()),
            Some(hash.clone()),
            metadata.size,
            metadata.modified_at.unwrap_or(now),
            identity_string(metadata),
            None,
            actor,
            SourcePlane::Reconciliation,
            None,
            "file.external_change",
            self.event_for(event_type, file_id, path, FileOperation::ExternalChange),
        );
        files
            .prepare_operation(
                context,
                payload.prepare_input(
                    operation_id,
                    Some(path.clone()),
                    None,
                    current.as_ref().and_then(|file| file.content_hash.clone()),
                    Some(hash.clone()),
                )?,
            )
            .await
            .map_err(VaultError::State)?;
        files
            .mark_file_committed(context, operation_id, Some(&hash))
            .await
            .map_err(VaultError::State)?;
        let history = self.history_store(context)?;
        self.ensure_history_blob(&self.storage(context), &history, path, Some(&hash))
            .await?;
        self.commit_payload(context, payload).await.map(|_| ())
    }

    async fn import_external_move(
        &self,
        context: &VaultContext,
        current: &FileRecord,
        destination: &VaultPath,
        metadata: &FileMetadata,
        content_hash: ContentHash,
        actor: Actor,
    ) -> Result<(), VaultError> {
        let _guards = self
            .acquire_locks(context, &[&current.path, destination])
            .await;
        let storage = self.storage(context);
        if !storage_absent(&storage, &current.path).await? {
            return Err(VaultError::ExternalMismatch);
        }
        let (_, observed_hash) = storage.hash_file(destination).await.map_err(map_storage)?;
        if observed_hash != content_hash {
            return Err(VaultError::ExternalMismatch);
        }
        let operation_id = mcp_vault_domain::OperationId::new();
        let hash = content_hash.to_string();
        let payload = CorePayload::new(
            operation_id,
            current.id,
            current.entry_type,
            FileOperation::Move,
            destination.clone(),
            Some(current.path.clone()),
            Some(destination.clone()),
            Some(current.current_revision),
            true,
            Some(hash.clone()),
            Some(hash.clone()),
            metadata.size,
            metadata.modified_at.unwrap_or_else(now_millis),
            identity_string(metadata),
            None,
            actor,
            SourcePlane::Reconciliation,
            None,
            "file.external_move",
            self.event_for("FileMoved", current.id, destination, FileOperation::Move),
        );
        let files = self.state.files();
        files
            .prepare_operation(
                context,
                payload.prepare_input(
                    operation_id,
                    Some(current.path.clone()),
                    None,
                    current.content_hash.clone(),
                    Some(hash),
                )?,
            )
            .await
            .map_err(VaultError::State)?;
        files
            .mark_file_committed(context, operation_id, current.content_hash.as_deref())
            .await
            .map_err(VaultError::State)?;
        self.commit_payload(context, payload).await.map(|_| ())
    }

    async fn import_external_delete(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        actor: Actor,
    ) -> Result<bool, VaultError> {
        let _guards = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        if !storage_absent(&storage, path).await? {
            return Ok(false);
        }
        let files = self.state.files();
        let Some(current) = files
            .get_active(context, path)
            .await
            .map_err(VaultError::State)?
        else {
            return Ok(false);
        };
        let operation_id = mcp_vault_domain::OperationId::new();
        let now = now_millis();
        let file_id = current.id;
        let payload = CorePayload::new(
            operation_id,
            file_id,
            EntryType::File,
            FileOperation::ExternalChange,
            path.clone(),
            Some(path.clone()),
            Some(path.clone()),
            Some(current.current_revision),
            false,
            None,
            current.content_hash.clone(),
            0,
            now,
            None,
            Some(now),
            actor,
            SourcePlane::Reconciliation,
            None,
            "file.external_delete",
            self.event_for("FileDeleted", file_id, path, FileOperation::ExternalChange),
        );
        files
            .prepare_operation(
                context,
                payload.prepare_input(
                    operation_id,
                    Some(path.clone()),
                    None,
                    current.content_hash.clone(),
                    None,
                )?,
            )
            .await
            .map_err(VaultError::State)?;
        files
            .mark_file_committed(context, operation_id, None)
            .await
            .map_err(VaultError::State)?;
        self.commit_payload(context, payload).await?;
        Ok(true)
    }

    /// Reconcile incomplete journal rows for one Vault.
    pub async fn recover(&self, context: &VaultContext) -> Result<RecoveryReport, VaultError> {
        let _operation = self.validate_context(context, true).await?;
        self.recover_inner(context).await
    }

    /// Recover incomplete journals while a staged restore owns the explicit
    /// process-offline boundary.
    pub async fn recover_during_maintenance(
        &self,
        context: &VaultContext,
        permit: &MaintenanceRecoveryPermit,
    ) -> Result<RecoveryReport, VaultError> {
        if !self.runtime.maintenance.is_same_gate(&permit.maintenance) {
            return Err(VaultError::Maintenance);
        }
        // Recovery is an internal consistency operation, not a user write.
        // Disabled/error Vaults still need their durable journal reconciled so
        // one unavailable Vault cannot prevent the process from restarting.
        self.validate_registered_context(context, false).await?;
        self.recover_inner(context).await
    }

    async fn recover_inner(&self, context: &VaultContext) -> Result<RecoveryReport, VaultError> {
        let storage = self.storage(context);
        let files = self.state.files();
        let history = self.history_store(context)?;
        let journals = files
            .list_incomplete(context)
            .await
            .map_err(VaultError::State)?;
        let mut report = RecoveryReport::default();
        for journal in journals {
            match self
                .recover_one(context, &storage, &history, &files, journal)
                .await?
            {
                RecoveryOutcome::RolledBack => report.rolled_back += 1,
                RecoveryOutcome::Finalized => report.finalized += 1,
                RecoveryOutcome::NeedsReview => report.needs_review += 1,
            }
        }
        Ok(report)
    }

    async fn recover_one(
        &self,
        context: &VaultContext,
        storage: &mcp_vault_storage_fs::VaultStorage,
        history: &HistoryStore,
        files: &FileStateRepository,
        journal: JournalRecord,
    ) -> Result<RecoveryOutcome, VaultError> {
        let payload: CorePayload =
            serde_json::from_value(journal.payload.clone()).map_err(|_| VaultError::NeedsReview)?;
        let old = self
            .recovery_state(storage, &journal, &payload, false)
            .await?;
        let new = self
            .recovery_state(storage, &journal, &payload, true)
            .await?;
        if old && !new {
            if let Some(temp) = journal.temp_path.as_ref() {
                let temp = TemporaryPath::parse(temp.clone()).map_err(VaultError::Storage)?;
                storage.remove_temporary(&temp).await.map_err(map_storage)?;
            }
            files
                .mark_rolled_back(
                    context,
                    journal.id,
                    Some("recovered before canonical commit"),
                )
                .await
                .map_err(VaultError::State)?;
            return Ok(RecoveryOutcome::RolledBack);
        }
        if new {
            // A no-replace compatibility commit may have linked the complete
            // temporary inode at the canonical name immediately before a
            // crash. Once the expected canonical hash proves the new state,
            // the journal-owned temporary name is safe to remove before
            // finalizing metadata. Ordinary rename commits reach the same
            // path with an already-absent temporary name.
            if let Some(temp) = journal.temp_path.as_ref() {
                let temp = TemporaryPath::parse(temp.clone()).map_err(VaultError::Storage)?;
                storage.remove_temporary(&temp).await.map_err(map_storage)?;
            }
            if journal.state == JournalState::Prepared {
                files
                    .mark_file_committed(context, journal.id, payload.content_hash.as_deref())
                    .await
                    .map_err(VaultError::State)?;
            }
            if payload.content_hash.is_some() && payload.operation != FileOperation::Delete.as_str()
            {
                self.ensure_history_blob_with_policy(
                    storage,
                    history,
                    &payload.path,
                    payload.content_hash.as_deref(),
                    self.path_policy.is_reserved(&payload.path),
                )
                .await?;
            }
            let result = self.commit_payload(context, payload).await;
            return match result {
                Ok(_) => Ok(RecoveryOutcome::Finalized),
                Err(VaultError::State(StateError::InvalidInput(_))) => {
                    files
                        .mark_needs_review(
                            context,
                            journal.id,
                            "recovery metadata commit was not provable",
                        )
                        .await
                        .map_err(VaultError::State)?;
                    Ok(RecoveryOutcome::NeedsReview)
                }
                Err(error) => Err(error),
            };
        }
        files
            .mark_needs_review(context, journal.id, "canonical old/new state is ambiguous")
            .await
            .map_err(VaultError::State)?;
        Ok(RecoveryOutcome::NeedsReview)
    }

    async fn recovery_state(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        journal: &JournalRecord,
        payload: &CorePayload,
        new: bool,
    ) -> Result<bool, VaultError> {
        if payload.operation == FileOperation::Move.as_str() {
            let source = journal
                .source_path
                .as_ref()
                .ok_or(VaultError::NeedsReview)?;
            let destination = journal
                .destination_path
                .as_ref()
                .ok_or(VaultError::NeedsReview)?;
            let source_exists = self
                .path_matches(storage, source, payload.content_hash.as_deref())
                .await?;
            let destination_exists = self
                .path_matches(storage, destination, payload.content_hash.as_deref())
                .await?;
            return Ok(if new {
                !source_exists && destination_exists
            } else {
                source_exists && !destination_exists
            });
        }
        let target = journal
            .destination_path
            .as_ref()
            .or(journal.source_path.as_ref())
            .ok_or(VaultError::NeedsReview)?;
        let is_delete = payload.operation == FileOperation::Delete.as_str()
            || (payload.operation == FileOperation::ExternalChange.as_str()
                && payload.deleted_at.is_some());
        let exists = self
            .path_matches(storage, target, payload.content_hash.as_deref())
            .await?;
        if new {
            Ok(if is_delete { !exists } else { exists })
        } else if is_delete {
            Ok(exists)
        } else if let Some(prior_hash) = journal.prior_hash.as_deref() {
            Ok(self.path_matches(storage, target, Some(prior_hash)).await?)
        } else {
            Ok(!exists)
        }
    }

    async fn path_matches(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        path: &VaultPath,
        hash: Option<&str>,
    ) -> Result<bool, VaultError> {
        let managed = self.path_policy.is_reserved(path);
        let metadata = match if managed {
            storage.stat_managed(path).await
        } else {
            storage.stat(path).await
        } {
            Ok(metadata) => metadata,
            Err(error) if storage_not_found(&error) => return Ok(false),
            Err(error) => return Err(map_storage(error)),
        };
        let Some(hash) = hash else {
            return Ok(true);
        };
        let (_, actual) = if managed {
            storage.hash_file_managed(path).await
        } else {
            storage.hash_file(path).await
        }
        .map_err(map_storage)?;
        Ok(
            actual.to_string() == hash.strip_prefix("sha256:").unwrap_or(hash)
                && metadata.size
                    == if managed {
                        storage.stat_managed(path).await
                    } else {
                        storage.stat(path).await
                    }
                    .map_err(map_storage)?
                    .size,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn content_mutation<R>(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        operation: FileOperation,
        expected_revision: Option<Revision>,
        require_absent: bool,
        reader: R,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError>
    where
        R: AsyncRead + Unpin,
    {
        self.content_mutation_with_policy(
            context,
            path,
            operation,
            expected_revision,
            require_absent,
            false,
            reader,
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn content_mutation_with_policy<R>(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        operation: FileOperation,
        expected_revision: Option<Revision>,
        require_absent: bool,
        managed: bool,
        mut reader: R,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
    ) -> Result<MutationResult, VaultError>
    where
        R: AsyncRead + Unpin,
    {
        let _operation = self.validate_context(context, true).await?;
        let _guards = self.acquire_locks(context, &[path]).await;
        let storage = self.storage(context);
        let history = self.history_store(context)?;
        let files = self.state.files();
        let target = if matches!(
            operation,
            FileOperation::Create | FileOperation::Copy | FileOperation::Restore
        ) {
            files
                .get_any_by_path(context, path)
                .await
                .map_err(VaultError::State)?
        } else {
            files
                .get_active(context, path)
                .await
                .map_err(VaultError::State)?
        };
        let signature_path_before =
            if matches!(operation, FileOperation::Create | FileOperation::Copy) {
                None
            } else {
                target.as_ref().map(|file| file.path.clone())
            };
        let signature_payload = CorePayload::new(
            mcp_vault_domain::OperationId::new(),
            target.as_ref().map_or_else(FileId::new, |file| file.id),
            EntryType::File,
            operation,
            path.clone(),
            signature_path_before,
            Some(path.clone()),
            expected_revision,
            require_absent,
            None,
            None,
            0,
            0,
            None,
            None,
            actor.clone(),
            source_plane,
            idempotency_key.map(str::to_owned),
            "file.mutate",
            self.event_for(
                event_type_for(operation),
                target.as_ref().map_or_else(FileId::new, |file| file.id),
                path,
                operation,
            ),
        );
        if let Some(result) = self
            .lookup_idempotency(context, idempotency_key, &signature_payload)
            .await?
        {
            return Ok(result);
        }
        if require_absent && target.as_ref().is_some_and(FileRecord::is_active) {
            return Err(VaultError::AlreadyExists);
        }
        if !matches!(operation, FileOperation::Create | FileOperation::Copy) && target.is_none() {
            return Err(VaultError::NotFound);
        }
        if let (Some(target), Some(expected)) = (target.as_ref(), expected_revision) {
            self.require_revision(target, expected)?;
        }
        if let Some(target) = target.as_ref().filter(|file| file.is_active()) {
            self.verify_current_with_policy(&storage, target, managed)
                .await?;
        } else if !matches!(
            operation,
            FileOperation::Create | FileOperation::Copy | FileOperation::Restore
        ) {
            return Err(VaultError::NotFound);
        } else if !storage_absent_with_policy(&storage, path, managed).await? {
            return Err(VaultError::ExternalMismatch);
        }
        if let Some(target) = target.as_ref().filter(|file| file.is_active())
            && target.entry_type != EntryType::File
        {
            return Err(VaultError::InvalidPatch("target is not a regular file"));
        }

        let file_id = target.as_ref().map_or_else(FileId::new, |file| file.id);
        let operation_id = mcp_vault_domain::OperationId::new();
        if let Some(parent) = path.parent() {
            if managed {
                storage
                    .create_dir_all_managed(&parent)
                    .await
                    .map_err(map_storage)?;
            } else {
                storage.create_dir_all(&parent).await.map_err(map_storage)?;
            }
        }
        let temporary = if managed {
            storage.temporary_path_for_managed(path)
        } else {
            storage.temporary_path_for(path)
        }
        .map_err(VaultError::Storage)?;
        let mut payload = CorePayload::new(
            operation_id,
            file_id,
            EntryType::File,
            operation,
            path.clone(),
            target.as_ref().map(|file| file.path.clone()),
            Some(path.clone()),
            expected_revision,
            require_absent,
            None,
            None,
            0,
            0,
            None,
            None,
            actor,
            source_plane,
            idempotency_key.map(str::to_owned),
            match operation {
                FileOperation::Create => "file.create",
                FileOperation::Copy => "file.copy",
                FileOperation::Patch => "file.patch",
                FileOperation::Append => "file.append",
                FileOperation::Restore => "file.restore",
                FileOperation::Replace => "file.replace",
                _ => "file.update",
            },
            self.event_for(event_type_for(operation), file_id, path, operation),
        );
        let mut prepare = payload.prepare_input(
            operation_id,
            Some(path.clone()),
            Some(temporary.as_path().clone()),
            target.as_ref().and_then(|file| file.content_hash.clone()),
            None,
        )?;
        prepare.destination_path = Some(path.clone());
        files
            .prepare_operation(context, prepare)
            .await
            .map_err(VaultError::State)?;
        self.inject(CommitPhase::JournalPrepared)?;
        let mut atomic = match if managed {
            storage
                .begin_atomic_write_at_managed(
                    path,
                    if require_absent {
                        DestinationPolicy::MustNotExist
                    } else {
                        DestinationPolicy::ReplaceExisting
                    },
                    &temporary,
                )
                .await
        } else {
            storage
                .begin_atomic_write_at(
                    path,
                    if require_absent {
                        DestinationPolicy::MustNotExist
                    } else {
                        DestinationPolicy::ReplaceExisting
                    },
                    &temporary,
                )
                .await
        } {
            Ok(atomic) => atomic,
            Err(error) => {
                let _ = files
                    .mark_rolled_back(
                        context,
                        operation_id,
                        Some("temporary write could not start"),
                    )
                    .await;
                return Err(map_storage(error));
            }
        };
        let progress = match atomic.write_from(&mut reader).await {
            Ok(progress) => progress,
            Err(error) => {
                let _ = atomic.abort().await;
                let _ = files
                    .mark_rolled_back(context, operation_id, Some("source stream failed"))
                    .await;
                return Err(map_storage(error));
            }
        };
        payload.content_hash = Some(progress.content_hash.to_string());
        payload.history_blob_hash = payload.content_hash.clone();
        payload.size = progress.size;
        self.update_payload(context, operation_id, &payload).await?;
        self.inject(CommitPhase::TempFileWritten)?;
        if let Err(error) = atomic.sync().await {
            let _ = files
                .mark_rolled_back(context, operation_id, Some("temporary sync failed"))
                .await;
            return Err(map_storage(error));
        }
        self.inject(CommitPhase::FileFsynced)?;
        let receipt = match atomic.commit().await {
            Ok(receipt) => receipt,
            Err(error) => {
                let _ = files
                    .mark_rolled_back(context, operation_id, Some("canonical rename failed"))
                    .await;
                return Err(map_storage(error));
            }
        };
        payload.modified_at = receipt.metadata.modified_at;
        payload.filesystem_identity = identity_string(&receipt.metadata);
        self.update_payload(context, operation_id, &payload).await?;
        self.inject(CommitPhase::RenameCommitted)?;
        files
            .mark_file_committed(context, operation_id, payload.content_hash.as_deref())
            .await
            .map_err(VaultError::State)?;
        self.ensure_history_blob_with_policy(
            &storage,
            &history,
            path,
            payload.content_hash.as_deref(),
            managed,
        )
        .await?;
        let result = self.commit_payload(context, payload).await?;
        self.inject(CommitPhase::MetadataCommitted)?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn heading_edit<F>(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        expected_revision: Revision,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<&str>,
        edit: F,
        operation: FileOperation,
    ) -> Result<MutationResult, VaultError>
    where
        F: FnOnce(&str) -> Result<String, VaultError>,
    {
        let storage = self.storage(context);
        let mut reader = storage.open_read(path).await.map_err(map_storage)?;
        let mut original = String::new();
        reader
            .read_to_string(&mut original)
            .await
            .map_err(|_| VaultError::BinaryTextOperation)?;
        let updated = edit(&original)?;
        self.content_mutation(
            context,
            path,
            operation,
            Some(expected_revision),
            false,
            ByteReader::new(updated.into_bytes()),
            actor,
            source_plane,
            idempotency_key,
        )
        .await
    }

    async fn commit_payload(
        &self,
        context: &VaultContext,
        payload: CorePayload,
    ) -> Result<MutationResult, VaultError> {
        let input = payload.to_commit_input()?;
        let hook = StateCommitHook {
            failure: self.failure.clone(),
        };
        let result = self
            .state
            .files()
            .commit_mutation(context, input, &hook)
            .await
            .map_err(map_state)?;
        Ok(MutationResult {
            etag: etag(&result.file),
            file: result.file,
            revision: result.revision,
        })
    }

    async fn update_payload(
        &self,
        context: &VaultContext,
        operation_id: mcp_vault_domain::OperationId,
        payload: &CorePayload,
    ) -> Result<(), VaultError> {
        self.state
            .files()
            .update_operation_payload(
                context,
                operation_id,
                &serde_json::to_value(payload)
                    .map_err(|_| VaultError::InvalidPatch("operation payload is invalid"))?,
                payload.content_hash.as_deref(),
            )
            .await
            .map_err(VaultError::State)
    }

    async fn lookup_idempotency(
        &self,
        context: &VaultContext,
        key: Option<&str>,
        payload: &CorePayload,
    ) -> Result<Option<MutationResult>, VaultError> {
        let Some(key) = key else {
            return Ok(None);
        };
        let lookup = self
            .state
            .files()
            .find_idempotency(context, key)
            .await
            .map_err(VaultError::State)?;
        let Some(lookup) = lookup else {
            return Ok(None);
        };
        let stored = match &lookup {
            IdempotencyLookup::Committed { payload, .. }
            | IdempotencyLookup::InFlight { payload, .. } => payload,
        };
        if !same_request_signature(
            stored,
            &serde_json::to_value(payload)
                .map_err(|_| VaultError::InvalidPatch("operation payload is invalid"))?,
        ) {
            return Err(VaultError::IdempotencyConflict);
        }
        match lookup {
            IdempotencyLookup::Committed { file, revision, .. } => Ok(Some(MutationResult {
                etag: etag(&file),
                file,
                revision: *revision,
            })),
            IdempotencyLookup::InFlight { .. } => Err(VaultError::InFlight),
        }
    }

    async fn capture_history(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        history: &HistoryStore,
        file: &FileRecord,
    ) -> Result<Option<String>, VaultError> {
        self.capture_history_with_policy(storage, history, file, false)
            .await
    }

    async fn capture_history_with_policy(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        history: &HistoryStore,
        file: &FileRecord,
        managed: bool,
    ) -> Result<Option<String>, VaultError> {
        if file.entry_type != EntryType::File || !file.is_active() {
            return Ok(None);
        }
        let mut reader = if managed {
            storage.open_read_managed(&file.path).await
        } else {
            storage.open_read(&file.path).await
        }
        .map_err(map_storage)?;
        let blob = history.put(&mut reader).await.map_err(map_storage)?;
        Ok(Some(blob.content_hash.to_string()))
    }

    async fn ensure_history_blob(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        history: &HistoryStore,
        path: &VaultPath,
        content_hash: Option<&str>,
    ) -> Result<(), VaultError> {
        self.ensure_history_blob_with_policy(storage, history, path, content_hash, false)
            .await
    }

    async fn ensure_history_blob_with_policy(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        history: &HistoryStore,
        path: &VaultPath,
        content_hash: Option<&str>,
        managed: bool,
    ) -> Result<(), VaultError> {
        let Some(content_hash) = content_hash else {
            return Ok(());
        };
        let hash = ContentHash::from_hex(content_hash).map_err(VaultError::Storage)?;
        if history.contains(hash).await.map_err(map_storage)? {
            return Ok(());
        }
        let mut reader = if managed {
            storage.open_read_managed(path).await
        } else {
            storage.open_read(path).await
        }
        .map_err(map_storage)?;
        let blob = history.put(&mut reader).await.map_err(map_storage)?;
        if blob.content_hash != hash {
            return Err(VaultError::ExternalMismatch);
        }
        Ok(())
    }

    async fn require_active_file(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<FileRecord, VaultError> {
        self.state
            .files()
            .get_active(context, path)
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::NotFound)
    }

    async fn ensure_destination_absent(
        &self,
        context: &VaultContext,
        storage: &mcp_vault_storage_fs::VaultStorage,
        path: &VaultPath,
    ) -> Result<(), VaultError> {
        if self
            .state
            .files()
            .get_active(context, path)
            .await
            .map_err(VaultError::State)?
            .is_some()
        {
            return Err(VaultError::AlreadyExists);
        }
        if !storage_absent(storage, path).await? {
            return Err(VaultError::ExternalMismatch);
        }
        Ok(())
    }

    fn tombstone_archive_path(&self, file_id: FileId) -> Result<VaultPath, VaultError> {
        let suffix =
            VaultPath::parse(&format!("tombstones/{file_id}")).map_err(VaultError::Domain)?;
        self.path_policy
            .reserved_root()
            .join(&suffix)
            .map_err(VaultError::Domain)
    }

    async fn verify_current(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        file: &FileRecord,
    ) -> Result<FileMetadata, VaultError> {
        self.verify_current_with_policy(storage, file, false).await
    }

    async fn verify_current_with_policy(
        &self,
        storage: &mcp_vault_storage_fs::VaultStorage,
        file: &FileRecord,
        managed: bool,
    ) -> Result<FileMetadata, VaultError> {
        let metadata = if managed {
            storage.stat_managed(&file.path).await
        } else {
            storage.stat(&file.path).await
        }
        .map_err(map_storage)?;
        if metadata.kind != entry_kind(file.entry_type)
            || (file.entry_type == EntryType::File && metadata.size != file.size)
        {
            return Err(VaultError::ExternalMismatch);
        }
        if let Some(expected) = file.content_hash.as_deref() {
            let (_, actual) = if managed {
                storage.hash_file_managed(&file.path).await
            } else {
                storage.hash_file(&file.path).await
            }
            .map_err(map_storage)?;
            if actual.to_string() != expected {
                return Err(VaultError::ExternalMismatch);
            }
        }
        if let (Some(expected), Some(actual)) =
            (&file.filesystem_identity, identity_string(&metadata))
            && expected != &actual
        {
            return Err(VaultError::ExternalMismatch);
        }
        Ok(metadata)
    }

    fn require_revision(&self, file: &FileRecord, expected: Revision) -> Result<(), VaultError> {
        if file.current_revision != expected {
            return Err(VaultError::RevisionConflict {
                expected,
                current: file.current_revision,
                current_hash: file.content_hash.clone(),
            });
        }
        Ok(())
    }

    async fn validate_context(
        &self,
        context: &VaultContext,
        write: bool,
    ) -> Result<MaintenanceOperationGuard, VaultError> {
        let operation = if write {
            self.runtime.maintenance.try_start_write()
        } else {
            self.runtime.maintenance.try_start_operation()
        }
        .ok_or(VaultError::Maintenance)?;
        self.validate_registered_context(context, write).await?;
        Ok(operation)
    }

    async fn validate_registered_context(
        &self,
        context: &VaultContext,
        write: bool,
    ) -> Result<(), VaultError> {
        let record = self
            .state
            .vaults()
            .find_by_id(context.id())
            .await
            .map_err(VaultError::State)?
            .ok_or(VaultError::VaultNotRegistered)?;
        if record.slug != *context.slug() || record.content_root != context.content_root() {
            return Err(VaultError::ContextMismatch);
        }
        if write && !matches!(record.status, mcp_vault_state::VaultStatus::Active) {
            return Err(VaultError::Maintenance);
        }
        Ok(())
    }

    fn storage(&self, context: &VaultContext) -> mcp_vault_storage_fs::VaultStorage {
        VaultStorage::new(context, self.path_policy.clone(), self.storage_options)
    }

    fn history_store(&self, context: &VaultContext) -> Result<HistoryStore, VaultError> {
        HistoryStore::new(context, self.history_root.clone(), self.storage_options)
            .map_err(VaultError::Storage)
    }

    async fn acquire_locks(
        &self,
        context: &VaultContext,
        paths: &[&VaultPath],
    ) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        self.runtime
            .locks
            .acquire(context, paths, self.path_policy.case_sensitivity())
            .await
    }

    fn inject(&self, phase: CommitPhase) -> Result<(), VaultError> {
        self.failure
            .fail(phase)
            .map_err(VaultError::InjectedFailure)
    }

    fn event_for(
        &self,
        event_type: &str,
        file_id: FileId,
        path: &VaultPath,
        operation: FileOperation,
    ) -> Vec<CoreOutboxEvent> {
        vec![CoreOutboxEvent {
            event_type: event_type.to_owned(),
            aggregate_type: "file".to_owned(),
            aggregate_id: file_id.to_string(),
            payload: json!({
                "file_id": file_id,
                "path": path,
                "operation": operation.as_str()
            }),
        }]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryOutcome {
    RolledBack,
    Finalized,
    NeedsReview,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CoreOutboxEvent {
    event_type: String,
    aggregate_type: String,
    aggregate_id: String,
    payload: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CorePayload {
    operation_id: mcp_vault_domain::OperationId,
    file_id: FileId,
    entry_type: String,
    operation: String,
    path: VaultPath,
    path_before: Option<VaultPath>,
    path_after: Option<VaultPath>,
    expected_revision: Option<Revision>,
    require_absent: bool,
    #[serde(default)]
    tombstone_archive_path: Option<VaultPath>,
    content_hash: Option<String>,
    history_blob_hash: Option<String>,
    size: u64,
    modified_at: Option<i64>,
    filesystem_identity: Option<String>,
    deleted_at: Option<i64>,
    actor: Actor,
    source_plane: SourcePlane,
    idempotency_key: Option<String>,
    audit_action: String,
    audit_metadata: Value,
    request_id: Option<String>,
    outbox_events: Vec<CoreOutboxEvent>,
}

impl CorePayload {
    #[allow(clippy::too_many_arguments)]
    fn new(
        operation_id: mcp_vault_domain::OperationId,
        file_id: FileId,
        entry_type: EntryType,
        operation: FileOperation,
        path: VaultPath,
        path_before: Option<VaultPath>,
        path_after: Option<VaultPath>,
        expected_revision: Option<Revision>,
        require_absent: bool,
        content_hash: Option<String>,
        history_blob_hash: Option<String>,
        size: u64,
        modified_at: i64,
        filesystem_identity: Option<String>,
        deleted_at: Option<i64>,
        actor: Actor,
        source_plane: SourcePlane,
        idempotency_key: Option<String>,
        audit_action: &str,
        outbox_events: Vec<CoreOutboxEvent>,
    ) -> Self {
        Self {
            operation_id,
            file_id,
            entry_type: entry_type.as_str().to_owned(),
            operation: operation.as_str().to_owned(),
            path,
            path_before,
            path_after,
            expected_revision,
            require_absent,
            tombstone_archive_path: None,
            content_hash,
            history_blob_hash,
            size,
            modified_at: Some(modified_at),
            filesystem_identity,
            deleted_at,
            actor,
            source_plane,
            idempotency_key,
            audit_action: audit_action.to_owned(),
            audit_metadata: json!({ "operation": operation.as_str() }),
            request_id: None,
            outbox_events,
        }
    }

    fn prepare_input(
        &self,
        operation_id: mcp_vault_domain::OperationId,
        source_path: Option<VaultPath>,
        temp_path: Option<VaultPath>,
        prior_hash: Option<String>,
        proposed_hash: Option<String>,
    ) -> Result<PrepareOperationInput, VaultError> {
        Ok(PrepareOperationInput {
            id: operation_id,
            operation: parse_operation(&self.operation)?,
            source_path,
            destination_path: Some(self.path.clone()),
            prior_file_id: Some(self.file_id),
            expected_revision: self.expected_revision,
            prior_hash,
            proposed_hash,
            temp_path,
            payload: serde_json::to_value(self)
                .map_err(|_| VaultError::InvalidPatch("operation payload is invalid"))?,
            idempotency_key: self.idempotency_key.clone(),
        })
    }

    fn to_commit_input(&self) -> Result<CommitMutationInput, VaultError> {
        Ok(CommitMutationInput {
            operation_id: self.operation_id,
            file_id: self.file_id,
            entry_type: parse_entry_type(&self.entry_type)?,
            path: self.path.clone(),
            path_before: self.path_before.clone(),
            path_after: self.path_after.clone(),
            expected_revision: self.expected_revision,
            require_absent: self.require_absent,
            tombstone_archive_path: self.tombstone_archive_path.clone(),
            content_hash: self.content_hash.clone(),
            history_blob_hash: self.history_blob_hash.clone(),
            size: self.size,
            modified_at: self.modified_at.unwrap_or_else(now_millis),
            filesystem_identity: self.filesystem_identity.clone(),
            deleted_at: self.deleted_at,
            operation: parse_operation(&self.operation)?,
            actor: self.actor.clone(),
            source_plane: self.source_plane,
            idempotency_key: self.idempotency_key.clone(),
            audit_action: self.audit_action.clone(),
            audit_metadata: self.audit_metadata.clone(),
            request_id: self.request_id.clone(),
            outbox_events: self
                .outbox_events
                .iter()
                .map(|event| OutboxEventInput {
                    event_type: event.event_type.clone(),
                    aggregate_type: event.aggregate_type.clone(),
                    aggregate_id: event.aggregate_id.clone(),
                    payload: event.payload.clone(),
                })
                .collect(),
        })
    }
}

struct StateCommitHook {
    failure: Arc<dyn FailureInjector>,
}

impl CommitHook for StateCommitHook {
    fn on_phase(&self, phase: CommitHookPhase) -> Result<(), StateError> {
        let phase = match phase {
            CommitHookPhase::MetadataTransactionStarted => CommitPhase::MetadataTransactionStarted,
            CommitHookPhase::OutboxInserted => CommitPhase::OutboxInserted,
        };
        self.failure.fail(phase).map_err(StateError::CommitHook)
    }
}

fn parse_entry_type(value: &str) -> Result<EntryType, VaultError> {
    match value {
        "file" => Ok(EntryType::File),
        "directory" => Ok(EntryType::Directory),
        _ => Err(VaultError::InvalidPatch("entry type is invalid")),
    }
}

fn parse_operation(value: &str) -> Result<FileOperation, VaultError> {
    match value {
        "create" => Ok(FileOperation::Create),
        "replace" => Ok(FileOperation::Replace),
        "patch" => Ok(FileOperation::Patch),
        "append" => Ok(FileOperation::Append),
        "move" => Ok(FileOperation::Move),
        "copy" => Ok(FileOperation::Copy),
        "delete" => Ok(FileOperation::Delete),
        "restore" => Ok(FileOperation::Restore),
        "external_change" => Ok(FileOperation::ExternalChange),
        _ => Err(VaultError::InvalidPatch("operation is invalid")),
    }
}

fn entry_kind(entry_type: EntryType) -> mcp_vault_domain::FilesystemEntryKind {
    match entry_type {
        EntryType::File => mcp_vault_domain::FilesystemEntryKind::RegularFile,
        EntryType::Directory => mcp_vault_domain::FilesystemEntryKind::Directory,
    }
}

fn entry_type_of(file: &FileRecord) -> EntryType {
    file.entry_type
}

fn event_type_for(operation: FileOperation) -> &'static str {
    match operation {
        FileOperation::Create => "FileCreated",
        FileOperation::Move => "FileMoved",
        FileOperation::Delete => "FileDeleted",
        FileOperation::Restore => "FileRestored",
        _ => "FileUpdated",
    }
}

fn identity_string(metadata: &FileMetadata) -> Option<String> {
    metadata
        .identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.device, identity.inode))
}

fn etag(file: &FileRecord) -> String {
    format!("\"{}\"", file_etag_tag(file))
}

fn file_etag_tag(file: &FileRecord) -> String {
    format!(
        "{}-{}",
        file.current_revision,
        file.content_hash.as_deref().unwrap_or("directory")
    )
}

fn filesystem_etag(metadata: &FileMetadata) -> String {
    let kind = metadata.kind.as_str();
    let modified = metadata.modified_at.unwrap_or_default();
    let identity = metadata
        .identity
        .as_ref()
        .map(|value| format!("{}-{}", value.device, value.inode))
        .unwrap_or_else(|| "unknown".to_owned());
    format!("fs-{kind}-{modified}-{identity}")
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn same_request_signature(left: &Value, right: &Value) -> bool {
    let fields = [
        "operation",
        "path",
        "path_before",
        "path_after",
        "expected_revision",
        "require_absent",
    ];
    fields
        .iter()
        .all(|field| left.get(*field) == right.get(*field))
}

async fn storage_absent(
    storage: &mcp_vault_storage_fs::VaultStorage,
    path: &VaultPath,
) -> Result<bool, VaultError> {
    storage_absent_with_policy(storage, path, false).await
}

async fn storage_absent_with_policy(
    storage: &mcp_vault_storage_fs::VaultStorage,
    path: &VaultPath,
    managed: bool,
) -> Result<bool, VaultError> {
    let result = if managed {
        storage.stat_managed(path).await
    } else {
        storage.stat(path).await
    };
    match result {
        Ok(_) => Ok(false),
        Err(error) if storage_not_found(&error) => Ok(true),
        Err(error) => Err(map_storage(error)),
    }
}

fn storage_not_found(error: &StorageError) -> bool {
    matches!(
        error,
        StorageError::SourceNotFound
            | StorageError::HistoryNotFound
            | StorageError::Io {
                kind: std::io::ErrorKind::NotFound,
                ..
            }
    )
}

fn map_storage(error: StorageError) -> VaultError {
    if storage_not_found(&error) {
        VaultError::NotFound
    } else {
        match error {
            StorageError::Domain(error) => VaultError::Domain(error),
            other => VaultError::Storage(other),
        }
    }
}

fn map_state(error: StateError) -> VaultError {
    match error {
        StateError::InvalidDomain(DomainError::RevisionConflict { expected, current }) => {
            VaultError::RevisionConflict {
                expected,
                current,
                current_hash: None,
            }
        }
        StateError::InvalidDomain(DomainError::PreconditionFailed { reason })
            if reason.contains("already exists") || reason.contains("destination") =>
        {
            VaultError::AlreadyExists
        }
        StateError::InvalidDomain(DomainError::PreconditionFailed { reason })
            if reason.contains("does not exist") =>
        {
            VaultError::NotFound
        }
        other => VaultError::State(other),
    }
}

struct ByteReader {
    bytes: Vec<u8>,
    offset: usize,
}

impl ByteReader {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes, offset: 0 }
    }
}

impl AsyncRead for ByteReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.offset == self.bytes.len() {
            return Poll::Ready(Ok(()));
        }
        let count = (self.bytes.len() - self.offset).min(buffer.remaining());
        buffer.put_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        Poll::Ready(Ok(()))
    }
}
