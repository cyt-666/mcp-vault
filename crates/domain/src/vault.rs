//! Vault identity and immutable request-scoped context.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{DomainError, Revision, VaultId};

const MAX_SLUG_BYTES: usize = 64;

/// Stable, URL-safe identifier for a Vault endpoint.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VaultSlug(String);

impl VaultSlug {
    /// Validate and construct a lowercase ASCII slug.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() {
            return Err(DomainError::InvalidValue {
                kind: "Vault slug",
                reason: "must not be empty",
            });
        }
        if value.len() > MAX_SLUG_BYTES {
            return Err(DomainError::InvalidValue {
                kind: "Vault slug",
                reason: "is too long",
            });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(DomainError::InvalidValue {
                kind: "Vault slug",
                reason: "must contain only lowercase ASCII letters, digits, and hyphens",
            });
        }
        if !value.as_bytes()[0].is_ascii_lowercase() && !value.as_bytes()[0].is_ascii_digit() {
            return Err(DomainError::InvalidValue {
                kind: "Vault slug",
                reason: "must start with a letter or digit",
            });
        }
        if !value.as_bytes()[value.len() - 1].is_ascii_lowercase()
            && !value.as_bytes()[value.len() - 1].is_ascii_digit()
        {
            return Err(DomainError::InvalidValue {
                kind: "Vault slug",
                reason: "must end with a letter or digit",
            });
        }

        Ok(Self(value.to_owned()))
    }

    /// Return the canonical URL-safe slug.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for VaultSlug {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for VaultSlug {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for VaultSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for VaultSlug {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for VaultSlug {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(D::Error::custom)
    }
}

/// Immutable identity and root binding carried by every user-data operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultContext {
    id: VaultId,
    slug: VaultSlug,
    content_root: PathBuf,
    settings_revision: Revision,
}

impl VaultContext {
    /// Construct a context after validating the configured absolute root.
    ///
    /// This function intentionally does not call `canonicalize` or inspect the
    /// filesystem. Existence, ownership, and no-follow checks belong to the
    /// storage boundary.
    pub fn new(
        id: VaultId,
        slug: VaultSlug,
        content_root: PathBuf,
        settings_revision: Revision,
    ) -> Result<Self, DomainError> {
        let content_root = normalize_content_root(&content_root)?;

        Ok(Self {
            id,
            slug,
            content_root,
            settings_revision,
        })
    }

    /// Stable Vault identity.
    pub fn id(&self) -> VaultId {
        self.id
    }

    /// URL slug used to select the endpoint before authorization is checked.
    pub fn slug(&self) -> &VaultSlug {
        &self.slug
    }

    /// Normalized absolute canonical-content root.
    pub fn content_root(&self) -> &Path {
        &self.content_root
    }

    /// Revision of the Vault settings used to construct this context.
    pub fn settings_revision(&self) -> Revision {
        self.settings_revision
    }

    /// Return whether two contexts refer to the same Vault identity.
    pub fn is_same_vault(&self, other: &Self) -> bool {
        self.id == other.id
    }

    /// Enforce same-Vault use at an application boundary.
    pub fn ensure_same_vault(&self, other: &Self) -> Result<(), DomainError> {
        if self.is_same_vault(other) {
            Ok(())
        } else {
            Err(DomainError::VaultMismatch {
                expected: self.id,
                actual: other.id,
            })
        }
    }
}

fn normalize_content_root(path: &Path) -> Result<PathBuf, DomainError> {
    if !path.is_absolute() || path.as_os_str().is_empty() {
        return Err(DomainError::InvalidContentRoot);
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return Err(DomainError::InvalidContentRoot),
            Component::Normal(segment) => normalized.push(segment),
        }
    }

    if normalized.as_os_str().is_empty()
        || !normalized.is_absolute()
        || normalized.parent().is_none()
    {
        return Err(DomainError::InvalidContentRoot);
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use uuid::Uuid;

    use super::{VaultContext, VaultSlug};
    use crate::{DomainError, Revision, VaultId};

    #[test]
    fn slug_validation_is_url_safe_and_deterministic() {
        assert_eq!(VaultSlug::new("work-vault").unwrap().as_str(), "work-vault");
        for invalid in ["", "Work", "work_vault", "-work", "work-", "工作"] {
            assert!(VaultSlug::new(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn context_normalizes_dot_segments_without_touching_the_filesystem() {
        let context = VaultContext::new(
            VaultId::from_uuid(Uuid::from_u128(1)),
            VaultSlug::new("default").unwrap(),
            PathBuf::from("/srv/./vault"),
            Revision::new(7),
        )
        .unwrap();

        assert_eq!(
            context.content_root(),
            PathBuf::from("/srv/vault").as_path()
        );
        assert_eq!(context.settings_revision(), Revision::new(7));
    }

    #[test]
    fn context_rejects_relative_or_parent_roots() {
        let slug = VaultSlug::new("default").unwrap();
        let id = VaultId::from_uuid(Uuid::from_u128(1));

        for path in [
            PathBuf::from("vault"),
            PathBuf::from("/srv/../vault"),
            PathBuf::from("/"),
        ] {
            assert_eq!(
                VaultContext::new(id, slug.clone(), path, Revision::ZERO).unwrap_err(),
                DomainError::InvalidContentRoot
            );
        }
    }

    #[test]
    fn contexts_are_explicitly_vault_scoped() {
        let slug = VaultSlug::new("default").unwrap();
        let first = VaultContext::new(
            VaultId::from_uuid(Uuid::from_u128(1)),
            slug.clone(),
            PathBuf::from("/srv/first"),
            Revision::ZERO,
        )
        .unwrap();
        let second = VaultContext::new(
            VaultId::from_uuid(Uuid::from_u128(2)),
            slug,
            PathBuf::from("/srv/second"),
            Revision::ZERO,
        )
        .unwrap();

        assert!(!first.is_same_vault(&second));
        assert!(matches!(
            first.ensure_same_vault(&second),
            Err(DomainError::VaultMismatch { .. })
        ));
    }
}
