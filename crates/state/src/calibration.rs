//! Vault-scoped bounded calibration checkpoints and transport request accounting.
use crate::{StateError, now_millis};
use mcp_vault_domain::VaultContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool};

/// Configurable engineering limits; quality gates are never configurable here.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalibrationBudget {
    /// Actual HTTP attempts, including retries, per authorized round.
    pub max_requests: u32,
    /// Serialized HTTP request bytes per round.
    pub max_bytes: u32,
    /// Maximum unique synthetic embedding inputs.
    pub max_inputs: u32,
    /// Embedding input count per batch.
    pub batch_size: u32,
    /// Total durable round deadline, including restart downtime.
    pub timeout_seconds: u32,
}
impl Default for CalibrationBudget {
    fn default() -> Self {
        Self {
            max_requests: 32,
            max_bytes: 2097152,
            max_inputs: 512,
            batch_size: 16,
            timeout_seconds: 900,
        }
    }
}
impl CalibrationBudget {
    /// Reject zero or unbounded work at the application/repository boundary.
    pub fn validate(&self) -> Result<(), StateError> {
        if !(1..=128).contains(&self.max_requests)
            || !(1024..=8388608).contains(&self.max_bytes)
            || !(1..=512).contains(&self.max_inputs)
            || !(1..=64).contains(&self.batch_size)
            || !(1..=3600).contains(&self.timeout_seconds)
        {
            return Err(StateError::InvalidInput("calibration budget is invalid"));
        }
        Ok(())
    }
}

/// Persisted execution state; synthetic vectors stay outside business indexes.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct CalibrationRun {
    /// Evaluated role channel.
    pub channel: String,
    /// Exact retrieval configuration signature.
    pub signature: String,
    /// Pending/running/terminal execution state.
    pub status: String,
    /// Resumable embedding cache; never returned through protocol DTOs.
    #[serde(skip)]
    pub checkpoint_json: String,
    /// Bounded, content-free metrics and failed case IDs.
    pub report_json: Option<String>,
    /// Actual transport attempts reserved immediately before dispatch.
    pub requests: i64,
    /// Serialized HTTP body bytes dispatched, including retries.
    pub request_bytes: i64,
    /// Authorized cumulative request ceiling (manual retries allocate a new round).
    pub request_limit: i64,
    /// Authorized cumulative serialized input ceiling.
    pub byte_limit: i64,
    /// Frozen engineering limits for this signature.
    pub budget_json: String,
    /// Initial admission time; restart cannot reset the deadline.
    pub started_at: i64,
    /// Most recent state change.
    pub updated_at: i64,
}

/// Operational calibration repository.
#[derive(Clone)]
pub struct CalibrationRepository {
    pool: SqlitePool,
}
impl CalibrationRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a checkpoint once without resetting an existing budget.
    pub async fn ensure(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
    ) -> Result<CalibrationRun, StateError> {
        self.ensure_with_budget(context, channel, signature, &CalibrationBudget::default())
            .await
    }

    /// Freeze explicitly configured limits only when first admitting a signature.
    pub async fn ensure_with_budget(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
        budget: &CalibrationBudget,
    ) -> Result<CalibrationRun, StateError> {
        budget.validate()?;
        let now = now_millis()?;
        sqlx::query("INSERT OR IGNORE INTO retrieval_calibration_runs (vault_id,channel,signature,started_at,updated_at,request_limit,byte_limit,budget_json) VALUES (?,?,?,?,?,?,?,?)")
            .bind(context.id().to_string()).bind(channel).bind(signature).bind(now).bind(now).bind(budget.max_requests).bind(budget.max_bytes).bind(serde_json::to_string(budget)?).execute(&self.pool).await?;
        self.get(context, channel, signature)
            .await?
            .ok_or(StateError::Conflict)
    }

    /// Bound obsolete operational history to eight terminal signatures per
    /// channel, retaining the effective signature and its published report.
    pub async fn prune(
        &self,
        context: &VaultContext,
        channel: &str,
        current: &str,
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let obsolete: Vec<String> = sqlx::query_scalar("SELECT signature FROM retrieval_calibration_runs WHERE vault_id=? AND channel=? AND signature<>? AND status IN ('passed','quality_failed','failed','cancelled') ORDER BY updated_at DESC,signature LIMIT -1 OFFSET 8")
            .bind(context.id().to_string()).bind(channel).bind(current).fetch_all(&mut *tx).await?;
        for signature in obsolete {
            sqlx::query("DELETE FROM vault_settings WHERE vault_id=? AND key=?")
                .bind(context.id().to_string())
                .bind(format!(
                    "retrieval.calibration.active.{channel}.{signature}"
                ))
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM retrieval_calibration_runs WHERE vault_id=? AND channel=? AND signature=?")
                .bind(context.id().to_string()).bind(channel).bind(signature).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Atomically publish only a saved passed report while maintenance is
    /// permitted. Signature-specific keys prevent obsolete-task overwrite.
    pub async fn publish(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
        report: &Value,
    ) -> Result<(), StateError> {
        let report = serde_json::to_string(report)?;
        if report.len() > 256 * 1024 {
            return Err(StateError::InvalidInput("calibration report exceeds bound"));
        }
        let changed = sqlx::query("INSERT INTO vault_settings(vault_id,key,value_json,revision,updated_at,updated_by) SELECT vault_id,?,report_json,1,?,NULL FROM retrieval_calibration_runs WHERE vault_id=? AND channel=? AND signature=? AND status='passed' AND report_json=? AND NOT EXISTS(SELECT 1 FROM vault_settings s WHERE s.vault_id=retrieval_calibration_runs.vault_id AND s.key='retrieval.calibration.automatic' AND s.value_json='false') ON CONFLICT(vault_id,key) DO UPDATE SET value_json=excluded.value_json,revision=vault_settings.revision+1,updated_at=excluded.updated_at")
            .bind(format!("retrieval.calibration.active.{channel}.{signature}")).bind(now_millis()?).bind(context.id().to_string()).bind(channel).bind(signature).bind(report).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    /// Read only the requested Vault/channel/signature.
    pub async fn get(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
    ) -> Result<Option<CalibrationRun>, StateError> {
        Ok(sqlx::query_as("SELECT channel,signature,status,checkpoint_json,report_json,requests,request_bytes,request_limit,byte_limit,budget_json,started_at,updated_at FROM retrieval_calibration_runs WHERE vault_id=? AND channel=? AND signature=?")
            .bind(context.id().to_string()).bind(channel).bind(signature).fetch_optional(&self.pool).await?)
    }

    /// Atomically charge every HTTP attempt before network dispatch, including
    /// adapter retries. The spent budget survives failures and process restarts.
    #[allow(clippy::too_many_arguments)]
    pub async fn reserve_request(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
        bytes: usize,
        deadline_ms: i64,
    ) -> Result<(), StateError> {
        let now = now_millis()?;
        let changed = sqlx::query("UPDATE retrieval_calibration_runs SET requests=requests+1,request_bytes=request_bytes+?,status='running',updated_at=? WHERE vault_id=? AND channel=? AND signature=? AND status IN ('pending','running') AND requests < request_limit AND request_bytes+? <= byte_limit AND started_at+? >= ?")
            .bind(i64::try_from(bytes).map_err(|_| StateError::InvalidInput("calibration request too large"))?).bind(now)
            .bind(context.id().to_string()).bind(channel).bind(signature).bind(bytes as i64).bind(deadline_ms).bind(now).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StateError::InvalidInput(
                "calibration budget exhausted or stopped",
            ));
        }
        Ok(())
    }

    /// An explicit Admin retry allocates one new bounded round. Lifetime
    /// consumption is retained; restart/automatic compensation cannot do this.
    pub async fn retry(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
    ) -> Result<bool, StateError> {
        let changed=sqlx::query("UPDATE retrieval_calibration_runs SET status='pending',previous_report_json=report_json,report_json=NULL,request_limit=requests+json_extract(budget_json,'$.max_requests'),byte_limit=request_bytes+json_extract(budget_json,'$.max_bytes'),started_at=?,updated_at=? WHERE vault_id=? AND channel=? AND signature=? AND status IN ('failed','quality_failed','cancelled')")
            .bind(now_millis()?).bind(now_millis()?).bind(context.id().to_string()).bind(channel).bind(signature).execute(&self.pool).await?;
        Ok(changed.rows_affected() == 1)
    }

    /// Save a completed batch before requesting the next one.
    pub async fn checkpoint(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
        value: &Value,
    ) -> Result<(), StateError> {
        let value = serde_json::to_string(value)?;
        // High-dimensional real embeddings exceed 4 MiB even for the bundled
        // 160-input corpus. This remains a bounded, non-business cache.
        if value.len() > 32 * 1024 * 1024 {
            return Err(StateError::InvalidInput(
                "calibration checkpoint exceeds bound",
            ));
        }
        let changed=sqlx::query("UPDATE retrieval_calibration_runs SET checkpoint_json=?,updated_at=? WHERE vault_id=? AND channel=? AND signature=? AND status IN ('pending','running')")
            .bind(value).bind(now_millis()?).bind(context.id().to_string()).bind(channel).bind(signature).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    /// Stop an explicitly cancelled active job, including a saved report that
    /// has not yet published. Existing applicable query settings remain intact.
    pub async fn cancel(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
    ) -> Result<(), StateError> {
        sqlx::query("UPDATE retrieval_calibration_runs SET status='cancelled',updated_at=? WHERE vault_id=? AND channel=? AND signature=? AND status IN ('pending','running','passed')")
            .bind(now_millis()?).bind(context.id().to_string()).bind(channel).bind(signature).execute(&self.pool).await?;
        Ok(())
    }

    /// Save the report before publication, permitting crash recovery to reuse it.
    pub async fn finish(
        &self,
        context: &VaultContext,
        channel: &str,
        signature: &str,
        status: &str,
        report: &Value,
    ) -> Result<(), StateError> {
        let report = serde_json::to_string(report)?;
        if report.len() > 256 * 1024 {
            return Err(StateError::InvalidInput("calibration report exceeds bound"));
        }
        sqlx::query("UPDATE retrieval_calibration_runs SET status=?,report_json=?,updated_at=? WHERE vault_id=? AND channel=? AND signature=? AND status IN ('pending','running')")
            .bind(status).bind(report).bind(now_millis()?).bind(context.id().to_string()).bind(channel).bind(signature).execute(&self.pool).await?;
        Ok(())
    }
}

/// Synthetic input for an isolated, short-lived benchmark FTS index.
#[derive(Clone)]
pub struct CalibrationDocument {
    /// Stable benchmark label (never a business memory ID).
    pub id: String,
    /// Synthetic path used by production note input rules.
    pub path: String,
    /// Synthetic note title.
    pub title: String,
    /// Body used by both FTS and relevance admission.
    pub content: String,
    /// Exact production-normalized memory body.
    pub normalized_content: String,
}

/// In-memory FTS candidate collection with the production column/tokenizer
/// shapes. No benchmark document is inserted into a business database.
pub struct CalibrationLexicalIndex {
    pool: SqlitePool,
    channel: String,
}
impl CalibrationLexicalIndex {
    /// Build one split's isolated candidate corpus.
    pub async fn new(
        context: &VaultContext,
        channel: &str,
        documents: &[CalibrationDocument],
    ) -> Result<Self, StateError> {
        if !matches!(channel, "memory" | "note") || documents.len() > 512 {
            return Err(StateError::InvalidInput("calibration corpus invalid"));
        }
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        if channel == "memory" {
            sqlx::query("CREATE VIRTUAL TABLE corpus USING fts5(vault_id UNINDEXED,object_id UNINDEXED,content,normalized_content,entities,tags,search_terms,tokenize='unicode61 remove_diacritics 2')").execute(&pool).await?;
            for doc in documents {
                sqlx::query("INSERT INTO corpus(vault_id,object_id,content,normalized_content,entities,tags,search_terms) VALUES (?,?,?,?,'','',?)")
                    .bind(context.id().to_string()).bind(&doc.id).bind(&doc.content).bind(&doc.normalized_content).bind(crate::memory_search_terms([doc.content.as_str()],4096)).execute(&pool).await?;
            }
        } else {
            sqlx::query("CREATE VIRTUAL TABLE corpus USING fts5(vault_id UNINDEXED,object_id UNINDEXED,path,title,aliases,tags,headings,plain_text,tokenize='unicode61 remove_diacritics 2')").execute(&pool).await?;
            for doc in documents {
                sqlx::query("INSERT INTO corpus(vault_id,object_id,path,title,aliases,tags,headings,plain_text) VALUES (?,?,?,?,'','',?,?)")
                    .bind(context.id().to_string()).bind(&doc.id).bind(&doc.path).bind(&doc.title).bind(&doc.title).bind(format!("{}\n\n{}",doc.title,doc.content)).execute(&pool).await?;
            }
        }
        Ok(Self {
            pool,
            channel: channel.to_owned(),
        })
    }
    /// Collect bounded real BM25 candidate ranks. Context is still mandatory.
    pub async fn candidates(
        &self,
        context: &VaultContext,
        fts_query: &str,
    ) -> Result<Vec<(String, f64)>, StateError> {
        if fts_query.len() > 16384 {
            return Err(StateError::InvalidInput("calibration query exceeds bound"));
        }
        let sql = if self.channel == "memory" {
            "SELECT object_id,bm25(corpus) AS rank FROM corpus WHERE vault_id=? AND corpus MATCH ? ORDER BY rank,object_id LIMIT 50"
        } else {
            "SELECT object_id,bm25(corpus) AS rank FROM corpus WHERE vault_id=? AND corpus MATCH ? ORDER BY rank,path,object_id LIMIT 100"
        };
        Ok(sqlx::query_as(sql)
            .bind(context.id().to_string())
            .bind(fts_query)
            .fetch_all(&self.pool)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StateStore, VaultStatus};
    use mcp_vault_domain::{Revision, VaultId, VaultSlug};

    #[tokio::test]
    async fn budget_checkpoint_stop_retry_and_vault_isolation() {
        let directory = tempfile::tempdir().unwrap();
        let database = format!(
            "sqlite://{}",
            directory.path().join("calibration.sqlite").display()
        );
        let state = StateStore::connect_and_migrate(&database).await.unwrap();
        let mut contexts = Vec::new();
        for slug in ["calibration-one", "calibration-two"] {
            let context = VaultContext::new(
                VaultId::new(),
                VaultSlug::new(slug).unwrap(),
                format!("/tmp/{slug}").into(),
                Revision::ZERO,
            )
            .unwrap();
            state
                .vaults()
                .insert(&context, slug, VaultStatus::Active)
                .await
                .unwrap();
            contexts.push(context);
        }
        let repo = state.calibrations();
        let context = &contexts[0];
        repo.ensure(context, "memory", "signature").await.unwrap();
        // A real 3072-dimensional corpus exceeds the former 4 MiB bound.
        let large = serde_json::json!({"cache": vec![vec![0.12345679_f32; 3072]; 160]});
        assert!(serde_json::to_string(&large).unwrap().len() > 4 * 1024 * 1024);
        repo.checkpoint(context, "memory", "signature", &large)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(
                &repo
                    .get(context, "memory", "signature")
                    .await
                    .unwrap()
                    .unwrap()
                    .checkpoint_json
            )
            .unwrap(),
            large
        );
        repo.checkpoint(
            context,
            "memory",
            "signature",
            &serde_json::json!({"cache":[1.0,0.0]}),
        )
        .await
        .unwrap();
        for _ in 0..32 {
            repo.reserve_request(context, "memory", "signature", 100, 900000)
                .await
                .unwrap();
        }
        assert!(
            repo.reserve_request(context, "memory", "signature", 100, 900000)
                .await
                .is_err()
        );
        let reopened = StateStore::connect_and_migrate(&database).await.unwrap();
        let recovered = reopened
            .calibrations()
            .ensure(context, "memory", "signature")
            .await
            .unwrap();
        assert!(
            reopened
                .calibrations()
                .reserve_request(context, "memory", "signature", 100, 900000)
                .await
                .is_err(),
            "reopening the database must not refill the spent budget"
        );
        assert_eq!(recovered.requests, 32);
        assert_eq!(recovered.request_bytes, 3200);
        assert!(recovered.checkpoint_json.contains("cache"));
        assert!(
            repo.get(&contexts[1], "memory", "signature")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            repo.get(context, "note", "signature")
                .await
                .unwrap()
                .is_none()
        );
        repo.finish(
            context,
            "memory",
            "signature",
            "cancelled",
            &serde_json::json!({"error_code":"cancelled"}),
        )
        .await
        .unwrap();
        assert!(
            repo.reserve_request(context, "memory", "signature", 1, 900000)
                .await
                .is_err()
        );
        assert!(repo.retry(context, "memory", "signature").await.unwrap());
        let retry = repo
            .get(context, "memory", "signature")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retry.requests, 32);
        assert_eq!(retry.request_limit, 64);
        assert_eq!(retry.checkpoint_json, recovered.checkpoint_json);
        repo.reserve_request(context, "memory", "signature", 100, 900000)
            .await
            .unwrap();
    }
}
