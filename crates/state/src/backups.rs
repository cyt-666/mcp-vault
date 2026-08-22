//! Authoritative backup catalog repository.

use mcp_vault_domain::BackupId;
use serde_json::Value;
use sqlx::{FromRow, SqlitePool};

use crate::{StateError, now_millis};

/// Persistent lifecycle state for one service-owned backup artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupStatus {
    /// A durable backup job has been admitted but not started.
    Queued,
    /// The artifact is being written.
    Running,
    /// The artifact was written and passed verification.
    Completed,
    /// The artifact or verification failed without changing source state.
    Failed,
    /// An archive is being validated in private staging.
    Validating,
    /// A verified archive is being applied in maintenance mode.
    Restoring,
}

impl BackupStatus {
    /// Return the stable SQL/API label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Validating => "validating",
            Self::Restoring => "restoring",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "validating" => Ok(Self::Validating),
            "restoring" => Ok(Self::Restoring),
            _ => Err(StateError::InvalidInput("stored backup status is invalid")),
        }
    }
}

impl std::fmt::Display for BackupStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Typed catalog row. The location is a service-owned path, not a user path.
#[derive(Clone, Debug, PartialEq)]
pub struct BackupRecord {
    /// Stable backup identity.
    pub id: BackupId,
    /// Current lifecycle state.
    pub status: BackupStatus,
    /// Service-owned artifact location.
    pub location: String,
    /// Redacted manifest after a successful creation/validation.
    pub manifest: Option<Value>,
    /// Creation/start timestamp.
    pub started_at: i64,
    /// Completion timestamp, if terminal.
    pub completed_at: Option<i64>,
    /// Successful verification timestamp.
    pub verified_at: Option<i64>,
    /// Bounded non-secret failure code/message.
    pub error: Option<String>,
    /// Non-secret initiating Admin identity, when known.
    pub created_by: Option<String>,
}

#[derive(Debug, FromRow)]
struct BackupRow {
    id: String,
    status: String,
    location: String,
    manifest_json: Option<String>,
    started_at: i64,
    completed_at: Option<i64>,
    verified_at: Option<i64>,
    error: Option<String>,
    created_by: Option<String>,
}

/// SQL boundary for backup catalog state.
#[derive(Clone)]
pub struct BackupRepository {
    pool: SqlitePool,
}

impl BackupRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a queued operation with bounded service-owned metadata.
    pub async fn insert_queued(
        &self,
        id: BackupId,
        location: &str,
        created_by: Option<&str>,
    ) -> Result<BackupRecord, StateError> {
        validate_location(location)?;
        validate_actor(created_by)?;
        let now = now_millis()?;
        sqlx::query(
            "INSERT INTO backups
             (id, status, location, manifest_json, started_at, created_by)
             VALUES (?, 'queued', ?, NULL, ?, ?)",
        )
        .bind(id.to_string())
        .bind(location)
        .bind(now)
        .bind(created_by)
        .execute(&self.pool)
        .await?;
        self.get(id)
            .await?
            .ok_or(StateError::InvalidInput("inserted backup was not found"))
    }

    /// Transition a queued backup to running.
    pub async fn mark_running(&self, id: BackupId) -> Result<(), StateError> {
        let result = sqlx::query(
            "UPDATE backups SET status = 'running', started_at = ?, error = NULL
             WHERE id = ? AND status = 'queued'",
        )
        .bind(now_millis()?)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("backup is not queued"));
        }
        Ok(())
    }

    /// Mark a successfully created and verified artifact.
    pub async fn mark_completed(
        &self,
        id: BackupId,
        manifest: &Value,
        verified_at: i64,
    ) -> Result<(), StateError> {
        let manifest_json = bounded_json(manifest)?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE backups
             SET status = 'completed', manifest_json = ?, completed_at = ?,
                 verified_at = ?, error = NULL
             WHERE id = ? AND status = 'running'",
        )
        .bind(manifest_json)
        .bind(now)
        .bind(verified_at)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("backup is not running"));
        }
        Ok(())
    }

    /// Record a later successful verification without changing the artifact
    /// lifecycle state.
    pub async fn mark_verified(&self, id: BackupId, manifest: &Value) -> Result<(), StateError> {
        let manifest_json = bounded_json(manifest)?;
        let result = sqlx::query(
            "UPDATE backups SET manifest_json = ?, verified_at = ?, error = NULL
             WHERE id = ? AND status = 'completed'",
        )
        .bind(manifest_json)
        .bind(now_millis()?)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("backup is not completed"));
        }
        Ok(())
    }

    /// Mark an operation as failed with a redacted bounded code.
    pub async fn mark_failed(&self, id: BackupId, error: &str) -> Result<(), StateError> {
        validate_error(error)?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE backups SET status = 'failed', completed_at = ?, error = ?
             WHERE id = ? AND status IN ('queued', 'running', 'validating', 'restoring')",
        )
        .bind(now)
        .bind(error)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("backup is not active"));
        }
        Ok(())
    }

    /// Mark a catalog row as entering archive validation.
    pub async fn mark_validating(&self, id: BackupId) -> Result<(), StateError> {
        self.transition(id, BackupStatus::Validating, &["queued", "failed"])
            .await
    }

    /// Mark a catalog row as entering restore application.
    pub async fn mark_restoring(&self, id: BackupId) -> Result<(), StateError> {
        self.transition(
            id,
            BackupStatus::Restoring,
            &["validating", "completed", "running", "restoring"],
        )
        .await
    }

    /// Fetch one catalog row.
    pub async fn get(&self, id: BackupId) -> Result<Option<BackupRecord>, StateError> {
        let row = sqlx::query_as::<_, BackupRow>(
            "SELECT id, status, location, manifest_json, started_at,
                    completed_at, verified_at, error, created_by
             FROM backups WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_record).transpose()
    }

    /// List catalog rows in deterministic newest-first order.
    pub async fn list(&self, limit: u32, offset: u32) -> Result<Vec<BackupRecord>, StateError> {
        if limit == 0 || limit > 200 || offset > 1_000_000 {
            return Err(StateError::InvalidInput("backup page is invalid"));
        }
        let rows = sqlx::query_as::<_, BackupRow>(
            "SELECT id, status, location, manifest_json, started_at,
                    completed_at, verified_at, error, created_by
             FROM backups ORDER BY started_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_record).collect()
    }

    /// Delete a failed or expired catalog row after its artifact is removed.
    pub async fn delete(&self, id: BackupId) -> Result<(), StateError> {
        let result =
            sqlx::query("DELETE FROM backups WHERE id = ? AND status IN ('failed', 'completed')")
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("backup is not deletable"));
        }
        Ok(())
    }

    async fn transition(
        &self,
        id: BackupId,
        status: BackupStatus,
        allowed: &[&str],
    ) -> Result<(), StateError> {
        let placeholders = std::iter::repeat_n("?", allowed.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE backups SET status = ?, error = NULL WHERE id = ? AND status IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql).bind(status.as_str()).bind(id.to_string());
        for value in allowed {
            query = query.bind(*value);
        }
        let result = query.execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("backup transition is invalid"));
        }
        Ok(())
    }
}

fn row_to_record(row: BackupRow) -> Result<BackupRecord, StateError> {
    Ok(BackupRecord {
        id: BackupId::parse(&row.id)?,
        status: BackupStatus::parse(&row.status)?,
        location: row.location,
        manifest: row
            .manifest_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        started_at: row.started_at,
        completed_at: row.completed_at,
        verified_at: row.verified_at,
        error: row.error,
        created_by: row.created_by,
    })
}

fn validate_location(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(StateError::InvalidInput("backup location is invalid"));
    }
    Ok(())
}

fn validate_actor(value: Option<&str>) -> Result<(), StateError> {
    if value.is_some_and(|value| {
        value.is_empty() || value.len() > 256 || value.chars().any(char::is_control)
    }) {
        return Err(StateError::InvalidInput("backup actor is invalid"));
    }
    Ok(())
}

fn validate_error(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(StateError::InvalidInput("backup error is invalid"));
    }
    Ok(())
}

fn bounded_json(value: &Value) -> Result<String, StateError> {
    let json = serde_json::to_string(value)?;
    if json.len() > 1024 * 1024 {
        return Err(StateError::InvalidInput("backup manifest is too large"));
    }
    Ok(json)
}

#[cfg(test)]
mod tests {
    use mcp_vault_domain::BackupId;
    use serde_json::json;

    use super::BackupStatus;
    use crate::StateStore;

    #[tokio::test]
    async fn backup_catalog_supports_completion_verification_and_restore_resume() {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let repository = state.backups();
        let id = BackupId::new();
        repository
            .insert_queued(id, "/var/lib/mcp-vault/backups/test.tar", Some("admin"))
            .await
            .unwrap();
        repository.mark_running(id).await.unwrap();
        repository.mark_restoring(id).await.unwrap();
        assert_eq!(
            repository.get(id).await.unwrap().unwrap().status,
            BackupStatus::Restoring
        );
        repository
            .mark_failed(id, "process_restarted")
            .await
            .unwrap();
        assert_eq!(
            repository.get(id).await.unwrap().unwrap().status,
            BackupStatus::Failed
        );

        let completed = BackupId::new();
        repository
            .insert_queued(completed, "/var/lib/mcp-vault/backups/completed.tar", None)
            .await
            .unwrap();
        repository.mark_running(completed).await.unwrap();
        repository
            .mark_completed(completed, &json!({"format_version": 1}), 10)
            .await
            .unwrap();
        repository
            .mark_verified(completed, &json!({"format_version": 1}))
            .await
            .unwrap();
        let record = repository.get(completed).await.unwrap().unwrap();
        assert_eq!(record.status, BackupStatus::Completed);
        assert!(record.verified_at.is_some());
    }
}
