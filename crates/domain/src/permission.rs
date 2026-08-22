//! Protocol-neutral scopes and application permissions.

use std::{collections::BTreeSet, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::DomainError;

/// A stable MCP authorization scope.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// Discover Vault structure and metadata.
    #[serde(rename = "vault:discover")]
    VaultDiscover,
    /// Read Vault files and deterministic projections.
    #[serde(rename = "vault:read")]
    VaultRead,
    /// Create or modify Vault files.
    #[serde(rename = "vault:write")]
    VaultWrite,
    /// Delete Vault files.
    #[serde(rename = "vault:delete")]
    VaultDelete,
    /// Read file revision history.
    #[serde(rename = "vault:history")]
    VaultHistory,
    /// Read durable memories.
    #[serde(rename = "memory:read")]
    MemoryRead,
    /// Create or update durable memories.
    #[serde(rename = "memory:write")]
    MemoryWrite,
    /// Manage memory lifecycle and candidates.
    #[serde(rename = "memory:manage")]
    MemoryManage,
}

impl Scope {
    /// All scopes in deterministic output order.
    pub const ALL: [Self; 8] = [
        Self::VaultDiscover,
        Self::VaultRead,
        Self::VaultWrite,
        Self::VaultDelete,
        Self::VaultHistory,
        Self::MemoryRead,
        Self::MemoryWrite,
        Self::MemoryManage,
    ];

    /// Map an external scope to the application capability it grants.
    pub const fn permission(self) -> Permission {
        match self {
            Self::VaultDiscover => Permission::DiscoverVault,
            Self::VaultRead => Permission::ReadVault,
            Self::VaultWrite => Permission::WriteVault,
            Self::VaultDelete => Permission::DeleteVault,
            Self::VaultHistory => Permission::ReadHistory,
            Self::MemoryRead => Permission::ReadMemory,
            Self::MemoryWrite => Permission::WriteMemory,
            Self::MemoryManage => Permission::ManageMemory,
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::VaultDiscover => "vault:discover",
            Self::VaultRead => "vault:read",
            Self::VaultWrite => "vault:write",
            Self::VaultDelete => "vault:delete",
            Self::VaultHistory => "vault:history",
            Self::MemoryRead => "memory:read",
            Self::MemoryWrite => "memory:write",
            Self::MemoryManage => "memory:manage",
        };
        formatter.write_str(value)
    }
}

impl FromStr for Scope {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "vault:discover" => Ok(Self::VaultDiscover),
            "vault:read" => Ok(Self::VaultRead),
            "vault:write" => Ok(Self::VaultWrite),
            "vault:delete" => Ok(Self::VaultDelete),
            "vault:history" => Ok(Self::VaultHistory),
            "memory:read" => Ok(Self::MemoryRead),
            "memory:write" => Ok(Self::MemoryWrite),
            "memory:manage" => Ok(Self::MemoryManage),
            _ => Err(DomainError::InvalidValue {
                kind: "scope",
                reason: "is not supported",
            }),
        }
    }
}

/// Application capability independent of how a caller was authenticated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Read Vault discovery metadata.
    DiscoverVault,
    /// Read Vault content.
    ReadVault,
    /// Write Vault content.
    WriteVault,
    /// Delete Vault content.
    DeleteVault,
    /// Read revision history.
    ReadHistory,
    /// Read durable memories.
    ReadMemory,
    /// Write durable memories.
    WriteMemory,
    /// Manage memory lifecycle.
    ManageMemory,
}

/// Deterministic set of external scopes.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ScopeSet(BTreeSet<Scope>);

impl ScopeSet {
    /// Create an empty scope set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a scope and report whether it was new.
    pub fn insert(&mut self, scope: Scope) -> bool {
        self.0.insert(scope)
    }

    /// Return whether a scope is present.
    pub fn contains(&self, scope: Scope) -> bool {
        self.0.contains(&scope)
    }

    /// Iterate in deterministic enum order.
    pub fn iter(&self) -> impl Iterator<Item = &Scope> {
        self.0.iter()
    }

    /// Convert all scopes into application permissions.
    pub fn permissions(&self) -> PermissionSet {
        PermissionSet(self.0.iter().map(|scope| scope.permission()).collect())
    }
}

impl FromIterator<Scope> for ScopeSet {
    fn from_iter<T: IntoIterator<Item = Scope>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// Deterministic set of application capabilities.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PermissionSet(BTreeSet<Permission>);

impl PermissionSet {
    /// Create an empty permission set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a permission and report whether it was new.
    pub fn insert(&mut self, permission: Permission) -> bool {
        self.0.insert(permission)
    }

    /// Return whether a capability is present.
    pub fn contains(&self, permission: Permission) -> bool {
        self.0.contains(&permission)
    }

    /// Iterate in deterministic enum order.
    pub fn iter(&self) -> impl Iterator<Item = &Permission> {
        self.0.iter()
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<T: IntoIterator<Item = Permission>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{Permission, Scope, ScopeSet};

    #[test]
    fn scopes_have_stable_wire_names_and_permissions() {
        assert_eq!(Scope::VaultDiscover.to_string(), "vault:discover");
        assert_eq!(
            Scope::from_str("memory:manage").unwrap(),
            Scope::MemoryManage
        );

        let scopes: ScopeSet = [Scope::VaultRead, Scope::MemoryRead].into_iter().collect();
        let permissions = scopes.permissions();
        assert!(permissions.contains(Permission::ReadVault));
        assert!(permissions.contains(Permission::ReadMemory));
    }

    #[test]
    fn all_scope_order_is_deterministic() {
        let names = Scope::ALL.map(|scope| scope.to_string());

        assert_eq!(
            names,
            [
                "vault:discover",
                "vault:read",
                "vault:write",
                "vault:delete",
                "vault:history",
                "memory:read",
                "memory:write",
                "memory:manage",
            ]
        );
    }
}
