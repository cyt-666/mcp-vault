//! Private filesystem primitives shared by the M6 live runner and Provider probe.

use std::{fs, io, path::Path};

#[cfg(unix)]
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Component,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
fn private_fs_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "private filesystem policy rejected the path",
    )
}

/// Reject symlink aliases in every existing component of an absolute path.
#[cfg(unix)]
pub(crate) fn validate_no_symlink_components(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(private_fs_error());
    }

    let mut current = Path::new("").to_path_buf();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(private_fs_error()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn validate_no_symlink_components(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private filesystem modes cannot be verified on this platform",
    ))
}

#[cfg(unix)]
fn validate_directory_identity(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || fs::canonicalize(path)? != path {
        return Err(private_fs_error());
    }
    Ok(metadata)
}

/// Existing private roots must have exactly mode 0700.
#[cfg(unix)]
pub(crate) fn validate_private_directory(path: &Path) -> io::Result<()> {
    validate_no_symlink_components(path)?;
    let metadata = validate_directory_identity(path)?;
    if metadata.permissions().mode() & 0o7777 != 0o700 {
        return Err(private_fs_error());
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn validate_private_directory(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private filesystem modes cannot be verified on this platform",
    ))
}

/// Validate a configured root if it already exists; a missing root is safe to
/// create later with [`ensure_private_directory`].
#[cfg(unix)]
pub(crate) fn validate_private_directory_if_exists(path: &Path) -> io::Result<()> {
    validate_no_symlink_components(path)?;
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
pub(crate) fn validate_private_directory_if_exists(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private filesystem modes cannot be verified on this platform",
    ))
}

/// Create every missing directory in the path with explicit mode 0700.
#[cfg(unix)]
pub(crate) fn ensure_private_directory(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(private_fs_error());
    }
    validate_no_symlink_components(path)?;
    match fs::symlink_metadata(path) {
        Ok(_) => return validate_private_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let parent = path.parent().ok_or_else(private_fs_error)?;
    match fs::symlink_metadata(parent) {
        Ok(_) => {
            validate_no_symlink_components(parent)?;
            validate_directory_identity(parent)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            ensure_private_directory(parent)?;
        }
        Err(error) => return Err(error),
    }

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    validate_private_directory(path)
}

#[cfg(not(unix))]
pub(crate) fn ensure_private_directory(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private filesystem modes cannot be verified on this platform",
    ))
}

/// Reject an existing target unless it is a private, single-link regular file.
#[cfg(unix)]
pub(crate) fn validate_private_file(path: &Path) -> io::Result<()> {
    validate_no_symlink_components(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != 0o600
        || fs::canonicalize(path)? != path
    {
        return Err(private_fs_error());
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn validate_private_file(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private filesystem modes cannot be verified on this platform",
    ))
}

/// Atomically write a sensitive file, explicitly setting mode 0600 before any
/// contents are written. Existing destinations must already satisfy the same
/// private-file policy.
#[cfg(unix)]
pub(crate) fn atomic_write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(private_fs_error)?;
    validate_private_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let file_name = path.file_name().ok_or_else(private_fs_error)?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{}.tmp-{}-{sequence}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temp)?;
    let write_result = (|| {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = write_result {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    File::open(parent)?.sync_all()?;
    validate_private_file(path)
}

#[cfg(not(unix))]
pub(crate) fn atomic_write_private_file(_path: &Path, _bytes: &[u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private filesystem modes cannot be verified on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn private_directory_and_atomic_file_have_exact_modes_and_refuse_unsafe_replacement() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap().join("private");
        ensure_private_directory(&root).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let artifact = root.join("report.md");
        atomic_write_private_file(&artifact, b"private report").unwrap();
        assert_eq!(
            fs::metadata(&artifact).unwrap().permissions().mode() & 0o777,
            0o600
        );
        atomic_write_private_file(&artifact, b"updated report").unwrap();
        assert_eq!(fs::read(&artifact).unwrap(), b"updated report");

        fs::set_permissions(&artifact, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(atomic_write_private_file(&artifact, b"must not replace").is_err());
        assert_eq!(fs::read(&artifact).unwrap(), b"updated report");

        let alias = temp.path().join("alias");
        symlink(&root, &alias).unwrap();
        assert!(ensure_private_directory(&alias).is_err());
    }

    #[cfg(not(unix))]
    #[test]
    fn private_filesystem_preflight_fails_closed_without_unix_modes() {
        let temp = tempdir().unwrap();
        assert!(validate_private_directory_if_exists(&temp.path().join("run")).is_err());
        assert!(ensure_private_directory(&temp.path().join("run")).is_err());
    }
}
