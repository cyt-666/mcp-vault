//! Durable selection checkpoints and source-bound generated navigation.
use super::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize, FromRow)]
pub struct UnitRuntimeState {
    pub paused: bool,
    pub generation: i64,
    pub overview_generation: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UnitOverviewRecord {
    pub scope_key: String,
    pub input_hash: String,
    pub generation: i64,
    pub content: Value,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitOverviewScope {
    pub key: String,
    pub label: String,
    pub path_prefix: Option<String>,
    pub topic_ids: Vec<String>,
}

impl UnitRepository {
    /// Walk existing knowledge-map scopes with current memory membership.
    /// One indexed scope per call bounds worker checkpoints without a new
    /// project registry or a full Vault scan.
    pub async fn next_overview_scope(
        &self,
        context: &VaultContext,
        after: Option<&str>,
    ) -> Result<Option<UnitOverviewScope>, StateError> {
        self.ensure_vault_context(context).await?;
        let row:Option<(String,String,String,Option<String>)>=sqlx::query_as(&format!(
            "SELECT node.stable_key,node.title,node.node_type,node.source_ref FROM index_nodes node WHERE node.vault_id=? AND node.node_type IN ('folder','manual_topic','tag','semantic_topic') AND (? IS NULL OR node.stable_key>?) AND EXISTS(SELECT 1 FROM index_memberships member JOIN memory_unit_sources source ON source.vault_id=member.vault_id AND source.note_file_id=member.file_id JOIN memory_units i ON i.vault_id=source.vault_id AND i.id=source.memory_id WHERE member.vault_id=node.vault_id AND member.node_id=node.id AND ({})) ORDER BY node.stable_key LIMIT 1",contribution_eligibility_sql()))
            .bind(context.id().to_string()).bind(after).bind(after).fetch_optional(&self.pool).await?;
        row.map(|(key, label, kind, path)| {
            if kind == "folder" {
                let path = path.ok_or(StateError::InvalidInput(
                    "folder navigation path is missing",
                ))?;
                VaultPath::parse(&path)?;
                Ok(UnitOverviewScope {
                    key,
                    label,
                    path_prefix: Some(path),
                    topic_ids: vec![],
                })
            } else {
                Ok(UnitOverviewScope {
                    topic_ids: vec![key.clone()],
                    key,
                    label,
                    path_prefix: None,
                })
            }
        })
        .transpose()
    }

    /// New stores are empty and ready. A populated predecessor requires the
    /// separate offline initializer; ordinary reads never adopt its records.
    pub async fn initialization_required(
        &self,
        context: &VaultContext,
    ) -> Result<bool, StateError> {
        self.ensure_vault_context(context).await?;
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_unit_initialization WHERE vault_id=? AND phase!='ready')")
            .bind(context.id().to_string()).fetch_one(&self.pool).await?)
    }

    pub async fn runtime(&self, context: &VaultContext) -> Result<UnitRuntimeState, StateError> {
        self.ensure_vault_context(context).await?;
        Ok(sqlx::query_as("SELECT paused,generation,overview_generation FROM memory_unit_runtime WHERE vault_id=?")
            .bind(context.id().to_string()).fetch_optional(&self.pool).await?.unwrap_or_default())
    }

    pub async fn complete_overview_generation(
        &self,
        context: &VaultContext,
        generation: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let result=sqlx::query("UPDATE memory_unit_runtime SET overview_generation=? WHERE vault_id=? AND generation=? AND paused=0")
            .bind(generation).bind(context.id().to_string()).bind(generation).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    pub async fn set_paused(&self, context: &VaultContext, paused: bool) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query("INSERT INTO memory_unit_runtime(vault_id,paused) VALUES(?,?) ON CONFLICT(vault_id) DO UPDATE SET paused=excluded.paused")
            .bind(context.id().to_string()).bind(paused).execute(&self.pool).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_selection_progress(
        &self,
        context: &VaultContext,
        file_id: FileId,
        source_hash: &str,
        profile_hash: &str,
        completed: u32,
        total: u32,
        skipped: &Value,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let result=sqlx::query("INSERT INTO memory_unit_selection_progress(vault_id,source_file_id,source_hash,profile_hash,completed_batches,total_batches,skipped_json,updated_at) SELECT ?,?,?,?,?,?,?,? WHERE EXISTS(SELECT 1 FROM file_entries WHERE vault_id=? AND id=? AND content_hash=? AND deleted_at IS NULL) ON CONFLICT(vault_id,source_file_id) DO UPDATE SET source_hash=excluded.source_hash,profile_hash=excluded.profile_hash,completed_batches=excluded.completed_batches,total_batches=excluded.total_batches,skipped_json=excluded.skipped_json,updated_at=excluded.updated_at")
            .bind(context.id().to_string()).bind(file_id.to_string()).bind(source_hash).bind(profile_hash).bind(completed).bind(total).bind(serde_json::to_string(skipped)?).bind(now_millis()?)
            .bind(context.id().to_string()).bind(file_id.to_string()).bind(source_hash).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }
    pub async fn selection_progress(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<Value>, StateError> {
        self.ensure_vault_context(context).await?;
        let rows:Vec<(String,String,i64,i64,String,i64)>=sqlx::query_as("SELECT p.source_file_id,f.path,p.completed_batches,p.total_batches,p.skipped_json,p.updated_at FROM memory_unit_selection_progress p JOIN file_entries f ON f.vault_id=p.vault_id AND f.id=p.source_file_id AND f.content_hash=p.source_hash AND f.deleted_at IS NULL WHERE p.vault_id=? ORDER BY p.updated_at DESC,p.source_file_id LIMIT 100")
            .bind(context.id().to_string()).fetch_all(&self.pool).await?;
        rows.into_iter().map(|(id,path,completed,total,skipped,time)|Ok(serde_json::json!({"file_id":id,"path":path,"completed_batches":completed,"total_batches":total,"skipped":serde_json::from_str::<Value>(&skipped)?,"updated_at":time}))).collect()
    }

    pub async fn selection_batch(
        &self,
        context: &VaultContext,
        source_file_id: FileId,
        source_hash: &str,
        input_hash: &str,
    ) -> Result<Option<Value>, StateError> {
        self.ensure_vault_context(context).await?;
        let value: Option<String> = sqlx::query_scalar("SELECT b.result_json FROM memory_unit_selection_batches b JOIN file_entries f ON f.vault_id=b.vault_id AND f.id=b.source_file_id WHERE b.vault_id=? AND b.source_file_id=? AND b.source_hash=? AND b.input_hash=? AND f.deleted_at IS NULL AND f.content_hash=b.source_hash")
            .bind(context.id().to_string()).bind(source_file_id.to_string()).bind(source_hash).bind(input_hash)
            .fetch_optional(&self.pool).await?;
        value
            .map(|text| serde_json::from_str(&text).map_err(StateError::from))
            .transpose()
    }

    /// A completed valid batch survives job/process interruption. Publication
    /// still separately validates the complete source snapshot and pause state.
    pub async fn save_selection_batch(
        &self,
        context: &VaultContext,
        source_file_id: FileId,
        source_hash: &str,
        input_hash: &str,
        result: &Value,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let encoded = serde_json::to_string(result)?;
        if encoded.len() > 256 * 1024
            || input_hash.len() != 71
            || !input_hash.starts_with("sha256:")
        {
            return Err(StateError::InvalidInput(
                "memory selection checkpoint is invalid",
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_entries WHERE vault_id=? AND id=? AND content_hash=? AND deleted_at IS NULL)")
            .bind(context.id().to_string()).bind(source_file_id.to_string()).bind(source_hash).fetch_one(&mut *tx).await?;
        if !current {
            return Err(StateError::Conflict);
        }
        sqlx::query("INSERT INTO memory_unit_selection_batches(vault_id,source_file_id,source_hash,input_hash,result_json,created_at) VALUES(?,?,?,?,?,?) ON CONFLICT(vault_id,source_file_id,source_hash,input_hash) DO NOTHING")
            .bind(context.id().to_string()).bind(source_file_id.to_string()).bind(source_hash).bind(input_hash).bind(encoded).bind(now_millis()?)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn overview(
        &self,
        context: &VaultContext,
        scope_key: &str,
    ) -> Result<Option<UnitOverviewRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row: Option<(String, i64, String, i64)> = sqlx::query_as(&format!(
            "SELECT o.input_hash,o.generation,o.content_json,o.updated_at FROM memory_unit_overviews o JOIN memory_unit_runtime r ON r.vault_id=o.vault_id AND r.generation=o.generation WHERE o.vault_id=? AND o.scope_key=? AND o.dependency_count=(SELECT count(*) FROM memory_unit_overview_dependencies d WHERE d.vault_id=o.vault_id AND d.scope_key=o.scope_key) AND NOT EXISTS(SELECT 1 FROM memory_unit_overview_dependencies d LEFT JOIN memory_units i ON i.vault_id=d.vault_id AND i.id=d.memory_id WHERE d.vault_id=o.vault_id AND d.scope_key=o.scope_key AND (i.id IS NULL OR i.revision!=d.revision OR i.content_hash!=d.content_hash OR NOT ({})))", contribution_eligibility_sql()
        )).bind(context.id().to_string()).bind(scope_key).fetch_optional(&self.pool).await?;
        row.map(|(input_hash, generation, content, updated_at)| {
            Ok(UnitOverviewRecord {
                scope_key: scope_key.to_owned(),
                input_hash,
                generation,
                content: serde_json::from_str(&content)?,
                updated_at,
            })
        })
        .transpose()
    }

    /// Store one derived overview against the exact generation it described.
    /// The complete dependency list prevents deletion from leaving a valid cache.
    pub async fn save_overview(
        &self,
        context: &VaultContext,
        record: &UnitOverviewRecord,
        dependencies: &[UnitRecord],
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        if dependencies.is_empty()
            || dependencies
                .iter()
                .any(|unit| unit.vault_id != context.id())
        {
            return Err(StateError::InvalidInput(
                "memory overview dependencies are invalid",
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let paused: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM memory_unit_runtime WHERE vault_id=? AND paused=1)",
        )
        .bind(context.id().to_string())
        .fetch_one(&mut *tx)
        .await?;
        if paused {
            return Err(StateError::Conflict);
        }
        let generation: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT generation FROM memory_unit_runtime WHERE vault_id=?),0)",
        )
        .bind(context.id().to_string())
        .fetch_one(&mut *tx)
        .await?;
        if generation != record.generation {
            return Err(StateError::Conflict);
        }
        for dependency in dependencies {
            let current: bool = sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM memory_units i WHERE i.vault_id=? AND i.id=? AND i.revision=? AND i.content_hash=? AND {})",contribution_eligibility_sql()))
                .bind(context.id().to_string()).bind(dependency.id.to_string()).bind(dependency.revision.as_i64()?).bind(&dependency.content_hash)
                .fetch_one(&mut *tx).await?;
            if !current {
                return Err(StateError::Conflict);
            }
        }
        sqlx::query("INSERT INTO memory_unit_overviews(vault_id,scope_key,input_hash,generation,content_json,dependency_count,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(vault_id,scope_key) DO UPDATE SET input_hash=excluded.input_hash,generation=excluded.generation,content_json=excluded.content_json,dependency_count=excluded.dependency_count,updated_at=excluded.updated_at")
            .bind(context.id().to_string()).bind(&record.scope_key).bind(&record.input_hash).bind(record.generation).bind(serde_json::to_string(&record.content)?)
            .bind(i64::try_from(dependencies.len()).map_err(|_| StateError::InvalidInput("too many overview dependencies"))?).bind(now_millis()?).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM memory_unit_overview_dependencies WHERE vault_id=? AND scope_key=?",
        )
        .bind(context.id().to_string())
        .bind(&record.scope_key)
        .execute(&mut *tx)
        .await?;
        for dependency in dependencies {
            sqlx::query("INSERT INTO memory_unit_overview_dependencies(vault_id,scope_key,memory_id,revision,content_hash) VALUES(?,?,?,?,?)")
                .bind(context.id().to_string()).bind(&record.scope_key).bind(dependency.id.to_string()).bind(dependency.revision.as_i64()?).bind(&dependency.content_hash)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
