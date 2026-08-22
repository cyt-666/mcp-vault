//! Unix descriptor-relative filesystem operations.

use std::{
    fs::File,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use mcp_vault_domain::{FilesystemEntryKind, VaultPath};
use rustix::{
    fs::{self, AtFlags, FileType, Mode, OFlags},
    io::Errno,
};

use crate::{DestinationPolicy, DurabilityPolicy, StorageError};

/// An opened directory and its private diagnostic path.
pub(crate) struct ParentDir {
    pub(crate) file: File,
    pub(crate) path: PathBuf,
}

pub(crate) fn open_root(path: &Path) -> Result<File, StorageError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| StorageError::io("inspect storage root", error.kind()))?;
    if metadata.file_type().is_symlink() {
        return Err(StorageError::RootSymlink);
    }
    if !metadata.is_dir() {
        return Err(StorageError::RootNotDirectory);
    }

    let descriptor = fs::openat(
        fs::ABS,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| errno("open storage root", error))?;
    Ok(descriptor.into())
}

pub(crate) fn open_directory(path: &Path) -> Result<ParentDir, StorageError> {
    let file = open_root(path)?;
    Ok(ParentDir {
        file,
        path: path.to_owned(),
    })
}

pub(crate) fn open_relative_directory(
    root: &Path,
    relative: &VaultPath,
    create_missing: bool,
    directory_mode: u32,
) -> Result<ParentDir, StorageError> {
    if relative.is_root() {
        return open_directory(root);
    }

    let (parent, leaf) = open_parent_with_leaf(root, relative, create_missing, directory_mode)?;
    match entry_kind(&parent, &leaf)? {
        Some(FilesystemEntryKind::Directory) => {
            let file = open_child_directory(&parent.file, &leaf)?;
            Ok(ParentDir {
                file,
                path: parent.path.join(&leaf),
            })
        }
        Some(kind) => Err(StorageError::unsafe_entry(kind)),
        None if create_missing => {
            fs::mkdirat(&parent.file, &leaf, mode(directory_mode))
                .map_err(|error| errno("create relative directory", error))?;
            let file = open_child_directory(&parent.file, &leaf)?;
            Ok(ParentDir {
                file,
                path: parent.path.join(&leaf),
            })
        }
        None => Err(StorageError::SourceNotFound),
    }
}

pub(crate) fn open_parent_with_leaf(
    root: &Path,
    path: &VaultPath,
    create_missing: bool,
    directory_mode: u32,
) -> Result<(ParentDir, String), StorageError> {
    let mut current = open_root(root)?;
    let mut current_path = root.to_owned();
    let parts: Vec<&str> = path.segments().collect();
    let (leaf, parent_parts) = parts.split_last().ok_or(StorageError::InvalidOperation(
        "the Vault root has no leaf entry",
    ))?;

    for segment in parent_parts.iter().copied() {
        match open_child_directory(&current, segment) {
            Ok(next) => {
                current = next;
                current_path.push(segment);
            }
            Err(StorageError::Io { kind, .. }) if kind == ErrorKind::NotFound && create_missing => {
                match fs::mkdirat(&current, segment, mode(directory_mode)) {
                    Ok(()) => {}
                    // Another safe writer may have created the same parent
                    // after our no-following lookup. Re-open it below so the
                    // existing symlink/type validation remains authoritative.
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(errno("create parent directory", error)),
                }
                current = open_child_directory(&current, segment)?;
                current_path.push(segment);
            }
            Err(error) => return Err(error),
        }
    }

    Ok((
        ParentDir {
            file: current,
            path: current_path,
        },
        (*leaf).to_owned(),
    ))
}

pub(crate) fn create_directory_all(
    root: &Path,
    path: &VaultPath,
    directory_mode: u32,
) -> Result<(), StorageError> {
    if path.is_root() {
        let _ = open_root(root)?;
        return Ok(());
    }

    let (parent, leaf) = open_parent_with_leaf(root, path, true, directory_mode)?;
    match entry_kind(&parent, &leaf)? {
        Some(FilesystemEntryKind::Directory) => Ok(()),
        Some(kind) => Err(StorageError::unsafe_entry(kind)),
        None => match fs::mkdirat(&parent.file, &leaf, mode(directory_mode)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                match entry_kind(&parent, &leaf)? {
                    Some(FilesystemEntryKind::Directory) => Ok(()),
                    Some(kind) => Err(StorageError::unsafe_entry(kind)),
                    None => Err(errno("create directory", error)),
                }
            }
            Err(error) => Err(errno("create directory", error)),
        },
    }
}

pub(crate) fn entry_kind(
    parent: &ParentDir,
    leaf: &str,
) -> Result<Option<FilesystemEntryKind>, StorageError> {
    match fs::statat(&parent.file, leaf, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let kind = if file_type.is_file() {
                FilesystemEntryKind::RegularFile
            } else if file_type.is_dir() {
                FilesystemEntryKind::Directory
            } else if file_type.is_symlink() {
                FilesystemEntryKind::Symlink
            } else if file_type.is_block_device() {
                FilesystemEntryKind::BlockDevice
            } else if file_type.is_char_device() {
                FilesystemEntryKind::CharacterDevice
            } else if file_type.is_socket() {
                FilesystemEntryKind::Socket
            } else if file_type.is_fifo() {
                FilesystemEntryKind::Fifo
            } else {
                FilesystemEntryKind::Other
            };
            Ok(Some(kind))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(errno("inspect filesystem entry", error)),
    }
}

pub(crate) fn metadata_at(
    parent: &ParentDir,
    leaf: &str,
    kind: FilesystemEntryKind,
) -> Result<std::fs::Metadata, StorageError> {
    let file = match kind {
        FilesystemEntryKind::RegularFile => open_regular(parent, leaf)?,
        FilesystemEntryKind::Directory => {
            open_child_directory(&parent.file, leaf).map_err(|error| match error {
                StorageError::Io { kind, .. } => StorageError::io("open directory metadata", kind),
                other => other,
            })?
        }
        _ => {
            return std::fs::symlink_metadata(parent.path.join(leaf))
                .map_err(|error| errno_std("read filesystem metadata", error));
        }
    };
    file.metadata()
        .map_err(|error| errno_std("read filesystem metadata", error))
}

pub(crate) fn open_regular(parent: &ParentDir, leaf: &str) -> Result<File, StorageError> {
    match entry_kind(parent, leaf)? {
        Some(FilesystemEntryKind::RegularFile) => {}
        Some(kind) => return Err(StorageError::unsafe_entry(kind)),
        None => return Err(StorageError::SourceNotFound),
    }

    let descriptor = fs::openat(
        &parent.file,
        leaf,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| errno("open file", error))?;
    Ok(descriptor.into())
}

pub(crate) fn prepare_temp(parent: &ParentDir, name: &str) -> Result<File, StorageError> {
    let descriptor = fs::openat(
        &parent.file,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|error| errno("create temporary file", error))?;
    Ok(descriptor.into())
}

pub(crate) fn commit_temp(
    parent: &ParentDir,
    temp_name: &str,
    target: &str,
    policy: DestinationPolicy,
    durability: DurabilityPolicy,
) -> Result<(), StorageError> {
    validate_destination(parent, target, policy)?;
    let result = match policy {
        DestinationPolicy::MustNotExist => fs::renameat_with(
            &parent.file,
            temp_name,
            &parent.file,
            target,
            fs::RenameFlags::NOREPLACE,
        ),
        DestinationPolicy::ReplaceExisting => {
            fs::renameat(&parent.file, temp_name, &parent.file, target)
        }
    }
    .map_err(|error| errno("atomically rename file", error));
    if result.is_err() {
        cleanup_temp(parent, temp_name);
        return result;
    }
    sync_parent(parent, durability)
}

pub(crate) fn commit_temp_between(
    source: &ParentDir,
    temp_name: &str,
    destination: &ParentDir,
    target: &str,
    durability: DurabilityPolicy,
) -> Result<(), StorageError> {
    validate_destination(destination, target, DestinationPolicy::MustNotExist)?;
    let result = fs::renameat(&source.file, temp_name, &destination.file, target)
        .map_err(|error| errno("atomically install history blob", error));
    if result.is_err() {
        cleanup_temp(source, temp_name);
        return result;
    }
    sync_parent(source, durability)?;
    if source.path != destination.path {
        sync_parent(destination, durability)?;
    }
    Ok(())
}

pub(crate) fn cleanup_temp(parent: &ParentDir, temp_name: &str) {
    let _ = fs::unlinkat(&parent.file, temp_name, AtFlags::empty());
}

pub(crate) fn move_entry(
    source_parent: &ParentDir,
    source_name: &str,
    destination_parent: &ParentDir,
    destination_name: &str,
    policy: DestinationPolicy,
    durability: DurabilityPolicy,
) -> Result<(), StorageError> {
    let source_kind =
        entry_kind(source_parent, source_name)?.ok_or(StorageError::SourceNotFound)?;
    if !matches!(
        source_kind,
        FilesystemEntryKind::RegularFile | FilesystemEntryKind::Directory
    ) {
        return Err(StorageError::unsafe_entry(source_kind));
    }
    validate_destination(destination_parent, destination_name, policy)?;

    match policy {
        DestinationPolicy::MustNotExist => fs::renameat_with(
            &source_parent.file,
            source_name,
            &destination_parent.file,
            destination_name,
            fs::RenameFlags::NOREPLACE,
        ),
        DestinationPolicy::ReplaceExisting => fs::renameat(
            &source_parent.file,
            source_name,
            &destination_parent.file,
            destination_name,
        ),
    }
    .map_err(|error| errno("move filesystem entry", error))?;
    sync_parent(source_parent, durability)?;
    if source_parent.path != destination_parent.path {
        sync_parent(destination_parent, durability)?;
    }
    Ok(())
}

pub(crate) fn delete_entry(
    parent: &ParentDir,
    leaf: &str,
    durability: DurabilityPolicy,
) -> Result<(), StorageError> {
    let kind = entry_kind(parent, leaf)?.ok_or(StorageError::SourceNotFound)?;
    let flags = match kind {
        FilesystemEntryKind::RegularFile => AtFlags::empty(),
        FilesystemEntryKind::Directory => AtFlags::REMOVEDIR,
        other => return Err(StorageError::unsafe_entry(other)),
    };
    fs::unlinkat(&parent.file, leaf, flags)
        .map_err(|error| errno("delete filesystem entry", error))?;
    sync_parent(parent, durability)
}

fn open_child_directory(parent: &File, name: &str) -> Result<File, StorageError> {
    let descriptor = fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| errno("open directory component", error))?;
    Ok(descriptor.into())
}

fn mode(value: u32) -> Mode {
    Mode::from_raw_mode(value as _)
}

fn validate_destination(
    parent: &ParentDir,
    target: &str,
    policy: DestinationPolicy,
) -> Result<(), StorageError> {
    match entry_kind(parent, target)? {
        None => Ok(()),
        Some(FilesystemEntryKind::RegularFile) if policy == DestinationPolicy::ReplaceExisting => {
            Ok(())
        }
        Some(FilesystemEntryKind::RegularFile) => Err(StorageError::DestinationExists),
        Some(FilesystemEntryKind::Directory) => {
            Err(StorageError::InvalidOperation("destination is a directory"))
        }
        Some(kind) => Err(StorageError::unsafe_entry(kind)),
    }
}

fn sync_parent(parent: &ParentDir, durability: DurabilityPolicy) -> Result<(), StorageError> {
    if durability == DurabilityPolicy::Strict {
        parent
            .file
            .sync_all()
            .map_err(|error| errno_std("sync parent directory", error))?;
    }
    Ok(())
}

fn errno(operation: &'static str, error: Errno) -> StorageError {
    if error == Errno::LOOP {
        return StorageError::unsafe_entry(FilesystemEntryKind::Symlink);
    }
    StorageError::io(operation, error.kind())
}

fn errno_std(operation: &'static str, error: std::io::Error) -> StorageError {
    StorageError::io(operation, error.kind())
}
