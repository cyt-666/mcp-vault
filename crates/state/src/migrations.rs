//! Embedded forward-only SQLx migrations.

use sqlx::migrate::Migrator;

/// The repository migration set. Applied migration files must never be edited.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");
