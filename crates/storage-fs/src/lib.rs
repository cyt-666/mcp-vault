//! Safe filesystem primitives for Vault Core.
//!
//! This crate owns root-relative, no-follow filesystem access, atomic writes,
//! streaming, metadata, and history blobs. It intentionally has no protocol,
//! SQL, or application-service dependency.

mod error;
mod platform;

use std::{
    fmt,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::{available_space, total_space};
use mcp_vault_domain::{
    DomainError, FilesystemEntryKind, FilesystemPolicy, VaultContext, VaultId, VaultPath,
    VaultPathPolicy,
};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self as async_fs, File},
    io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncWriteExt, ReadBuf, SeekFrom},
    sync::mpsc,
};
use uuid::Uuid;

pub use error::StorageError;

const COPY_BUFFER_SIZE: usize = 128 * 1024;
const DEFAULT_MINIMUM_FREE_BYTES: u64 = 16 * 1024 * 1024;

/// Durability level for temporary-file and directory synchronization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DurabilityPolicy {
    /// Sync file data/metadata and the containing directory after rename.
    #[default]
    Strict,
    /// Sync file data but omit the directory sync for lower latency.
    Relaxed,
    /// Do not issue explicit sync calls. Intended for tests only.
    None,
}

/// Replacement behavior for a file primitive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DestinationPolicy {
    /// Reject an existing regular-file destination.
    MustNotExist,
    /// Replace an existing regular-file destination atomically.
    ReplaceExisting,
}

/// Configuration for a Vault filesystem or history store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageOptions {
    /// Synchronization policy for committed writes.
    pub durability: DurabilityPolicy,
    /// Minimum free bytes required before a write starts.
    pub minimum_free_bytes: u64,
    /// Mode used when creating directories on Unix-like hosts.
    pub directory_mode: u32,
    /// Additional entry policy. Symlink traversal remains denied regardless of
    /// this value because the storage boundary always opens with no-follow.
    pub filesystem_policy: FilesystemPolicy,
}

impl Default for StorageOptions {
    fn default() -> Self {
        Self {
            durability: DurabilityPolicy::Strict,
            minimum_free_bytes: DEFAULT_MINIMUM_FREE_BYTES,
            directory_mode: 0o755,
            filesystem_policy: FilesystemPolicy::default(),
        }
    }
}

/// Free-space information for the filesystem containing a storage root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskSpace {
    /// Total bytes reported by the host filesystem.
    pub total_bytes: u64,
    /// Currently available bytes for the service process.
    pub available_bytes: u64,
}

/// One atomically installed directory and its rollback location.
///
/// This primitive is intentionally path-based only for service-owned roots;
/// callers must validate the target identity before invoking it. The backup
/// application coordinates several of these swaps, while the storage boundary
/// owns no-follow checks and filesystem mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectorySwap {
    /// Installed target root.
    pub target: PathBuf,
    /// Previous target root moved aside for rollback, if one existed.
    pub old: Option<PathBuf>,
}

/// Install a staged directory under a configured root without following a
/// symlink target. Both paths must be absolute service-owned paths on one
/// filesystem.
pub async fn install_staged_directory(
    source: &Path,
    target: &Path,
    rollback_name: &str,
) -> Result<DirectorySwap, StorageError> {
    validate_swap_path(source)?;
    validate_swap_path(target)?;
    validate_rollback_name(rollback_name)?;
    let source_metadata = async_fs::symlink_metadata(source).await.map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            StorageError::SourceNotFound
        } else {
            StorageError::io("inspect staged directory", error.kind())
        }
    })?;
    if source_metadata.file_type().is_symlink() {
        return Err(StorageError::RootSymlink);
    }
    if !source_metadata.is_dir() {
        return Err(StorageError::RootNotDirectory);
    }
    let parent = target.parent().ok_or(StorageError::InvalidOperation(
        "restore target has no parent",
    ))?;
    async_fs::create_dir_all(parent)
        .await
        .map_err(|error| StorageError::io("create restore target parent", error.kind()))?;
    let target_metadata = match async_fs::symlink_metadata(target).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(StorageError::io("inspect restore target", error.kind())),
    };
    if target_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(StorageError::RootSymlink);
    }
    if target_metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_dir())
    {
        return Err(StorageError::RootNotDirectory);
    }
    let old_path = parent.join(rollback_name);
    if let Ok(metadata) = async_fs::symlink_metadata(&old_path).await {
        if metadata.file_type().is_symlink() {
            return Err(StorageError::RootSymlink);
        }
        return Err(StorageError::DestinationExists);
    }
    let old = if target_metadata.is_some() {
        async_fs::rename(target, &old_path)
            .await
            .map_err(|error| StorageError::io("move restore target aside", error.kind()))?;
        Some(old_path)
    } else {
        None
    };
    if let Err(error) = async_fs::rename(source, target).await {
        if let Some(old_path) = old.as_ref() {
            let _ = async_fs::rename(old_path, target).await;
        }
        return Err(StorageError::io(
            if error.kind() == ErrorKind::CrossesDevices {
                "restore roots are on different filesystems"
            } else {
                "install restored root"
            },
            error.kind(),
        ));
    }
    Ok(DirectorySwap {
        target: target.to_owned(),
        old,
    })
}

/// Roll back a set of installed directory swaps in reverse order.
pub async fn rollback_directory_swaps(swaps: &[DirectorySwap]) -> Result<(), StorageError> {
    for swap in swaps.iter().rev() {
        let target_metadata = match async_fs::symlink_metadata(&swap.target).await {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(StorageError::io("inspect installed root", error.kind())),
        };
        if target_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(StorageError::RootSymlink);
        }
        if target_metadata
            .as_ref()
            .is_some_and(|metadata| !metadata.is_dir())
        {
            return Err(StorageError::RootNotDirectory);
        }
        if target_metadata.is_some() {
            async_fs::remove_dir_all(&swap.target)
                .await
                .map_err(|error| StorageError::io("remove failed restored root", error.kind()))?;
        }
        if let Some(old) = swap.old.as_ref() {
            if let Some(metadata) = async_fs::symlink_metadata(old).await.ok()
                && metadata.file_type().is_symlink()
            {
                return Err(StorageError::RootSymlink);
            }
            async_fs::rename(old, &swap.target)
                .await
                .map_err(|error| StorageError::io("rollback restored root", error.kind()))?;
        }
    }
    Ok(())
}

/// Remove rollback directories after a successful restore.
pub async fn cleanup_directory_swaps(swaps: &[DirectorySwap]) -> Result<(), StorageError> {
    for swap in swaps {
        if let Some(old) = swap.old.as_ref() {
            let metadata = match async_fs::symlink_metadata(old).await {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(StorageError::io("inspect old restore root", error.kind()));
                }
            };
            if let Some(metadata) = metadata {
                if metadata.file_type().is_symlink() {
                    return Err(StorageError::RootSymlink);
                }
                if !metadata.is_dir() {
                    return Err(StorageError::RootNotDirectory);
                }
                async_fs::remove_dir_all(old)
                    .await
                    .map_err(|error| StorageError::io("remove old restore root", error.kind()))?;
            }
        }
    }
    Ok(())
}

fn validate_swap_path(path: &Path) -> Result<(), StorageError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(StorageError::InvalidOperation(
            "restore root path is invalid",
        ));
    }
    Ok(())
}

fn validate_rollback_name(name: &str) -> Result<(), StorageError> {
    if name.is_empty()
        || name.chars().any(char::is_control)
        || Path::new(name)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(StorageError::InvalidOperation(
            "restore rollback name is invalid",
        ));
    }
    Ok(())
}

/// A validated SHA-256 content address.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Construct an address from its digest bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the digest bytes.
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Parse exactly 64 hexadecimal characters.
    pub fn from_hex(value: &str) -> Result<Self, StorageError> {
        if value.len() != 64 {
            return Err(StorageError::InvalidContentHash);
        }

        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_value(pair[0]).ok_or(StorageError::InvalidContentHash)?;
            let low = hex_value(pair[1]).ok_or(StorageError::InvalidContentHash)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    /// Return the canonical lowercase hexadecimal representation.
    pub fn as_hex(self) -> String {
        self.to_string()
    }

    fn prefix(self) -> String {
        self.to_string()[..2].to_owned()
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::str::FromStr for ContentHash {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

/// Platform identity information used as a reconciliation hint.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FileIdentityHint {
    /// Device or volume identifier when the host exposes one.
    pub device: u64,
    /// Inode or file-index identifier when the host exposes one.
    pub inode: u64,
}

/// Safe metadata for one canonical entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata {
    /// The Vault-relative path, or `None` for a history blob.
    pub path: Option<VaultPath>,
    /// Classified and policy-checked entry kind.
    pub kind: FilesystemEntryKind,
    /// File length in bytes. Directories report the host metadata length.
    pub size: u64,
    /// Modification time as UTC Unix milliseconds when representable.
    pub modified_at: Option<i64>,
    /// Best-effort filesystem identity hint.
    pub identity: Option<FileIdentityHint>,
}

/// Bounded filesystem-enumeration result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanSummary {
    /// Safe regular files emitted to the consumer.
    pub files_seen: u64,
    /// Safe directories emitted to the consumer.
    pub directories_seen: u64,
    /// Symlinks/special/invalid-name entries skipped without traversal.
    pub unsafe_entries_skipped: u64,
}

/// Result of a successful atomic canonical write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteReceipt {
    /// Committed Vault-relative destination.
    pub path: VaultPath,
    /// Number of streamed bytes.
    pub size: u64,
    /// SHA-256 of the committed payload.
    pub content_hash: ContentHash,
    /// Metadata observed after the rename.
    pub metadata: FileMetadata,
}

/// Typed relative name of a temporary canonical-write file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemporaryPath(VaultPath);

impl TemporaryPath {
    /// Parse a journal-provided temporary path after enforcing the storage
    /// naming convention.
    pub fn parse(path: VaultPath) -> Result<Self, StorageError> {
        let Some(name) = path.file_name() else {
            return Err(StorageError::InvalidOperation(
                "temporary path has no file name",
            ));
        };
        if !name.starts_with(".mcp-vault-tmp-") {
            return Err(StorageError::InvalidOperation(
                "temporary path name is invalid",
            ));
        }
        Ok(Self(path))
    }

    /// Return the Vault-relative temporary path.
    pub fn as_path(&self) -> &VaultPath {
        &self.0
    }
}

/// Progress returned after a phased stream has completed but before rename.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteProgress {
    /// Number of streamed bytes.
    pub size: u64,
    /// SHA-256 of the temporary payload.
    pub content_hash: ContentHash,
}

/// An opaque phased canonical write used by Vault Core journaling.
pub struct AtomicWrite {
    storage: VaultStorage,
    path: VaultPath,
    managed: bool,
    destination: DestinationPolicy,
    options: StorageOptions,
    parent: Option<platform::ParentDir>,
    leaf: String,
    temp_name: String,
    temp_file: Option<File>,
    progress: Option<WriteProgress>,
    hasher: Option<Sha256>,
    streamed_size: u64,
    synced: bool,
}

/// Result of a history write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryBlob {
    /// Content address of the stored payload.
    pub content_hash: ContentHash,
    /// Number of bytes in the payload.
    pub size: u64,
    /// Whether this call created the blob rather than deduplicating it.
    pub created: bool,
}

/// A streaming reader with safe metadata but no filesystem path.
pub struct ReadFile {
    inner: File,
    metadata: FileMetadata,
}

impl ReadFile {
    /// Return the metadata captured when the descriptor was opened.
    pub fn metadata(&self) -> &FileMetadata {
        &self.metadata
    }

    /// Consume the wrapper and return the async file handle.
    pub fn into_inner(self) -> File {
        self.inner
    }
}

impl AsyncRead for ReadFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncSeek for ReadFile {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
        Pin::new(&mut self.inner).start_seek(position)
    }

    fn poll_complete(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<u64>> {
        Pin::new(&mut self.inner).poll_complete(context)
    }
}

impl AtomicWrite {
    /// Return the typed relative temporary path recorded by the journal.
    pub fn temporary_path(&self) -> Result<TemporaryPath, StorageError> {
        let temp = VaultPath::parse(&self.temp_name)?;
        let path = match self.path.parent() {
            Some(parent) => parent.join(&temp)?,
            None => temp,
        };
        Ok(TemporaryPath(path))
    }

    /// Stream input into the temporary file and compute its content hash.
    pub async fn write_from<R>(&mut self, reader: &mut R) -> Result<WriteProgress, StorageError>
    where
        R: AsyncRead + Unpin,
    {
        if self.progress.is_some() {
            return Err(StorageError::InvalidOperation(
                "atomic write was already streamed",
            ));
        }
        let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
        let result = async {
            loop {
                let read = reader
                    .read(&mut buffer)
                    .await
                    .map_err(|error| StorageError::io("read source stream", error.kind()))?;
                if read == 0 {
                    break;
                }
                self.write_chunk(&buffer[..read]).await?;
            }
            self.finish()
        }
        .await;
        if result.is_err() {
            self.abort().await;
        }
        result
    }

    /// Append one bounded request chunk to the temporary payload.
    pub async fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), StorageError> {
        if self.progress.is_some() || self.hasher.is_none() {
            return Err(StorageError::InvalidOperation(
                "atomic write stream is already finalized",
            ));
        }
        let file = self
            .temp_file
            .as_mut()
            .ok_or(StorageError::InvalidOperation("atomic write is closed"))?;
        file.write_all(bytes)
            .await
            .map_err(|error| StorageError::io("write temporary file", error.kind()))?;
        let size = self
            .streamed_size
            .checked_add(bytes.len() as u64)
            .ok_or(StorageError::InvalidOperation("stream size overflow"))?;
        if let Some(hasher) = self.hasher.as_mut() {
            hasher.update(bytes);
        }
        self.streamed_size = size;
        Ok(())
    }

    /// Finalize the incremental payload and return its content address.
    pub fn finish(&mut self) -> Result<WriteProgress, StorageError> {
        if let Some(progress) = self.progress.as_ref() {
            return Ok(progress.clone());
        }
        let hasher = self.hasher.take().ok_or(StorageError::InvalidOperation(
            "atomic write stream is closed",
        ))?;
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        let progress = WriteProgress {
            size: self.streamed_size,
            content_hash: ContentHash::from_bytes(bytes),
        };
        self.progress = Some(progress.clone());
        Ok(progress)
    }

    /// Fsync the temporary payload according to the configured policy.
    pub async fn sync(&mut self) -> Result<(), StorageError> {
        if self.progress.is_none() {
            return Err(StorageError::InvalidOperation(
                "atomic write has no payload",
            ));
        }
        let result = {
            let file = self
                .temp_file
                .as_mut()
                .ok_or(StorageError::InvalidOperation("atomic write is closed"))?;
            sync_file(file, self.options.durability).await
        };
        match result {
            Ok(()) => {
                self.synced = true;
                Ok(())
            }
            Err(error) => {
                self.abort().await;
                Err(error)
            }
        }
    }

    /// Return the stream result before the physical rename.
    pub fn progress(&self) -> Option<&WriteProgress> {
        self.progress.as_ref()
    }

    /// Atomically rename the temporary payload and return post-commit metadata.
    pub async fn commit(mut self) -> Result<WriteReceipt, StorageError> {
        let progress = self.progress.clone().ok_or(StorageError::InvalidOperation(
            "atomic write has no payload",
        ))?;
        if !self.synced {
            return Err(StorageError::InvalidOperation(
                "atomic write was not synced before commit",
            ));
        }
        let temp_file = self.temp_file.take();
        drop(temp_file);
        let parent = self
            .parent
            .take()
            .ok_or(StorageError::InvalidOperation("atomic write is closed"))?;
        let leaf = self.leaf.clone();
        let temp_name = self.temp_name.clone();
        let destination = self.destination;
        let durability = self.options.durability;
        run_blocking(move || {
            platform::commit_temp(&parent, &temp_name, &leaf, destination, durability)
        })
        .await?;

        let metadata = if self.managed {
            self.storage.stat_managed(&self.path).await
        } else {
            self.storage.stat(&self.path).await
        }?;
        Ok(WriteReceipt {
            path: self.path,
            size: progress.size,
            content_hash: progress.content_hash,
            metadata,
        })
    }

    /// Safely remove the temporary file after an ordinary failed operation.
    pub async fn abort(&mut self) {
        self.hasher.take();
        self.progress.take();
        self.temp_file.take();
        if let Some(parent) = self.parent.take() {
            let name = self.temp_name.clone();
            let _ = cleanup_temp(parent, name).await;
        }
    }
}

/// Filesystem operations bound to one immutable Vault context.
#[derive(Clone)]
pub struct VaultStorage {
    vault_id: VaultId,
    root: PathBuf,
    path_policy: VaultPathPolicy,
    options: StorageOptions,
}

impl fmt::Debug for VaultStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VaultStorage")
            .field("vault_id", &self.vault_id)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl VaultStorage {
    /// Bind storage to a validated Vault context and path policy.
    pub fn new(
        context: &VaultContext,
        path_policy: VaultPathPolicy,
        options: StorageOptions,
    ) -> Self {
        Self {
            vault_id: context.id(),
            root: context.content_root().to_owned(),
            path_policy,
            options,
        }
    }

    /// Construct storage using the default reserved-path and durability policy.
    pub fn with_defaults(context: &VaultContext) -> Self {
        Self::new(
            context,
            VaultPathPolicy::default(),
            StorageOptions::default(),
        )
    }

    /// Return the bound Vault identity without exposing its absolute root.
    pub const fn vault_id(&self) -> VaultId {
        self.vault_id
    }

    /// Validate or create the configured content root.
    pub async fn ensure_root(&self) -> Result<(), StorageError> {
        let root = self.root.clone();
        run_blocking(move || {
            ensure_directory_root(&root)?;
            platform::open_root(&root).map(|_| ())
        })
        .await
    }

    /// Return whether the configured root contains no entries at all.
    ///
    /// Provisioning uses this before registering a service-managed Vault. The
    /// check deliberately includes the reserved namespace and unsafe entry
    /// kinds; an unregistered non-empty directory must never be claimed merely
    /// because ordinary Vault listing would hide one of its entries.
    pub async fn is_root_empty(&self) -> Result<bool, StorageError> {
        self.ensure_root().await?;
        let root = self.root.clone();
        run_blocking(move || {
            platform::open_root(&root)?;
            let mut entries = std::fs::read_dir(&root)
                .map_err(|error| StorageError::io("inspect Vault root entries", error.kind()))?;
            Ok(entries.next().is_none())
        })
        .await
    }

    /// Return free-space diagnostics for the content root filesystem.
    pub async fn disk_space(&self) -> Result<DiskSpace, StorageError> {
        disk_space_for(&self.root).await
    }

    /// Begin a journalable phased atomic write.
    pub fn temporary_path_for(&self, path: &VaultPath) -> Result<TemporaryPath, StorageError> {
        self.temporary_path_for_with_policy(path, false)
    }

    /// Begin a journalable phased atomic write for an explicit managed path.
    pub fn temporary_path_for_managed(
        &self,
        path: &VaultPath,
    ) -> Result<TemporaryPath, StorageError> {
        self.temporary_path_for_with_policy(path, true)
    }

    fn temporary_path_for_with_policy(
        &self,
        path: &VaultPath,
        managed: bool,
    ) -> Result<TemporaryPath, StorageError> {
        self.validate_write_path_for(path, managed)?;
        let name = VaultPath::parse(&temporary_name("mcp-vault-tmp"))?;
        let path = match path.parent() {
            Some(parent) => parent.join(&name)?,
            None => name,
        };
        Ok(TemporaryPath(path))
    }

    /// Begin a phased write using a previously journaled temporary path.
    pub async fn begin_atomic_write_at(
        &self,
        path: &VaultPath,
        destination: DestinationPolicy,
        temporary: &TemporaryPath,
    ) -> Result<AtomicWrite, StorageError> {
        self.begin_atomic_write_at_with_policy(path, destination, temporary, false)
            .await
    }

    /// Begin a phased atomic write for an explicit managed path.
    pub async fn begin_atomic_write_at_managed(
        &self,
        path: &VaultPath,
        destination: DestinationPolicy,
        temporary: &TemporaryPath,
    ) -> Result<AtomicWrite, StorageError> {
        self.begin_atomic_write_at_with_policy(path, destination, temporary, true)
            .await
    }

    async fn begin_atomic_write_at_with_policy(
        &self,
        path: &VaultPath,
        destination: DestinationPolicy,
        temporary: &TemporaryPath,
        managed: bool,
    ) -> Result<AtomicWrite, StorageError> {
        self.validate_write_path_for(path, managed)?;
        let expected_parent = path.parent();
        let actual_parent = temporary.as_path().parent();
        if expected_parent != actual_parent {
            return Err(StorageError::InvalidOperation(
                "temporary path is outside the destination directory",
            ));
        }
        self.ensure_root().await?;
        self.ensure_free_space().await?;

        let root = self.root.clone();
        let path_for_prepare = path.clone();
        let options = self.options;
        let temp_path = temporary.as_path().clone();
        let temp_name = temp_path
            .file_name()
            .ok_or(StorageError::InvalidOperation(
                "temporary path has no file name",
            ))?
            .to_owned();
        let temp_name_for_prepare = temp_name.to_owned();
        let (parent, leaf, temp_file) = run_blocking(move || {
            let (parent, leaf) = platform::open_parent_with_leaf(
                &root,
                &path_for_prepare,
                false,
                options.directory_mode,
            )?;
            let temp_file = platform::prepare_temp(&parent, &temp_name_for_prepare)?;
            Ok((parent, leaf, temp_file))
        })
        .await?;

        Ok(AtomicWrite {
            storage: self.clone(),
            path: path.clone(),
            managed,
            destination,
            options,
            parent: Some(parent),
            leaf,
            temp_name,
            temp_file: Some(File::from_std(temp_file)),
            progress: None,
            hasher: Some(Sha256::new()),
            streamed_size: 0,
            synced: false,
        })
    }

    /// Begin a journalable phased atomic write with a generated temp name.
    pub async fn begin_atomic_write(
        &self,
        path: &VaultPath,
        destination: DestinationPolicy,
    ) -> Result<AtomicWrite, StorageError> {
        let temporary = self.temporary_path_for(path)?;
        self.begin_atomic_write_at(path, destination, &temporary)
            .await
    }

    /// Create a Vault-relative directory tree without following links.
    pub async fn create_dir_all(&self, path: &VaultPath) -> Result<(), StorageError> {
        self.create_dir_all_with_policy(path, false).await
    }

    /// Create a directory tree inside the explicit managed namespace.
    pub async fn create_dir_all_managed(&self, path: &VaultPath) -> Result<(), StorageError> {
        self.create_dir_all_with_policy(path, true).await
    }

    async fn create_dir_all_with_policy(
        &self,
        path: &VaultPath,
        managed: bool,
    ) -> Result<(), StorageError> {
        self.validate_path_for(path, managed)?;
        self.ensure_root().await?;
        let root = self.root.clone();
        let options = self.options;
        let path = path.clone();
        run_blocking(move || platform::create_directory_all(&root, &path, options.directory_mode))
            .await
    }

    /// Read metadata for a Vault-relative file or directory.
    pub async fn stat(&self, path: &VaultPath) -> Result<FileMetadata, StorageError> {
        self.stat_with_policy(path, false).await
    }

    /// Read metadata for an explicit managed path.
    pub async fn stat_managed(&self, path: &VaultPath) -> Result<FileMetadata, StorageError> {
        self.stat_with_policy(path, true).await
    }

    async fn stat_with_policy(
        &self,
        path: &VaultPath,
        managed: bool,
    ) -> Result<FileMetadata, StorageError> {
        self.validate_path_for(path, managed)?;
        self.ensure_root().await?;
        let root = self.root.clone();
        let path = path.clone();
        let policy = self.options.filesystem_policy;
        run_blocking(move || {
            if path.is_root() {
                let root_file = platform::open_root(&root)?;
                let metadata = root_file.metadata().map_err(|error| {
                    StorageError::io("read storage root metadata", error.kind())
                })?;
                return Ok(file_metadata(
                    Some(path),
                    FilesystemEntryKind::Directory,
                    metadata,
                ));
            }

            let (parent, leaf) = platform::open_parent_with_leaf(&root, &path, false, 0o755)?;
            let kind = platform::entry_kind(&parent, &leaf)?.ok_or(StorageError::SourceNotFound)?;
            validate_entry(policy, kind)?;
            let metadata = platform::metadata_at(&parent, &leaf, kind)?;
            if metadata.file_type().is_symlink() {
                return Err(StorageError::unsafe_entry(FilesystemEntryKind::Symlink));
            }
            Ok(file_metadata(Some(path), kind, metadata))
        })
        .await
    }

    /// List one safe directory level without following links or exposing the
    /// absolute storage root.
    pub async fn list_directory(
        &self,
        path: &VaultPath,
    ) -> Result<Vec<FileMetadata>, StorageError> {
        self.validate_user_path(path)?;
        self.ensure_root().await?;
        let root = self.root.clone();
        let path = path.clone();
        let policy = self.options.filesystem_policy;
        let path_policy = self.path_policy.clone();
        run_blocking(move || {
            let directory = platform::open_relative_directory(&root, &path, false, 0o755)?;
            let mut entries = std::fs::read_dir(&directory.path)
                .map_err(|error| StorageError::io("list Vault directory", error.kind()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| StorageError::io("read Vault directory entry", error.kind()))?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            let mut result = Vec::with_capacity(entries.len());
            for entry in entries {
                let name = match entry.file_name().to_str() {
                    Some(name) => name.to_owned(),
                    None => continue,
                };
                let child = if path.is_root() {
                    VaultPath::parse(&name)?
                } else {
                    path.join(&VaultPath::parse(&name)?)?
                };
                if path_policy.is_reserved(&child) {
                    continue;
                }
                let metadata = entry
                    .metadata()
                    .map_err(|error| StorageError::io("read Vault entry metadata", error.kind()))?;
                let kind = if metadata.file_type().is_dir() {
                    FilesystemEntryKind::Directory
                } else if metadata.file_type().is_file() {
                    FilesystemEntryKind::RegularFile
                } else if metadata.file_type().is_symlink() {
                    FilesystemEntryKind::Symlink
                } else {
                    FilesystemEntryKind::Other
                };
                if validate_entry(policy, kind).is_err() {
                    continue;
                }
                result.push(file_metadata(Some(child), kind, metadata));
            }
            Ok(result)
        })
        .await
    }

    /// Open a regular canonical file for asynchronous streaming reads.
    pub async fn open_read(&self, path: &VaultPath) -> Result<ReadFile, StorageError> {
        self.validate_user_path(path)?;
        self.open_read_validated(path).await
    }

    /// Open one service-managed file for an explicit Core operation.
    ///
    /// Managed paths are deliberately not accepted by ordinary user reads;
    /// callers must opt into this method after applying the managed operation
    /// boundary in Vault Core.
    pub async fn open_read_managed(&self, path: &VaultPath) -> Result<ReadFile, StorageError> {
        self.path_policy
            .validate_managed_path(path)
            .map_err(StorageError::from)?;
        self.open_read_validated(path).await
    }

    async fn open_read_validated(&self, path: &VaultPath) -> Result<ReadFile, StorageError> {
        if path.is_root() {
            return Err(StorageError::InvalidOperation(
                "the Vault root is not a file",
            ));
        }
        self.ensure_root().await?;
        let root = self.root.clone();
        let path = path.clone();
        let policy = self.options.filesystem_policy;
        let (file, metadata) = run_blocking(move || {
            let (parent, leaf) = platform::open_parent_with_leaf(&root, &path, false, 0o755)?;
            let kind = platform::entry_kind(&parent, &leaf)?.ok_or(StorageError::SourceNotFound)?;
            validate_entry(policy, kind)?;
            if kind != FilesystemEntryKind::RegularFile {
                return Err(StorageError::InvalidOperation(
                    "entry is not a regular file",
                ));
            }
            let file = platform::open_regular(&parent, &leaf)?;
            let metadata = file
                .metadata()
                .map_err(|error| StorageError::io("read opened file metadata", error.kind()))?;
            Ok((file, file_metadata(Some(path), kind, metadata)))
        })
        .await?;

        Ok(ReadFile {
            inner: File::from_std(file),
            metadata,
        })
    }

    /// Stream a payload to a same-directory temporary file and atomically
    /// install it at the validated destination.
    pub async fn write_atomic<R>(
        &self,
        path: &VaultPath,
        mut reader: R,
        destination: DestinationPolicy,
    ) -> Result<WriteReceipt, StorageError>
    where
        R: AsyncRead + Unpin,
    {
        let mut atomic = self.begin_atomic_write(path, destination).await?;
        atomic.write_from(&mut reader).await?;
        atomic.sync().await?;
        atomic.commit().await
    }

    /// Convenience wrapper for a complete in-memory payload.
    pub async fn write_bytes(
        &self,
        path: &VaultPath,
        bytes: &[u8],
        destination: DestinationPolicy,
    ) -> Result<WriteReceipt, StorageError> {
        self.write_atomic(path, ByteReader::new(bytes.to_owned()), destination)
            .await
    }

    /// Hash an existing regular file without exposing its path to the reader.
    pub async fn hash_file(&self, path: &VaultPath) -> Result<(u64, ContentHash), StorageError> {
        self.hash_file_with_policy(path, false).await
    }

    /// Hash one regular managed file.
    pub async fn hash_file_managed(
        &self,
        path: &VaultPath,
    ) -> Result<(u64, ContentHash), StorageError> {
        self.hash_file_with_policy(path, true).await
    }

    async fn hash_file_with_policy(
        &self,
        path: &VaultPath,
        managed: bool,
    ) -> Result<(u64, ContentHash), StorageError> {
        let mut reader = if managed {
            self.open_read_managed(path).await?
        } else {
            self.open_read(path).await?
        };
        hash_reader(&mut reader).await
    }

    /// Enumerate safe user entries with bounded-channel backpressure.
    ///
    /// The absolute root and host paths remain inside this storage boundary.
    /// Symlinks, special files, invalid UTF-8 names, and the reserved managed
    /// namespace are skipped and counted; no unsafe entry is followed.
    pub async fn walk_entries(
        &self,
        sender: mpsc::Sender<FileMetadata>,
    ) -> Result<ScanSummary, StorageError> {
        self.walk_entries_with_policy(sender, false).await
    }

    /// Enumerate safe entries inside the explicit managed namespace.
    pub async fn walk_managed_entries(
        &self,
        sender: mpsc::Sender<FileMetadata>,
    ) -> Result<ScanSummary, StorageError> {
        self.walk_entries_with_policy(sender, true).await
    }

    async fn walk_entries_with_policy(
        &self,
        sender: mpsc::Sender<FileMetadata>,
        include_managed: bool,
    ) -> Result<ScanSummary, StorageError> {
        self.ensure_root().await?;
        let root = self.root.clone();
        let policy = self.options.filesystem_policy;
        let path_policy = self.path_policy.clone();
        run_blocking(move || {
            let mut directories = vec![(String::new(), root)];
            let mut summary = ScanSummary::default();
            while let Some((relative, absolute)) = directories.pop() {
                let mut entries = std::fs::read_dir(&absolute)
                    .map_err(|error| StorageError::io("enumerate Vault directory", error.kind()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        StorageError::io("read Vault directory entry", error.kind())
                    })?;
                entries.sort_by_key(std::fs::DirEntry::file_name);
                for entry in entries {
                    let name = match entry.file_name().to_str() {
                        Some(name) => name.to_owned(),
                        None => {
                            summary.unsafe_entries_skipped += 1;
                            continue;
                        }
                    };
                    let relative_path = if relative.is_empty() {
                        name
                    } else {
                        format!("{relative}/{name}")
                    };
                    let path = match VaultPath::parse(&relative_path) {
                        Ok(path) => path,
                        Err(_) => {
                            summary.unsafe_entries_skipped += 1;
                            continue;
                        }
                    };
                    if path_policy.is_reserved(&path) != include_managed {
                        continue;
                    }
                    let metadata = entry.metadata().map_err(|error| {
                        StorageError::io("read Vault entry metadata", error.kind())
                    })?;
                    let file_type = metadata.file_type();
                    let kind = if file_type.is_dir() {
                        FilesystemEntryKind::Directory
                    } else if file_type.is_file() {
                        FilesystemEntryKind::RegularFile
                    } else if file_type.is_symlink() {
                        FilesystemEntryKind::Symlink
                    } else {
                        FilesystemEntryKind::Other
                    };
                    if validate_entry(policy, kind).is_err() {
                        summary.unsafe_entries_skipped += 1;
                        continue;
                    }
                    let file_metadata = file_metadata(Some(path.clone()), kind, metadata);
                    sender
                        .blocking_send(file_metadata)
                        .map_err(|_| StorageError::InvalidOperation("scan consumer cancelled"))?;
                    match kind {
                        FilesystemEntryKind::Directory => {
                            summary.directories_seen += 1;
                            directories.push((relative_path, entry.path()));
                        }
                        FilesystemEntryKind::RegularFile => summary.files_seen += 1,
                        _ => {}
                    }
                }
            }
            Ok(summary)
        })
        .await
    }

    /// Atomically copy one regular file to another Vault-relative path.
    pub async fn copy_file(
        &self,
        source: &VaultPath,
        destination: &VaultPath,
        policy: DestinationPolicy,
    ) -> Result<WriteReceipt, StorageError> {
        self.validate_write_path(destination)?;
        let mut source_file = self.open_read(source).await?;
        self.write_atomic(destination, &mut source_file, policy)
            .await
    }

    /// Move a regular file or directory without following symlinks.
    pub async fn move_entry(
        &self,
        source: &VaultPath,
        destination: &VaultPath,
        policy: DestinationPolicy,
    ) -> Result<(), StorageError> {
        self.validate_write_path(source)?;
        self.validate_write_path(destination)?;
        if source == destination {
            return Err(StorageError::InvalidOperation(
                "source and destination match",
            ));
        }
        self.ensure_root().await?;
        let root = self.root.clone();
        let source = source.clone();
        let destination = destination.clone();
        let options = self.options;
        run_blocking(move || {
            let (source_parent, source_name) =
                platform::open_parent_with_leaf(&root, &source, false, options.directory_mode)?;
            let (destination_parent, destination_name) = platform::open_parent_with_leaf(
                &root,
                &destination,
                false,
                options.directory_mode,
            )?;
            platform::move_entry(
                &source_parent,
                &source_name,
                &destination_parent,
                &destination_name,
                policy,
                options.durability,
            )
        })
        .await
    }

    /// Delete a regular file or an empty directory.
    pub async fn delete(&self, path: &VaultPath) -> Result<(), StorageError> {
        self.validate_write_path(path)?;
        self.delete_validated(path).await
    }

    /// Delete one regular managed file through an explicit reserved-path
    /// operation.
    pub async fn delete_managed(&self, path: &VaultPath) -> Result<(), StorageError> {
        self.validate_write_path_for(path, true)?;
        self.delete_validated(path).await
    }

    async fn delete_validated(&self, path: &VaultPath) -> Result<(), StorageError> {
        self.ensure_root().await?;
        let root = self.root.clone();
        let path = path.clone();
        let options = self.options;
        run_blocking(move || {
            let (parent, leaf) =
                platform::open_parent_with_leaf(&root, &path, false, options.directory_mode)?;
            platform::delete_entry(&parent, &leaf, options.durability)
        })
        .await
    }

    /// Remove a previously journaled temporary canonical-write file.
    pub async fn remove_temporary(&self, temporary: &TemporaryPath) -> Result<(), StorageError> {
        self.ensure_root().await?;
        let root = self.root.clone();
        let path = temporary.as_path().clone();
        let options = self.options;
        run_blocking(move || {
            let (parent, leaf) =
                platform::open_parent_with_leaf(&root, &path, false, options.directory_mode)?;
            match platform::entry_kind(&parent, &leaf)? {
                None => Ok(()),
                Some(FilesystemEntryKind::RegularFile) => {
                    platform::cleanup_temp(&parent, &leaf);
                    Ok(())
                }
                Some(kind) => Err(StorageError::unsafe_entry(kind)),
            }
        })
        .await
    }

    fn validate_path_for(&self, path: &VaultPath, managed: bool) -> Result<(), StorageError> {
        if managed {
            self.path_policy
                .validate_managed_path(path)
                .map_err(Into::into)
        } else {
            self.path_policy
                .validate_user_path(path)
                .map_err(Into::into)
        }
    }

    fn validate_user_path(&self, path: &VaultPath) -> Result<(), StorageError> {
        self.validate_path_for(path, false)
    }

    fn validate_write_path_for(&self, path: &VaultPath, managed: bool) -> Result<(), StorageError> {
        self.validate_path_for(path, managed)?;
        if path.is_root() {
            return Err(StorageError::InvalidOperation(
                "the Vault root is not an entry",
            ));
        }
        Ok(())
    }

    fn validate_write_path(&self, path: &VaultPath) -> Result<(), StorageError> {
        self.validate_write_path_for(path, false)
    }

    async fn ensure_free_space(&self) -> Result<(), StorageError> {
        let required = self.options.minimum_free_bytes;
        let available = self.disk_space().await?.available_bytes;
        if available < required {
            return Err(StorageError::InsufficientDiskSpace {
                available,
                required,
            });
        }
        Ok(())
    }
}

/// Content-addressed history blobs scoped to one Vault identity.
#[derive(Clone)]
pub struct HistoryStore {
    vault_id: VaultId,
    history_root: PathBuf,
    relative_blob_root: VaultPath,
    blob_root: PathBuf,
    options: StorageOptions,
}

impl fmt::Debug for HistoryStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoryStore")
            .field("vault_id", &self.vault_id)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl HistoryStore {
    /// Bind a history store to `<history-root>/<vault-id>/blobs`.
    pub fn new(
        context: &VaultContext,
        history_root: impl Into<PathBuf>,
        options: StorageOptions,
    ) -> Result<Self, StorageError> {
        let history_root = history_root.into();
        validate_private_root(&history_root)?;
        let relative_blob_root = VaultPath::parse(&format!("{}/blobs", context.id()))?;
        Ok(Self {
            vault_id: context.id(),
            history_root: history_root.clone(),
            relative_blob_root,
            blob_root: history_root.join(context.id().to_string()).join("blobs"),
            options,
        })
    }

    /// Return the bound Vault identity.
    pub const fn vault_id(&self) -> VaultId {
        self.vault_id
    }

    /// Create and validate the private history directory.
    pub async fn ensure_root(&self) -> Result<(), StorageError> {
        let history_root = self.history_root.clone();
        let relative_root = self.relative_blob_root.clone();
        let options = self.options;
        run_blocking(move || {
            ensure_configured_directory(&history_root)?;
            platform::open_relative_directory(
                &history_root,
                &relative_root,
                true,
                options.directory_mode,
            )
            .map(|_| ())
        })
        .await
    }

    /// Store a stream as a deduplicated content-addressed blob.
    pub async fn put<R>(&self, mut reader: R) -> Result<HistoryBlob, StorageError>
    where
        R: AsyncRead + Unpin,
    {
        self.ensure_root().await?;
        self.ensure_free_space().await?;

        let history_root = self.history_root.clone();
        let relative_root = self.relative_blob_root.clone();
        let options = self.options;
        let temp_name = temporary_name("mcp-vault-history-tmp");
        let temp_name_for_prepare = temp_name.clone();
        let (temp_parent, temp_file) = run_blocking(move || {
            let parent = platform::open_relative_directory(
                &history_root,
                &relative_root,
                true,
                options.directory_mode,
            )?;
            let file = platform::prepare_temp(&parent, &temp_name_for_prepare)?;
            Ok((parent, file))
        })
        .await?;

        let mut temp_file = File::from_std(temp_file);
        let stream_result = stream_to_file(&mut reader, &mut temp_file).await;
        if let Err(error) = stream_result {
            drop(temp_file);
            cleanup_temp(temp_parent, temp_name).await;
            return Err(error);
        }
        let (size, content_hash) = stream_result.expect("stream result was checked above");
        if let Err(error) = sync_file(&mut temp_file, options.durability).await {
            drop(temp_file);
            cleanup_temp(temp_parent, temp_name).await;
            return Err(error);
        }
        drop(temp_file);

        let prefix = VaultPath::parse(&content_hash.prefix())?;
        let prefix_relative = self.relative_blob_root.join(&prefix)?;
        let history_root = self.history_root.clone();
        let prefix_result = run_blocking(move || {
            platform::open_relative_directory(
                &history_root,
                &prefix_relative,
                true,
                options.directory_mode,
            )
            .map(|_| ())
        })
        .await;
        if let Err(error) = prefix_result {
            cleanup_temp(temp_parent, temp_name).await;
            return Err(error);
        }
        let target_name = content_hash.to_string();
        let history_root = self.history_root.clone();
        let prefix_relative = self.relative_blob_root.join(&prefix)?;
        run_blocking(move || {
            let result = (|| {
                let destination_parent = platform::open_relative_directory(
                    &history_root,
                    &prefix_relative,
                    false,
                    options.directory_mode,
                )?;
                match platform::entry_kind(&destination_parent, &target_name)? {
                    Some(FilesystemEntryKind::RegularFile) => {
                        platform::cleanup_temp(&temp_parent, &temp_name);
                        Ok(HistoryBlob {
                            content_hash,
                            size,
                            created: false,
                        })
                    }
                    Some(kind) => Err(StorageError::unsafe_entry(kind)),
                    None => {
                        platform::commit_temp_between(
                            &temp_parent,
                            &temp_name,
                            &destination_parent,
                            &target_name,
                            options.durability,
                        )?;
                        Ok(HistoryBlob {
                            content_hash,
                            size,
                            created: true,
                        })
                    }
                }
            })();
            if result.is_err() {
                platform::cleanup_temp(&temp_parent, &temp_name);
            }
            result
        })
        .await
    }

    /// Convenience wrapper for a complete history payload.
    pub async fn put_bytes(&self, bytes: &[u8]) -> Result<HistoryBlob, StorageError> {
        self.put(ByteReader::new(bytes.to_owned())).await
    }

    /// Open a history blob after validating its content address.
    pub async fn open(&self, content_hash: ContentHash) -> Result<ReadFile, StorageError> {
        let history_root = self.history_root.clone();
        let hex = content_hash.to_string();
        let prefix = VaultPath::parse(&content_hash.prefix())?;
        let relative_root = self.relative_blob_root.join(&prefix)?;
        let policy = self.options.filesystem_policy;
        let (file, metadata) = run_blocking(move || {
            let parent =
                platform::open_relative_directory(&history_root, &relative_root, false, 0o755)
                    .map_err(|error| match error {
                        StorageError::SourceNotFound
                        | StorageError::Io {
                            kind: ErrorKind::NotFound,
                            ..
                        } => StorageError::HistoryNotFound,
                        other => other,
                    })?;
            let kind = platform::entry_kind(&parent, &hex)
                .map_err(|error| match error {
                    StorageError::SourceNotFound
                    | StorageError::Io {
                        kind: ErrorKind::NotFound,
                        ..
                    } => StorageError::HistoryNotFound,
                    other => other,
                })?
                .ok_or(StorageError::HistoryNotFound)?;
            validate_entry(policy, kind)?;
            if kind != FilesystemEntryKind::RegularFile {
                return Err(StorageError::HistoryNotFound);
            }
            let file = platform::open_regular(&parent, &hex)?;
            let metadata = file
                .metadata()
                .map_err(|error| StorageError::io("read history metadata", error.kind()))?;
            Ok((file, file_metadata(None, kind, metadata)))
        })
        .await?;

        Ok(ReadFile {
            inner: File::from_std(file),
            metadata,
        })
    }

    /// Return whether a regular history blob exists.
    pub async fn contains(&self, content_hash: ContentHash) -> Result<bool, StorageError> {
        match self.open(content_hash).await {
            Ok(_) => Ok(true),
            Err(StorageError::HistoryNotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn ensure_free_space(&self) -> Result<(), StorageError> {
        let required = self.options.minimum_free_bytes;
        let available = disk_space_for(&self.blob_root).await?.available_bytes;
        if available < required {
            return Err(StorageError::InsufficientDiskSpace {
                available,
                required,
            });
        }
        Ok(())
    }
}

async fn stream_to_file<R>(
    reader: &mut R,
    writer: &mut File,
) -> Result<(u64, ContentHash), StorageError>
where
    R: AsyncRead + Unpin,
{
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| StorageError::io("read source stream", error.kind()))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .await
            .map_err(|error| StorageError::io("write temporary file", error.kind()))?;
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or(StorageError::InvalidOperation("stream size overflow"))?;
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    Ok((size, ContentHash::from_bytes(bytes)))
}

async fn hash_reader<R>(reader: &mut R) -> Result<(u64, ContentHash), StorageError>
where
    R: AsyncRead + Unpin,
{
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| StorageError::io("read source stream", error.kind()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or(StorageError::InvalidOperation("stream size overflow"))?;
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    Ok((size, ContentHash::from_bytes(bytes)))
}

async fn sync_file(file: &mut File, durability: DurabilityPolicy) -> Result<(), StorageError> {
    match durability {
        DurabilityPolicy::Strict => file
            .sync_all()
            .await
            .map_err(|error| StorageError::io("sync temporary file", error.kind())),
        DurabilityPolicy::Relaxed => file
            .sync_data()
            .await
            .map_err(|error| StorageError::io("sync temporary file data", error.kind())),
        DurabilityPolicy::None => Ok(()),
    }
}

async fn cleanup_temp(parent: platform::ParentDir, name: String) {
    let _ = run_blocking(move || {
        platform::cleanup_temp(&parent, &name);
        Ok(())
    })
    .await;
}

async fn disk_space_for(path: &Path) -> Result<DiskSpace, StorageError> {
    let path = path.to_owned();
    run_blocking(move || {
        let available = available_space(&path)
            .map_err(|error| StorageError::io("read available disk space", error.kind()))?;
        let total = total_space(&path)
            .map_err(|error| StorageError::io("read total disk space", error.kind()))?;
        Ok(DiskSpace {
            total_bytes: total,
            available_bytes: available,
        })
    })
    .await
}

async fn run_blocking<T, F>(operation: F) -> Result<T, StorageError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, StorageError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| StorageError::TaskCancelled)?
}

fn ensure_directory_root(root: &Path) -> Result<(), StorageError> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StorageError::RootSymlink),
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(StorageError::RootNotDirectory),
        Err(error) if error.kind() == ErrorKind::NotFound => std::fs::create_dir_all(root)
            .map_err(|create_error| StorageError::io("create storage root", create_error.kind())),
        Err(error) => Err(StorageError::io("inspect storage root", error.kind())),
    }
}

fn ensure_configured_directory(path: &Path) -> Result<(), StorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StorageError::RootSymlink),
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(StorageError::RootNotDirectory),
        Err(error) if error.kind() == ErrorKind::NotFound => std::fs::create_dir_all(path)
            .map_err(|create_error| StorageError::io("create history root", create_error.kind())),
        Err(error) => Err(StorageError::io("inspect history root", error.kind())),
    }
}

fn validate_private_root(path: &Path) -> Result<(), StorageError> {
    if !path.is_absolute() || path.as_os_str().is_empty() {
        return Err(StorageError::Domain(DomainError::InvalidContentRoot));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(StorageError::Domain(DomainError::InvalidContentRoot));
    }
    Ok(())
}

fn validate_entry(policy: FilesystemPolicy, kind: FilesystemEntryKind) -> Result<(), StorageError> {
    if !matches!(
        kind,
        FilesystemEntryKind::RegularFile | FilesystemEntryKind::Directory
    ) {
        return Err(StorageError::unsafe_entry(kind));
    }
    policy.validate_entry_kind(kind).map_err(Into::into)
}

fn file_metadata(
    path: Option<VaultPath>,
    kind: FilesystemEntryKind,
    metadata: std::fs::Metadata,
) -> FileMetadata {
    FileMetadata {
        path,
        kind,
        size: metadata.len(),
        modified_at: metadata.modified().ok().and_then(system_time_millis),
        identity: file_identity(&metadata),
    }
}

fn system_time_millis(value: SystemTime) -> Option<i64> {
    let duration = value.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> Option<FileIdentityHint> {
    use std::os::unix::fs::MetadataExt;

    Some(FileIdentityHint {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn file_identity(_metadata: &std::fs::Metadata) -> Option<FileIdentityHint> {
    None
}

fn temporary_name(prefix: &str) -> String {
    format!(".{prefix}-{}", Uuid::now_v7().simple())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
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
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.offset == self.bytes.len() {
            return Poll::Ready(Ok(()));
        }
        let remaining = &self.bytes[self.offset..];
        let count = remaining.len().min(buffer.remaining());
        buffer.put_slice(&remaining[..count]);
        self.offset += count;
        Poll::Ready(Ok(()))
    }
}
