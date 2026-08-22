//! Vault registry records and Vault-scoped repository operations.

use std::{path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug};

use crate::{StateError, now_millis};

/// Lifecycle status stored by the Vault registry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultStatus {
    /// Normal operation.
    Active,
    /// Reads may continue but writes are coordinated by maintenance.
    Maintenance,
    /// Vault is disabled by administration.
    Disabled,
    /// Vault configuration or reconciliation needs operator attention.
    Error,
}

impl VaultStatus {
    /// Return the stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Maintenance => "maintenance",
            Self::Disabled => "disabled",
            Self::Error => "error",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "active" => Ok(Self::Active),
            "maintenance" => Ok(Self::Maintenance),
            "disabled" => Ok(Self::Disabled),
            "error" => Ok(Self::Error),
            _ => Err(StateError::InvalidInput("stored Vault status is invalid")),
        }
    }
}

impl std::fmt::Display for VaultStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Decoded Vault registry row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultRecord {
    /// Stable Vault ID.
    pub id: VaultId,
    /// Endpoint slug.
    pub slug: VaultSlug,
    /// Human-readable display name.
    pub name: String,
    /// Absolute canonical content root.
    pub content_root: PathBuf,
    /// Reserved managed namespace.
    pub reserved_root: VaultPath,
    /// Operational lifecycle status.
    pub status: VaultStatus,
    /// Creation timestamp in UTC Unix milliseconds.
    pub created_at: i64,
    /// Last registry update timestamp.
    pub updated_at: i64,
    /// Settings revision used by the current context.
    pub settings_revision: Revision,
}

impl VaultRecord {
    /// Reconstruct a validated domain context from the stored registry row.
    pub fn context(&self) -> Result<VaultContext, StateError> {
        Ok(VaultContext::new(
            self.id,
            self.slug.clone(),
            self.content_root.clone(),
            self.settings_revision,
        )?)
    }
}

#[derive(Debug, FromRow)]
struct VaultRow {
    id: String,
    slug: String,
    name: String,
    content_root: String,
    reserved_root: String,
    status: String,
    created_at: i64,
    updated_at: i64,
    settings_revision: i64,
}

/// Repository for Vault registry state.
#[derive(Clone)]
pub struct VaultRepository {
    pool: SqlitePool,
}

impl VaultRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a Vault registry row bound to a validated context.
    pub async fn insert(
        &self,
        context: &VaultContext,
        name: &str,
        status: VaultStatus,
    ) -> Result<VaultRecord, StateError> {
        let default_policy = VaultPathPolicy::default();
        self.insert_with_reserved_root(context, name, default_policy.reserved_root(), status)
            .await
    }

    /// Insert a Vault with an explicit managed namespace root.
    pub async fn insert_with_reserved_root(
        &self,
        context: &VaultContext,
        name: &str,
        reserved_root: &VaultPath,
        status: VaultStatus,
    ) -> Result<VaultRecord, StateError> {
        validate_name(name)?;
        VaultPathPolicy::new(reserved_root.clone(), Default::default())?;
        let content_root = context
            .content_root()
            .to_str()
            .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
        let reserved_root = reserved_root.as_str().to_owned();
        let timestamp = now_millis()?;

        sqlx::query(
            "INSERT INTO vaults
             (id, slug, name, content_root, reserved_root, status,
              created_at, updated_at, settings_revision)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .bind(name)
        .bind(content_root)
        .bind(&reserved_root)
        .bind(status.as_str())
        .bind(timestamp)
        .bind(timestamp)
        .bind(context.settings_revision().as_i64()?)
        .execute(&self.pool)
        .await?;

        Ok(VaultRecord {
            id: context.id(),
            slug: context.slug().clone(),
            name: name.to_owned(),
            content_root: context.content_root().to_owned(),
            reserved_root: VaultPath::parse(&reserved_root)?,
            status,
            created_at: timestamp,
            updated_at: timestamp,
            settings_revision: context.settings_revision(),
        })
    }

    /// Find a Vault by its typed ID.
    pub async fn find_by_id(&self, id: VaultId) -> Result<Option<VaultRecord>, StateError> {
        let row = sqlx::query_as::<_, VaultRow>(
            "SELECT id, slug, name, content_root, reserved_root, status,
                    created_at, updated_at, settings_revision
             FROM vaults
             WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_record).transpose()
    }

    /// Find a Vault by its validated endpoint slug.
    pub async fn find_by_slug(&self, slug: &VaultSlug) -> Result<Option<VaultRecord>, StateError> {
        let row = sqlx::query_as::<_, VaultRow>(
            "SELECT id, slug, name, content_root, reserved_root, status,
                    created_at, updated_at, settings_revision
             FROM vaults
             WHERE slug = ?",
        )
        .bind(slug.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_record).transpose()
    }

    /// List registry rows in deterministic slug order.
    pub async fn list(&self) -> Result<Vec<VaultRecord>, StateError> {
        let rows = sqlx::query_as::<_, VaultRow>(
            "SELECT id, slug, name, content_root, reserved_root, status,
                    created_at, updated_at, settings_revision
             FROM vaults
             ORDER BY slug ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_record).collect()
    }

    /// Update status only for the Vault represented by the supplied context.
    pub async fn set_status(
        &self,
        context: &VaultContext,
        status: VaultStatus,
    ) -> Result<(), StateError> {
        let updated_at = now_millis()?;
        let result = sqlx::query(
            "UPDATE vaults
             SET status = ?, settings_revision = settings_revision + 1, updated_at = ?
             WHERE id = ? AND slug = ?",
        )
        .bind(status.as_str())
        .bind(updated_at)
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("Vault context is not registered"));
        }

        Ok(())
    }

    /// Update the display name while preserving the registered Vault binding.
    pub async fn update_name(&self, context: &VaultContext, name: &str) -> Result<(), StateError> {
        validate_name(name)?;
        let updated_at = now_millis()?;
        let result = sqlx::query(
            "UPDATE vaults
             SET name = ?, settings_revision = settings_revision + 1, updated_at = ?
             WHERE id = ? AND slug = ?",
        )
        .bind(name)
        .bind(updated_at)
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("Vault context is not registered"));
        }
        Ok(())
    }
}

fn row_to_record(row: VaultRow) -> Result<VaultRecord, StateError> {
    Ok(VaultRecord {
        id: VaultId::parse(&row.id)?,
        slug: VaultSlug::new(&row.slug)?,
        name: row.name,
        content_root: PathBuf::from(row.content_root),
        reserved_root: VaultPath::parse(&row.reserved_root)?,
        status: VaultStatus::parse(&row.status)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
        settings_revision: Revision::try_from(row.settings_revision)?,
    })
}

fn validate_name(name: &str) -> Result<(), StateError> {
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(StateError::InvalidInput("Vault name is invalid"));
    }

    Ok(())
}

impl FromStr for VaultStatus {
    type Err = StateError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
