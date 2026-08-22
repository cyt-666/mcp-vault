//! Safe logical Vault paths and filesystem safety policies.

use std::{collections::HashMap, fmt, str::FromStr};

use percent_encoding::percent_decode_str;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use unicode_normalization::UnicodeNormalization;

use crate::{DomainError, PathError};

const MAX_PATH_BYTES: usize = 4096;
const MAX_SEGMENTS: usize = 64;
const MAX_SEGMENT_BYTES: usize = 255;

/// Logical case-comparison behavior selected by Vault setup or reconciliation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PathCaseSensitivity {
    /// Preserve case as distinct path identity.
    Sensitive,
    /// Compare paths using Unicode lowercase for collision detection.
    #[default]
    Insensitive,
}

/// A normalized key used only for collision detection and lookup maps.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PathComparisonKey(String);

impl PathComparisonKey {
    /// Return the normalized comparison string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for PathComparisonKey {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// A validated, normalized path relative to one Vault content root.
///
/// The empty string is the Vault root. It is the only valid empty path; all
/// non-root paths contain non-empty slash-separated segments.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VaultPath(String);

impl VaultPath {
    /// Construct the logical Vault root.
    pub const fn root() -> Self {
        Self(String::new())
    }

    /// Parse an already-decoded logical path.
    ///
    /// Callers handling URL paths should use from_url_path so the decode
    /// boundary remains explicit and happens exactly once.
    pub fn parse(input: &str) -> Result<Self, DomainError> {
        if input.is_empty() {
            return Ok(Self::root());
        }

        reject_encoded_unsafe_components(input)?;

        if input.starts_with('/') {
            return Err(PathError::Absolute.into());
        }
        if input.contains('\\') {
            return Err(PathError::Backslash.into());
        }
        if input.contains('\0') {
            return Err(PathError::Nul.into());
        }
        if input.chars().any(char::is_control) {
            return Err(PathError::ControlCharacter.into());
        }

        let normalized: String = input.nfc().collect();
        if normalized.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong.into());
        }

        let segments: Vec<&str> = normalized.split('/').collect();
        if segments.len() > MAX_SEGMENTS {
            return Err(PathError::TooDeep.into());
        }
        if segments.iter().any(|segment| segment.is_empty()) {
            return Err(PathError::EmptySegment.into());
        }

        for segment in &segments {
            validate_segment(segment)?;
        }

        Ok(Self(normalized))
    }

    /// Decode one URL-path layer and parse the resulting logical path.
    ///
    /// A URL leading slash is accepted as the transport representation of the
    /// Vault root. A second leading slash remains invalid.
    pub fn from_url_path(input: &str) -> Result<Self, DomainError> {
        let without_mount_leading_slash = input.strip_prefix('/').unwrap_or(input);
        validate_percent_encoding(without_mount_leading_slash)?;
        let decoded = percent_decode_str(without_mount_leading_slash)
            .decode_utf8()
            .map_err(|_| PathError::InvalidUtf8)?;

        Self::parse(&decoded)
    }

    /// Return the canonical relative representation. The Vault root is empty.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return whether this path is the Vault root.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Return the number of logical segments.
    pub fn depth(&self) -> usize {
        if self.is_root() {
            0
        } else {
            self.0.split('/').count()
        }
    }

    /// Iterate over non-empty logical segments.
    pub fn segments(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.0.split('/').filter(|segment| !segment.is_empty())
    }

    /// Return the final segment, or none for the Vault root.
    pub fn file_name(&self) -> Option<&str> {
        self.segments().next_back()
    }

    /// Return the parent path, or none for the Vault root.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }

        match self.0.rsplit_once('/') {
            Some((parent, _)) => Some(Self(parent.to_owned())),
            None => Some(Self::root()),
        }
    }

    /// Join two already-validated logical paths without a host filesystem path.
    pub fn join(&self, child: &Self) -> Result<Self, DomainError> {
        if self.is_root() {
            return Ok(child.clone());
        }
        if child.is_root() {
            return Ok(self.clone());
        }

        let joined = format!("{}/{}", self.0, child.0);
        Self::parse(&joined)
    }

    /// Return whether this path is equal to or beneath ancestor.
    pub fn starts_with(&self, ancestor: &Self) -> bool {
        ancestor.is_root()
            || self.0 == ancestor.0
            || self
                .0
                .strip_prefix(&ancestor.0)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }

    /// Build the explicit comparison key for the selected filesystem policy.
    pub fn comparison_key(&self, sensitivity: PathCaseSensitivity) -> PathComparisonKey {
        let value: String = match sensitivity {
            PathCaseSensitivity::Sensitive => self.0.clone(),
            PathCaseSensitivity::Insensitive => {
                self.0.chars().flat_map(char::to_lowercase).collect()
            }
        };

        PathComparisonKey(value)
    }
}

impl TryFrom<&str> for VaultPath {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for VaultPath {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl FromStr for VaultPath {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for VaultPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            formatter.write_str("/")
        } else {
            formatter.write_str(self.as_str())
        }
    }
}

impl Serialize for VaultPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for VaultPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

/// Detect path collisions under an explicit case-sensitivity policy.
pub fn detect_path_collisions<'a, I>(
    paths: I,
    sensitivity: PathCaseSensitivity,
) -> Result<(), DomainError>
where
    I: IntoIterator<Item = &'a VaultPath>,
{
    let mut seen: HashMap<PathComparisonKey, VaultPath> = HashMap::new();

    for path in paths {
        let key = path.comparison_key(sensitivity);
        if let Some(first) = seen.insert(key, path.clone()) {
            return Err(DomainError::PathCollision {
                first,
                second: path.clone(),
            });
        }
    }

    Ok(())
}

/// Policy for the reserved service-managed path namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultPathPolicy {
    reserved_root: VaultPath,
    case_sensitivity: PathCaseSensitivity,
}

impl Default for VaultPathPolicy {
    fn default() -> Self {
        Self {
            reserved_root: VaultPath::parse("_mcp-vault").expect("default reserved path is valid"),
            case_sensitivity: PathCaseSensitivity::default(),
        }
    }
}

impl VaultPathPolicy {
    /// Construct a policy with an explicit non-root reserved namespace.
    pub fn new(
        reserved_root: VaultPath,
        case_sensitivity: PathCaseSensitivity,
    ) -> Result<Self, DomainError> {
        if reserved_root.is_root() {
            return Err(DomainError::InvalidValue {
                kind: "reserved path root",
                reason: "must not be the Vault root",
            });
        }

        Ok(Self {
            reserved_root,
            case_sensitivity,
        })
    }

    /// Return the service-managed namespace root.
    pub fn reserved_root(&self) -> &VaultPath {
        &self.reserved_root
    }

    /// Return the selected collision policy.
    pub fn case_sensitivity(&self) -> PathCaseSensitivity {
        self.case_sensitivity
    }

    /// Return whether a path is inside the reserved namespace.
    pub fn is_reserved(&self, path: &VaultPath) -> bool {
        let path_key = path.comparison_key(self.case_sensitivity);
        let root_key = self.reserved_root.comparison_key(self.case_sensitivity);

        path_key.as_str() == root_key.as_str()
            || path_key
                .as_str()
                .strip_prefix(root_key.as_str())
                .is_some_and(|suffix| suffix.starts_with('/'))
    }

    /// Validate a path for ordinary user/Vault content operations.
    pub fn validate_user_path(&self, path: &VaultPath) -> Result<(), DomainError> {
        if self.is_reserved(path) {
            return Err(DomainError::ReservedPath(path.clone()));
        }

        Ok(())
    }

    /// Validate a path for service-managed content operations.
    pub fn validate_managed_path(&self, path: &VaultPath) -> Result<(), DomainError> {
        if self.is_reserved(path) {
            Ok(())
        } else {
            Err(DomainError::PathNotManaged)
        }
    }

    /// Detect collisions using this policy's case behavior.
    pub fn detect_collisions<'a, I>(&self, paths: I) -> Result<(), DomainError>
    where
        I: IntoIterator<Item = &'a VaultPath>,
    {
        detect_path_collisions(paths, self.case_sensitivity)
    }
}

/// Filesystem entry kinds that storage-fs must classify before exposure.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FilesystemEntryKind {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// A block device.
    BlockDevice,
    /// A character device.
    CharacterDevice,
    /// A Unix socket or equivalent special endpoint.
    Socket,
    /// A named pipe/FIFO.
    Fifo,
    /// Any unsupported platform-specific special kind.
    Other,
}

impl FilesystemEntryKind {
    /// Return a stable diagnostic label without exposing filesystem details.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegularFile => "regular_file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            Self::BlockDevice => "block_device",
            Self::CharacterDevice => "character_device",
            Self::Socket => "socket",
            Self::Fifo => "fifo",
            Self::Other => "other",
        }
    }
}

/// Default-deny policy for filesystem entries crossing the domain boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FilesystemPolicy {
    allow_symlinks: bool,
    allow_special_files: bool,
}

impl FilesystemPolicy {
    /// Construct an explicit entry policy.
    pub const fn new(allow_symlinks: bool, allow_special_files: bool) -> Self {
        Self {
            allow_symlinks,
            allow_special_files,
        }
    }

    /// Return whether symlink entries are allowed at this policy boundary.
    pub const fn allow_symlinks(self) -> bool {
        self.allow_symlinks
    }

    /// Return whether non-file/non-directory special entries are allowed.
    pub const fn allow_special_files(self) -> bool {
        self.allow_special_files
    }

    /// Validate a classified filesystem entry kind.
    pub fn validate_entry_kind(self, kind: FilesystemEntryKind) -> Result<(), DomainError> {
        match kind {
            FilesystemEntryKind::RegularFile | FilesystemEntryKind::Directory => Ok(()),
            FilesystemEntryKind::Symlink if self.allow_symlinks => Ok(()),
            FilesystemEntryKind::Symlink => Err(DomainError::UnsafeFilesystemEntry {
                kind: kind.as_str(),
            }),
            _ if self.allow_special_files => Ok(()),
            _ => Err(DomainError::UnsafeFilesystemEntry {
                kind: kind.as_str(),
            }),
        }
    }
}

fn validate_segment(segment: &str) -> Result<(), PathError> {
    if segment == "." || segment == ".." {
        return Err(PathError::Traversal);
    }
    if segment.len() > MAX_SEGMENT_BYTES {
        return Err(PathError::SegmentTooLong);
    }
    if segment.ends_with([' ', '.']) {
        return Err(PathError::TrailingSpaceOrPeriod);
    }
    if segment
        .chars()
        .any(|character| matches!(character, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err(PathError::InvalidPlatformCharacter);
    }
    if is_windows_reserved_name(segment) {
        return Err(PathError::ReservedPlatformName);
    }

    Ok(())
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment);
    let upper = stem.to_ascii_uppercase();

    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn validate_percent_encoding(input: &str) -> Result<(), PathError> {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || hex_value(bytes[index + 1]).is_none()
                || hex_value(bytes[index + 2]).is_none()
            {
                return Err(PathError::InvalidPercentEncoding);
            }
            index += 3;
        } else {
            index += 1;
        }
    }

    Ok(())
}

fn reject_encoded_unsafe_components(input: &str) -> Result<(), PathError> {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] == b'%' {
            let Some(value) = hex_pair(bytes[index + 1], bytes[index + 2]) else {
                index += 1;
                continue;
            };

            let nested_unsafe = value == b'%'
                && index + 4 < bytes.len()
                && hex_pair(bytes[index + 3], bytes[index + 4])
                    .is_some_and(|nested| matches!(nested, 0x00 | 0x2e | 0x2f | 0x5c));
            if matches!(value, 0x00 | 0x2e | 0x2f | 0x5c) || nested_unsafe {
                return Err(PathError::EncodedUnsafeComponent);
            }
        }
        index += 1;
    }

    Ok(())
}

fn hex_pair(first: u8, second: u8) -> Option<u8> {
    Some(hex_value(first)? * 16 + hex_value(second)?)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FilesystemEntryKind, FilesystemPolicy, PathCaseSensitivity, VaultPath, VaultPathPolicy,
        detect_path_collisions,
    };
    use crate::{DomainError, PathError};

    fn path(value: &str) -> VaultPath {
        VaultPath::parse(value).unwrap()
    }

    #[test]
    fn root_and_path_operations_are_unambiguous() {
        let root = VaultPath::root();
        let notes = path("notes");
        let today = path("2026/today.md");

        assert!(root.is_root());
        assert_eq!(root.as_str(), "");
        assert_eq!(root.to_string(), "/");
        assert_eq!(today.depth(), 2);
        assert_eq!(today.file_name(), Some("today.md"));
        assert_eq!(today.parent(), Some(path("2026")));
        assert_eq!(notes.join(&today).unwrap(), path("notes/2026/today.md"));
        assert!(today.starts_with(&path("2026")));
        assert!(today.starts_with(&root));
    }

    #[test]
    fn rejects_absolute_traversal_duplicate_separator_and_backslash_paths() {
        for (value, expected) in [
            ("/notes/today.md", PathError::Absolute),
            ("../secret.md", PathError::Traversal),
            ("notes/../secret.md", PathError::Traversal),
            ("notes//today.md", PathError::EmptySegment),
            ("notes\\today.md", PathError::Backslash),
            ("notes/./today.md", PathError::Traversal),
        ] {
            assert_eq!(
                VaultPath::parse(value).unwrap_err(),
                DomainError::InvalidPath(expected)
            );
        }
    }

    #[test]
    fn decodes_a_url_path_once_and_rejects_encoded_traversal() {
        assert_eq!(
            VaultPath::from_url_path("/notes%2Ftoday.md").unwrap(),
            path("notes/today.md")
        );
        assert_eq!(VaultPath::from_url_path("/").unwrap(), VaultPath::root());
        assert_eq!(
            VaultPath::from_url_path("notes/today.md").unwrap(),
            path("notes/today.md")
        );

        for value in [
            "/notes/%2e%2e/secret.md",
            "/notes/%2Fsecret.md",
            "/notes/%5Csecret.md",
            "/notes/%252e%252e/secret.md",
        ] {
            assert!(matches!(
                VaultPath::from_url_path(value),
                Err(DomainError::InvalidPath(_))
            ));
        }

        assert_eq!(
            VaultPath::parse("notes/%2e%2e/secret.md").unwrap_err(),
            DomainError::InvalidPath(PathError::EncodedUnsafeComponent)
        );
        assert_eq!(
            VaultPath::from_url_path("/notes/%zz.md").unwrap_err(),
            DomainError::InvalidPath(PathError::InvalidPercentEncoding)
        );
    }

    #[test]
    fn normalizes_unicode_to_nfc_before_identity_comparison() {
        let decomposed = VaultPath::parse("cafe\u{301}/notes.md").unwrap();
        let composed = VaultPath::parse("caf\u{e9}/notes.md").unwrap();

        assert_eq!(decomposed, composed);
        assert_eq!(decomposed.as_str(), "caf\u{e9}/notes.md");
    }

    #[test]
    fn enforces_cross_platform_segment_policy() {
        for value in [
            "CON.md",
            "notes/NUL.txt",
            "notes/trailing-space ",
            "notes/trailing-period.",
            "notes/a:b.md",
            "notes/a*b.md",
            "notes/a?b.md",
        ] {
            assert!(VaultPath::parse(value).is_err(), "{value}");
        }

        assert_eq!(
            VaultPath::parse("notes/normal.md\0").unwrap_err(),
            DomainError::InvalidPath(PathError::Nul)
        );
    }

    #[test]
    fn detects_case_collisions_only_when_policy_requires_it() {
        let upper = path("Notes/Readme.md");
        let lower = path("notes/readme.md");
        let paths = [&upper, &lower];

        assert!(
            detect_path_collisions(paths.iter().copied(), PathCaseSensitivity::Sensitive).is_ok()
        );
        assert!(matches!(
            detect_path_collisions(paths.iter().copied(), PathCaseSensitivity::Insensitive),
            Err(DomainError::PathCollision { .. })
        ));
    }

    #[test]
    fn reserved_namespace_requires_explicit_managed_access() {
        let policy = VaultPathPolicy::default();
        let reserved = path("_mcp-vault/memory/record.md");
        let reserved_with_different_case = path("_MCP-VAULT/memory/record.md");
        let ordinary = path("notes/record.md");

        assert!(policy.validate_user_path(&ordinary).is_ok());
        assert!(matches!(
            policy.validate_user_path(&reserved),
            Err(DomainError::ReservedPath(_))
        ));
        assert!(matches!(
            policy.validate_user_path(&reserved_with_different_case),
            Err(DomainError::ReservedPath(_))
        ));
        assert!(policy.validate_managed_path(&reserved).is_ok());
        assert_eq!(
            policy.validate_managed_path(&ordinary).unwrap_err(),
            DomainError::PathNotManaged
        );
    }

    #[test]
    fn filesystem_policy_denies_symlinks_and_special_files_by_default() {
        let policy = FilesystemPolicy::default();

        assert!(
            policy
                .validate_entry_kind(FilesystemEntryKind::RegularFile)
                .is_ok()
        );
        assert!(
            policy
                .validate_entry_kind(FilesystemEntryKind::Directory)
                .is_ok()
        );
        assert!(matches!(
            policy.validate_entry_kind(FilesystemEntryKind::Symlink),
            Err(DomainError::UnsafeFilesystemEntry { kind: "symlink" })
        ));
        assert!(
            policy
                .validate_entry_kind(FilesystemEntryKind::Socket)
                .is_err()
        );
        assert!(
            FilesystemPolicy::new(true, false)
                .validate_entry_kind(FilesystemEntryKind::Symlink)
                .is_ok()
        );
        assert!(
            FilesystemPolicy::new(false, true)
                .validate_entry_kind(FilesystemEntryKind::Symlink)
                .is_err()
        );
    }
}
