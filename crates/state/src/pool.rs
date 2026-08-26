//! SQLite pool configuration, migrations, diagnostics, and unit-of-work seam.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use sqlx::{
    Acquire, FromRow, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tokio::sync::Semaphore;

use crate::{
    audit::AuditRepository,
    auth::AuthStateRepository,
    background::{JobRepository, OutboxRepository, ScanCheckpointRepository},
    backups::BackupRepository,
    error::{IntegrityReport, StateError},
    files::FileStateRepository,
    index::IndexRepository,
    memory::MemoryRepository,
    migrations::MIGRATOR,
    providers::ProviderRepository,
    settings::SettingsRepository,
    vaults::VaultRepository,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_CONNECTIONS: u32 = 8;

/// Opened and configured operational SQLite state.
#[derive(Clone)]
pub struct StateStore {
    pool: SqlitePool,
    write_gate: Arc<Semaphore>,
}

impl StateStore {
    /// Connect to a SQLite URL without applying migrations.
    pub async fn connect(database_url: &str) -> Result<Self, StateError> {
        let in_memory = is_memory_database(database_url);
        prepare_sqlite_parent(database_url).await?;
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(|error| StateError::Connection(error.to_string()))?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .max_connections(if in_memory {
                1
            } else {
                DEFAULT_MAX_CONNECTIONS
            })
            .min_connections(if in_memory { 1 } else { 0 })
            // Request concurrency can exceed the small SQLite connection
            // pool while short write phases queue behind SQLite's single
            // writer. Pool acquisition is therefore a bounded admission
            // wait, distinct from SQLite's lock-level busy timeout.
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(options)
            .await?;

        Ok(Self {
            pool,
            write_gate: Arc::new(Semaphore::new(1)),
        })
    }

    /// Connect and apply all embedded forward-only migrations.
    pub async fn connect_and_migrate(database_url: &str) -> Result<Self, StateError> {
        let store = Self::connect(database_url).await?;
        store.migrate().await?;
        Ok(store)
    }

    /// Apply pending migrations and validate already-applied checksums.
    pub async fn migrate(&self) -> Result<(), StateError> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Begin a transaction that rolls back when dropped before commit.
    pub async fn begin(&self) -> Result<StateTransaction<'_>, StateError> {
        Ok(StateTransaction {
            transaction: self.pool.begin().await?,
        })
    }

    /// Return a Vault registry repository bound to this state store.
    pub fn vaults(&self) -> VaultRepository {
        VaultRepository::new(self.pool.clone())
    }

    /// Return a settings repository bound to this state store.
    pub fn settings(&self) -> SettingsRepository {
        SettingsRepository::new(self.pool.clone())
    }

    /// Return Vault-scoped file/revision/journal repository operations.
    pub fn files(&self) -> FileStateRepository {
        FileStateRepository::new(self.pool.clone(), self.write_gate.clone())
    }

    /// Return Vault-scoped Markdown/index projection operations.
    pub fn index(&self) -> IndexRepository {
        IndexRepository::new(self.pool.clone())
    }

    /// Return Vault-scoped durable memory and candidate operations.
    pub fn memory(&self) -> MemoryRepository {
        MemoryRepository::new(self.pool.clone())
    }

    /// Return provider/model/binding/embedding repository operations.
    pub fn providers(&self) -> ProviderRepository {
        ProviderRepository::new(self.pool.clone(), self.write_gate.clone())
    }

    /// Return authentication, authorization, and secret metadata repositories.
    pub fn auth(&self) -> AuthStateRepository {
        AuthStateRepository::new(self.pool.clone(), self.write_gate.clone())
    }

    /// Return the redacted audit-log query boundary.
    pub fn audit(&self) -> AuditRepository {
        AuditRepository::new(self.pool.clone())
    }

    /// Return the authoritative backup catalog repository.
    pub fn backups(&self) -> BackupRepository {
        BackupRepository::new(self.pool.clone())
    }

    /// Return the durable transactional outbox repository.
    pub fn outbox(&self) -> OutboxRepository {
        OutboxRepository::new(self.pool.clone())
    }

    /// Return the persistent job queue repository.
    pub fn jobs(&self) -> JobRepository {
        JobRepository::new(self.pool.clone())
    }

    /// Return resumable Vault scan checkpoint operations.
    pub fn scan_checkpoints(&self) -> ScanCheckpointRepository {
        ScanCheckpointRepository::new(self.pool.clone())
    }

    /// Verify SQLite integrity, foreign-key rows, and migration version.
    pub async fn integrity_check(&self) -> Result<IntegrityReport, StateError> {
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await?;
        let foreign_key_violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&self.pool)
            .await?
            .len() as u64;
        let migration_version = self.migration_version().await?;

        Ok(IntegrityReport {
            integrity_ok: integrity == "ok",
            foreign_key_violations,
            migration_version,
        })
    }

    /// Return whether SQLite foreign-key enforcement is enabled.
    pub async fn foreign_keys_enabled(&self) -> Result<bool, StateError> {
        let value: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&self.pool)
            .await?;
        Ok(value == 1)
    }

    /// Return the active SQLite journal mode.
    pub async fn journal_mode(&self) -> Result<String, StateError> {
        Ok(sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Return SQLite synchronous mode as its numeric PRAGMA value.
    pub async fn synchronous_mode(&self) -> Result<i64, StateError> {
        Ok(sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Return SQLite busy timeout in milliseconds.
    pub async fn busy_timeout_millis(&self) -> Result<i64, StateError> {
        Ok(sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Check for a named table without interpolating an identifier into SQL.
    pub async fn has_table(&self, table: &str) -> Result<bool, StateError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&self.pool)
        .await?;
        Ok(count == 1)
    }

    /// Create a consistent SQLite snapshot at a caller-provided service-owned
    /// destination. The destination must not already exist; SQL stays inside
    /// the state boundary rather than leaking a raw pool to backup code.
    pub async fn snapshot_to(&self, destination: &Path) -> Result<(), StateError> {
        if !destination.is_absolute() || destination.as_os_str().is_empty() {
            return Err(StateError::InvalidInput(
                "SQLite snapshot destination must be absolute",
            ));
        }
        if destination.exists() {
            return Err(StateError::InvalidInput(
                "SQLite snapshot destination already exists",
            ));
        }
        let destination = destination.to_str().ok_or(StateError::InvalidInput(
            "SQLite snapshot path is not UTF-8",
        ))?;
        sqlx::query("VACUUM INTO ?")
            .bind(destination)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Replace the live operational schema/data from a validated SQLite
    /// snapshot while keeping the pool and application services alive. The
    /// caller must already have stopped protocol/worker mutations through its
    /// maintenance gate; all SQL remains inside this state boundary.
    pub async fn restore_from_snapshot(&self, snapshot: &Path) -> Result<(), StateError> {
        if !snapshot.is_absolute() || snapshot.as_os_str().is_empty() {
            return Err(StateError::InvalidInput(
                "SQLite restore snapshot must be absolute",
            ));
        }
        let metadata = tokio::fs::metadata(snapshot)
            .await
            .map_err(|error| StateError::Filesystem(error.to_string()))?;
        if !metadata.is_file() {
            return Err(StateError::InvalidInput(
                "SQLite restore snapshot is not a file",
            ));
        }
        let snapshot = snapshot
            .to_str()
            .ok_or(StateError::InvalidInput("SQLite restore path is not UTF-8"))?;
        let mut connection = self.pool.acquire().await?;
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await?;
        let result = async {
            sqlx::query("ATTACH DATABASE ? AS restored")
                .bind(snapshot)
                .execute(&mut *connection)
                .await?;
            let result = async {
                let mut transaction = connection.begin().await?;
                let objects: Vec<SqliteObjectRow> = sqlx::query_as::<_, SqliteObjectRow>(
                    "SELECT type, name, sql FROM restored.sqlite_master
                     WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL
                     ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, name",
                )
                .fetch_all(&mut *transaction)
                .await?;
                let virtual_tables = objects
                    .iter()
                    .filter(|object| {
                        object
                            .sql
                            .as_deref()
                            .is_some_and(|sql| sql.contains("VIRTUAL TABLE"))
                    })
                    .map(|object| object.name.clone())
                    .collect::<Vec<_>>();
                let objects = objects
                    .into_iter()
                    .filter(|object| {
                        !virtual_tables.iter().any(|virtual_table| {
                            object.name.starts_with(virtual_table)
                                && object.name.as_bytes().get(virtual_table.len()) == Some(&b'_')
                        })
                    })
                    .collect::<Vec<_>>();

                let live_objects: Vec<SqliteObjectRow> = sqlx::query_as::<_, SqliteObjectRow>(
                    "SELECT type, name, sql FROM main.sqlite_master
                     WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL",
                )
                .fetch_all(&mut *transaction)
                .await?;
                let live_virtual_tables = live_objects
                    .iter()
                    .filter(|object| {
                        object
                            .sql
                            .as_deref()
                            .is_some_and(|sql| sql.contains("VIRTUAL TABLE"))
                    })
                    .map(|object| object.name.clone())
                    .collect::<Vec<_>>();
                let live_objects = live_objects
                    .into_iter()
                    .filter(|object| {
                        !live_virtual_tables.iter().any(|virtual_table| {
                            object.name.starts_with(virtual_table)
                                && object.name.as_bytes().get(virtual_table.len()) == Some(&b'_')
                        })
                    })
                    .collect::<Vec<_>>();

                // Restore is replacement, not a merge. Drop every live-only
                // object as well as objects present in the snapshot so an old
                // backup cannot retain tables introduced by newer migrations.
                // FTS shadow objects are omitted because dropping their
                // virtual table removes them atomically.
                for object in live_objects
                    .iter()
                    .filter(|object| object.object_type != "table")
                {
                    let object_type = match object.object_type.as_str() {
                        "index" => "INDEX",
                        "trigger" => "TRIGGER",
                        "view" => "VIEW",
                        _ => continue,
                    };
                    let sql = format!(
                        "DROP {object_type} IF EXISTS main.{}",
                        quote_identifier(&object.name),
                    );
                    sqlx::query(&sql).execute(&mut *transaction).await?;
                }
                for object in live_objects
                    .iter()
                    .filter(|object| object.object_type == "table")
                {
                    let sql = format!(
                        "DROP TABLE IF EXISTS main.{}",
                        quote_identifier(&object.name),
                    );
                    sqlx::query(&sql).execute(&mut *transaction).await?;
                }
                for object in objects
                    .iter()
                    .filter(|object| object.object_type == "table")
                {
                    let sql = object.sql.as_deref().ok_or(sqlx::Error::Protocol(
                        "restored table DDL is missing".to_owned(),
                    ))?;
                    sqlx::query(sql).execute(&mut *transaction).await?;
                }
                for object in objects
                    .iter()
                    .filter(|object| object.object_type == "table")
                {
                    let table = quote_identifier(&object.name);
                    let sql = format!("INSERT INTO main.{table} SELECT * FROM restored.{table}");
                    sqlx::query(&sql).execute(&mut *transaction).await?;
                }
                for object in objects
                    .iter()
                    .filter(|object| object.object_type != "table")
                {
                    let sql = object.sql.as_deref().ok_or(sqlx::Error::Protocol(
                        "restored object DDL is missing".to_owned(),
                    ))?;
                    sqlx::query(sql).execute(&mut *transaction).await?;
                }
                transaction.commit().await?;
                Ok::<(), sqlx::Error>(())
            }
            .await;
            let detach = sqlx::query("DETACH DATABASE restored")
                .execute(&mut *connection)
                .await;
            result.and(detach.map(|_| ()))
        }
        .await;
        let foreign_keys = sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *connection)
            .await;
        result.and(foreign_keys.map(|_| ()))?;
        Ok(())
    }

    async fn migration_version(&self) -> Result<i64, StateError> {
        if !self.has_table("_sqlx_migrations").await? {
            return Ok(0);
        }

        Ok(sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
        )
        .fetch_one(&self.pool)
        .await?)
    }
}

#[derive(Debug, FromRow)]
struct SqliteObjectRow {
    #[sqlx(rename = "type")]
    object_type: String,
    name: String,
    sql: Option<String>,
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Explicit state unit-of-work. Repository transaction methods will use this
/// seam when Vault Core needs atomic metadata/outbox writes.
pub struct StateTransaction<'a> {
    transaction: Transaction<'a, Sqlite>,
}

impl StateTransaction<'_> {
    /// Commit the unit of work.
    pub async fn commit(self) -> Result<(), StateError> {
        self.transaction.commit().await?;
        Ok(())
    }

    /// Roll back the unit of work explicitly.
    pub async fn rollback(self) -> Result<(), StateError> {
        self.transaction.rollback().await?;
        Ok(())
    }
}

fn is_memory_database(database_url: &str) -> bool {
    database_url == "sqlite::memory:"
        || database_url.contains("mode=memory")
        || database_url.contains(":memory:")
}

async fn prepare_sqlite_parent(database_url: &str) -> Result<(), StateError> {
    let Some(path) = sqlite_file_path(database_url) else {
        return Ok(());
    };
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }

    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| StateError::Filesystem(error.to_string()))
}

fn sqlite_file_path(database_url: &str) -> Option<PathBuf> {
    let path = database_url.strip_prefix("sqlite://")?;
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() || path == ":memory:" || path.starts_with("file:") {
        return None;
    }
    Some(Path::new(path).to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use mcp_vault_domain::{FileId, MemoryId, MemoryRawId, RevisionId, VaultId};
    use sqlx::Executor;
    use tempfile::TempDir;

    use super::StateStore;

    fn database_url(directory: &Path) -> String {
        format!("sqlite://{}", directory.join("state.sqlite3").display())
    }

    async fn migrated_file_store() -> (TempDir, StateStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::connect_and_migrate(&database_url(directory.path()))
            .await
            .unwrap();
        (directory, store)
    }

    #[tokio::test]
    async fn migration_creates_operational_tables_and_integrity_is_green() {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();

        for table in [
            "vaults",
            "system_settings",
            "vault_settings",
            "encrypted_secrets",
            "installation_key_checks",
            "admin_users",
            "admin_sessions",
            "webdav_credentials",
            "mcp_tokens",
            "oauth_issuers",
            "oauth_subject_grants",
            "file_entries",
            "file_revisions",
            "operation_journal",
            "outbox_events",
            "jobs",
            "scan_checkpoints",
            "providers",
            "models",
            "model_bindings",
            "provider_health",
            "embedding_records",
            "embedding_vectors",
            "memories",
            "memory_sources",
            "memory_entities",
            "memory_tags",
            "memory_relations",
            "memory_candidates",
            "memory_idempotency",
            "memory_diagnostics",
            "memory_stage1_outputs",
            "memory_consolidation_proposals",
            "memory_consolidation_state",
            "memory_fts",
            "audit_log",
            "backups",
            "notes",
            "note_headings",
            "note_tags",
            "note_links",
            "index_nodes",
            "index_memberships",
            "index_status",
        ] {
            assert!(store.has_table(table).await.unwrap(), "{table}");
        }

        let report = store.integrity_check().await.unwrap();
        assert!(report.integrity_ok);
        assert_eq!(report.foreign_key_violations, 0);
        assert_eq!(report.migration_version, 11);
        assert!(store.foreign_keys_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn sqlite_durability_pragmas_are_applied_to_file_databases() {
        let (_directory, store) = migrated_file_store().await;

        assert_eq!(store.journal_mode().await.unwrap(), "wal");
        assert_eq!(store.synchronous_mode().await.unwrap(), 2);
        assert_eq!(store.busy_timeout_millis().await.unwrap(), 5000);
    }

    #[tokio::test]
    async fn file_database_parent_is_prepared_by_the_state_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("nested/state.sqlite3");
        let store = StateStore::connect_and_migrate(&format!("sqlite://{}", database.display()))
            .await
            .unwrap();

        assert!(database.exists());
        assert!(store.integrity_check().await.unwrap().integrity_ok);
    }

    #[tokio::test]
    async fn sqlite_snapshot_can_restore_without_replacing_the_pool() {
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::connect_and_migrate(&database_url(directory.path()))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO system_settings (key, value_json, revision, updated_at)
             VALUES ('snapshot.test', '{\"value\":1}', 1, 1)",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        let snapshot = directory.path().join("snapshot.sqlite3");
        store.snapshot_to(&snapshot).await.unwrap();
        sqlx::query(
            "UPDATE system_settings SET value_json = '{\"value\":2}'
             WHERE key = 'snapshot.test'",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        store.restore_from_snapshot(&snapshot).await.unwrap();
        let value: String = sqlx::query_scalar(
            "SELECT value_json FROM system_settings WHERE key = 'snapshot.test'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(value, "{\"value\":1}");
        assert!(store.integrity_check().await.unwrap().integrity_ok);
    }

    #[tokio::test]
    async fn sqlite_snapshot_restore_removes_live_only_schema_objects() {
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::connect_and_migrate(&database_url(directory.path()))
            .await
            .unwrap();
        let snapshot = directory.path().join("older-snapshot.sqlite3");
        store.snapshot_to(&snapshot).await.unwrap();

        sqlx::query("CREATE TABLE live_only (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("CREATE INDEX live_only_value_idx ON live_only(value)")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("CREATE VIEW live_only_view AS SELECT id, value FROM live_only")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TRIGGER live_only_trigger AFTER INSERT ON live_only
             BEGIN UPDATE live_only SET value = NEW.value WHERE id = NEW.id; END",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        store.restore_from_snapshot(&snapshot).await.unwrap();

        for object in [
            "live_only",
            "live_only_value_idx",
            "live_only_view",
            "live_only_trigger",
        ] {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = ?")
                    .bind(object)
                    .fetch_one(&store.pool)
                    .await
                    .unwrap();
            assert_eq!(count, 0, "live-only schema object survived: {object}");
        }
        assert!(store.integrity_check().await.unwrap().integrity_ok);
    }

    #[tokio::test]
    async fn empty_pre_wp02_fixture_upgrades_through_embedded_migration() {
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::connect(&database_url(directory.path()))
            .await
            .unwrap();

        for statement in include_str!("../tests/fixtures/pre_wp02.sql").split(';') {
            let statement = statement.trim();
            if !statement.is_empty() {
                store.pool.execute(statement).await.unwrap();
            }
        }

        store.migrate().await.unwrap();
        assert_eq!(store.integrity_check().await.unwrap().migration_version, 11);
    }

    #[tokio::test]
    async fn migration_0009_clears_legacy_jwks_and_adds_key_verifier_state() {
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::connect(&database_url(directory.path()))
            .await
            .unwrap();
        let mut pre_hardening = sqlx::migrate::Migrator::DEFAULT;
        pre_hardening.migrations = std::borrow::Cow::Owned(
            crate::migrations::MIGRATOR
                .iter()
                .filter(|migration| migration.version <= 8)
                .cloned()
                .collect(),
        );
        pre_hardening.run(&store.pool).await.unwrap();
        sqlx::query(
            "INSERT INTO oauth_issuers
             (id, name, issuer_url, discovery_url, audience, resource,
              jwks_cache_json, jwks_cached_at, enabled, created_at, updated_at)
             VALUES ('legacy', 'Legacy', 'https://issuer.example.test', NULL,
                     'mcp-vault', 'https://vault.example.test/mcp',
                     '{\"keys\":[{\"kty\":\"oct\",\"k\":\"plaintext\"}]}',
                     1, 1, 1, 1)",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        store.migrate().await.unwrap();

        let (jwks, enabled): (Option<String>, i64) = sqlx::query_as(
            "SELECT jwks_cache_json, enabled FROM oauth_issuers WHERE id = 'legacy'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert!(jwks.is_none());
        assert_eq!(enabled, 0);
        assert!(store.has_table("installation_key_checks").await.unwrap());
        assert_eq!(store.integrity_check().await.unwrap().migration_version, 11);
    }

    #[tokio::test]
    async fn migration_0010_adds_codex_style_two_phase_memory_state() {
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::connect(&database_url(directory.path()))
            .await
            .unwrap();
        let mut pre_two_phase = sqlx::migrate::Migrator::DEFAULT;
        pre_two_phase.migrations = std::borrow::Cow::Owned(
            crate::migrations::MIGRATOR
                .iter()
                .filter(|migration| migration.version <= 9)
                .cloned()
                .collect(),
        );
        pre_two_phase.run(&store.pool).await.unwrap();
        for table in [
            "memory_stage1_outputs",
            "memory_consolidation_proposals",
            "memory_consolidation_state",
        ] {
            assert!(!store.has_table(table).await.unwrap(), "{table}");
        }

        let mut two_phase = sqlx::migrate::Migrator::DEFAULT;
        two_phase.migrations = std::borrow::Cow::Owned(
            crate::migrations::MIGRATOR
                .iter()
                .filter(|migration| migration.version <= 10)
                .cloned()
                .collect(),
        );
        two_phase.run(&store.pool).await.unwrap();
        for table in [
            "memory_stage1_outputs",
            "memory_consolidation_proposals",
            "memory_consolidation_state",
        ] {
            assert!(store.has_table(table).await.unwrap(), "{table}");
        }
        assert!(
            !store
                .has_table("memory_extraction_evaluations")
                .await
                .unwrap()
        );
        assert_eq!(store.integrity_check().await.unwrap().migration_version, 10);

        store.migrate().await.unwrap();
        assert_eq!(store.integrity_check().await.unwrap().migration_version, 11);
    }

    #[tokio::test]
    async fn migration_0011_discards_only_prerelease_memory_state_and_jobs() {
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::connect(&database_url(directory.path()))
            .await
            .unwrap();
        let mut pre_cutover = sqlx::migrate::Migrator::DEFAULT;
        pre_cutover.migrations = std::borrow::Cow::Owned(
            crate::migrations::MIGRATOR
                .iter()
                .filter(|migration| migration.version <= 10)
                .cloned()
                .collect(),
        );
        pre_cutover.run(&store.pool).await.unwrap();
        let vault = VaultId::new();
        let file = FileId::new();
        let memory = MemoryId::new();
        let raw = MemoryRawId::new();
        insert_vault(&store, vault, "memory-cutover").await;
        sqlx::query(
            "INSERT INTO file_entries
             (id, vault_id, path, entry_type, current_revision, size,
              modified_at, created_at, updated_at)
             VALUES (?, ?, 'notes/keep.md', 'file', 1, 4, 1, 1, 1)",
        )
        .bind(file.to_string())
        .bind(vault.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO providers
             (id, name, provider_type, base_url, settings_json, enabled,
              created_at, updated_at)
             VALUES ('provider-cutover', 'Keep provider', 'openai_compatible',
                     'https://example.test/v1/', '{}', 1, 1, 1)",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO models
             (id, provider_id, external_model_id, capability_json,
              settings_json, enabled, created_at, updated_at)
             VALUES ('model-cutover', 'provider-cutover', 'model', '{}', '{}', 1, 1, 1)",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO memories
             (id, vault_id, memory_type, status, content, normalized_content,
              content_hash, importance, confidence, origin, revision,
              extraction_json, created_at, updated_at)
             VALUES (?, ?, 'fact', 'active', 'old memory', 'old memory',
                     'old-memory-hash', 1.0, 1.0, 'extracted', 1, '{}', 1, 1)",
        )
        .bind(memory.to_string())
        .bind(vault.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO memory_stage1_outputs
             (id, vault_id, source_type, source_key, profile_hash,
              pipeline_version, prompt_version, raw_memory, source_summary,
              output_hash, status, generated_at, updated_at)
             VALUES (?, ?, 'note', 'old-source', 'old-profile', 7,
                     'old-prompt', 'old raw', 'old summary', 'old-output',
                     'ready', 1, 1)",
        )
        .bind(raw.to_string())
        .bind(vault.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO embedding_records
             (id, vault_id, object_type, object_id, chunk_key, provider_id,
              model_id, dimension, content_hash, vector_backend_key,
              created_at, updated_at)
             VALUES ('memory-vector', ?, 'memory', ?, 'content',
                     'provider-cutover', 'model-cutover', 2, 'hash', 'key', 1, 1)",
        )
        .bind(vault.to_string())
        .bind(memory.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        for (job_type, dedup) in [
            ("memory.extract", "old-memory-job"),
            ("index.rebuild", "keep-index-job"),
        ] {
            sqlx::query(
                "INSERT INTO jobs
                 (id, vault_id, job_type, dedup_key, payload_json, status,
                  max_attempts, available_at, created_at, updated_at)
                 VALUES (?, ?, ?, ?, '{}', 'queued', 3, 1, 1, 1)",
            )
            .bind(VaultId::new().to_string())
            .bind(vault.to_string())
            .bind(job_type)
            .bind(dedup)
            .execute(&store.pool)
            .await
            .unwrap();
        }

        store.migrate().await.unwrap();

        for table in [
            "memories",
            "memory_stage1_outputs",
            "memory_consolidation_proposals",
            "embedding_records",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&store.pool)
                .await
                .unwrap();
            assert_eq!(count, 0, "{table} retained prerelease memory state");
        }
        let memory_jobs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE job_type LIKE 'memory.%'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(memory_jobs, 0);
        let retained_jobs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE job_type = 'index.rebuild'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(retained_jobs, 1);
        let retained_files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_entries")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(retained_files, 1);
        let retained_providers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM providers")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(retained_providers, 1);
        let pipeline_column: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('memory_consolidation_state')
             WHERE name = 'pipeline_generation'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(pipeline_column, 1);
        assert_eq!(store.integrity_check().await.unwrap().migration_version, 11);
    }

    #[tokio::test]
    async fn dropped_unit_of_work_rolls_back_without_exposing_raw_pool() {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let mut unit = store.begin().await.unwrap();

        sqlx::query(
            "INSERT INTO system_settings
             (key, value_json, revision, updated_at)
             VALUES ('transaction.test', '{}', 1, 1)",
        )
        .execute(&mut *unit.transaction)
        .await
        .unwrap();
        drop(unit);

        assert!(
            store
                .settings()
                .get_system("transaction.test")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn composite_file_foreign_key_rejects_cross_vault_reference() {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let vault_a = VaultId::new();
        let vault_b = VaultId::new();
        let file_id = FileId::new();
        let revision_id = RevisionId::new();

        insert_vault(&store, vault_a, "a").await;
        insert_vault(&store, vault_b, "b").await;
        sqlx::query(
            "INSERT INTO file_entries
             (id, vault_id, path, entry_type, current_revision, size,
              modified_at, created_at, updated_at)
             VALUES (?, ?, 'notes/a.md', 'file', 0, 0, 1, 1, 1)",
        )
        .bind(file_id.to_string())
        .bind(vault_a.to_string())
        .execute(&store.pool)
        .await
        .unwrap();

        let result = sqlx::query(
            "INSERT INTO file_revisions
             (id, vault_id, file_id, revision, operation, actor_type,
              source_plane, created_at)
             VALUES (?, ?, ?, 1, 'create', 'system', 'system', 1)",
        )
        .bind(revision_id.to_string())
        .bind(vault_b.to_string())
        .bind(file_id.to_string())
        .execute(&store.pool)
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn revision_history_prevents_cascading_file_identity_deletion() {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let vault = VaultId::new();
        let file_id = FileId::new();
        let revision_id = RevisionId::new();
        insert_vault(&store, vault, "history").await;
        sqlx::query(
            "INSERT INTO file_entries
             (id, vault_id, path, entry_type, current_revision, size,
              modified_at, created_at, updated_at)
             VALUES (?, ?, 'notes/history.md', 'file', 1, 0, 1, 1, 1)",
        )
        .bind(file_id.to_string())
        .bind(vault.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO file_revisions
             (id, vault_id, file_id, revision, operation, actor_type,
              source_plane, created_at)
             VALUES (?, ?, ?, 1, 'create', 'system', 'system', 1)",
        )
        .bind(revision_id.to_string())
        .bind(vault.to_string())
        .bind(file_id.to_string())
        .execute(&store.pool)
        .await
        .unwrap();

        let result = sqlx::query("DELETE FROM file_entries WHERE id = ?")
            .bind(file_id.to_string())
            .execute(&store.pool)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn nullable_global_deduplication_is_still_unique() {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let vault = VaultId::new();
        insert_vault(&store, vault, "scoped").await;

        insert_job(&store, None, "global-scan").await.unwrap();
        assert!(insert_job(&store, None, "global-scan").await.is_err());
        insert_job(&store, Some(vault), "global-scan")
            .await
            .unwrap();
    }

    async fn insert_vault(store: &StateStore, id: VaultId, slug: &str) {
        sqlx::query(
            "INSERT INTO vaults
             (id, slug, name, content_root, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', 1, 1)",
        )
        .bind(id.to_string())
        .bind(slug)
        .bind(slug)
        .bind(format!("/srv/{slug}"))
        .execute(&store.pool)
        .await
        .unwrap();
    }

    async fn insert_job(
        store: &StateStore,
        vault_id: Option<VaultId>,
        dedup_key: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO jobs
             (id, vault_id, job_type, dedup_key, payload_json, status,
              max_attempts, available_at, created_at, updated_at)
             VALUES (?, ?, 'scan', ?, '{}', 'queued', 3, 1, 1, 1)",
        )
        .bind(VaultId::new().to_string())
        .bind(vault_id.map(|id| id.to_string()))
        .bind(dedup_key)
        .execute(&store.pool)
        .await
        .map(|_| ())
    }
}
