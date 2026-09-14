//! The only predecessor-memory boundary: offline discard, never conversion.
use super::*;
use std::collections::BTreeMap;

// Reviewed business tables only; FTS shadow tables are owned by SQLite. Every
// statement below has a Vault predicate, including the FTS virtual tables.
const LEGACY_TABLES: &[&str] = &[
    "memory_consolidation_state",
    "memory_consolidation_proposals",
    "memory_note_set_snapshots",
    "memory_formal_operations",
    "memory_formal_fact_supports",
    "memory_formal_facts",
    "memory_formal_supports",
    "memory_formal_items",
    "memory_formal_mode",
    "memory_formal_maintenance",
    "memory_formal_examined",
    "memory_formal_pairs",
    "memory_formal_identity_reservations",
    "memory_current_fts",
    "memory_current_idempotency",
    "memory_current_explicit_reservations",
    "memory_current_sources",
    "memory_current_items",
    "memory_note_sets",
    "memory_equivalence_decisions",
    "memory_equivalence_dispatches",
    "memory_equivalence_rewrites",
    "memory_dedup_new_contributions",
    "memory_dedup_progress",
    "memory_source_health",
    "memory_source_audit_state",
    "memory_retrieval_metadata",
    "memory_retrieval_proposals",
    "memory_candidates",
    "memory_stage1_outputs",
    "memory_relations",
    "memory_idempotency",
    "memory_fts",
    "memory_diagnostics",
    "memory_sources",
    "memory_entities",
    "memory_tags",
    "memories",
    // Source/file triggers in historical migrations can refill these. Clear
    // them last, after all predecessor sources and contributions are gone.
    "memory_organization_decisions",
    "memory_organization_dispatches",
    "memory_organization_items",
    "memory_organization_sources",
    "memory_organization_vectors",
    "memory_organization_state",
    "memory_v2_migration_state",
    "retrieval_calibration_runs",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryInitializationState {
    pub phase: String,
    pub manifest: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MemoryInitializationTask {
    pub vault_id: String,
    pub task_id: String,
    pub state: String,
    pub requested_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub completed_files: i64,
    pub total_files: i64,
    pub error_code: Option<String>,
    pub error_stage: Option<String>,
    pub error_path: Option<String>,
    pub error_source_code: Option<String>,
    pub resumable: bool,
    pub maintenance_previous_mode: String,
}

impl UnitRepository {
    pub async fn initialization_task(
        &self,
        context: &VaultContext,
    ) -> Result<Option<MemoryInitializationTask>, StateError> {
        self.ensure_vault_context(context).await?;
        Ok(sqlx::query_as(
            "SELECT vault_id,task_id,state,requested_at,started_at,finished_at,
                    completed_files,total_files,error_code,error_stage,error_path,error_source_code,resumable,maintenance_previous_mode
             FROM memory_initialization_tasks WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn queue_initialization_task(
        &self,
        context: &VaultContext,
        task_id: &str,
        total_files: u64,
        resume_failed: bool,
        maintenance_previous_mode: &str,
    ) -> Result<MemoryInitializationTask, StateError> {
        self.ensure_vault_context(context).await?;
        let now = now_millis()?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing: Option<MemoryInitializationTask> = sqlx::query_as(
            "SELECT vault_id,task_id,state,requested_at,started_at,finished_at,
                    completed_files,total_files,error_code,error_stage,error_path,error_source_code,resumable,maintenance_previous_mode
             FROM memory_initialization_tasks WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing
            && (!resume_failed || existing.state != "failed")
        {
            tx.commit().await?;
            return Ok(existing);
        }
        sqlx::query(
            "INSERT INTO memory_initialization_tasks
             (vault_id,task_id,state,requested_at,completed_files,total_files,error_code,error_stage,error_path,error_source_code,resumable,maintenance_previous_mode)
             VALUES(?,?, 'queued', ?, 0, ?, NULL, NULL, NULL, NULL, 1, ?)
             ON CONFLICT(vault_id) DO UPDATE SET
               task_id=excluded.task_id,state='queued',requested_at=excluded.requested_at,
               started_at=NULL,finished_at=NULL,completed_files=0,total_files=excluded.total_files,
               error_code=NULL,error_stage=NULL,error_path=NULL,error_source_code=NULL,resumable=1",
        )
        .bind(context.id().to_string())
        .bind(task_id)
        .bind(now)
        .bind(
            i64::try_from(total_files)
                .map_err(|_| StateError::InvalidInput("file count is too large"))?,
        )
        .bind(maintenance_previous_mode)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.initialization_task(context)
            .await?
            .ok_or(StateError::InvalidInput(
                "initialization task was not persisted",
            ))
    }

    pub async fn start_initialization_task(
        &self,
        context: &VaultContext,
        task_id: &str,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let result = sqlx::query(
            "UPDATE memory_initialization_tasks
             SET state='running',started_at=COALESCE(started_at,?),error_code=NULL,error_stage=NULL,error_path=NULL,error_source_code=NULL
             WHERE vault_id=? AND task_id=? AND state='queued'",
        )
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            let current = self.initialization_task(context).await?;
            if current
                .as_ref()
                .is_some_and(|task| task.task_id == task_id && task.state == "running")
            {
                return Ok(());
            }
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    pub async fn finish_initialization_task(
        &self,
        context: &VaultContext,
        task_id: &str,
        completed_files: u64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE memory_initialization_tasks
             SET state='ready',finished_at=?,completed_files=?,error_code=NULL,error_stage=NULL,error_path=NULL,error_source_code=NULL,resumable=0
             WHERE vault_id=? AND task_id=?",
        )
        .bind(now_millis()?)
        .bind(
            i64::try_from(completed_files)
                .map_err(|_| StateError::InvalidInput("file count is too large"))?,
        )
        .bind(context.id().to_string())
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fail_initialization_task(
        &self,
        context: &VaultContext,
        task_id: &str,
        error_code: &str,
        completed_files: u64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE memory_initialization_tasks
             SET state='failed',finished_at=?,completed_files=?,error_code=?,error_stage=NULL,error_path=NULL,error_source_code=NULL,resumable=1
             WHERE vault_id=? AND task_id=?",
        )
        .bind(now_millis()?)
        .bind(
            i64::try_from(completed_files)
                .map_err(|_| StateError::InvalidInput("file count is too large"))?,
        )
        .bind(error_code)
        .bind(context.id().to_string())
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fail_initialization_task_with_diagnostic(
        &self,
        context: &VaultContext,
        task_id: &str,
        error_code: &str,
        error_stage: Option<&str>,
        error_path: Option<&str>,
        error_source_code: Option<&str>,
        completed_files: u64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE memory_initialization_tasks
             SET state='failed',finished_at=?,completed_files=?,error_code=?,error_stage=?,error_path=?,error_source_code=?,resumable=1
             WHERE vault_id=? AND task_id=?",
        )
        .bind(now_millis()?)
        .bind(i64::try_from(completed_files).map_err(|_| StateError::InvalidInput("file count is too large"))?)
        .bind(error_code)
        .bind(error_stage)
        .bind(error_path)
        .bind(error_source_code)
        .bind(context.id().to_string())
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Convert in-process work left queued/running by a prior process into an
    /// explicit resumable interruption. It never touches canonical files.
    pub async fn mark_interrupted_initialization_task(
        &self,
        context: &VaultContext,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE memory_initialization_tasks
             SET state='failed',finished_at=?,error_code='initialization_interrupted',error_stage='startup',error_path=NULL,error_source_code=NULL,resumable=1
             WHERE vault_id=? AND state IN ('queued','running')",
        )
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Reconcile task metadata after canonical initialization reached ready
    /// but the process stopped before the task terminal update. This never
    /// revisits files or legacy tables.
    pub async fn reconcile_ready_initialization_task(
        &self,
        context: &VaultContext,
        completed_files: u64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE memory_initialization_tasks
             SET state='ready',finished_at=COALESCE(finished_at,?),completed_files=?,error_code=NULL,error_stage=NULL,error_path=NULL,error_source_code=NULL,resumable=0
             WHERE vault_id=? AND EXISTS(SELECT 1 FROM memory_unit_initialization WHERE vault_id=? AND phase='ready')",
        )
        .bind(now_millis()?)
        .bind(i64::try_from(completed_files).map_err(|_| StateError::InvalidInput("file count is too large"))?)
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn initialization(
        &self,
        context: &VaultContext,
    ) -> Result<Option<MemoryInitializationState>, StateError> {
        self.ensure_vault_context(context).await?;
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT phase,manifest_json FROM memory_unit_initialization WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(phase, json)| {
            Ok(MemoryInitializationState {
                phase,
                manifest: serde_json::from_str(&json)?,
            })
        })
        .transpose()
    }
    pub async fn legacy_counts(
        &self,
        context: &VaultContext,
    ) -> Result<BTreeMap<String, u64>, StateError> {
        self.ensure_vault_context(context).await?;
        let mut counts = BTreeMap::new();
        for table in LEGACY_TABLES {
            let count: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE vault_id=?"))
                    .bind(context.id().to_string())
                    .fetch_one(&self.pool)
                    .await?;
            counts.insert((*table).to_owned(), count as u64);
        }
        let vectors: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM embedding_records WHERE vault_id=? AND object_type='memory'",
        )
        .bind(context.id().to_string())
        .fetch_one(&self.pool)
        .await?;
        let jobs:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE vault_id=? AND (job_type LIKE 'memory.%' OR job_type='retrieval.calibrate')").bind(context.id().to_string()).fetch_one(&self.pool).await?;
        counts.insert("legacy_memory_vectors".into(), vectors as u64);
        counts.insert("legacy_memory_jobs".into(), jobs as u64);
        Ok(counts)
    }
    pub async fn begin_initialization(
        &self,
        context: &VaultContext,
        manifest: &Value,
    ) -> Result<(), StateError> {
        if !self.initialization_allowed() {
            return Err(StateError::InvalidInput(
                "memory initialization requires exclusive offline access",
            ));
        }
        self.ensure_vault_context(context).await?;
        let result=sqlx::query("INSERT INTO memory_unit_initialization(vault_id,phase,manifest_json,updated_at) VALUES(?,'clearing',?,?) ON CONFLICT(vault_id) DO UPDATE SET phase='clearing',manifest_json=excluded.manifest_json,updated_at=excluded.updated_at WHERE memory_unit_initialization.phase='required'")
            .bind(context.id().to_string()).bind(serde_json::to_string(manifest)?).bind(now_millis()?).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }
    pub async fn checkpoint_initialization(
        &self,
        context: &VaultContext,
        manifest: &Value,
    ) -> Result<(), StateError> {
        if !self.initialization_allowed() {
            return Err(StateError::InvalidInput(
                "memory initialization requires exclusive offline access",
            ));
        }
        self.ensure_vault_context(context).await?;
        let result=sqlx::query("UPDATE memory_unit_initialization SET manifest_json=?,updated_at=? WHERE vault_id=? AND phase='clearing'")
            .bind(serde_json::to_string(manifest)?).bind(now_millis()?).bind(context.id().to_string()).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }
    /// Called only after all manifest files have been retired through Vault Core.
    /// A completed initializer never enters this branch again.
    pub async fn finish_initialization(
        &self,
        context: &VaultContext,
        manifest: &Value,
    ) -> Result<(), StateError> {
        if !self.initialization_allowed() {
            return Err(StateError::InvalidInput(
                "memory initialization requires exclusive offline access",
            ));
        }
        self.ensure_vault_context(context).await?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let clearing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_unit_initialization WHERE vault_id=? AND phase='clearing')").bind(context.id().to_string()).fetch_one(&mut *tx).await?;
        if !clearing {
            return Err(StateError::Conflict);
        }
        sqlx::query("PRAGMA defer_foreign_keys=ON")
            .execute(&mut *tx)
            .await?;
        for table in LEGACY_TABLES {
            sqlx::query(&format!("DELETE FROM {table} WHERE vault_id=?"))
                .bind(context.id().to_string())
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM embedding_records WHERE vault_id=? AND object_type='memory'")
            .bind(context.id().to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM jobs WHERE vault_id=? AND (job_type LIKE 'memory.%' OR job_type='retrieval.calibrate')").bind(context.id().to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM outbox_events WHERE vault_id=? AND aggregate_type IN ('memory','memory_set','memory_consolidation')").bind(context.id().to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM vault_settings WHERE vault_id=? AND ((key LIKE 'memory.%' AND key NOT LIKE 'memory.units.%') OR key LIKE 'retrieval.calibration%')").bind(context.id().to_string()).execute(&mut *tx).await?;
        for table in LEGACY_TABLES {
            let remaining: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE vault_id=?"))
                    .bind(context.id().to_string())
                    .fetch_one(&mut *tx)
                    .await?;
            if remaining != 0 {
                return Err(StateError::Conflict);
            }
        }
        // This new maintenance hold is not an inherited legacy pause. The
        // operator releases it after login/protocol smoke tests, then backfill
        // continues automatically without any source-level exclusions.
        sqlx::query("INSERT INTO memory_unit_runtime(vault_id,paused) VALUES(?,1) ON CONFLICT(vault_id) DO UPDATE SET paused=1")
            .bind(context.id().to_string()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO vault_settings(vault_id,key,value_json,revision,updated_at) VALUES(?,'memory.units.policy','{\"enabled\":true,\"request_timeout_seconds\":300}',1,?) ON CONFLICT(vault_id,key) DO NOTHING")
            .bind(context.id().to_string()).bind(now_millis()?).execute(&mut *tx).await?;
        sqlx::query("UPDATE memory_unit_initialization SET phase='ready',manifest_json=?,updated_at=? WHERE vault_id=? AND phase='clearing'")
            .bind(serde_json::to_string(manifest)?).bind(now_millis()?).bind(context.id().to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
