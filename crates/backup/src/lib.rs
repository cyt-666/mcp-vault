//! Portable, manifest-backed backup and staged-restore application service.
//!
//! This crate is the recovery boundary between the Admin adapter and the
//! canonical Vault/state stores. It owns archive format validation and
//! orchestration; SQL remains in `mcp-vault-state` and ordinary protocol
//! handlers never receive filesystem paths or archive handles.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mcp_vault_core::VaultCoreRuntime;
use mcp_vault_domain::{
    BackupId, FilesystemEntryKind, MaintenanceGate, MaintenanceMode, VaultId, VaultPath,
};
use mcp_vault_state::{BackupRecord, BackupStatus, JobRecord, StateError, StateStore};
use mcp_vault_storage_fs::{
    ContentHash, DirectorySwap, FileMetadata, StorageOptions, VaultStorage,
    cleanup_directory_swaps, install_staged_directory, rollback_directory_swaps,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, EntryType};
use thiserror::Error;
use tokio::{
    fs as async_fs,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
    time::{sleep, timeout},
};

const FORMAT_VERSION: u32 = 1;
const STATE_ARCHIVE_PATH: &str = "state/mcp-vault.sqlite3";
const MANIFEST_ARCHIVE_PATH: &str = "manifest.json";
const DEFAULT_MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DEFAULT_MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DEFAULT_MAX_ENTRIES: u64 = 250_000;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAINTENANCE_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);
const MAINTENANCE_DRAIN_POLL: Duration = Duration::from_millis(10);

/// Limits applied before reading, staging, or extracting backup entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackupLimits {
    /// Maximum size of one regular archive entry.
    pub max_entry_bytes: u64,
    /// Maximum sum of regular entry sizes.
    pub max_total_bytes: u64,
    /// Maximum bytes in the final artifact.
    pub max_archive_bytes: u64,
    /// Maximum number of regular entries.
    pub max_entries: u64,
    /// Number of verified backups retained at minimum.
    pub keep_count: u32,
}

impl Default for BackupLimits {
    fn default() -> Self {
        Self {
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_archive_bytes: DEFAULT_MAX_ARCHIVE_BYTES,
            max_entries: DEFAULT_MAX_ENTRIES,
            keep_count: 3,
        }
    }
}

/// Runtime dependencies for the backup boundary.
#[derive(Clone, Debug)]
pub struct BackupConfig {
    /// Service-owned directory containing final artifacts and staging roots.
    pub backup_root: PathBuf,
    /// Content-addressed revision-history root.
    pub history_root: PathBuf,
    /// Storage policy used for canonical reads.
    pub storage_options: StorageOptions,
    /// Archive and retention limits.
    pub limits: BackupLimits,
    /// Version placed in manifests.
    pub service_version: String,
    /// Retained installation-key version identifiers, never key material.
    pub key_version_ids: Vec<u32>,
    /// Shared process maintenance gate.
    pub maintenance: MaintenanceGate,
    /// Shared Core path-lock and maintenance runtime.
    pub core_runtime: VaultCoreRuntime,
    /// Readiness bit cleared while restore is offline.
    pub readiness: Arc<AtomicBool>,
}

/// One manifest entry with a portable archive path and SHA-256 checksum.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackupArtifact {
    /// Relative path inside the backup tar.
    pub path: String,
    /// Byte length of the artifact.
    pub size: u64,
    /// Lowercase SHA-256 digest.
    pub sha256: String,
}

/// Manifest information for one registered Vault.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VaultBackupManifest {
    /// Stable Vault identity.
    pub vault_id: VaultId,
    /// Endpoint slug at backup time.
    pub slug: String,
    /// Registered canonical root. Restore requires the configured target root
    /// to match rather than allowing an archive to redirect writes.
    pub content_root: String,
    /// Settings revision at the coordination point.
    pub settings_revision: u64,
    /// Ordinary and managed canonical files.
    pub content: Vec<BackupArtifact>,
    /// History blobs for this Vault.
    pub history: Vec<BackupArtifact>,
}

/// Portable manifest stored as `manifest.json` in every artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackupManifest {
    /// Backup format version, independent from the service version.
    pub format_version: u32,
    /// Service version that produced the artifact.
    pub service_version: String,
    /// Highest successful SQLx migration in the snapshot.
    pub schema_version: i64,
    /// UTC Unix millisecond creation/completion times.
    pub created_at: i64,
    pub completed_at: i64,
    /// SQLite operational snapshot.
    pub state: BackupArtifact,
    /// Installation-key versions required to decrypt restored secrets.
    pub key_version_ids: Vec<u32>,
    /// Registered Vault snapshots.
    pub vaults: Vec<VaultBackupManifest>,
    /// Total regular files and bytes represented by the manifest.
    pub file_count: u64,
    pub total_bytes: u64,
}

/// Safe non-secret result returned by restore validation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestorePreview {
    /// The validated manifest.
    pub manifest: BackupManifest,
    /// Whether all Vault identities/roots match the current registry.
    pub target_matches: bool,
    /// Number of archive entries, including state/history/content files.
    pub archive_entries: u64,
    /// Archive byte length.
    pub archive_bytes: u64,
}

/// A catalog row plus a durable operation admitted to the worker queue.
#[derive(Clone, Debug)]
pub struct BackupOperation {
    /// Catalog identity/status.
    pub backup: BackupRecord,
    /// Durable job identity.
    pub job: JobRecord,
}

/// Typed errors deliberately mapped to redacted Admin/worker outcomes.
#[derive(Debug, Error)]
pub enum BackupError {
    /// A Vault identity/path value failed domain validation.
    #[error("backup domain value is invalid")]
    Domain(#[from] mcp_vault_domain::DomainError),
    /// Operational state failed.
    #[error("backup state unavailable")]
    State(#[from] StateError),
    /// Safe storage boundary failed.
    #[error("backup storage unavailable")]
    Storage(#[from] mcp_vault_storage_fs::StorageError),
    /// Vault Core recovery failed.
    #[error("backup Vault recovery failed")]
    Core(#[from] mcp_vault_core::VaultError),
    /// An OS operation failed; the public mapping does not expose its path.
    #[error("backup filesystem operation failed: {0}")]
    Io(&'static str),
    /// Manifest/archive JSON was malformed.
    #[error("backup manifest is invalid")]
    Json(#[from] serde_json::Error),
    /// Tar structure or entry policy failed.
    #[error("backup archive is invalid: {0}")]
    Archive(&'static str),
    /// A configured identity/root does not match the current service.
    #[error("backup target does not match the configured Vault")]
    TargetMismatch,
    /// The restored state requires an unavailable installation-key version.
    #[error("backup encryption key version is unavailable")]
    KeyVersionMismatch,
    /// Archive or operation exceeded an explicit bound.
    #[error("backup resource limit exceeded: {0}")]
    Limit(&'static str),
    /// Operation was attempted while process coordination disallows it.
    #[error("backup operation is unavailable during maintenance")]
    Maintenance,
    /// A catalog/artifact was not found.
    #[error("backup not found")]
    NotFound,
    /// A source changed while it was being copied.
    #[error("backup source changed during snapshot")]
    InconsistentSource,
}

impl BackupError {
    /// Whether a worker may safely retry this error.
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::State(_) | Self::Io(_) | Self::Storage(_))
    }
}

/// Backup application service. Cloning it shares the operation lock and gate.
#[derive(Clone)]
pub struct BackupService {
    state: StateStore,
    config: BackupConfig,
    operation_lock: Arc<Mutex<()>>,
    operation_active: Arc<AtomicBool>,
}

struct ActiveBackupOperation(Arc<AtomicBool>);

impl ActiveBackupOperation {
    fn new(active: Arc<AtomicBool>) -> Self {
        active.store(true, Ordering::Release);
        Self(active)
    }
}

impl Drop for ActiveBackupOperation {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl BackupService {
    /// Compose the service from the authoritative state boundary.
    pub fn new(state: StateStore, config: BackupConfig) -> Self {
        Self {
            state,
            config,
            operation_lock: Arc::new(Mutex::new(())),
            operation_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Return the shared maintenance gate for composition/tests.
    pub fn maintenance(&self) -> MaintenanceGate {
        self.config.maintenance.clone()
    }

    /// Return whether a serialized backup/restore operation currently owns
    /// the maintenance transition boundary.
    pub fn operation_active(&self) -> bool {
        self.operation_active.load(Ordering::Acquire)
    }

    /// Reopen the process after an interrupted restore only when the current
    /// SQLite/Vault state passes the same integrity and journal-recovery checks
    /// used after a successful restore. This is an authenticated control-plane
    /// recovery action; it never guesses or silently discards a failed swap.
    pub async fn recover_maintenance(&self) -> Result<(), BackupError> {
        let _lock = self.operation_lock.lock().await;
        let _active = ActiveBackupOperation::new(self.operation_active.clone());
        if self.config.maintenance.mode() == MaintenanceMode::Normal {
            self.config.readiness.store(true, Ordering::Release);
            return Ok(());
        }
        self.verify_live_state().await?;
        self.config.maintenance.set(MaintenanceMode::Normal);
        self.config.readiness.store(true, Ordering::Release);
        Ok(())
    }

    /// Admit a new global durable backup job.
    pub async fn enqueue_create(
        &self,
        created_by: Option<&str>,
    ) -> Result<BackupOperation, BackupError> {
        if self.config.maintenance.mode() == MaintenanceMode::Offline {
            return Err(BackupError::Maintenance);
        }
        let id = BackupId::new();
        let location = self.artifact_path(id)?;
        let backup = self
            .state
            .backups()
            .insert_queued(id, &location.to_string_lossy(), created_by)
            .await?;
        let payload = json!({"backup_id": id.to_string()});
        let job = match self
            .state
            .jobs()
            .enqueue_global(
                "backup.create",
                &format!("backup:create:{id}"),
                &payload,
                20,
                3,
                now_millis(),
            )
            .await
        {
            Ok(job) => job,
            Err(error) => {
                let _ = self
                    .state
                    .backups()
                    .mark_failed(id, "job_admission_failed")
                    .await;
                return Err(error.into());
            }
        };
        Ok(BackupOperation { backup, job })
    }

    /// Admit a verification job for an existing artifact.
    pub async fn enqueue_verify(&self, id: BackupId) -> Result<JobRecord, BackupError> {
        self.require_record(id).await?;
        Ok(self
            .state
            .jobs()
            .enqueue_global(
                "backup.verify",
                &format!("backup:verify:{id}"),
                &json!({"backup_id": id.to_string()}),
                15,
                3,
                now_millis(),
            )
            .await?)
    }

    /// Admit an explicit restore job. Validation remains a separate endpoint.
    pub async fn enqueue_restore(
        &self,
        id: BackupId,
        request_key: &str,
    ) -> Result<JobRecord, BackupError> {
        if request_key.is_empty()
            || request_key.len() > 128
            || request_key.chars().any(char::is_control)
        {
            return Err(BackupError::Archive("restore request key is invalid"));
        }
        let record = self.require_record(id).await?;
        if record.status != BackupStatus::Completed || record.verified_at.is_none() {
            return Err(BackupError::Archive("backup is not verified"));
        }
        Ok(self
            .state
            .jobs()
            .enqueue_global(
                "backup.restore",
                &format!("backup:restore:{id}:{request_key}"),
                &json!({"backup_id": id.to_string()}),
                30,
                1,
                now_millis(),
            )
            .await?)
    }

    /// List catalog records through the typed state boundary.
    pub async fn list(&self, limit: u32, offset: u32) -> Result<Vec<BackupRecord>, BackupError> {
        Ok(self.state.backups().list(limit, offset).await?)
    }

    /// Get one catalog record.
    pub async fn get(&self, id: BackupId) -> Result<BackupRecord, BackupError> {
        self.require_record(id).await
    }

    /// Execute one queued backup operation in a worker.
    pub async fn create(&self, id: BackupId) -> Result<BackupManifest, BackupError> {
        let _lock = self.operation_lock.lock().await;
        let _active = ActiveBackupOperation::new(self.operation_active.clone());
        let record = self.require_record(id).await?;
        if !matches!(record.status, BackupStatus::Queued | BackupStatus::Running) {
            return Err(BackupError::Archive("backup operation is not resumable"));
        }
        if record.status == BackupStatus::Queued {
            self.state.backups().mark_running(id).await?;
        }
        let previous = self.config.maintenance.mode();
        if previous == MaintenanceMode::Offline {
            let _ = self
                .state
                .backups()
                .mark_failed(id, "maintenance_offline")
                .await;
            return Err(BackupError::Maintenance);
        }
        self.config.maintenance.set(MaintenanceMode::ReadOnly);
        if let Err(error) = self.wait_for_active_writes().await {
            self.config.maintenance.set(previous);
            let _ = self
                .state
                .backups()
                .mark_failed(id, error_code(&error))
                .await;
            return Err(error);
        }
        let result = self.create_artifact(id).await;
        self.config.maintenance.set(previous);
        match result {
            Ok(manifest) => {
                self.state
                    .backups()
                    .mark_completed(id, &serde_json::to_value(&manifest)?, manifest.completed_at)
                    .await?;
                if let Err(error) = self.apply_retention().await {
                    tracing::warn!(%error, "backup retention cleanup was deferred");
                }
                Ok(manifest)
            }
            Err(error) => {
                let _ = self
                    .state
                    .backups()
                    .mark_failed(id, error_code(&error))
                    .await;
                let _ = self.remove_staging(id).await;
                Err(error)
            }
        }
    }

    /// Verify an existing artifact and update its catalog verification time.
    pub async fn verify(&self, id: BackupId) -> Result<BackupManifest, BackupError> {
        let _lock = self.operation_lock.lock().await;
        let _active = ActiveBackupOperation::new(self.operation_active.clone());
        let record = self.require_record(id).await?;
        let path = self.validate_record_location(id, &record)?;
        let preview = self.validate_archive_path(path).await?;
        self.state
            .backups()
            .mark_verified(id, &serde_json::to_value(&preview.manifest)?)
            .await?;
        Ok(preview.manifest)
    }

    /// Validate an archive without extracting into configured roots.
    pub async fn validate_restore(&self, id: BackupId) -> Result<RestorePreview, BackupError> {
        let record = self.require_record(id).await?;
        let path = self.validate_record_location(id, &record)?;
        let mut preview = self.validate_archive_path(path).await?;
        self.validate_targets(&preview.manifest).await?;
        preview.target_matches = true;
        Ok(preview)
    }

    /// Apply a validated archive through the staged restore boundary.
    pub async fn restore(&self, id: BackupId) -> Result<RestorePreview, BackupError> {
        let _lock = self.operation_lock.lock().await;
        let _active = ActiveBackupOperation::new(self.operation_active.clone());
        let record = self.require_record(id).await?;
        if !matches!(
            record.status,
            BackupStatus::Completed | BackupStatus::Running | BackupStatus::Restoring
        ) || (record.status == BackupStatus::Completed && record.verified_at.is_none())
        {
            return Err(BackupError::Archive("backup is not verified"));
        }
        let archive_path = self.validate_record_location(id, &record)?;
        let mut preview = self.validate_archive_path(archive_path).await?;
        self.validate_targets(&preview.manifest).await?;
        preview.target_matches = true;

        let previous = self.config.maintenance.mode();
        if previous == MaintenanceMode::Offline {
            return Err(BackupError::Maintenance);
        }
        let pre_restore_id = BackupId::new();
        let pre_location = self.artifact_path(pre_restore_id)?;
        self.state.backups().mark_restoring(id).await?;

        // Always capture a safety backup before touching configured roots.
        if let Err(error) = self
            .state
            .backups()
            .insert_queued(pre_restore_id, &pre_location.to_string_lossy(), None)
            .await
        {
            let error: BackupError = error.into();
            let _ = self
                .state
                .backups()
                .mark_failed(id, error_code(&error))
                .await;
            return Err(error);
        }
        if let Err(error) = self.state.backups().mark_running(pre_restore_id).await {
            let error: BackupError = error.into();
            let _ = self
                .state
                .backups()
                .mark_failed(pre_restore_id, error_code(&error))
                .await;
            let _ = self
                .state
                .backups()
                .mark_failed(id, error_code(&error))
                .await;
            return Err(error);
        }
        self.config.maintenance.set(MaintenanceMode::ReadOnly);
        if let Err(error) = self.wait_for_active_writes().await {
            self.config.maintenance.set(previous);
            let _ = self
                .state
                .backups()
                .mark_failed(pre_restore_id, error_code(&error))
                .await;
            let _ = self
                .state
                .backups()
                .mark_failed(id, error_code(&error))
                .await;
            return Err(error);
        }
        let pre_manifest = match self.create_artifact(pre_restore_id).await {
            Ok(manifest) => manifest,
            Err(error) => {
                self.config.maintenance.set(previous);
                let _ = self
                    .state
                    .backups()
                    .mark_failed(pre_restore_id, error_code(&error))
                    .await;
                let _ = self
                    .state
                    .backups()
                    .mark_failed(id, error_code(&error))
                    .await;
                return Err(error);
            }
        };
        if let Err(error) = self
            .state
            .backups()
            .mark_completed(
                pre_restore_id,
                &serde_json::to_value(&pre_manifest)?,
                pre_manifest.completed_at,
            )
            .await
        {
            let error: BackupError = error.into();
            self.config.maintenance.set(previous);
            let _ = self
                .state
                .backups()
                .mark_failed(pre_restore_id, error_code(&error))
                .await;
            let _ = self
                .state
                .backups()
                .mark_failed(id, error_code(&error))
                .await;
            return Err(error);
        }

        self.config.maintenance.set(MaintenanceMode::Offline);
        self.config.readiness.store(false, Ordering::Release);
        let result = match self.wait_for_active_operations().await {
            Ok(()) => self.apply_restore(&preview.manifest, archive_path).await,
            Err(error) => Err(error),
        };
        if result.is_ok() {
            if let Some(restored) = self.state.backups().get(id).await?
                && restored.status == BackupStatus::Running
            {
                self.state
                    .backups()
                    .mark_completed(
                        id,
                        &serde_json::to_value(&preview.manifest)?,
                        preview.manifest.completed_at,
                    )
                    .await?;
            }
            if self.state.backups().get(pre_restore_id).await?.is_none() {
                self.state
                    .backups()
                    .insert_queued(pre_restore_id, &pre_location.to_string_lossy(), None)
                    .await?;
                self.state.backups().mark_running(pre_restore_id).await?;
            }
            self.state
                .backups()
                .mark_completed(
                    pre_restore_id,
                    &serde_json::to_value(&pre_manifest)?,
                    pre_manifest.completed_at,
                )
                .await?;
            self.config.maintenance.set(MaintenanceMode::Normal);
            self.config.readiness.store(true, Ordering::Release);
        }
        let mut reopened_after_failure = false;
        if result.is_err() {
            let _ = self
                .state
                .backups()
                .mark_failed(
                    id,
                    result
                        .as_ref()
                        .err()
                        .map(error_code)
                        .unwrap_or("restore_failed"),
                )
                .await;
            if self.verify_live_state().await.is_ok() {
                reopened_after_failure = true;
                tracing::warn!(backup_id = %id, pre_restore_backup_id = %pre_restore_id, "restore failed but rollback integrity checks passed; service reopened");
            } else {
                tracing::error!(backup_id = %id, pre_restore_backup_id = %pre_restore_id, "restore failed; service remains offline for operator recovery");
            }
        }
        let can_reopen = result.is_ok() || reopened_after_failure;
        self.config.maintenance.set(if can_reopen {
            previous
        } else {
            MaintenanceMode::Offline
        });
        self.config.readiness.store(can_reopen, Ordering::Release);
        result.map(|_| preview)
    }

    async fn require_record(&self, id: BackupId) -> Result<BackupRecord, BackupError> {
        self.state
            .backups()
            .get(id)
            .await?
            .ok_or(BackupError::NotFound)
    }

    async fn wait_for_active_writes(&self) -> Result<(), BackupError> {
        self.wait_for_drain(|| self.config.maintenance.active_writes() == 0)
            .await
    }

    async fn wait_for_active_operations(&self) -> Result<(), BackupError> {
        self.wait_for_drain(|| self.config.maintenance.active_operations() == 0)
            .await
    }

    async fn wait_for_drain(&self, drained: impl Fn() -> bool) -> Result<(), BackupError> {
        timeout(MAINTENANCE_DRAIN_TIMEOUT, async {
            while !drained() {
                sleep(MAINTENANCE_DRAIN_POLL).await;
            }
        })
        .await
        .map_err(|_| BackupError::Maintenance)
    }

    fn validate_record_location<'a>(
        &self,
        id: BackupId,
        record: &'a BackupRecord,
    ) -> Result<&'a Path, BackupError> {
        let expected = self.artifact_path(id)?;
        let actual = Path::new(&record.location);
        if actual != expected {
            return Err(BackupError::Archive("backup catalog location is invalid"));
        }
        Ok(actual)
    }

    fn artifact_path(&self, id: BackupId) -> Result<PathBuf, BackupError> {
        validate_private_root(&self.config.backup_root)?;
        Ok(self.config.backup_root.join(format!("{id}.tar")))
    }

    async fn create_artifact(&self, id: BackupId) -> Result<BackupManifest, BackupError> {
        ensure_private_directory(&self.config.backup_root).await?;
        self.ensure_backup_space().await?;
        let final_path = self.artifact_path(id)?;
        if async_fs::try_exists(&final_path)
            .await
            .map_err(|_| BackupError::Io("inspect existing artifact"))?
        {
            return Ok(self.validate_archive_path(&final_path).await?.manifest);
        }
        let staging = self.config.backup_root.join(format!(".staging-{id}"));
        if async_fs::try_exists(&staging)
            .await
            .map_err(|_| BackupError::Io("inspect staging"))?
        {
            async_fs::remove_dir_all(&staging)
                .await
                .map_err(|_| BackupError::Io("remove stale staging"))?;
        }
        async_fs::create_dir_all(&staging)
            .await
            .map_err(|_| BackupError::Io("create staging"))?;
        let creation = now_millis();
        let mut total_bytes = 0_u64;
        let mut file_count = 0_u64;
        let mut vault_manifests = Vec::new();
        let result = async {
            let schema_version = self.state.integrity_check().await?.migration_version;
            let vaults = self.state.vaults().list().await?;
            for vault in vaults {
                let context = vault.context()?;
                let storage = VaultStorage::new(
                    &context,
                    mcp_vault_domain::VaultPathPolicy::new(
                        vault.reserved_root.clone(),
                        Default::default(),
                    )?,
                    self.config.storage_options,
                );
                storage.ensure_root().await?;
                let mut content = Vec::new();
                for managed in [false, true] {
                    let entries = collect_entries(storage.clone(), managed).await?;
                    for entry in entries {
                        if entry.kind != FilesystemEntryKind::RegularFile {
                            continue;
                        }
                        let path = entry
                            .path
                            .clone()
                            .ok_or(BackupError::Archive("Vault entry path missing"))?;
                        let (size_before, hash_before) = if managed {
                            storage.hash_file_managed(&path).await?
                        } else {
                            storage.hash_file(&path).await?
                        };
                        let archive_path = format!("vaults/{}/content/{}", context.id(), path);
                        let target = safe_join(&staging, &archive_path)?;
                        copy_storage_file(&storage, &path, managed, &target).await?;
                        let (size_after, hash_after) = if managed {
                            storage.hash_file_managed(&path).await?
                        } else {
                            storage.hash_file(&path).await?
                        };
                        if size_before != size_after || hash_before != hash_after {
                            return Err(BackupError::InconsistentSource);
                        }
                        let artifact = artifact(&archive_path, size_after, hash_after);
                        register_artifact(
                            &self.config.limits,
                            &mut file_count,
                            &mut total_bytes,
                            &artifact,
                        )?;
                        content.push(artifact);
                    }
                }
                async_fs::create_dir_all(
                    staging
                        .join("vaults")
                        .join(context.id().to_string())
                        .join("content"),
                )
                .await
                .map_err(|_| BackupError::Io("create content staging root"))?;
                let history_source = self.config.history_root.join(context.id().to_string());
                let history_target = staging.join("history").join(context.id().to_string());
                async_fs::create_dir_all(&history_target)
                    .await
                    .map_err(|_| BackupError::Io("create history staging root"))?;
                let history = if async_fs::try_exists(&history_source)
                    .await
                    .map_err(|_| BackupError::Io("inspect history"))?
                {
                    copy_private_tree(
                        &history_source,
                        &history_target,
                        &format!("history/{}/", context.id()),
                        &self.config.limits,
                        &mut file_count,
                        &mut total_bytes,
                    )
                    .await?
                } else {
                    Vec::new()
                };
                vault_manifests.push(VaultBackupManifest {
                    vault_id: context.id(),
                    slug: context.slug().to_string(),
                    content_root: context.content_root().display().to_string(),
                    settings_revision: context.settings_revision().value(),
                    content,
                    history,
                });
            }
            let state_path = staging.join(STATE_ARCHIVE_PATH);
            if let Some(parent) = state_path.parent() {
                async_fs::create_dir_all(parent)
                    .await
                    .map_err(|_| BackupError::Io("create state staging"))?;
            }
            self.state.snapshot_to(&state_path).await?;
            let (state_size, state_hash) = hash_path(&state_path).await?;
            let state_artifact = artifact(STATE_ARCHIVE_PATH, state_size, state_hash);
            register_artifact(
                &self.config.limits,
                &mut file_count,
                &mut total_bytes,
                &state_artifact,
            )?;
            let completed_at = now_millis();
            let manifest = BackupManifest {
                format_version: FORMAT_VERSION,
                service_version: self.config.service_version.clone(),
                schema_version,
                created_at: creation,
                completed_at,
                state: state_artifact,
                key_version_ids: self.config.key_version_ids.clone(),
                vaults: vault_manifests,
                file_count,
                total_bytes,
            };
            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
                return Err(BackupError::Limit("manifest_bytes"));
            }
            let manifest_path = staging.join(MANIFEST_ARCHIVE_PATH);
            let mut manifest_file = async_fs::File::create(&manifest_path)
                .await
                .map_err(|_| BackupError::Io("create manifest"))?;
            manifest_file
                .write_all(&manifest_bytes)
                .await
                .map_err(|_| BackupError::Io("write manifest"))?;
            manifest_file
                .sync_all()
                .await
                .map_err(|_| BackupError::Io("sync manifest"))?;
            let temporary_archive = self.config.backup_root.join(format!(".{id}.tar.tmp"));
            if async_fs::try_exists(&temporary_archive)
                .await
                .map_err(|_| BackupError::Io("inspect archive temp"))?
            {
                async_fs::remove_file(&temporary_archive)
                    .await
                    .map_err(|_| BackupError::Io("remove archive temp"))?;
            }
            let staging_for_tar = staging.clone();
            let temporary_for_tar = temporary_archive.clone();
            let max_archive_bytes = self.config.limits.max_archive_bytes;
            tokio::task::spawn_blocking(move || {
                write_tar(&staging_for_tar, &temporary_for_tar, max_archive_bytes)
            })
            .await
            .map_err(|_| BackupError::Io("write archive task"))??;
            // A completed catalog row is also a verified artifact. Validate the
            // temporary tar before publication so a malformed archive can never
            // be advertised as restorable or retained as the latest backup.
            let verified = match self.validate_archive_path(&temporary_archive).await {
                Ok(verified) => verified,
                Err(error) => {
                    let _ = async_fs::remove_file(&temporary_archive).await;
                    return Err(error);
                }
            };
            if verified.manifest != manifest {
                let _ = async_fs::remove_file(&temporary_archive).await;
                return Err(BackupError::Archive("created archive manifest changed"));
            }
            if async_fs::try_exists(&final_path)
                .await
                .map_err(|_| BackupError::Io("inspect artifact"))?
            {
                let _ = async_fs::remove_file(&temporary_archive).await;
                return Err(BackupError::Archive("backup artifact already exists"));
            }
            async_fs::rename(&temporary_archive, &final_path)
                .await
                .map_err(|_| BackupError::Io("publish backup artifact"))?;
            Ok::<BackupManifest, BackupError>(manifest)
        }
        .await;
        let _ = async_fs::remove_dir_all(&staging).await;
        result
    }

    async fn ensure_backup_space(&self) -> Result<(), BackupError> {
        let root = self.config.backup_root.clone();
        let available = tokio::task::spawn_blocking(move || {
            fs2::available_space(&root).map_err(|_| BackupError::Io("read backup free space"))
        })
        .await
        .map_err(|_| BackupError::Io("read backup free space task"))??;
        if available < self.config.storage_options.minimum_free_bytes {
            return Err(BackupError::Limit("backup_free_space"));
        }
        Ok(())
    }

    async fn validate_archive_path(&self, path: &Path) -> Result<RestorePreview, BackupError> {
        validate_private_path(path)?;
        let metadata = async_fs::metadata(path)
            .await
            .map_err(|_| BackupError::NotFound)?;
        if !metadata.is_file() || metadata.len() > self.config.limits.max_archive_bytes {
            return Err(BackupError::Limit("archive_bytes"));
        }
        let limits = self.config.limits;
        let path = path.to_owned();
        let preview = tokio::task::spawn_blocking(move || validate_archive_file(&path, limits))
            .await
            .map_err(|_| BackupError::Io("validate archive task"))??;
        Ok(preview)
    }

    async fn validate_targets(&self, manifest: &BackupManifest) -> Result<(), BackupError> {
        if manifest.format_version != FORMAT_VERSION {
            return Err(BackupError::Archive("unsupported backup format"));
        }
        if manifest
            .key_version_ids
            .iter()
            .any(|version| !self.config.key_version_ids.contains(version))
        {
            return Err(BackupError::KeyVersionMismatch);
        }
        let current_schema = self.state.integrity_check().await?.migration_version;
        if manifest.schema_version > current_schema {
            return Err(BackupError::Archive(
                "backup schema is newer than this service",
            ));
        }
        let current = self.state.vaults().list().await?;
        if current.len() != manifest.vaults.len() {
            return Err(BackupError::TargetMismatch);
        }
        for vault in current {
            let found = manifest
                .vaults
                .iter()
                .find(|item| item.vault_id == vault.id)
                .ok_or(BackupError::TargetMismatch)?;
            if found.slug != vault.slug.to_string()
                || found.content_root != vault.content_root.display().to_string()
            {
                return Err(BackupError::TargetMismatch);
            }
        }
        Ok(())
    }

    async fn apply_restore(
        &self,
        manifest: &BackupManifest,
        archive_path: &Path,
    ) -> Result<(), BackupError> {
        let stage = self
            .config
            .backup_root
            .join(format!(".restore-stage-{}", BackupId::new()));
        if async_fs::try_exists(&stage)
            .await
            .map_err(|_| BackupError::Io("inspect restore staging"))?
        {
            async_fs::remove_dir_all(&stage)
                .await
                .map_err(|_| BackupError::Io("remove restore staging"))?;
        }
        async_fs::create_dir_all(&stage)
            .await
            .map_err(|_| BackupError::Io("create restore staging"))?;
        self.ensure_backup_space().await?;
        let path = archive_path.to_owned();
        let stage_for_task = stage.clone();
        let limits = self.config.limits;
        let extracted =
            tokio::task::spawn_blocking(move || extract_archive(&path, &stage_for_task, limits))
                .await
                .map_err(|_| BackupError::Io("extract restore archive task"))??;
        let result = async {
            if extracted.manifest != *manifest {
                return Err(BackupError::Archive("restore manifest changed"));
            }
            let rollback_state = self
                .config
                .backup_root
                .join(format!(".rollback-state-{}", BackupId::new()));
            self.state.snapshot_to(&rollback_state).await?;
            let swapped = swap_roots(&self.state, &self.config, &stage).await?;
            let post_restore = async {
                self.state
                    .restore_from_snapshot(&stage.join(STATE_ARCHIVE_PATH))
                    .await?;
                self.state.migrate().await?;
                let integrity = self.state.integrity_check().await?;
                if !integrity.integrity_ok || integrity.foreign_key_violations != 0 {
                    return Err(BackupError::State(StateError::IntegrityFailure));
                }
                let recovery_permit = self.config.core_runtime.maintenance_recovery_permit();
                for vault in self.state.vaults().list().await? {
                    let context = vault.context()?;
                    let storage = VaultStorage::new(
                        &context,
                        mcp_vault_domain::VaultPathPolicy::new(
                            vault.reserved_root.clone(),
                            Default::default(),
                        )?,
                        self.config.storage_options,
                    );
                    storage.ensure_root().await?;
                    let core = mcp_vault_core::VaultCore::new(
                        self.state.clone(),
                        self.config.history_root.clone(),
                        mcp_vault_domain::VaultPathPolicy::new(
                            vault.reserved_root.clone(),
                            Default::default(),
                        )?,
                        self.config.storage_options,
                        self.config.core_runtime.clone(),
                    );
                    let recovery = core
                        .recover_during_maintenance(&context, &recovery_permit)
                        .await?;
                    if recovery.needs_review != 0 {
                        return Err(BackupError::Archive("restore recovery needs review"));
                    }
                }
                Ok::<(), BackupError>(())
            }
            .await;
            if let Err(error) = post_restore {
                let _ = rollback_roots(&swapped).await;
                let _ = self.state.restore_from_snapshot(&rollback_state).await;
                let _ = async_fs::remove_file(&rollback_state).await;
                return Err(error);
            }
            // Old roots remain as a local safety window until all checks above
            // pass. Failure to delete them is non-fatal and does not corrupt
            // the newly restored roots.
            let _ = cleanup_old_roots(&swapped).await;
            let _ = async_fs::remove_file(&rollback_state).await;
            Ok::<(), BackupError>(())
        }
        .await;
        if result.is_ok() {
            let _ = async_fs::remove_dir_all(&stage).await;
        }
        result
    }

    async fn verify_live_state(&self) -> Result<(), BackupError> {
        let integrity = self.state.integrity_check().await?;
        if !integrity.integrity_ok || integrity.foreign_key_violations != 0 {
            return Err(BackupError::State(StateError::IntegrityFailure));
        }
        let recovery_permit = self.config.core_runtime.maintenance_recovery_permit();
        for vault in self.state.vaults().list().await? {
            let context = vault.context()?;
            let policy = mcp_vault_domain::VaultPathPolicy::new(
                vault.reserved_root.clone(),
                Default::default(),
            )?;
            VaultStorage::new(&context, policy.clone(), self.config.storage_options)
                .ensure_root()
                .await?;
            let core = mcp_vault_core::VaultCore::new(
                self.state.clone(),
                self.config.history_root.clone(),
                policy,
                self.config.storage_options,
                self.config.core_runtime.clone(),
            );
            if core
                .recover_during_maintenance(&context, &recovery_permit)
                .await?
                .needs_review
                != 0
            {
                return Err(BackupError::Archive("restore recovery needs review"));
            }
        }
        Ok(())
    }

    async fn remove_staging(&self, id: BackupId) -> Result<(), BackupError> {
        let staging = self.config.backup_root.join(format!(".staging-{id}"));
        if async_fs::try_exists(&staging)
            .await
            .map_err(|_| BackupError::Io("inspect staging"))?
        {
            async_fs::remove_dir_all(staging)
                .await
                .map_err(|_| BackupError::Io("remove staging"))?;
        }
        Ok(())
    }

    async fn apply_retention(&self) -> Result<(), BackupError> {
        let records = self.state.backups().list(200, 0).await?;
        let mut verified = records
            .into_iter()
            .filter(|record| {
                record.status == BackupStatus::Completed && record.verified_at.is_some()
            })
            .collect::<Vec<_>>();
        if verified.len() <= self.config.limits.keep_count as usize {
            return Ok(());
        }
        verified.sort_by_key(|record| (record.started_at, record.id));
        let remove_count = verified
            .len()
            .saturating_sub(self.config.limits.keep_count as usize);
        for record in verified.into_iter().take(remove_count) {
            let path = PathBuf::from(&record.location);
            if path == self.artifact_path(record.id)? {
                let _ = async_fs::remove_file(&path).await;
                let _ = self.state.backups().delete(record.id).await;
            }
        }
        Ok(())
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn error_code(error: &BackupError) -> &'static str {
    match error {
        BackupError::Domain(_) => "domain_invalid",
        BackupError::State(_) => "state_unavailable",
        BackupError::Storage(_) => "storage_unavailable",
        BackupError::Core(_) => "recovery_failed",
        BackupError::Io(_) => "io_failed",
        BackupError::Json(_) | BackupError::Archive(_) => "archive_invalid",
        BackupError::TargetMismatch => "restore_target_mismatch",
        BackupError::KeyVersionMismatch => "key_version_mismatch",
        BackupError::Limit(_) => "resource_limit",
        BackupError::Maintenance => "maintenance",
        BackupError::NotFound => "not_found",
        BackupError::InconsistentSource => "source_changed",
    }
}

fn artifact(path: &str, size: u64, hash: ContentHash) -> BackupArtifact {
    BackupArtifact {
        path: path.to_owned(),
        size,
        sha256: hash.to_string(),
    }
}

fn register_artifact(
    limits: &BackupLimits,
    file_count: &mut u64,
    total_bytes: &mut u64,
    artifact: &BackupArtifact,
) -> Result<(), BackupError> {
    if artifact.size > limits.max_entry_bytes {
        return Err(BackupError::Limit("entry_bytes"));
    }
    *file_count = file_count
        .checked_add(1)
        .ok_or(BackupError::Limit("entry_count"))?;
    *total_bytes = total_bytes
        .checked_add(artifact.size)
        .ok_or(BackupError::Limit("total_bytes"))?;
    if *file_count > limits.max_entries {
        return Err(BackupError::Limit("entry_count"));
    }
    if *total_bytes > limits.max_total_bytes {
        return Err(BackupError::Limit("total_bytes"));
    }
    Ok(())
}

async fn ensure_private_directory(path: &Path) -> Result<(), BackupError> {
    validate_private_root(path)?;
    if let Ok(metadata) = async_fs::symlink_metadata(path).await {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(BackupError::Io("backup root is unsafe"));
        }
        return Ok(());
    }
    async_fs::create_dir_all(path)
        .await
        .map_err(|_| BackupError::Io("create backup root"))
}

fn validate_private_root(path: &Path) -> Result<(), BackupError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path == Path::new("/")
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BackupError::Archive("backup root is invalid"));
    }
    Ok(())
}

fn validate_private_path(path: &Path) -> Result<(), BackupError> {
    validate_private_root(path)?;
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, BackupError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BackupError::Archive("generated archive path is unsafe"));
    }
    Ok(root.join(relative_path))
}

async fn collect_entries(
    storage: VaultStorage,
    managed: bool,
) -> Result<Vec<FileMetadata>, BackupError> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
    let task = tokio::spawn(async move {
        if managed {
            storage.walk_managed_entries(sender).await
        } else {
            storage.walk_entries(sender).await
        }
    });
    let mut entries = Vec::new();
    while let Some(entry) = receiver.recv().await {
        entries.push(entry);
    }
    task.await
        .map_err(|_| BackupError::Io("collect Vault entries task"))??;
    entries.sort_by_key(|entry| entry.path.as_ref().map(ToString::to_string));
    Ok(entries)
}

async fn copy_storage_file(
    storage: &VaultStorage,
    path: &VaultPath,
    managed: bool,
    target: &Path,
) -> Result<(), BackupError> {
    if let Some(parent) = target.parent() {
        async_fs::create_dir_all(parent)
            .await
            .map_err(|_| BackupError::Io("create content staging"))?;
    }
    let mut reader = if managed {
        storage.open_read_managed(path).await?
    } else {
        storage.open_read(path).await?
    };
    let mut output = async_fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .await
        .map_err(|_| BackupError::Io("create content staging file"))?;
    tokio::io::copy(&mut reader, &mut output)
        .await
        .map_err(|_| BackupError::Io("copy content staging file"))?;
    output
        .sync_all()
        .await
        .map_err(|_| BackupError::Io("sync content staging file"))?;
    Ok(())
}

async fn copy_private_tree(
    source: &Path,
    target: &Path,
    archive_prefix: &str,
    limits: &BackupLimits,
    file_count: &mut u64,
    total_bytes: &mut u64,
) -> Result<Vec<BackupArtifact>, BackupError> {
    let mut stack = vec![(
        source.to_owned(),
        target.to_owned(),
        archive_prefix.to_owned(),
    )];
    let mut artifacts = Vec::new();
    while let Some((current, destination, prefix)) = stack.pop() {
        let metadata = async_fs::symlink_metadata(&current)
            .await
            .map_err(|_| BackupError::Io("inspect private tree"))?;
        if metadata.file_type().is_symlink() {
            return Err(BackupError::Archive("symlink in private tree"));
        }
        if metadata.is_dir() {
            async_fs::create_dir_all(&destination)
                .await
                .map_err(|_| BackupError::Io("create private staging directory"))?;
            let mut entries = async_fs::read_dir(&current)
                .await
                .map_err(|_| BackupError::Io("read private tree"))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|_| BackupError::Io("read private tree entry"))?
            {
                let name = entry
                    .file_name()
                    .to_str()
                    .ok_or(BackupError::Archive("non-UTF8 private path"))?
                    .to_owned();
                let child_archive = format!("{prefix}{name}");
                let child_current = entry.path();
                let child_destination = destination.join(&name);
                stack.push((
                    child_current,
                    child_destination,
                    format!("{child_archive}/"),
                ));
            }
            continue;
        }
        if !metadata.is_file() {
            return Err(BackupError::Archive("special entry in private tree"));
        }
        let archive_path = prefix.strip_suffix('/').unwrap_or(&prefix).to_owned();
        let mut input = async_fs::File::open(&current)
            .await
            .map_err(|_| BackupError::Io("open private tree file"))?;
        if metadata.len() > limits.max_entry_bytes {
            return Err(BackupError::Limit("entry_bytes"));
        }
        if let Some(parent) = destination.parent() {
            async_fs::create_dir_all(parent)
                .await
                .map_err(|_| BackupError::Io("create private staging parent"))?;
        }
        let mut output = async_fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .await
            .map_err(|_| BackupError::Io("create private staging file"))?;
        tokio::io::copy(&mut input, &mut output)
            .await
            .map_err(|_| BackupError::Io("copy private tree file"))?;
        output
            .sync_all()
            .await
            .map_err(|_| BackupError::Io("sync private tree file"))?;
        let (_, hash) = hash_path(&destination).await?;
        let item = artifact(&archive_path, metadata.len(), hash);
        register_artifact(limits, file_count, total_bytes, &item)?;
        artifacts.push(item);
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(artifacts)
}

async fn hash_path(path: &Path) -> Result<(u64, ContentHash), BackupError> {
    let mut file = async_fs::File::open(path)
        .await
        .map_err(|_| BackupError::Io("open file for hash"))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|_| BackupError::Io("read file for hash"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or(BackupError::Limit("total_bytes"))?;
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    Ok((size, ContentHash::from_bytes(bytes)))
}

fn write_tar(staging: &Path, destination: &Path, max_bytes: u64) -> Result<(), BackupError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| BackupError::Io("create archive"))?;
    let mut builder = Builder::new(file);
    append_tree(&mut builder, staging, staging)?;
    let mut file = builder
        .into_inner()
        .map_err(|_| BackupError::Io("finish archive"))?;
    file.flush().map_err(|_| BackupError::Io("flush archive"))?;
    file.sync_all()
        .map_err(|_| BackupError::Io("sync archive"))?;
    let size = file
        .metadata()
        .map_err(|_| BackupError::Io("stat archive"))?
        .len();
    if size > max_bytes {
        return Err(BackupError::Limit("archive_bytes"));
    }
    Ok(())
}

fn append_tree(
    builder: &mut Builder<File>,
    root: &Path,
    current: &Path,
) -> Result<(), BackupError> {
    let mut entries = fs::read_dir(current)
        .map_err(|_| BackupError::Io("read staging tree"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| BackupError::Io("read staging entry"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| BackupError::Io("inspect staging entry"))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| BackupError::Archive("staging path escaped"))?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(BackupError::Archive("staging path is unsafe"));
        }
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Err(BackupError::Archive("unsafe staging entry"));
        }
        if metadata.is_dir() {
            builder
                .append_dir(relative, &path)
                .map_err(|_| BackupError::Io("append directory"))?;
            append_tree(builder, root, &path)?;
        } else {
            builder
                .append_path_with_name(&path, relative)
                .map_err(|_| BackupError::Io("append file"))?;
        }
    }
    Ok(())
}

fn validate_archive_file(path: &Path, limits: BackupLimits) -> Result<RestorePreview, BackupError> {
    let metadata = fs::metadata(path).map_err(|_| BackupError::NotFound)?;
    if metadata.len() > limits.max_archive_bytes {
        return Err(BackupError::Limit("archive_bytes"));
    }
    let file = File::open(path).map_err(|_| BackupError::Io("open archive"))?;
    let mut archive = Archive::new(file);
    let mut files = BTreeMap::<String, (u64, String)>::new();
    let mut paths = BTreeSet::new();
    let mut manifest_bytes = None;
    let mut archive_entries = 0_u64;
    let mut total_bytes = 0_u64;
    let entries = archive
        .entries()
        .map_err(|_| BackupError::Archive("read tar entries"))?;
    for entry_result in entries {
        let mut entry = entry_result.map_err(|_| BackupError::Archive("read tar entry"))?;
        archive_entries = archive_entries
            .checked_add(1)
            .ok_or(BackupError::Limit("entry_count"))?;
        if archive_entries > limits.max_entries.saturating_add(1000) {
            return Err(BackupError::Limit("entry_count"));
        }
        let path = archive_path_string(&entry)?;
        if !paths.insert(path.clone()) {
            return Err(BackupError::Archive("duplicate archive path"));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink()
            || entry_type.is_hard_link()
            || entry_type == EntryType::Char
            || entry_type == EntryType::Block
            || entry_type == EntryType::Fifo
        {
            return Err(BackupError::Archive("link or special entry"));
        }
        if entry_type.is_dir() {
            continue;
        }
        if !entry_type.is_file() {
            return Err(BackupError::Archive("unsupported tar entry"));
        }
        if files.contains_key(&path) {
            return Err(BackupError::Archive("duplicate file"));
        }
        let size = entry.size();
        if size > limits.max_entry_bytes {
            return Err(BackupError::Limit("entry_bytes"));
        }
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or(BackupError::Limit("total_bytes"))?;
        if total_bytes > limits.max_total_bytes {
            return Err(BackupError::Limit("total_bytes"));
        }
        let (hash, payload) = if path == MANIFEST_ARCHIVE_PATH {
            if size > MAX_MANIFEST_BYTES {
                return Err(BackupError::Limit("manifest_bytes"));
            }
            let mut bytes = Vec::with_capacity(size as usize);
            entry
                .read_to_end(&mut bytes)
                .map_err(|_| BackupError::Archive("read manifest"))?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            (hex_digest(hasher.finalize()), Some(bytes))
        } else {
            (hash_reader(&mut entry, size)?, None)
        };
        if let Some(bytes) = payload {
            manifest_bytes = Some(bytes);
        }
        files.insert(path, (size, hash));
    }
    let manifest_bytes = manifest_bytes.ok_or(BackupError::Archive("manifest is missing"))?;
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes)?;
    validate_manifest(&manifest, limits)?;
    let mut expected = BTreeMap::new();
    expected.insert(
        manifest.state.path.clone(),
        (manifest.state.size, manifest.state.sha256.clone()),
    );
    for vault in &manifest.vaults {
        for item in vault.content.iter().chain(vault.history.iter()) {
            if expected
                .insert(item.path.clone(), (item.size, item.sha256.clone()))
                .is_some()
            {
                return Err(BackupError::Archive("duplicate manifest artifact"));
            }
        }
    }
    if files.len() != expected.len().saturating_add(1) || !files.contains_key(MANIFEST_ARCHIVE_PATH)
    {
        return Err(BackupError::Archive("archive and manifest entries differ"));
    }
    for (path, expected_value) in expected {
        if files.get(&path) != Some(&expected_value) {
            return Err(BackupError::Archive("artifact checksum mismatch"));
        }
    }
    Ok(RestorePreview {
        manifest,
        target_matches: false,
        archive_entries,
        archive_bytes: metadata.len(),
    })
}

fn validate_manifest(manifest: &BackupManifest, limits: BackupLimits) -> Result<(), BackupError> {
    if manifest.format_version != FORMAT_VERSION
        || manifest.service_version.is_empty()
        || manifest.schema_version <= 0
        || manifest.key_version_ids.is_empty()
        || manifest.key_version_ids.contains(&0)
        || manifest
            .key_version_ids
            .windows(2)
            .any(|versions| versions[0] >= versions[1])
    {
        return Err(BackupError::Archive("manifest version is invalid"));
    }
    if manifest.file_count > limits.max_entries || manifest.total_bytes > limits.max_total_bytes {
        return Err(BackupError::Limit("manifest totals"));
    }
    let mut paths = BTreeSet::new();
    let mut count = 0_u64;
    let mut total = 0_u64;
    let mut add = |item: &BackupArtifact| -> Result<(), BackupError> {
        if !paths.insert(item.path.clone())
            || item.size > limits.max_entry_bytes
            || item.sha256.len() != 64
        {
            return Err(BackupError::Archive("manifest artifact is invalid"));
        }
        if item.path != STATE_ARCHIVE_PATH
            && !item.path.starts_with("vaults/")
            && !item.path.starts_with("history/")
        {
            return Err(BackupError::Archive("manifest artifact root is invalid"));
        }
        let path = Path::new(&item.path);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(BackupError::Archive("manifest artifact path is unsafe"));
        }
        ContentHash::from_hex(&item.sha256)?;
        count += 1;
        total = total
            .checked_add(item.size)
            .ok_or(BackupError::Limit("manifest totals"))?;
        Ok(())
    };
    if manifest.state.path != STATE_ARCHIVE_PATH {
        return Err(BackupError::Archive("state artifact path is invalid"));
    }
    add(&manifest.state)?;
    for vault in &manifest.vaults {
        if vault.slug.is_empty() || vault.content_root.is_empty() {
            return Err(BackupError::Archive("Vault manifest identity is invalid"));
        }
        for item in &vault.content {
            if !item
                .path
                .starts_with(&format!("vaults/{}/content/", vault.vault_id))
            {
                return Err(BackupError::Archive("Vault content path is not isolated"));
            }
            add(item)?;
        }
        for item in &vault.history {
            if !item
                .path
                .starts_with(&format!("history/{}/", vault.vault_id))
            {
                return Err(BackupError::Archive("Vault history path is not isolated"));
            }
            add(item)?;
        }
    }
    if count != manifest.file_count || total != manifest.total_bytes {
        return Err(BackupError::Archive("manifest totals do not match"));
    }
    Ok(())
}

fn archive_path_string(entry: &tar::Entry<'_, File>) -> Result<String, BackupError> {
    let path = entry
        .path()
        .map_err(|_| BackupError::Archive("archive path is invalid"))?;
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BackupError::Archive("archive traversal path"));
    }
    let value = path
        .to_str()
        .ok_or(BackupError::Archive("archive path is not UTF-8"))?;
    if value.is_empty() || value.contains('\\') || value.starts_with("./") {
        return Err(BackupError::Archive("archive path normalization failed"));
    }
    let root = value.split('/').next().unwrap_or_default();
    if value != MANIFEST_ARCHIVE_PATH && !matches!(root, "state" | "vaults" | "history") {
        return Err(BackupError::Archive("archive path root is not allowed"));
    }
    Ok(value.to_owned())
}

fn hash_reader<R: Read>(reader: &mut R, size: u64) -> Result<String, BackupError> {
    let mut hasher = Sha256::new();
    let mut remaining = size;
    let mut buffer = vec![0_u8; 128 * 1024];
    while remaining != 0 {
        let requested = remaining.min(buffer.len() as u64) as usize;
        let read = reader
            .read(&mut buffer[..requested])
            .map_err(|_| BackupError::Archive("read archive payload"))?;
        if read == 0 {
            return Err(BackupError::Archive("archive payload is truncated"));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hex_digest(hasher.finalize()))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn extract_archive(
    path: &Path,
    destination: &Path,
    limits: BackupLimits,
) -> Result<ExtractedArchive, BackupError> {
    let preview = validate_archive_file(path, limits)?;
    fs::create_dir_all(destination).map_err(|_| BackupError::Io("create extraction root"))?;
    let file = File::open(path).map_err(|_| BackupError::Io("open archive for extraction"))?;
    let mut archive = Archive::new(file);
    for entry_result in archive
        .entries()
        .map_err(|_| BackupError::Archive("read archive for extraction"))?
    {
        let mut entry = entry_result.map_err(|_| BackupError::Archive("read extraction entry"))?;
        let relative = archive_path_string(&entry)?;
        let target = safe_join(destination, &relative)?;
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&target)
                .map_err(|_| BackupError::Io("create extracted directory"))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| BackupError::Io("create extraction parent"))?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|_| BackupError::Io("create extracted file"))?;
        io::copy(&mut entry, &mut output).map_err(|_| BackupError::Io("extract file"))?;
        output
            .sync_all()
            .map_err(|_| BackupError::Io("sync extracted file"))?;
    }
    Ok(ExtractedArchive {
        manifest: preview.manifest,
    })
}

struct ExtractedArchive {
    manifest: BackupManifest,
}

type RootSwap = DirectorySwap;

async fn swap_roots(
    state: &StateStore,
    config: &BackupConfig,
    stage: &Path,
) -> Result<Vec<RootSwap>, BackupError> {
    let mut swaps = Vec::new();
    for vault in state.vaults().list().await? {
        let content_source = stage
            .join("vaults")
            .join(vault.id.to_string())
            .join("content");
        let history_source = stage.join("history").join(vault.id.to_string());
        let content_target = vault.content_root.clone();
        let history_target = config.history_root.join(vault.id.to_string());
        async_fs::create_dir_all(&content_source)
            .await
            .map_err(|_| BackupError::Io("create staged content root"))?;
        async_fs::create_dir_all(&history_source)
            .await
            .map_err(|_| BackupError::Io("create staged history root"))?;
        if let Err(error) = install_staged_directory(
            &content_source,
            &content_target,
            &format!(".mcp-vault-restore-old-{}", vault.id),
        )
        .await
        .map_err(BackupError::Storage)
        .map(|swap| swaps.push(swap))
        {
            let _ = rollback_roots(&swaps).await;
            return Err(error);
        }
        if let Err(error) = install_staged_directory(
            &history_source,
            &history_target,
            &format!(".mcp-vault-restore-old-{}", vault.id),
        )
        .await
        .map_err(BackupError::Storage)
        .map(|swap| swaps.push(swap))
        {
            let _ = rollback_roots(&swaps).await;
            return Err(error);
        }
    }
    Ok(swaps)
}

async fn rollback_roots(swaps: &[RootSwap]) -> Result<(), BackupError> {
    rollback_directory_swaps(swaps)
        .await
        .map_err(BackupError::Storage)
}

async fn cleanup_old_roots(swaps: &[RootSwap]) -> Result<(), BackupError> {
    cleanup_directory_swaps(swaps)
        .await
        .map_err(BackupError::Storage)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{File, read, write},
        path::PathBuf,
    };

    use mcp_vault_domain::{
        MaintenanceGate, MaintenanceMode, Revision, VaultContext, VaultId, VaultSlug,
    };
    use mcp_vault_state::{StateStore, VaultStatus};
    use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
    use tar::{Builder, Header};
    use tempfile::TempDir;
    use tokio::time::{Duration, sleep, timeout};

    use super::{
        BackupConfig, BackupLimits, BackupManifest, BackupService, FORMAT_VERSION,
        validate_manifest,
    };

    #[test]
    fn manifest_limits_and_version_are_enforced() {
        let manifest = BackupManifest {
            format_version: FORMAT_VERSION + 1,
            service_version: "test".to_owned(),
            schema_version: 8,
            created_at: 1,
            completed_at: 2,
            state: super::BackupArtifact {
                path: "state/mcp-vault.sqlite3".to_owned(),
                size: 1,
                sha256: "0".repeat(64),
            },
            key_version_ids: vec![1],
            vaults: Vec::new(),
            file_count: 1,
            total_bytes: 1,
        };
        assert!(validate_manifest(&manifest, BackupLimits::default()).is_err());
    }

    #[tokio::test]
    async fn create_validate_and_restore_round_trip_isolated_roots() {
        let root = TempDir::new().unwrap();
        let database = root.path().join("state.sqlite3");
        let state = StateStore::connect_and_migrate(&format!("sqlite://{}", database.display()))
            .await
            .unwrap();
        let content = root.path().join("vaults/default/content");
        tokio::fs::create_dir_all(&content).await.unwrap();
        tokio::fs::write(content.join("note.md"), b"before restore")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("default").unwrap(),
            PathBuf::from(&content),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Default", VaultStatus::Active)
            .await
            .unwrap();
        let gate = MaintenanceGate::new();
        let core_runtime = mcp_vault_core::VaultCoreRuntime::new(gate.clone());
        let service = BackupService::new(
            state.clone(),
            BackupConfig {
                backup_root: root.path().join("backups"),
                history_root: root.path().join("history"),
                storage_options: StorageOptions {
                    durability: DurabilityPolicy::None,
                    minimum_free_bytes: 0,
                    ..StorageOptions::default()
                },
                limits: BackupLimits {
                    keep_count: 1,
                    ..BackupLimits::default()
                },
                service_version: "test".to_owned(),
                key_version_ids: vec![1],
                maintenance: gate.clone(),
                core_runtime: core_runtime.clone(),
                readiness: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            },
        );
        let operation = service.enqueue_create(None).await.unwrap();
        let backup_id = operation.backup.id;
        let active_write = gate.try_start_write().unwrap();
        let create_task = tokio::spawn({
            let service = service.clone();
            async move { service.create(backup_id).await }
        });
        timeout(Duration::from_secs(2), async {
            while gate.mode() != MaintenanceMode::ReadOnly {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!create_task.is_finished());
        drop(active_write);
        let manifest = create_task.await.unwrap().unwrap();
        assert_eq!(manifest.file_count, 2);
        let preview = service.validate_restore(backup_id).await.unwrap();
        assert!(preview.target_matches);

        tokio::fs::write(content.join("note.md"), b"after restore")
            .await
            .unwrap();
        let active_read = gate.try_start_operation().unwrap();
        let restore_task = tokio::spawn({
            let service = service.clone();
            async move { service.restore(backup_id).await }
        });
        timeout(Duration::from_secs(5), async {
            while gate.mode() != MaintenanceMode::Offline {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!restore_task.is_finished());
        assert_eq!(
            tokio::fs::read(content.join("note.md")).await.unwrap(),
            b"after restore"
        );
        drop(active_read);
        let restored = restore_task.await.unwrap().unwrap();
        assert!(restored.target_matches);
        assert_eq!(
            tokio::fs::read(content.join("note.md")).await.unwrap(),
            b"before restore"
        );
        assert_eq!(gate.mode(), MaintenanceMode::Normal);
        assert!(state.integrity_check().await.unwrap().integrity_ok);

        let low_space_service = BackupService::new(
            state.clone(),
            BackupConfig {
                backup_root: root.path().join("low-space-backups"),
                history_root: root.path().join("history"),
                storage_options: StorageOptions {
                    durability: DurabilityPolicy::None,
                    minimum_free_bytes: u64::MAX,
                    ..StorageOptions::default()
                },
                limits: BackupLimits::default(),
                service_version: "test".to_owned(),
                key_version_ids: vec![1],
                maintenance: gate.clone(),
                core_runtime,
                readiness: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            },
        );
        let low_space = low_space_service.enqueue_create(None).await.unwrap();
        assert!(matches!(
            low_space_service.create(low_space.backup.id).await,
            Err(super::BackupError::Limit("backup_free_space"))
        ));
        assert_eq!(
            tokio::fs::read(content.join("note.md")).await.unwrap(),
            b"before restore"
        );
    }

    #[test]
    fn archive_traversal_is_rejected_before_manifest_processing() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("malicious.tar");
        let mut builder = Builder::new(File::create(&archive_path).unwrap());
        let mut header = Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o600);
        header.set_cksum();
        builder.append_data(&mut header, "safe", &[][..]).unwrap();
        builder.finish().unwrap();

        let mut bytes = read(&archive_path).unwrap();
        bytes[0..100].fill(0);
        bytes[..10].copy_from_slice(b"../escape\0");
        bytes[148..156].fill(b' ');
        let checksum: u32 = bytes[..512].iter().map(|byte| u32::from(*byte)).sum();
        let checksum_field = format!("{checksum:06o}\0 ");
        bytes[148..156].copy_from_slice(checksum_field.as_bytes());
        write(&archive_path, bytes).unwrap();

        let result = super::validate_archive_file(&archive_path, BackupLimits::default());
        assert!(matches!(result, Err(super::BackupError::Archive(_))));
        assert!(!directory.path().join("escape").exists());
    }
}
