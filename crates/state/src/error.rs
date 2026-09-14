//! Typed errors and non-sensitive SQLite diagnostics.

use thiserror::Error;

use mcp_vault_domain::DomainError;

/// Errors raised by the operational state boundary.
#[derive(Debug, Error)]
pub enum StateError {
    /// SQLite/SQLx failed internally.
    #[error("state database error")]
    Database(#[from] sqlx::Error),
    /// An embedded migration failed or an applied migration was modified.
    #[error("state migration error")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// A stored value could not be converted into a domain value.
    #[error("invalid state value: {0}")]
    InvalidDomain(#[from] DomainError),
    /// JSON settings could not be serialized or validated.
    #[error("settings JSON error")]
    Json(#[from] serde_json::Error),
    /// A caller supplied an invalid operational value.
    #[error("invalid state input: {0}")]
    InvalidInput(&'static str),
    /// Compared operational input changed before a multi-step commit.
    #[error("state changed during operation")]
    Conflict,
    /// The connection URL or options could not be parsed.
    #[error("invalid state connection configuration: {0}")]
    Connection(String),
    /// Operational state directory preparation failed.
    #[error("state directory error: {0}")]
    Filesystem(String),
    /// The database reported an integrity violation.
    #[error("state database integrity check failed")]
    IntegrityFailure,
    /// A deterministic test commit hook stopped a metadata phase.
    #[error("state commit hook rejected phase: {0}")]
    CommitHook(&'static str),
}

impl StateError {
    /// Classify failures without returning driver messages, SQL, stored values,
    /// connection strings or filesystem paths.
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Database(sqlx::Error::Database(error)) => {
                let primary = error
                    .code()
                    .and_then(|code| code.parse::<u32>().ok())
                    .map(|code| code & 0xff);
                if matches!(primary, Some(5 | 6)) {
                    "state_database_busy"
                } else if error.is_unique_violation() {
                    "state_database_unique_violation"
                } else if error.is_foreign_key_violation() {
                    "state_database_foreign_key_violation"
                } else if error.is_check_violation() {
                    "state_database_check_violation"
                } else if primary == Some(13) {
                    "state_database_full"
                } else if primary == Some(8) {
                    "state_database_read_only"
                } else {
                    "state_database_error"
                }
            }
            Self::Database(sqlx::Error::PoolTimedOut) => "state_pool_timeout",
            Self::Database(sqlx::Error::PoolClosed) => "state_pool_closed",
            Self::Database(sqlx::Error::RowNotFound) => "state_row_missing",
            Self::Database(sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_)) => {
                "state_value_decode_error"
            }
            Self::Database(_) => "state_database_error",
            Self::Conflict => "state_revision_conflict",
            Self::InvalidInput(_) => "state_invalid_input",
            Self::InvalidDomain(_) => "state_invalid_domain",
            Self::Json(_) => "state_json_invalid",
            Self::Migration(_) => "state_migration_error",
            Self::Connection(_) => "state_connection_error",
            Self::Filesystem(_) => "state_filesystem_error",
            Self::IntegrityFailure => "state_integrity_error",
            Self::CommitHook(_) => "state_commit_interrupted",
        }
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[tokio::test]
    async fn database_diagnostics_never_return_driver_or_stored_text() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE private_diagnostic_fixture(value TEXT UNIQUE)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO private_diagnostic_fixture VALUES('private-example')")
            .execute(&pool)
            .await
            .unwrap();
        let error = sqlx::query("INSERT INTO private_diagnostic_fixture VALUES('private-example')")
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            StateError::Database(error).diagnostic_code(),
            "state_database_unique_violation"
        );
        assert_eq!(
            StateError::Database(sqlx::Error::PoolTimedOut).diagnostic_code(),
            "state_pool_timeout"
        );
        assert_eq!(
            StateError::Conflict.diagnostic_code(),
            "state_revision_conflict"
        );
        assert_eq!(
            StateError::Filesystem("/private/secret-path".into()).diagnostic_code(),
            "state_filesystem_error"
        );
    }

    #[tokio::test]
    async fn sqlite_lock_contention_has_a_distinct_safe_code() {
        use sqlx::Connection;
        let dir = tempfile::tempdir().unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(dir.path().join("contention.sqlite"))
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_millis(5));
        let mut writer = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let mut contender = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut writer)
            .await
            .unwrap();
        let error = sqlx::query("CREATE TABLE contention_fixture(id INTEGER)")
            .execute(&mut contender)
            .await
            .unwrap_err();
        assert_eq!(
            StateError::Database(error).diagnostic_code(),
            "state_database_busy"
        );
        sqlx::query("ROLLBACK").execute(&mut writer).await.unwrap();
    }
}

/// Non-sensitive database integrity information for readiness/admin checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrityReport {
    /// SQLite integrity-check result was exactly ok.
    pub integrity_ok: bool,
    /// Number of rows reported by SQLite foreign-key checking.
    pub foreign_key_violations: u64,
    /// Highest successfully applied SQLx migration version.
    pub migration_version: i64,
}
