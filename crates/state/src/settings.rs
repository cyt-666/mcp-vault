//! Typed JSON settings repositories with optimistic revision checks.

use serde_json::Value;
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use mcp_vault_domain::{ActorId, DomainError, Revision, VaultContext, VaultId, WritePrecondition};

use crate::{StateError, now_millis};

/// A decoded settings row with its optional Vault scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingRecord {
    /// None for a global system setting, Some for a Vault setting.
    pub vault_id: Option<VaultId>,
    /// Typed settings key.
    pub key: String,
    /// Validated JSON payload.
    pub value: Value,
    /// Monotonic optimistic-concurrency revision.
    pub revision: Revision,
    /// UTC Unix milliseconds.
    pub updated_at: i64,
    /// Non-secret actor identifier, if the write had one.
    pub updated_by: Option<ActorId>,
}

#[derive(Debug, FromRow)]
struct SettingRow {
    key: String,
    value_json: String,
    revision: i64,
    updated_at: i64,
    updated_by: Option<String>,
}

/// Repository for global and Vault-scoped typed JSON settings.
#[derive(Clone)]
pub struct SettingsRepository {
    pool: SqlitePool,
}

struct SettingWrite<'a> {
    key: &'a str,
    value_json: &'a str,
    current: Option<SettingRow>,
    precondition: WritePrecondition,
    updated_at: i64,
    updated_by: Option<&'a ActorId>,
}

impl SettingsRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Read a global system setting.
    pub async fn get_system(&self, key: &str) -> Result<Option<SettingRecord>, StateError> {
        validate_key(key)?;
        let row = sqlx::query_as::<_, SettingRow>(
            "SELECT key, value_json, revision, updated_at, updated_by
             FROM system_settings
             WHERE key = ?",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| setting_row_to_record(row, None)).transpose()
    }

    /// Read a setting bound to the supplied Vault context.
    pub async fn get_vault(
        &self,
        context: &VaultContext,
        key: &str,
    ) -> Result<Option<SettingRecord>, StateError> {
        validate_key(key)?;
        let row = sqlx::query_as::<_, SettingRow>(
            "SELECT key, value_json, revision, updated_at, updated_by
             FROM vault_settings
             WHERE vault_id = ? AND key = ?",
        )
        .bind(context.id().to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| setting_row_to_record(row, Some(context.id())))
            .transpose()
    }

    /// Insert or update a global system setting under a write precondition.
    pub async fn set_system(
        &self,
        key: &str,
        value: &Value,
        precondition: WritePrecondition,
        updated_by: Option<&ActorId>,
    ) -> Result<SettingRecord, StateError> {
        validate_key(key)?;
        let value_json = serde_json::to_string(value)?;
        let updated_at = now_millis()?;
        let mut transaction = self.pool.begin().await?;
        let current = sqlx::query_as::<_, SettingRow>(
            "SELECT key, value_json, revision, updated_at, updated_by
             FROM system_settings
             WHERE key = ?",
        )
        .bind(key)
        .fetch_optional(&mut *transaction)
        .await?;
        let record = write_system_row(
            &mut transaction,
            SettingWrite {
                key,
                value_json: &value_json,
                current,
                precondition,
                updated_at,
                updated_by,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(record)
    }

    /// Insert or update a Vault setting under a write precondition.
    pub async fn set_vault(
        &self,
        context: &VaultContext,
        key: &str,
        value: &Value,
        precondition: WritePrecondition,
        updated_by: Option<&ActorId>,
    ) -> Result<SettingRecord, StateError> {
        validate_key(key)?;
        let value_json = serde_json::to_string(value)?;
        let updated_at = now_millis()?;
        let vault_id = context.id();
        let vault_id_string = vault_id.to_string();
        let mut transaction = self.pool.begin().await?;
        let current = sqlx::query_as::<_, SettingRow>(
            "SELECT key, value_json, revision, updated_at, updated_by
             FROM vault_settings
             WHERE vault_id = ? AND key = ?",
        )
        .bind(&vault_id_string)
        .bind(key)
        .fetch_optional(&mut *transaction)
        .await?;
        let record = write_vault_row(
            &mut transaction,
            &vault_id_string,
            vault_id,
            SettingWrite {
                key,
                value_json: &value_json,
                current,
                precondition,
                updated_at,
                updated_by,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(record)
    }
}

async fn write_system_row(
    transaction: &mut Transaction<'_, Sqlite>,
    input: SettingWrite<'_>,
) -> Result<SettingRecord, StateError> {
    let SettingWrite {
        key,
        value_json,
        current,
        precondition,
        updated_at,
        updated_by,
    } = input;
    let current_revision = current
        .as_ref()
        .map(|row| Revision::try_from(row.revision))
        .transpose()?;
    precondition.check(current_revision)?;
    let revision = next_setting_revision(current_revision)?;
    let updated_by = updated_by.map(ActorId::as_str);

    if let Some(current) = current {
        let result = sqlx::query(
            "UPDATE system_settings
             SET value_json = ?, revision = ?, updated_at = ?, updated_by = ?
             WHERE key = ? AND revision = ?",
        )
        .bind(value_json)
        .bind(revision.as_i64()?)
        .bind(updated_at)
        .bind(updated_by)
        .bind(key)
        .bind(current.revision)
        .execute(&mut **transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(DomainError::PreconditionFailed {
                reason: "setting changed during update",
            }
            .into());
        }
    } else {
        sqlx::query(
            "INSERT INTO system_settings
             (key, value_json, revision, updated_at, updated_by)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(key)
        .bind(value_json)
        .bind(revision.as_i64()?)
        .bind(updated_at)
        .bind(updated_by)
        .execute(&mut **transaction)
        .await?;
    }

    Ok(SettingRecord {
        vault_id: None,
        key: key.to_owned(),
        value: serde_json::from_str(value_json)?,
        revision,
        updated_at,
        updated_by: updated_by.map(ActorId::new).transpose()?,
    })
}

async fn write_vault_row(
    transaction: &mut Transaction<'_, Sqlite>,
    vault_id_string: &str,
    vault_id: VaultId,
    input: SettingWrite<'_>,
) -> Result<SettingRecord, StateError> {
    let SettingWrite {
        key,
        value_json,
        current,
        precondition,
        updated_at,
        updated_by,
    } = input;
    let current_revision = current
        .as_ref()
        .map(|row| Revision::try_from(row.revision))
        .transpose()?;
    precondition.check(current_revision)?;
    let revision = next_setting_revision(current_revision)?;
    let updated_by = updated_by.map(ActorId::as_str);

    if let Some(current) = current {
        let result = sqlx::query(
            "UPDATE vault_settings
             SET value_json = ?, revision = ?, updated_at = ?, updated_by = ?
             WHERE vault_id = ? AND key = ? AND revision = ?",
        )
        .bind(value_json)
        .bind(revision.as_i64()?)
        .bind(updated_at)
        .bind(updated_by)
        .bind(vault_id_string)
        .bind(key)
        .bind(current.revision)
        .execute(&mut **transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(DomainError::PreconditionFailed {
                reason: "setting changed during update",
            }
            .into());
        }
    } else {
        sqlx::query(
            "INSERT INTO vault_settings
             (vault_id, key, value_json, revision, updated_at, updated_by)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(vault_id_string)
        .bind(key)
        .bind(value_json)
        .bind(revision.as_i64()?)
        .bind(updated_at)
        .bind(updated_by)
        .execute(&mut **transaction)
        .await?;
    }

    Ok(SettingRecord {
        vault_id: Some(vault_id),
        key: key.to_owned(),
        value: serde_json::from_str(value_json)?,
        revision,
        updated_at,
        updated_by: updated_by.map(ActorId::new).transpose()?,
    })
}

fn setting_row_to_record(
    row: SettingRow,
    vault_id: Option<VaultId>,
) -> Result<SettingRecord, StateError> {
    Ok(SettingRecord {
        vault_id,
        key: row.key,
        value: serde_json::from_str(&row.value_json)?,
        revision: Revision::try_from(row.revision)?,
        updated_at: row.updated_at,
        updated_by: row.updated_by.as_deref().map(ActorId::new).transpose()?,
    })
}

fn validate_key(key: &str) -> Result<(), StateError> {
    if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
        return Err(StateError::InvalidInput("setting key is invalid"));
    }

    Ok(())
}

fn next_setting_revision(current: Option<Revision>) -> Result<Revision, StateError> {
    match current {
        Some(revision) => Ok(revision.next()?),
        None => Ok(Revision::new(1)),
    }
}
