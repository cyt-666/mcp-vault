//! Vault registry records and Vault-scoped repository operations.

use std::{path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

use mcp_vault_domain::{
    JobId, Revision, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug,
};

use crate::{JobRecord, JobStatus, StateError, now_millis};

/// Typed system-setting key that preserves the Vault targeted by legacy
/// unscoped Admin routes after multi-Vault management is enabled.
pub const LEGACY_DEFAULT_VAULT_SETTING: &str = "vault.legacy_default_id";

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

/// Effective data-plane availability derived from registry state and the
/// durable managed-Vault initialization job.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultAvailability {
    /// Initial reconciliation/index construction is still durable work.
    Initializing,
    /// The Vault accepts its normal data-plane behavior.
    Ready,
    /// Reads may continue but writes are blocked.
    Maintenance,
    /// The Vault is deliberately unavailable.
    Disabled,
    /// Initialization or registered Vault health needs operator attention.
    Error,
}

impl VaultAvailability {
    /// Stable API label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Maintenance => "maintenance",
            Self::Disabled => "disabled",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for VaultAvailability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
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

    /// Atomically register a managed Vault, create its initialization job, and
    /// establish the legacy default only when no earlier Vault owns it.
    pub async fn insert_managed_with_initialization(
        &self,
        context: &VaultContext,
        name: &str,
    ) -> Result<(VaultRecord, JobRecord), StateError> {
        validate_name(name)?;
        let policy = VaultPathPolicy::default();
        let content_root = context
            .content_root()
            .to_str()
            .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
        let reserved_root = policy.reserved_root().as_str();
        let timestamp = now_millis()?;
        let vault_id = context.id().to_string();
        let job_id = JobId::new();
        let dedup_key = format!("vault:{}:initialize", context.id());
        let payload = serde_json::json!({"reason": "managed_vault_created"});
        let payload_json = serde_json::to_string(&payload)?;
        let legacy_value = serde_json::to_string(&vault_id)?;
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO vaults
             (id, slug, name, content_root, reserved_root, status,
              created_at, updated_at, settings_revision)
             VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?)",
        )
        .bind(&vault_id)
        .bind(context.slug().as_str())
        .bind(name)
        .bind(content_root)
        .bind(reserved_root)
        .bind(timestamp)
        .bind(timestamp)
        .bind(context.settings_revision().as_i64()?)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO jobs
             (id, vault_id, job_type, dedup_key, payload_json, status,
              priority, max_attempts, available_at, created_at, updated_at)
             VALUES (?, ?, 'vault.initialize', ?, ?, 'queued', 20, 10, 0, ?, ?)",
        )
        .bind(job_id.to_string())
        .bind(&vault_id)
        .bind(&dedup_key)
        .bind(payload_json)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO system_settings
             (key, value_json, revision, updated_at, updated_by)
             VALUES (?, ?, 1, ?, NULL)
             ON CONFLICT(key) DO NOTHING",
        )
        .bind(LEGACY_DEFAULT_VAULT_SETTING)
        .bind(legacy_value)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok((
            VaultRecord {
                id: context.id(),
                slug: context.slug().clone(),
                name: name.to_owned(),
                content_root: context.content_root().to_owned(),
                reserved_root: policy.reserved_root().clone(),
                status: VaultStatus::Active,
                created_at: timestamp,
                updated_at: timestamp,
                settings_revision: context.settings_revision(),
            },
            JobRecord {
                id: job_id,
                vault_id: Some(context.id()),
                job_type: "vault.initialize".to_owned(),
                dedup_key,
                payload,
                status: JobStatus::Queued,
                priority: 20,
                attempts: 0,
                max_attempts: 10,
                available_at: 0,
                lease_owner: None,
                lease_until: None,
                progress: None,
                last_error: None,
                created_at: timestamp,
                updated_at: timestamp,
                completed_at: None,
                cancel_requested: false,
            },
        ))
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

    /// Resolve effective availability without treating optional Provider or
    /// derived-feature degradation as a canonical Vault outage.
    pub async fn availability(&self, vault: &VaultRecord) -> Result<VaultAvailability, StateError> {
        match vault.status {
            VaultStatus::Maintenance => return Ok(VaultAvailability::Maintenance),
            VaultStatus::Disabled => return Ok(VaultAvailability::Disabled),
            VaultStatus::Error => return Ok(VaultAvailability::Error),
            VaultStatus::Active => {}
        }
        let status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM jobs
             WHERE vault_id = ? AND dedup_key = ?
             LIMIT 1",
        )
        .bind(vault.id.to_string())
        .bind(format!("vault:{}:initialize", vault.id))
        .fetch_optional(&self.pool)
        .await?;
        Ok(match status.as_deref() {
            Some("queued" | "running" | "retry_wait") => VaultAvailability::Initializing,
            Some("failed" | "cancelled") => VaultAvailability::Error,
            Some("completed") | None => VaultAvailability::Ready,
            Some(_) => {
                return Err(StateError::InvalidInput(
                    "stored initialization job status is invalid",
                ));
            }
        })
    }

    /// Resolve the stable Vault used by legacy unscoped Admin routes.
    ///
    /// Existing installations did not persist an explicit default because
    /// they exposed one Vault. The first resolution therefore prefers the
    /// historical `default` slug, otherwise the sole registered Vault. Once
    /// chosen, the ID is persisted and adding another Vault cannot change it.
    pub async fn legacy_default(&self) -> Result<Option<VaultRecord>, StateError> {
        if let Some(record) = self.stored_legacy_default().await? {
            return Ok(Some(record));
        }

        let candidate = match self
            .find_by_slug(&VaultSlug::new("default").expect("default slug is valid"))
            .await?
        {
            Some(record) => Some(record),
            None => {
                let mut rows = sqlx::query_as::<_, VaultRow>(
                    "SELECT id, slug, name, content_root, reserved_root, status,
                            created_at, updated_at, settings_revision
                     FROM vaults
                     ORDER BY slug ASC
                     LIMIT 2",
                )
                .fetch_all(&self.pool)
                .await?;
                if rows.len() == 1 {
                    Some(row_to_record(rows.remove(0))?)
                } else {
                    None
                }
            }
        };

        let Some(candidate) = candidate else {
            return Ok(None);
        };
        self.ensure_legacy_default(&candidate.context()?).await?;
        self.stored_legacy_default().await
    }

    /// Persist the legacy default if no earlier caller has selected one.
    ///
    /// The insert is create-only so concurrent setup or Vault creation has one
    /// stable winner. The returned record is the persisted winner, which can
    /// differ from `context` only when another caller committed first.
    pub async fn ensure_legacy_default(
        &self,
        context: &VaultContext,
    ) -> Result<VaultRecord, StateError> {
        let registered = self
            .find_by_id(context.id())
            .await?
            .ok_or(StateError::InvalidInput("Vault context is not registered"))?;
        if registered.slug != *context.slug() || registered.content_root != context.content_root() {
            return Err(StateError::InvalidInput(
                "Vault context does not match registered state",
            ));
        }

        let timestamp = now_millis()?;
        let value_json = serde_json::to_string(&context.id().to_string())?;
        sqlx::query(
            "INSERT INTO system_settings
             (key, value_json, revision, updated_at, updated_by)
             VALUES (?, ?, 1, ?, NULL)
             ON CONFLICT(key) DO NOTHING",
        )
        .bind(LEGACY_DEFAULT_VAULT_SETTING)
        .bind(value_json)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;

        self.stored_legacy_default()
            .await?
            .ok_or(StateError::InvalidInput(
                "legacy default Vault setting was not saved",
            ))
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

    async fn stored_legacy_default(&self) -> Result<Option<VaultRecord>, StateError> {
        let value_json =
            sqlx::query_scalar::<_, String>("SELECT value_json FROM system_settings WHERE key = ?")
                .bind(LEGACY_DEFAULT_VAULT_SETTING)
                .fetch_optional(&self.pool)
                .await?;
        let Some(value_json) = value_json else {
            return Ok(None);
        };
        let id = serde_json::from_str::<String>(&value_json)
            .map_err(StateError::Json)
            .and_then(|value| VaultId::parse(&value).map_err(StateError::InvalidDomain))?;
        self.find_by_id(id)
            .await?
            .map(Some)
            .ok_or(StateError::InvalidInput(
                "legacy default Vault is not registered",
            ))
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
