//! UUIDv7-backed identifiers with distinct Rust types per aggregate.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::DomainError;

macro_rules! typed_uuid_id {
    ($(#[$meta:meta])* $name:ident, $label:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a time-ordered UUIDv7 identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Construct the typed identifier from an existing UUID.
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Return the underlying UUID without changing its type at call
            /// sites.
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }

            /// Parse a canonical UUID string into this identifier category.
            pub fn parse(value: &str) -> Result<Self, DomainError> {
                value.parse()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::from_uuid(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<Uuid> for $name {
            fn as_ref(&self) -> &Uuid {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value)
                    .map(Self)
                    .map_err(|_| DomainError::InvalidValue {
                        kind: $label,
                        reason: "must be a UUID",
                    })
            }
        }
    };
}

typed_uuid_id!(VaultId, "Vault ID");
typed_uuid_id!(FileId, "file ID");
typed_uuid_id!(RevisionId, "revision ID");
typed_uuid_id!(MemoryId, "memory ID");
typed_uuid_id!(IdentityId, "identity ID");
typed_uuid_id!(AdminUserId, "Admin user ID");
typed_uuid_id!(AdminSessionId, "Admin session ID");
typed_uuid_id!(CredentialId, "credential ID");
typed_uuid_id!(SecretId, "secret ID");
typed_uuid_id!(OAuthIssuerId, "OAuth issuer ID");
typed_uuid_id!(OAuthGrantId, "OAuth grant ID");
typed_uuid_id!(ScanId, "scan ID");
typed_uuid_id!(JobId, "job ID");
typed_uuid_id!(OperationId, "operation ID");
typed_uuid_id!(EventId, "event ID");
typed_uuid_id!(ProviderId, "provider ID");
typed_uuid_id!(ModelId, "model ID");
typed_uuid_id!(EmbeddingId, "embedding ID");
typed_uuid_id!(MemoryCandidateId, "memory candidate ID");
typed_uuid_id!(MemorySourceId, "memory source ID");
typed_uuid_id!(MemoryRelationId, "memory relation ID");
typed_uuid_id!(BackupId, "backup ID");

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use uuid::Uuid;

    use super::{FileId, MemoryId, VaultId};
    use crate::DomainError;

    #[test]
    fn identifier_categories_are_not_interchangeable() {
        let uuid = Uuid::from_u128(1);
        let vault = VaultId::from_uuid(uuid);
        let file = FileId::from_uuid(uuid);
        let memory = MemoryId::from_uuid(uuid);

        assert_eq!(vault.as_uuid(), file.as_uuid());
        assert_eq!(file.as_uuid(), memory.as_uuid());
        assert_ne!(format!("{vault:?}"), format!("{file:?}"));
    }

    #[test]
    fn identifiers_round_trip_as_uuid_strings() {
        let original = VaultId::from_uuid(Uuid::from_u128(0x1234));
        let parsed = VaultId::from_str(&original.to_string()).unwrap();

        assert_eq!(parsed, original);
    }

    #[test]
    fn invalid_identifier_is_a_domain_error_without_raw_parser_leakage() {
        let error = VaultId::parse("not-a-uuid").unwrap_err();

        assert_eq!(
            error,
            DomainError::InvalidValue {
                kind: "Vault ID",
                reason: "must be a UUID",
            }
        );
    }
}
