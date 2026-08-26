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
