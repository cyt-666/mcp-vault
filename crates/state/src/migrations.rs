//! Embedded forward-only SQLx migrations.

use sqlx::migrate::Migrator;

/// The repository migration set. Applied migration files must never be edited.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

// Deployed prerelease variants observed in the operator-supplied schema-17
// snapshot. Keep the historical ledger intact; accept only these exact hashes
// and only after the resulting schema has been checked against our migrations.
const LEGACY: [(i64, &str); 2] = [
    (
        16,
        "B9509BECDBCD2576D4C712D8CE4D90BCE6B55FD552098EF898CF7B8DB4B31779B8AD7ACFF750FA63572D11EE491953F5",
    ),
    (
        17,
        "C22C8F033CBC4E918CA20C6B90EDBCA4C0352124562135D717D141B669C40BD612FD65F72B92BDC2D847CFD445BE4CCA",
    ),
];

pub(crate) async fn run(pool: &sqlx::SqlitePool) -> Result<(), crate::StateError> {
    use std::borrow::Cow;
    let exists: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name='_sqlx_migrations'")
            .fetch_one(pool)
            .await?;
    if exists == 0 {
        MIGRATOR.run(pool).await?;
        return Ok(());
    }
    let applied: Vec<(i64, Vec<u8>, String)> = sqlx::query_as(
        "SELECT version, checksum, hex(checksum) FROM _sqlx_migrations WHERE success=1 AND version IN (16,17)")
        .fetch_all(pool).await?;
    let mut migrations = MIGRATOR.iter().cloned().collect::<Vec<_>>();
    for (version, checksum, hex) in applied {
        let migration = migrations
            .iter_mut()
            .find(|m| m.version == version)
            .expect("embedded legacy migration");
        if migration.checksum.as_ref() == checksum.as_slice() {
            continue;
        }
        if !LEGACY.contains(&(version, hex.as_str())) || !matching_schema(pool, version).await? {
            return Err(sqlx::migrate::MigrateError::VersionMismatch(version).into());
        }
        migration.checksum = Cow::Owned(checksum);
    }
    let mut compatible = Migrator::DEFAULT;
    compatible.migrations = Cow::Owned(migrations);
    compatible.run(pool).await?;
    Ok(())
}

async fn matching_schema(pool: &sqlx::SqlitePool, version: i64) -> Result<bool, crate::StateError> {
    // Derive reference DDL from the unchanged migrations, not a hand-maintained
    // subset of columns. Include all indexes/triggers on the affected table and
    // the calibration job uniqueness index. Unknown schema variants fail closed.
    let reference = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let mut prior = Migrator::DEFAULT;
    prior.migrations = std::borrow::Cow::Owned(
        MIGRATOR
            .iter()
            .filter(|m| m.version <= version)
            .cloned()
            .collect(),
    );
    prior.run(&reference).await?;
    let table = if version == 16 {
        "memory_note_set_snapshots"
    } else {
        "retrieval_calibration_runs"
    };
    let index = if version == 17 {
        "retrieval_calibration_one_active_job"
    } else {
        ""
    };
    let expected = schema(&reference, table, index).await?;
    let actual = schema(pool, table, index).await?;
    reference.close().await;
    Ok(expected == actual)
}

async fn schema(
    pool: &sqlx::SqlitePool,
    table: &str,
    index: &str,
) -> Result<Vec<(String, String, String)>, crate::StateError> {
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT type,name,sql FROM sqlite_master WHERE tbl_name=? OR name=? ORDER BY type,name",
    )
    .bind(table)
    .bind(index)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, name, sql)| {
            // Only normalize trailing line whitespace observed in deployed v17.
            // Do not normalize tokens, quoted literals, constraints or column order.
            let ddl = sql
                .unwrap_or_default()
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n");
            (kind, name, ddl)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn legacy_store() -> sqlx::SqlitePool {
        let store = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let mut prior = Migrator::DEFAULT;
        prior.migrations = std::borrow::Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|m| m.version <= 17)
                .cloned()
                .collect(),
        );
        prior.run(&store).await.unwrap();
        for (version, hex) in LEGACY {
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|n| u8::from_str_radix(&hex[n..n + 2], 16).unwrap())
                .collect::<Vec<_>>();
            sqlx::query("UPDATE _sqlx_migrations SET checksum=? WHERE version=?")
                .bind(bytes)
                .bind(version)
                .execute(&store)
                .await
                .unwrap();
        }
        store
    }
    #[tokio::test]
    async fn deployed_checksums_upgrade_and_restart_without_rewriting_history() {
        let store = legacy_store().await;
        run(&store).await.unwrap();
        run(&store).await.unwrap();
        for (version, expected) in LEGACY {
            let actual: String =
                sqlx::query_scalar("SELECT hex(checksum) FROM _sqlx_migrations WHERE version=?")
                    .bind(version)
                    .fetch_one(&store)
                    .await
                    .unwrap();
            assert_eq!(actual, expected);
        }
        let version: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&store)
            .await
            .unwrap();
        assert_eq!(version, 19);
    }
    #[tokio::test]
    async fn unknown_checksum_and_known_checksum_with_schema_drift_fail_closed() {
        let unknown = legacy_store().await;
        sqlx::query("UPDATE _sqlx_migrations SET checksum=X'00' WHERE version=16")
            .execute(&unknown)
            .await
            .unwrap();
        assert!(matches!(
            run(&unknown).await,
            Err(crate::StateError::Migration(
                sqlx::migrate::MigrateError::VersionMismatch(16)
            ))
        ));
        let drift = legacy_store().await;
        sqlx::query("DROP INDEX retrieval_calibration_one_active_job")
            .execute(&drift)
            .await
            .unwrap();
        assert!(matches!(
            run(&drift).await,
            Err(crate::StateError::Migration(
                sqlx::migrate::MigrateError::VersionMismatch(17)
            ))
        ));
        let version: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&drift)
            .await
            .unwrap();
        assert_eq!(version, 17);
    }
}
