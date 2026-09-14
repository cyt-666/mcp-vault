//! Test-only predecessor snapshot SQL kept inside the state boundary.
use sqlx::{Connection, sqlite::SqliteConnectOptions};
use std::str::FromStr;

pub async fn create_v27(database: &str) {
    let mut connection = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::from_str(database)
            .unwrap()
            .create_if_missing(true),
    )
    .await
    .unwrap();
    let mut migrator = sqlx::migrate!("../../migrations");
    migrator.migrations = std::borrow::Cow::Owned(
        migrator
            .iter()
            .filter(|migration| migration.version <= 27)
            .cloned()
            .collect(),
    );
    migrator.run(&mut connection).await.unwrap();
    connection.close().await.unwrap();
}

pub async fn seed_legacy(database: &str, vault: &str, memory: &str, file: &str, path: &str) {
    let mut connection = sqlx::SqliteConnection::connect(database).await.unwrap();
    sqlx::query("INSERT INTO memories(id,vault_id,memory_type,status,content,normalized_content,content_hash,importance,confidence,origin,canonical_file_id,canonical_path,canonical_revision,created_at,updated_at) VALUES(?,?,'fact','active','legacy body; never convert','legacy body','legacy-hash',0.5,0.5,'explicit_admin',?,?,1,1,1)")
        .bind(memory).bind(vault).bind(file).bind(path).execute(&mut connection).await.unwrap();
    sqlx::query("INSERT INTO memory_organization_state(vault_id,paused) VALUES(?,1) ON CONFLICT(vault_id) DO UPDATE SET paused=1").bind(vault).execute(&mut connection).await.unwrap();
    connection.close().await.unwrap();
}
