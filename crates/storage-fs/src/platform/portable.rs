//! Checked portable fallback for platforms without Unix `*at` APIs.

use std::{
    fs::File,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use mcp_vault_domain::{FilesystemEntryKind, VaultPath};

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
    File::open(path).map_err(|error| StorageError::io("open storage root", error.kind()))
}

pub(crate) fn open_directory(path: &Path) -> Result<ParentDir, StorageError> {
    Ok(ParentDir {
        file: open_root(path)?,
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
        Some(FilesystemEntryKind::Directory) => open_directory(&parent.path.join(&leaf)),
        Some(kind) => Err(StorageError::unsafe_entry(kind)),
        None if create_missing => {
            std::fs::create_dir(parent.path.join(&leaf))
                .map_err(|error| StorageError::io("create relative directory", error.kind()))?;
            open_directory(&parent.path.join(&leaf))
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
    let parts: Vec<&str> = path.segments().collect();
    let (leaf, parent_parts) = parts.split_last().ok_or(StorageError::InvalidOperation(
        "the Vault root has no leaf entry",
    ))?;
    let mut current_path = root.to_owned();

    for segment in parent_parts.iter().copied() {
        let next_path = current_path.join(segment);
        match inspect_path(&next_path)? {
            Some(FilesystemEntryKind::Directory) => {}
            Some(kind) => return Err(StorageError::unsafe_entry(kind)),
            None if create_missing => match std::fs::create_dir(&next_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    match inspect_path(&next_path)? {
                        Some(FilesystemEntryKind::Directory) => {}
                        Some(kind) => return Err(StorageError::unsafe_entry(kind)),
                        None => {
                            return Err(StorageError::io("create parent directory", error.kind()));
                        }
                    }
                }
                Err(error) => {
                    return Err(StorageError::io("create parent directory", error.kind()));
                }
            },
            None => return Err(StorageError::SourceNotFound),
        }
        current_path = next_path;
    }

    let _ = directory_mode;
    Ok((
        ParentDir {
            file: open_root(&current_path)?,
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

    let mut current = root.to_owned();
    for segment in path.segments() {
        current.push(segment);
        match inspect_path(&current)? {
            Some(FilesystemEntryKind::Directory) => {}
            Some(kind) => return Err(StorageError::unsafe_entry(kind)),
            None => {
                let _ = directory_mode;
                match std::fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        match inspect_path(&current)? {
                            Some(FilesystemEntryKind::Directory) => {}
                            Some(kind) => return Err(StorageError::unsafe_entry(kind)),
                            None => {
                                return Err(StorageError::io("create directory", error.kind()));
                            }
                        }
                    }
                    Err(error) => {
                        return Err(StorageError::io("create directory", error.kind()));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn entry_kind(
    parent: &ParentDir,
    leaf: &str,
) -> Result<Option<FilesystemEntryKind>, StorageError> {
    inspect_path(&parent.path.join(leaf))
}

pub(crate) fn metadata_at(
    parent: &ParentDir,
    leaf: &str,
    _kind: FilesystemEntryKind,
) -> Result<std::fs::Metadata, StorageError> {
    std::fs::symlink_metadata(parent.path.join(leaf))
        .map_err(|error| StorageError::io("read filesystem metadata", error.kind()))
}

pub(crate) fn open_regular(parent: &ParentDir, leaf: &str) -> Result<File, StorageError> {
    match entry_kind(parent, leaf)? {
        Some(FilesystemEntryKind::RegularFile) => File::open(parent.path.join(leaf))
            .map_err(|error| StorageError::io("open file", error.kind())),
        Some(kind) => Err(StorageError::unsafe_entry(kind)),
        None => Err(StorageError::SourceNotFound),
    }
}

pub(crate) fn prepare_temp(parent: &ParentDir, name: &str) -> Result<File, StorageError> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(parent.path.join(name))
        .map_err(|error| StorageError::io("create temporary file", error.kind()))
}

pub(crate) fn commit_temp(
    parent: &ParentDir,
    temp_name: &str,
    target: &str,
    policy: DestinationPolicy,
    durability: DurabilityPolicy,
) -> Result<(), StorageError> {
    validate_destination(parent, target, policy)?;
    let result = std::fs::rename(parent.path.join(temp_name), parent.path.join(target))
        .map_err(|error| StorageError::io("atomically rename file", error.kind()));
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
    let result = std::fs::rename(source.path.join(temp_name), destination.path.join(target))
        .map_err(|error| StorageError::io("atomically install history blob", error.kind()));
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
    let _ = std::fs::remove_file(parent.path.join(temp_name));
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
    std::fs::rename(
        source_parent.path.join(source_name),
        destination_parent.path.join(destination_name),
    )
    .map_err(|error| StorageError::io("move filesystem entry", error.kind()))?;
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
    match kind {
        FilesystemEntryKind::RegularFile => std::fs::remove_file(parent.path.join(leaf)),
        FilesystemEntryKind::Directory => std::fs::remove_dir(parent.path.join(leaf)),
        other => return Err(StorageError::unsafe_entry(other)),
    }
    .map_err(|error| StorageError::io("delete filesystem entry", error.kind()))?;
    sync_parent(parent, durability)
}

fn inspect_path(path: &Path) -> Result<Option<FilesystemEntryKind>, StorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            let kind = if file_type.is_file() {
                FilesystemEntryKind::RegularFile
            } else if file_type.is_dir() {
                FilesystemEntryKind::Directory
            } else if file_type.is_symlink() {
                FilesystemEntryKind::Symlink
            } else {
                FilesystemEntryKind::Other
            };
            Ok(Some(kind))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StorageError::io("inspect filesystem entry", error.kind())),
    }
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
            .map_err(|error| StorageError::io("sync parent directory", error.kind()))?;
    }
    Ok(())
}
