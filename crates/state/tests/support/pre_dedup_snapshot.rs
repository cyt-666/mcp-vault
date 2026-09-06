//! Synthetic test-only snapshot adapter. All SQL remains in the state boundary.
use sqlx::{Connection, SqliteConnection};

pub async fn strip_new_dedup_schema(database: &str) {
    let mut connection = SqliteConnection::connect(database).await.unwrap();
    sqlx::raw_sql("PRAGMA foreign_keys=OFF; DROP VIEW memory_valid_formal_supports; DROP VIEW memory_public_items; DROP INDEX memory_dedup_one_active_job;
        DROP TABLE memory_formal_identity_reservations; DROP TABLE memory_formal_supports; DROP TABLE memory_formal_items; DROP TABLE memory_formal_mode; DROP TABLE memory_formal_operations;
        DROP TABLE memory_dedup_progress; DROP TABLE memory_formal_maintenance; DROP TABLE memory_formal_examined; DROP TABLE memory_formal_pairs; DROP TABLE memory_equivalence_rewrites;
        DROP TABLE memory_equivalence_decisions; DROP TABLE memory_equivalence_dispatches;
        ALTER TABLE memory_current_items DROP COLUMN semantic_hash;
        DELETE FROM jobs WHERE job_type = 'memory.deduplicate';
        DELETE FROM _sqlx_migrations WHERE version > 19;")
        .execute(&mut connection).await.unwrap();
    connection.close().await.unwrap();
}

pub async fn clear_derived_memory_projections(database: &str) {
    let mut connection = SqliteConnection::connect(database).await.unwrap();
    sqlx::raw_sql("PRAGMA foreign_keys=ON; DELETE FROM memory_formal_supports; DELETE FROM memory_formal_items; DELETE FROM memory_formal_mode;
        DELETE FROM memory_current_items WHERE ownership='note_derived'; DELETE FROM memory_note_sets; DELETE FROM memory_current_fts;")
        .execute(&mut connection).await.unwrap();
    connection.close().await.unwrap();
}

pub async fn assert_public_fts_only(database: &str, vault_id: &str, expected: i64) {
    let mut connection = SqliteConnection::connect(database).await.unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(DISTINCT memory_id) FROM memory_current_fts WHERE vault_id=?",
    )
    .bind(vault_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        counts,
        (expected, expected),
        "hidden contributions polluted FTS"
    );
    connection.close().await.unwrap();
}
