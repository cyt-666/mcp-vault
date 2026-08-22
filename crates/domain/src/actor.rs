//! Auditable actor and source-plane values.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::DomainError;

const MAX_ACTOR_ID_BYTES: usize = 256;

/// Non-secret category of caller or background component.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// Authenticated Admin user.
    Admin,
    /// Dedicated WebDAV app credential.
    #[serde(rename = "webdav_credential")]
    WebDavCredential,
    /// MCP personal access token.
    #[serde(rename = "mcp_pat")]
    McpPat,
    /// MCP OAuth subject grant.
    #[serde(rename = "mcp_oauth_subject")]
    McpOAuthSubject,
    /// Filesystem reconciliation worker.
    Reconciler,
    /// Memory materialization or lifecycle worker.
    MemoryWorker,
    /// Service bootstrap or maintenance action.
    System,
}

/// Transport/application plane from which an operation originated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePlane {
    /// Admin control plane.
    Admin,
    /// WebDAV data-plane adapter.
    #[serde(rename = "webdav")]
    WebDav,
    /// MCP data-plane adapter.
    Mcp,
    /// Out-of-band filesystem reconciliation.
    Reconciliation,
    /// Memory service materialization.
    Memory,
    /// Internal system operation.
    System,
}

impl SourcePlane {
    /// Return a stable storage/audit label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::WebDav => "webdav",
            Self::Mcp => "mcp",
            Self::Reconciliation => "reconciliation",
            Self::Memory => "memory",
            Self::System => "system",
        }
    }
}

impl fmt::Display for SourcePlane {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SourcePlane {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "admin" => Ok(Self::Admin),
            "webdav" => Ok(Self::WebDav),
            "mcp" => Ok(Self::Mcp),
            "reconciliation" => Ok(Self::Reconciliation),
            "memory" => Ok(Self::Memory),
            "system" => Ok(Self::System),
            _ => Err(DomainError::InvalidValue {
                kind: "source plane",
                reason: "is not supported",
            }),
        }
    }
}

/// Opaque, non-empty audit actor identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActorId(String);

impl ActorId {
    /// Validate and construct an actor identifier.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() {
            return Err(DomainError::InvalidValue {
                kind: "actor ID",
                reason: "must not be empty",
            });
        }
        if value.len() > MAX_ACTOR_ID_BYTES {
            return Err(DomainError::InvalidValue {
                kind: "actor ID",
                reason: "is too long",
            });
        }
        if value.chars().any(char::is_control) {
            return Err(DomainError::InvalidValue {
                kind: "actor ID",
                reason: "must not contain control characters",
            });
        }

        Ok(Self(value.to_owned()))
    }

    /// Return the identifier for repository/audit storage.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ActorId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ActorId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ActorId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(D::Error::custom)
    }
}

/// Actor provenance attached to revisions, jobs, and audit facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Actor {
    actor_type: ActorType,
    actor_id: Option<ActorId>,
}

impl Actor {
    /// Construct an actor with an optional non-secret identifier.
    pub fn new(actor_type: ActorType, actor_id: Option<ActorId>) -> Self {
        Self {
            actor_type,
            actor_id,
        }
    }

    /// Construct an identified actor.
    pub fn identified(actor_type: ActorType, actor_id: ActorId) -> Self {
        Self::new(actor_type, Some(actor_id))
    }

    /// Construct an internal system actor.
    pub fn system() -> Self {
        Self::new(ActorType::System, None)
    }

    /// Return the actor category.
    pub const fn actor_type(&self) -> ActorType {
        self.actor_type
    }

    /// Return the optional audit identifier.
    pub fn actor_id(&self) -> Option<&ActorId> {
        self.actor_id.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{Actor, ActorId, ActorType, SourcePlane};

    #[test]
    fn source_planes_have_stable_storage_labels() {
        assert_eq!(SourcePlane::WebDav.to_string(), "webdav");
        assert_eq!(SourcePlane::from_str("mcp").unwrap(), SourcePlane::Mcp);
        assert!(SourcePlane::from_str("unknown").is_err());
    }

    #[test]
    fn actor_ids_are_validated_and_actor_provenance_is_typed() {
        assert!(ActorId::new("").is_err());
        assert!(ActorId::new("user-42").is_ok());

        let actor = Actor::identified(ActorType::McpPat, ActorId::new("credential-42").unwrap());
        assert_eq!(actor.actor_type(), ActorType::McpPat);
        assert_eq!(actor.actor_id().unwrap().as_str(), "credential-42");
        assert!(Actor::system().actor_id().is_none());
    }
}
