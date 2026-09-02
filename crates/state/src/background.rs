//! Durable outbox, job, and scan-checkpoint repositories.
//!
//! Worker orchestration lives above this module. These repositories own all
//! SQL transitions so a process crash leaves rows reclaimable and observable.

use serde_json::Value;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

use mcp_vault_domain::{EventId, JobId, ScanId, VaultContext, VaultId, VaultPath};

use crate::{StateError, now_millis};

const MAX_WORKER_ID_BYTES: usize = 128;
const MAX_JOB_TYPE_BYTES: usize = 128;
const MAX_DEDUP_KEY_BYTES: usize = 512;
const MAX_ERROR_BYTES: usize = 2048;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_BATCH_SIZE: u32 = 128;

/// One durable event awaiting handler admission/acknowledgement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxEventRecord {
    /// Stable event ID.
    pub id: EventId,
    /// Optional Vault scope; file events always carry one.
    pub vault_id: Option<VaultId>,
    /// Stable event type.
    pub event_type: String,
    /// Aggregate category.
    pub aggregate_type: String,
    /// Aggregate ID.
    pub aggregate_id: String,
    /// Bounded event metadata.
    pub payload: Value,
    /// Creation timestamp.
    pub created_at: i64,
    /// Earliest retry timestamp.
    pub available_at: i64,
    /// Current lease owner.
    pub claimed_by: Option<String>,
    /// Current lease expiry.
    pub claimed_until: Option<i64>,
    /// Successful delivery/ack timestamp.
    pub delivered_at: Option<i64>,
    /// Number of claim attempts.
    pub attempts: u32,
    /// Last safe error summary.
    pub last_error: Option<String>,
    /// Terminal dead-letter marker.
    pub dead_lettered: bool,
    /// Reason for terminal dead-lettering.
    pub dead_letter_reason: Option<String>,
}

/// Persistent job lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStatus {
    /// Waiting for a worker.
    Queued,
    /// Currently leased to a worker.
    Running,
    /// Waiting for a retry timestamp.
    RetryWait,
    /// Handler completed successfully.
    Completed,
    /// Retry policy exhausted or permanently failed.
    Failed,
    /// Explicitly cancelled.
    Cancelled,
}

impl JobStatus {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::RetryWait => "retry_wait",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "retry_wait" => Ok(Self::RetryWait),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(StateError::InvalidInput("stored job status is invalid")),
        }
    }
}

/// Exact Vault-scoped job counts used by operational projections.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JobStatusCounts {
    /// Jobs not yet leased.
    pub queued: u64,
    /// Jobs with an active worker lease.
    pub running: u64,
    /// Jobs waiting for their next retry timestamp.
    pub retry_wait: u64,
    /// Successfully completed jobs.
    pub completed: u64,
    /// Permanently failed jobs.
    pub failed: u64,
    /// Explicitly cancelled jobs.
    pub cancelled: u64,
}

/// One leased persistent job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRecord {
    /// Stable job ID.
    pub id: JobId,
    /// Optional Vault scope; global jobs are explicit.
    pub vault_id: Option<VaultId>,
    /// Stable handler category.
    pub job_type: String,
    /// Vault-scoped or global deterministic deduplication key.
    pub dedup_key: String,
    /// Bounded handler input.
    pub payload: Value,
    /// Lifecycle status.
    pub status: JobStatus,
    /// Higher values are claimed first.
    pub priority: i32,
    /// Number of claims.
    pub attempts: u32,
    /// Maximum claims before failed state.
    pub max_attempts: u32,
    /// Earliest claim timestamp.
    pub available_at: i64,
    /// Current lease owner.
    pub lease_owner: Option<String>,
    /// Current lease expiry.
    pub lease_until: Option<i64>,
    /// Bounded structured progress.
    pub progress: Option<Value>,
    /// Last safe error summary.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last state update timestamp.
    pub updated_at: i64,
    /// Completion timestamp.
    pub completed_at: Option<i64>,
    /// Cooperative cancellation requested by an operator.
    pub cancel_requested: bool,
}

/// Durable scan lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStatus {
    /// A generation is running.
    Running,
    /// The generation completed a full pass.
    Completed,
    /// The generation stopped with an error.
    Failed,
    /// The generation was cancelled.
    Cancelled,
}

impl ScanStatus {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(StateError::InvalidInput("stored scan status is invalid")),
        }
    }
}

/// Resumable per-Vault scan checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanCheckpointRecord {
    /// Stable checkpoint row ID.
    pub id: ScanId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Scan category, e.g. `initial` or `reconciliation`.
    pub scan_type: String,
    /// Generation ID used to reject stale progress updates.
    pub generation: String,
    /// Last safely observed relative path.
    pub cursor_path: Option<VaultPath>,
    /// Lifecycle status.
    pub status: ScanStatus,
    /// Total entries observed.
    pub entries_seen: u64,
    /// Files observed.
    pub files_seen: u64,
    /// Directories observed.
    pub directories_seen: u64,
    /// External changes imported through Core.
    pub changes_imported: u64,
    /// Unsafe/reserved/invalid entries skipped by the scanner.
    pub unsafe_entries_skipped: u64,
    /// Whether missing-file deletion inference was suppressed.
    pub missing_deletes_skipped: bool,
    /// Last redacted error.
    pub last_error: Option<String>,
    /// Start timestamp.
    pub started_at: i64,
    /// Last update timestamp.
    pub updated_at: i64,
    /// Completion timestamp.
    pub completed_at: Option<i64>,
}

#[derive(Debug, FromRow)]
struct OutboxRow {
    id: String,
    vault_id: Option<String>,
    event_type: String,
    aggregate_type: String,
    aggregate_id: String,
    payload_json: String,
    created_at: i64,
    available_at: i64,
    claimed_by: Option<String>,
    claimed_until: Option<i64>,
    delivered_at: Option<i64>,
    attempts: i64,
    last_error: Option<String>,
    dead_lettered: i64,
    dead_letter_reason: Option<String>,
}

#[derive(Debug, FromRow)]
struct JobRow {
    id: String,
    vault_id: Option<String>,
    job_type: String,
    dedup_key: String,
    payload_json: String,
    status: String,
    priority: i64,
    attempts: i64,
    max_attempts: i64,
    available_at: i64,
    lease_owner: Option<String>,
    lease_until: Option<i64>,
    progress_json: Option<String>,
    last_error: Option<String>,
    created_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
    cancel_requested: i64,
}

#[derive(Debug, FromRow)]
struct ScanCheckpointRow {
    id: String,
    vault_id: String,
    scan_type: String,
    generation: String,
    cursor_path: Option<String>,
    status: String,
    entries_seen: i64,
    files_seen: i64,
    directories_seen: i64,
    changes_imported: i64,
    unsafe_entries_skipped: i64,
    missing_deletes_skipped: i64,
    last_error: Option<String>,
    started_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
}

/// Repository for durable transactional outbox delivery.
#[derive(Clone)]
pub struct OutboxRepository {
    pool: SqlitePool,
}

impl OutboxRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Claim up to `limit` available events with one conditional lease.
    pub async fn claim_batch(
        &self,
        worker_id: &str,
        now: i64,
        lease_until: i64,
        limit: u32,
    ) -> Result<Vec<OutboxEventRecord>, StateError> {
        validate_worker(worker_id)?;
        validate_lease(now, lease_until)?;
        let limit = validate_batch(limit)?;
        let mut transaction = self.pool.begin().await?;
        let candidates = sqlx::query_as::<_, OutboxRow>(
            "SELECT id, vault_id, event_type, aggregate_type, aggregate_id,
                    payload_json, created_at, available_at, claimed_by,
                    claimed_until, delivered_at, attempts, last_error,
                    dead_lettered, dead_letter_reason
             FROM outbox_events
             WHERE dead_lettered = 0 AND delivered_at IS NULL
               AND available_at <= ?
               AND (claimed_until IS NULL OR claimed_until <= ?)
             ORDER BY created_at ASC, id ASC
             LIMIT ?",
        )
        .bind(now)
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await?;

        let mut claimed = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let result = sqlx::query(
                "UPDATE outbox_events
                 SET claimed_by = ?, claimed_until = ?, attempts = attempts + 1
                 WHERE id = ? AND dead_lettered = 0 AND delivered_at IS NULL
                   AND available_at <= ?
                   AND (claimed_until IS NULL OR claimed_until <= ?)",
            )
            .bind(worker_id)
            .bind(lease_until)
            .bind(&candidate.id)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() == 1 {
                let mut record = row_to_outbox(candidate)?;
                record.claimed_by = Some(worker_id.to_owned());
                record.claimed_until = Some(lease_until);
                record.attempts = record.attempts.saturating_add(1);
                claimed.push(record);
            }
        }
        transaction.commit().await?;
        Ok(claimed)
    }

    /// Mark a leased event delivered exactly once.
    pub async fn ack(
        &self,
        event_id: EventId,
        worker_id: &str,
        now: i64,
    ) -> Result<(), StateError> {
        validate_worker(worker_id)?;
        let result = sqlx::query(
            "UPDATE outbox_events
             SET delivered_at = ?, claimed_by = NULL, claimed_until = NULL,
                 last_error = NULL
             WHERE id = ? AND claimed_by = ? AND delivered_at IS NULL
               AND dead_lettered = 0",
        )
        .bind(now)
        .bind(event_id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("outbox lease is not owned"));
        }
        Ok(())
    }

    /// Retry a leased event or dead-letter it when the attempt budget is
    /// exhausted.
    pub async fn retry_or_dead_letter(
        &self,
        event_id: EventId,
        worker_id: &str,
        available_at: i64,
        error: &str,
        max_attempts: u32,
    ) -> Result<bool, StateError> {
        validate_worker(worker_id)?;
        validate_error(error)?;
        if max_attempts == 0 {
            return Err(StateError::InvalidInput("outbox max attempts is invalid"));
        }
        let result = sqlx::query(
            "UPDATE outbox_events
             SET available_at = CASE WHEN attempts >= ? THEN available_at ELSE ? END,
                 last_error = ?,
                 dead_lettered = CASE WHEN attempts >= ? THEN 1 ELSE 0 END,
                 dead_letter_reason = CASE WHEN attempts >= ? THEN ? ELSE NULL END,
                 claimed_by = NULL, claimed_until = NULL
             WHERE id = ? AND claimed_by = ? AND delivered_at IS NULL
               AND dead_lettered = 0",
        )
        .bind(i64::from(max_attempts))
        .bind(available_at)
        .bind(error)
        .bind(i64::from(max_attempts))
        .bind(i64::from(max_attempts))
        .bind(error)
        .bind(event_id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("outbox lease is not owned"));
        }
        Ok(self
            .get(event_id)
            .await?
            .is_some_and(|event| event.dead_lettered))
    }

    /// Release a worker's leases immediately during graceful shutdown.
    pub async fn release_worker_leases(&self, worker_id: &str) -> Result<(), StateError> {
        validate_worker(worker_id)?;
        sqlx::query(
            "UPDATE outbox_events
             SET claimed_by = NULL, claimed_until = NULL
             WHERE claimed_by = ? AND delivered_at IS NULL AND dead_lettered = 0",
        )
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetch one event for diagnostics/handler tests.
    pub async fn get(&self, event_id: EventId) -> Result<Option<OutboxEventRecord>, StateError> {
        let row = sqlx::query_as::<_, OutboxRow>(
            "SELECT id, vault_id, event_type, aggregate_type, aggregate_id,
                    payload_json, created_at, available_at, claimed_by,
                    claimed_until, delivered_at, attempts, last_error,
                    dead_lettered, dead_letter_reason
             FROM outbox_events WHERE id = ?",
        )
        .bind(event_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_outbox).transpose()
    }

    /// List events for one registered Vault aggregate in creation order.
    pub async fn find_by_aggregate(
        &self,
        context: &VaultContext,
        aggregate_id: &str,
    ) -> Result<Vec<OutboxEventRecord>, StateError> {
        self.ensure_context(context).await?;
        validate_aggregate_id(aggregate_id)?;
        let rows = sqlx::query_as::<_, OutboxRow>(
            "SELECT id, vault_id, event_type, aggregate_type, aggregate_id,
                    payload_json, created_at, available_at, claimed_by,
                    claimed_until, delivered_at, attempts, last_error,
                    dead_lettered, dead_letter_reason
             FROM outbox_events
             WHERE vault_id = ? AND aggregate_id = ?
             ORDER BY created_at ASC, id ASC",
        )
        .bind(context.id().to_string())
        .bind(aggregate_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_outbox).collect()
    }

    /// Count events not yet acknowledged/dead-lettered.
    pub async fn pending_count(&self) -> Result<u64, StateError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outbox_events
             WHERE delivered_at IS NULL AND dead_lettered = 0",
        )
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(count).map_err(|_| StateError::InvalidInput("outbox count is invalid"))
    }

    async fn ensure_context(&self, context: &VaultContext) -> Result<(), StateError> {
        let root = context
            .content_root()
            .to_str()
            .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM vaults WHERE id = ? AND slug = ? AND content_root = ?
             )",
        )
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .bind(root)
        .fetch_one(&self.pool)
        .await?;
        if exists == 0 {
            return Err(StateError::InvalidInput("Vault context is not registered"));
        }
        Ok(())
    }
}

/// Repository for durable, deduplicated background jobs.
#[derive(Clone)]
pub struct JobRepository {
    pool: SqlitePool,
}

impl JobRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Enqueue or return the existing Vault-scoped deduplicated job.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue(
        &self,
        context: &VaultContext,
        job_type: &str,
        dedup_key: &str,
        payload: &Value,
        priority: i32,
        max_attempts: u32,
        available_at: i64,
    ) -> Result<JobRecord, StateError> {
        self.ensure_context(context).await?;
        self.enqueue_inner(
            Some(context.id()),
            job_type,
            dedup_key,
            payload,
            priority,
            max_attempts,
            available_at,
        )
        .await
    }

    /// Enqueue at most one non-terminal job of this type for a Vault.
    ///
    /// A schema-level partial unique index supplies cross-task/process safety
    /// for singleton job types; callers still provide a per-trigger dedup key
    /// so a later trigger can proceed after an earlier terminal failure.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_singleton(
        &self,
        context: &VaultContext,
        job_type: &str,
        dedup_key: &str,
        payload: &Value,
        priority: i32,
        max_attempts: u32,
        available_at: i64,
    ) -> Result<JobRecord, StateError> {
        if let Some(existing) = self.find_active_by_type(context, job_type).await? {
            return Ok(existing);
        }
        match self
            .enqueue(
                context,
                job_type,
                dedup_key,
                payload,
                priority,
                max_attempts,
                available_at,
            )
            .await
        {
            Ok(job) => Ok(job),
            Err(error) => {
                if let Some(existing) = self.find_active_by_type(context, job_type).await? {
                    Ok(existing)
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Enqueue or return an explicitly global job.
    pub async fn enqueue_global(
        &self,
        job_type: &str,
        dedup_key: &str,
        payload: &Value,
        priority: i32,
        max_attempts: u32,
        available_at: i64,
    ) -> Result<JobRecord, StateError> {
        self.enqueue_inner(
            None,
            job_type,
            dedup_key,
            payload,
            priority,
            max_attempts,
            available_at,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn enqueue_inner(
        &self,
        vault_id: Option<VaultId>,
        job_type: &str,
        dedup_key: &str,
        payload: &Value,
        priority: i32,
        max_attempts: u32,
        available_at: i64,
    ) -> Result<JobRecord, StateError> {
        validate_job_labels(job_type, dedup_key)?;
        validate_attempts(max_attempts)?;
        let payload_json = validate_payload(payload)?;
        if let Some(existing) = self.find_by_scope(vault_id, dedup_key).await? {
            return Ok(existing);
        }
        let now = now_millis()?;
        let id = JobId::new();
        let result = sqlx::query(
            "INSERT INTO jobs
             (id, vault_id, job_type, dedup_key, payload_json, status,
              priority, max_attempts, available_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(vault_id.map(|id| id.to_string()))
        .bind(job_type)
        .bind(dedup_key)
        .bind(payload_json)
        .bind(priority)
        .bind(i64::from(max_attempts))
        .bind(available_at)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await;
        if let Err(error) = result {
            if let Some(existing) = self.find_by_scope(vault_id, dedup_key).await? {
                return Ok(existing);
            }
            return Err(StateError::Database(error));
        }
        self.get_unscoped(id)
            .await?
            .ok_or(StateError::InvalidInput("inserted job was not found"))
    }

    /// Find a Vault-scoped job by deterministic deduplication key.
    pub async fn find_by_dedup(
        &self,
        context: &VaultContext,
        dedup_key: &str,
    ) -> Result<Option<JobRecord>, StateError> {
        self.ensure_context(context).await?;
        self.find_by_scope(Some(context.id()), dedup_key).await
    }

    /// Return the newest non-terminal job of one type in this exact Vault.
    pub async fn find_active_by_type(
        &self,
        context: &VaultContext,
        job_type: &str,
    ) -> Result<Option<JobRecord>, StateError> {
        self.ensure_context(context).await?;
        validate_job_labels(job_type, "active-job-type")?;
        let row = sqlx::query_as::<_, JobRow>(
            "SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                    priority, attempts, max_attempts, available_at, lease_owner,
                    lease_until, progress_json, last_error, created_at,
                    updated_at, completed_at, cancel_requested
             FROM jobs
             WHERE vault_id = ? AND job_type = ?
               AND status IN ('queued', 'running', 'retry_wait')
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
        )
        .bind(context.id().to_string())
        .bind(job_type)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_job).transpose()
    }

    /// Claim queued/retry or expired-running jobs with one worker lease.
    pub async fn claim_batch(
        &self,
        worker_id: &str,
        now: i64,
        lease_until: i64,
        limit: u32,
    ) -> Result<Vec<JobRecord>, StateError> {
        validate_worker(worker_id)?;
        validate_lease(now, lease_until)?;
        let limit = validate_batch(limit)?;
        let mut transaction = self.pool.begin().await?;
        let candidates = sqlx::query_as::<_, JobRow>(
            "WITH eligible AS (
               SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                      priority, attempts, max_attempts, available_at, lease_owner,
                      lease_until, progress_json, last_error, created_at,
                      updated_at, completed_at, cancel_requested,
                      ROW_NUMBER() OVER (
                        PARTITION BY COALESCE(vault_id, '__global__')
                        ORDER BY priority DESC, created_at ASC, id ASC
                      ) AS vault_rank
               FROM jobs
               WHERE status IN ('queued', 'retry_wait', 'running')
                 AND available_at <= ?
                 AND (lease_until IS NULL OR lease_until <= ?)
                 AND (attempts < max_attempts OR cancel_requested = 1)
             )
             SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                    priority, attempts, max_attempts, available_at, lease_owner,
                    lease_until, progress_json, last_error, created_at,
                    updated_at, completed_at, cancel_requested
             FROM eligible
             ORDER BY priority DESC, vault_rank ASC, created_at ASC, id ASC
             LIMIT ?",
        )
        .bind(now)
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await?;
        let mut claimed = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let result = sqlx::query(
                "UPDATE jobs
                 SET status = 'running', lease_owner = ?, lease_until = ?,
                     attempts = attempts + 1, updated_at = ?
                 WHERE id = ? AND status IN ('queued', 'retry_wait', 'running')
                   AND available_at <= ?
                   AND (lease_until IS NULL OR lease_until <= ?)
                   AND (attempts < max_attempts OR cancel_requested = 1)",
            )
            .bind(worker_id)
            .bind(lease_until)
            .bind(now)
            .bind(&candidate.id)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() == 1 {
                let mut record = row_to_job(candidate)?;
                record.status = JobStatus::Running;
                record.lease_owner = Some(worker_id.to_owned());
                record.lease_until = Some(lease_until);
                record.attempts = record.attempts.saturating_add(1);
                claimed.push(record);
            }
        }
        transaction.commit().await?;
        Ok(claimed)
    }

    /// Store bounded progress for a worker-owned job.
    pub async fn update_progress(
        &self,
        job_id: JobId,
        worker_id: &str,
        progress: &Value,
    ) -> Result<(), StateError> {
        validate_worker(worker_id)?;
        let progress_json = validate_payload(progress)?;
        let result = sqlx::query(
            "UPDATE jobs SET progress_json = ?, updated_at = ?
             WHERE id = ? AND status = 'running' AND lease_owner = ?",
        )
        .bind(progress_json)
        .bind(now_millis()?)
        .bind(job_id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("job lease is not owned"));
        }
        Ok(())
    }

    /// Mark a worker-owned job completed.
    pub async fn complete(&self, job_id: JobId, worker_id: &str) -> Result<(), StateError> {
        self.finish(job_id, worker_id, JobStatus::Completed, None, None)
            .await
    }

    /// Retry a worker-owned job or transition it to failed after max attempts.
    pub async fn retry_or_fail(
        &self,
        job_id: JobId,
        worker_id: &str,
        available_at: i64,
        error: &str,
    ) -> Result<JobStatus, StateError> {
        validate_error(error)?;
        let job = self
            .get_unscoped(job_id)
            .await?
            .ok_or(StateError::InvalidInput("job does not exist"))?;
        if job.lease_owner.as_deref() != Some(worker_id) || job.status != JobStatus::Running {
            return Err(StateError::InvalidInput("job lease is not owned"));
        }
        let status = if job.attempts >= job.max_attempts {
            JobStatus::Failed
        } else {
            JobStatus::RetryWait
        };
        self.finish(job_id, worker_id, status, Some(available_at), Some(error))
            .await?;
        Ok(status)
    }

    /// Mark a worker-owned job permanently failed without consuming another
    /// retry attempt.
    pub async fn fail_permanently(
        &self,
        job_id: JobId,
        worker_id: &str,
        error: &str,
    ) -> Result<(), StateError> {
        validate_error(error)?;
        self.finish(job_id, worker_id, JobStatus::Failed, None, Some(error))
            .await
    }

    /// Mark a cancelled worker-owned job terminal.
    pub async fn cancel_claimed(&self, job_id: JobId, worker_id: &str) -> Result<(), StateError> {
        self.finish(
            job_id,
            worker_id,
            JobStatus::Cancelled,
            None,
            Some("cancelled"),
        )
        .await
    }

    /// Return a claimed job to the queue without consuming an attempt. This
    /// is used when no handler is registered for the job type yet.
    pub async fn release_claimed(
        &self,
        job_id: JobId,
        worker_id: &str,
        available_at: i64,
    ) -> Result<(), StateError> {
        validate_worker(worker_id)?;
        let result = sqlx::query(
            "UPDATE jobs
             SET status = 'queued', available_at = ?, lease_owner = NULL,
                 lease_until = NULL,
                 attempts = CASE WHEN attempts > 0 THEN attempts - 1 ELSE 0 END,
                 updated_at = ?
             WHERE id = ? AND status = 'running' AND lease_owner = ?
               AND cancel_requested = 0",
        )
        .bind(available_at)
        .bind(now_millis()?)
        .bind(job_id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("job lease is not owned"));
        }
        Ok(())
    }

    /// Request cancellation; queued jobs become terminal immediately and
    /// running jobs are observed by their handler at the next checkpoint.
    pub async fn request_cancel(
        &self,
        context: &VaultContext,
        job_id: JobId,
    ) -> Result<(), StateError> {
        self.ensure_context(context).await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE jobs
             SET cancel_requested = 1,
                 status = CASE WHEN status IN ('queued', 'retry_wait') THEN 'cancelled' ELSE status END,
                 completed_at = CASE WHEN status IN ('queued', 'retry_wait') THEN ? ELSE completed_at END,
                 updated_at = ?
             WHERE id = ? AND vault_id = ? AND status NOT IN ('completed', 'failed', 'cancelled')
               AND NOT (
                 status = 'running'
                 AND (job_type LIKE 'backup.%'
                      OR job_type IN ('memory.consolidate', 'memory.reset_pipeline'))
               )",
        )
        .bind(now)
        .bind(now)
        .bind(job_id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("job is not cancellable"));
        }
        Ok(())
    }

    /// Request cancellation for every non-terminal job of one Vault/type.
    pub async fn request_cancel_type(
        &self,
        context: &VaultContext,
        job_type: &str,
    ) -> Result<u64, StateError> {
        self.ensure_context(context).await?;
        validate_job_labels(job_type, "job-type-cancel")?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE jobs
             SET cancel_requested = 1,
                 status = CASE WHEN status IN ('queued', 'retry_wait') THEN 'cancelled' ELSE status END,
                 completed_at = CASE WHEN status IN ('queued', 'retry_wait') THEN ? ELSE completed_at END,
                 updated_at = ?
             WHERE vault_id = ? AND job_type = ?
               AND status IN ('queued', 'running', 'retry_wait')
               AND NOT (
                 status = 'running'
                 AND (job_type LIKE 'backup.%'
                      OR job_type IN ('memory.consolidate', 'memory.reset_pipeline'))
               )",
        )
        .bind(now)
        .bind(now)
        .bind(context.id().to_string())
        .bind(job_type)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Request cancellation for every cancellable non-terminal job of one
    /// Vault. Global and other-Vault jobs are never selected.
    pub async fn request_cancel_all(&self, context: &VaultContext) -> Result<u64, StateError> {
        self.ensure_context(context).await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE jobs
             SET cancel_requested = 1,
                 status = CASE WHEN status IN ('queued', 'retry_wait') THEN 'cancelled' ELSE status END,
                 completed_at = CASE WHEN status IN ('queued', 'retry_wait') THEN ? ELSE completed_at END,
                 updated_at = ?
             WHERE vault_id = ?
               AND status IN ('queued', 'running', 'retry_wait')
               AND NOT (
                 status = 'running'
                 AND (job_type LIKE 'backup.%'
                      OR job_type IN ('memory.consolidate', 'memory.reset_pipeline'))
               )",
        )
        .bind(now)
        .bind(now)
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Requeue one failed job after an explicit Admin retry action.
    ///
    /// Memory extraction keeps its paid-work cursor so a full-Vault retry
    /// resumes after the last durably completed note. Other job types retain
    /// their existing reset-to-start behavior.
    pub async fn request_retry(
        &self,
        context: &VaultContext,
        job_id: JobId,
    ) -> Result<(), StateError> {
        self.ensure_context(context).await?;
        let result = sqlx::query(
            "UPDATE jobs
             SET status = 'queued', available_at = ?, lease_owner = NULL,
                 lease_until = NULL, attempts = 0,
                 progress_json = CASE
                     WHEN job_type = 'memory.extract' THEN progress_json
                     ELSE NULL
                 END,
                 cancel_requested = 0, last_error = NULL, completed_at = NULL,
                 updated_at = ?
             WHERE id = ? AND vault_id = ? AND status = 'failed'",
        )
        .bind(now_millis()?)
        .bind(now_millis()?)
        .bind(job_id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("job is not retryable"));
        }
        Ok(())
    }

    /// Restart one failed or cancelled Vault job for an explicit lifecycle
    /// recovery action.
    pub async fn request_restart_terminal(
        &self,
        context: &VaultContext,
        job_id: JobId,
    ) -> Result<(), StateError> {
        self.ensure_context(context).await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE jobs
             SET status = 'queued', available_at = ?, lease_owner = NULL,
                 lease_until = NULL, attempts = 0, progress_json = NULL,
                 cancel_requested = 0, last_error = NULL, completed_at = NULL,
                 updated_at = ?
             WHERE id = ? AND vault_id = ? AND status IN ('failed', 'cancelled')",
        )
        .bind(now)
        .bind(now)
        .bind(job_id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("job is not restartable"));
        }
        Ok(())
    }

    /// Poll the cancellation bit for a worker-owned running job. Losing the
    /// lease also returns `true` so stale work stops instead of committing.
    pub async fn should_cancel_claimed(
        &self,
        job_id: JobId,
        worker_id: &str,
    ) -> Result<bool, StateError> {
        validate_worker(worker_id)?;
        let value: Option<i64> = sqlx::query_scalar(
            "SELECT cancel_requested FROM jobs
             WHERE id = ? AND status = 'running' AND lease_owner = ?",
        )
        .bind(job_id.to_string())
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(value.is_none_or(|value| value != 0))
    }

    /// Renew a running lease and report cancellation/lease loss. The update
    /// is conditional so a stale worker can never take ownership back.
    pub async fn renew_claimed(
        &self,
        job_id: JobId,
        worker_id: &str,
        lease_until: i64,
    ) -> Result<bool, StateError> {
        validate_worker(worker_id)?;
        let now = now_millis()?;
        if lease_until <= now {
            return Err(StateError::InvalidInput("job lease renewal is invalid"));
        }
        let result = sqlx::query(
            "UPDATE jobs SET lease_until = ?, updated_at = ?
             WHERE id = ? AND status = 'running' AND lease_owner = ?
               AND cancel_requested = 0",
        )
        .bind(lease_until)
        .bind(now)
        .bind(job_id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 0)
    }

    /// Return whether a newer non-terminal job of the same type exists in
    /// this exact Vault. Used to collapse event bursts into one full rebuild.
    pub async fn has_newer_active_job(
        &self,
        context: &VaultContext,
        job_type: &str,
        created_at: i64,
        job_id: JobId,
    ) -> Result<bool, StateError> {
        self.ensure_context(context).await?;
        validate_job_labels(job_type, "coalesce-check")?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM jobs
                 WHERE vault_id = ? AND job_type = ?
                   AND status IN ('queued', 'running', 'retry_wait')
                   AND (created_at > ? OR (created_at = ? AND id > ?))
             )",
        )
        .bind(context.id().to_string())
        .bind(job_type)
        .bind(created_at)
        .bind(created_at)
        .bind(job_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        Ok(exists != 0)
    }

    /// Release a worker's job leases for immediate restart reclaim.
    pub async fn release_worker_leases(&self, worker_id: &str) -> Result<(), StateError> {
        validate_worker(worker_id)?;
        sqlx::query(
            "UPDATE jobs SET lease_owner = NULL, lease_until = NULL,
                    status = CASE WHEN status = 'running' THEN 'retry_wait' ELSE status END,
                    updated_at = ?
             WHERE lease_owner = ? AND status = 'running'",
        )
        .bind(now_millis()?)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Read one job only when the caller proves its Vault context.
    pub async fn get(
        &self,
        context: &VaultContext,
        job_id: JobId,
    ) -> Result<Option<JobRecord>, StateError> {
        self.ensure_context(context).await?;
        self.get_scoped(job_id, Some(context.id())).await
    }

    /// List bounded Vault-scoped jobs for Admin diagnostics.
    pub async fn list(
        &self,
        context: &VaultContext,
        status: Option<JobStatus>,
        job_type: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<JobRecord>, StateError> {
        self.ensure_context(context).await?;
        if limit == 0 || limit > 200 || offset > 1_000_000 {
            return Err(StateError::InvalidInput("job page is invalid"));
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                    priority, attempts, max_attempts, available_at, lease_owner,
                    lease_until, progress_json, last_error, created_at,
                    updated_at, completed_at, cancel_requested
             FROM jobs WHERE vault_id = ",
        );
        query.push_bind(context.id().to_string());
        if let Some(status) = status {
            query.push(" AND status = ");
            query.push_bind(status.as_str());
        }
        if let Some(job_type) = job_type {
            validate_job_labels(job_type, "job-type-filter")?;
            query.push(" AND job_type = ");
            query.push_bind(job_type);
        }
        query.push(" ORDER BY created_at DESC, id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));
        query
            .build_query_as::<JobRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(row_to_job)
            .collect()
    }

    /// List bounded terminal history without allowing active work to consume
    /// the history page.
    pub async fn list_terminal(
        &self,
        context: &VaultContext,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<JobRecord>, StateError> {
        self.ensure_context(context).await?;
        if limit == 0 || limit > 200 || offset > 1_000_000 {
            return Err(StateError::InvalidInput("job page is invalid"));
        }
        sqlx::query_as::<_, JobRow>(
            "SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                    priority, attempts, max_attempts, available_at, lease_owner,
                    lease_until, progress_json, last_error, created_at,
                    updated_at, completed_at, cancel_requested
             FROM jobs
             WHERE vault_id = ? AND status IN ('completed', 'failed', 'cancelled')
             ORDER BY COALESCE(completed_at, updated_at) DESC, id DESC
             LIMIT ? OFFSET ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(row_to_job)
        .collect()
    }

    /// Count every lifecycle status for one exact Vault.
    pub async fn status_counts(
        &self,
        context: &VaultContext,
    ) -> Result<JobStatusCounts, StateError> {
        self.ensure_context(context).await?;
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT status, COUNT(*)
             FROM jobs
             WHERE vault_id = ?
             GROUP BY status",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut counts = JobStatusCounts::default();
        for (status, count) in rows {
            let count = u64::try_from(count)
                .map_err(|_| StateError::InvalidInput("job count is invalid"))?;
            match JobStatus::parse(&status)? {
                JobStatus::Queued => counts.queued = count,
                JobStatus::Running => counts.running = count,
                JobStatus::RetryWait => counts.retry_wait = count,
                JobStatus::Completed => counts.completed = count,
                JobStatus::Failed => counts.failed = count,
                JobStatus::Cancelled => counts.cancelled = count,
            }
        }
        Ok(counts)
    }

    /// Count non-terminal jobs for one Vault.
    pub async fn pending_count_for(&self, context: &VaultContext) -> Result<u64, StateError> {
        self.ensure_context(context).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs
             WHERE vault_id = ? AND status IN ('queued', 'running', 'retry_wait')",
        )
        .bind(context.id().to_string())
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(count).map_err(|_| StateError::InvalidInput("job count is invalid"))
    }

    /// Count non-terminal jobs for readiness/health reporting.
    pub async fn pending_count(&self) -> Result<u64, StateError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE status IN ('queued', 'running', 'retry_wait')",
        )
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(count).map_err(|_| StateError::InvalidInput("job count is invalid"))
    }

    async fn finish(
        &self,
        job_id: JobId,
        worker_id: &str,
        status: JobStatus,
        available_at: Option<i64>,
        error: Option<&str>,
    ) -> Result<(), StateError> {
        validate_worker(worker_id)?;
        if let Some(error) = error {
            validate_error(error)?;
        }
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE jobs
             SET status = ?, available_at = COALESCE(?, available_at),
                 lease_owner = NULL, lease_until = NULL, last_error = ?,
                 completed_at = CASE WHEN ? IN ('completed', 'failed', 'cancelled') THEN ? ELSE completed_at END,
                 updated_at = ?
             WHERE id = ? AND status = 'running' AND lease_owner = ?",
        )
        .bind(status.as_str())
        .bind(available_at)
        .bind(error)
        .bind(status.as_str())
        .bind(now)
        .bind(now)
        .bind(job_id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("job lease is not owned"));
        }
        Ok(())
    }

    async fn find_by_scope(
        &self,
        vault_id: Option<VaultId>,
        dedup_key: &str,
    ) -> Result<Option<JobRecord>, StateError> {
        let row = if let Some(vault_id) = vault_id {
            sqlx::query_as::<_, JobRow>(
                "SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                        priority, attempts, max_attempts, available_at, lease_owner,
                        lease_until, progress_json, last_error, created_at,
                        updated_at, completed_at, cancel_requested
                 FROM jobs WHERE vault_id = ? AND dedup_key = ?",
            )
            .bind(vault_id.to_string())
            .bind(dedup_key)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, JobRow>(
                "SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                        priority, attempts, max_attempts, available_at, lease_owner,
                        lease_until, progress_json, last_error, created_at,
                        updated_at, completed_at, cancel_requested
                 FROM jobs WHERE vault_id IS NULL AND dedup_key = ?",
            )
            .bind(dedup_key)
            .fetch_optional(&self.pool)
            .await?
        };
        row.map(row_to_job).transpose()
    }

    async fn get_unscoped(&self, job_id: JobId) -> Result<Option<JobRecord>, StateError> {
        self.get_scoped(job_id, None).await
    }

    async fn get_scoped(
        &self,
        job_id: JobId,
        vault_id: Option<VaultId>,
    ) -> Result<Option<JobRecord>, StateError> {
        let row = if let Some(vault_id) = vault_id {
            sqlx::query_as::<_, JobRow>(
                "SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                        priority, attempts, max_attempts, available_at, lease_owner,
                        lease_until, progress_json, last_error, created_at,
                        updated_at, completed_at, cancel_requested
                 FROM jobs WHERE id = ? AND vault_id = ?",
            )
            .bind(job_id.to_string())
            .bind(vault_id.to_string())
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, JobRow>(
                "SELECT id, vault_id, job_type, dedup_key, payload_json, status,
                        priority, attempts, max_attempts, available_at, lease_owner,
                        lease_until, progress_json, last_error, created_at,
                        updated_at, completed_at, cancel_requested
                 FROM jobs WHERE id = ?",
            )
            .bind(job_id.to_string())
            .fetch_optional(&self.pool)
            .await?
        };
        row.map(row_to_job).transpose()
    }

    async fn ensure_context(&self, context: &VaultContext) -> Result<(), StateError> {
        let root = context
            .content_root()
            .to_str()
            .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM vaults WHERE id = ? AND slug = ? AND content_root = ?
             )",
        )
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .bind(root)
        .fetch_one(&self.pool)
        .await?;
        if exists == 0 {
            return Err(StateError::InvalidInput("Vault context is not registered"));
        }
        Ok(())
    }
}

/// Repository for one Vault's resumable scan checkpoint.
#[derive(Clone)]
pub struct ScanCheckpointRepository {
    pool: SqlitePool,
}

impl ScanCheckpointRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Start a new generation, replacing any prior checkpoint for this scan
    /// type while preserving the row identity.
    pub async fn start(
        &self,
        context: &VaultContext,
        scan_type: &str,
        generation: &str,
    ) -> Result<ScanCheckpointRecord, StateError> {
        self.ensure_context(context).await?;
        validate_scan_label(scan_type)?;
        validate_scan_label(generation)?;
        let now = now_millis()?;
        let id = ScanId::new();
        sqlx::query(
            "INSERT INTO scan_checkpoints
             (id, vault_id, scan_type, generation, status, started_at, updated_at)
             VALUES (?, ?, ?, ?, 'running', ?, ?)
             ON CONFLICT(vault_id, scan_type) DO UPDATE SET
                generation = excluded.generation, cursor_path = NULL,
                 status = 'running', entries_seen = 0, files_seen = 0,
                 directories_seen = 0, changes_imported = 0,
                 unsafe_entries_skipped = 0, missing_deletes_skipped = 0,
                 last_error = NULL, started_at = excluded.started_at,
                 updated_at = excluded.updated_at, completed_at = NULL",
        )
        .bind(id.to_string())
        .bind(context.id().to_string())
        .bind(scan_type)
        .bind(generation)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get(context, scan_type)
            .await?
            .ok_or(StateError::InvalidInput(
                "started scan checkpoint was not found",
            ))
    }

    /// Update only the current generation's bounded progress.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_progress(
        &self,
        context: &VaultContext,
        scan_type: &str,
        generation: &str,
        cursor_path: Option<&VaultPath>,
        entries_seen: u64,
        files_seen: u64,
        directories_seen: u64,
        changes_imported: u64,
        unsafe_entries_skipped: u64,
        missing_deletes_skipped: bool,
    ) -> Result<(), StateError> {
        self.ensure_context(context).await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE scan_checkpoints
             SET cursor_path = ?, entries_seen = ?, files_seen = ?,
                 directories_seen = ?, changes_imported = ?,
                 unsafe_entries_skipped = ?, missing_deletes_skipped = ?,
                 updated_at = ?
             WHERE vault_id = ? AND scan_type = ? AND generation = ?
               AND status = 'running'",
        )
        .bind(cursor_path.map(VaultPath::as_str))
        .bind(to_i64(entries_seen, "entries_seen")?)
        .bind(to_i64(files_seen, "files_seen")?)
        .bind(to_i64(directories_seen, "directories_seen")?)
        .bind(to_i64(changes_imported, "changes_imported")?)
        .bind(to_i64(unsafe_entries_skipped, "unsafe_entries_skipped")?)
        .bind(i64::from(missing_deletes_skipped))
        .bind(now)
        .bind(context.id().to_string())
        .bind(scan_type)
        .bind(generation)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput(
                "scan checkpoint generation is stale",
            ));
        }
        Ok(())
    }

    /// Mark the current generation terminal.
    pub async fn finish(
        &self,
        context: &VaultContext,
        scan_type: &str,
        generation: &str,
        status: ScanStatus,
        error: Option<&str>,
    ) -> Result<(), StateError> {
        self.ensure_context(context).await?;
        if let Some(error) = error {
            validate_error(error)?;
        }
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE scan_checkpoints
             SET status = ?, last_error = ?, updated_at = ?, completed_at = ?
             WHERE vault_id = ? AND scan_type = ? AND generation = ?
               AND status = 'running'",
        )
        .bind(status.as_str())
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(context.id().to_string())
        .bind(scan_type)
        .bind(generation)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput(
                "scan checkpoint generation is stale",
            ));
        }
        Ok(())
    }

    /// Read one checkpoint after proving Vault scope.
    pub async fn get(
        &self,
        context: &VaultContext,
        scan_type: &str,
    ) -> Result<Option<ScanCheckpointRecord>, StateError> {
        self.ensure_context(context).await?;
        let row = sqlx::query_as::<_, ScanCheckpointRow>(
            "SELECT id, vault_id, scan_type, generation, cursor_path, status,
                    entries_seen, files_seen, directories_seen, changes_imported,
                    unsafe_entries_skipped, missing_deletes_skipped,
                    last_error, started_at, updated_at, completed_at
             FROM scan_checkpoints WHERE vault_id = ? AND scan_type = ?",
        )
        .bind(context.id().to_string())
        .bind(scan_type)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_checkpoint).transpose()
    }

    async fn ensure_context(&self, context: &VaultContext) -> Result<(), StateError> {
        let root = context
            .content_root()
            .to_str()
            .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM vaults WHERE id = ? AND slug = ? AND content_root = ?
             )",
        )
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .bind(root)
        .fetch_one(&self.pool)
        .await?;
        if exists == 0 {
            return Err(StateError::InvalidInput("Vault context is not registered"));
        }
        Ok(())
    }
}

fn row_to_outbox(row: OutboxRow) -> Result<OutboxEventRecord, StateError> {
    Ok(OutboxEventRecord {
        id: EventId::parse(&row.id)?,
        vault_id: row.vault_id.as_deref().map(VaultId::parse).transpose()?,
        event_type: row.event_type,
        aggregate_type: row.aggregate_type,
        aggregate_id: row.aggregate_id,
        payload: serde_json::from_str(&row.payload_json)?,
        created_at: row.created_at,
        available_at: row.available_at,
        claimed_by: row.claimed_by,
        claimed_until: row.claimed_until,
        delivered_at: row.delivered_at,
        attempts: u32::try_from(row.attempts)
            .map_err(|_| StateError::InvalidInput("outbox attempts are invalid"))?,
        last_error: row.last_error,
        dead_lettered: row.dead_lettered != 0,
        dead_letter_reason: row.dead_letter_reason,
    })
}

fn row_to_job(row: JobRow) -> Result<JobRecord, StateError> {
    Ok(JobRecord {
        id: JobId::parse(&row.id)?,
        vault_id: row.vault_id.as_deref().map(VaultId::parse).transpose()?,
        job_type: row.job_type,
        dedup_key: row.dedup_key,
        payload: serde_json::from_str(&row.payload_json)?,
        status: JobStatus::parse(&row.status)?,
        priority: i32::try_from(row.priority)
            .map_err(|_| StateError::InvalidInput("job priority is invalid"))?,
        attempts: u32::try_from(row.attempts)
            .map_err(|_| StateError::InvalidInput("job attempts are invalid"))?,
        max_attempts: u32::try_from(row.max_attempts)
            .map_err(|_| StateError::InvalidInput("job max attempts are invalid"))?,
        available_at: row.available_at,
        lease_owner: row.lease_owner,
        lease_until: row.lease_until,
        progress: row
            .progress_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        last_error: row.last_error,
        created_at: row.created_at,
        updated_at: row.updated_at,
        completed_at: row.completed_at,
        cancel_requested: row.cancel_requested != 0,
    })
}

fn row_to_checkpoint(row: ScanCheckpointRow) -> Result<ScanCheckpointRecord, StateError> {
    Ok(ScanCheckpointRecord {
        id: ScanId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        scan_type: row.scan_type,
        generation: row.generation,
        cursor_path: row
            .cursor_path
            .as_deref()
            .map(VaultPath::parse)
            .transpose()?,
        status: ScanStatus::parse(&row.status)?,
        entries_seen: to_u64(row.entries_seen, "entries_seen")?,
        files_seen: to_u64(row.files_seen, "files_seen")?,
        directories_seen: to_u64(row.directories_seen, "directories_seen")?,
        changes_imported: to_u64(row.changes_imported, "changes_imported")?,
        unsafe_entries_skipped: to_u64(row.unsafe_entries_skipped, "unsafe_entries_skipped")?,
        missing_deletes_skipped: row.missing_deletes_skipped != 0,
        last_error: row.last_error,
        started_at: row.started_at,
        updated_at: row.updated_at,
        completed_at: row.completed_at,
    })
}

fn validate_worker(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > MAX_WORKER_ID_BYTES || value.chars().any(char::is_control)
    {
        return Err(StateError::InvalidInput("worker ID is invalid"));
    }
    Ok(())
}

fn validate_lease(now: i64, lease_until: i64) -> Result<(), StateError> {
    if lease_until <= now {
        return Err(StateError::InvalidInput("lease must expire in the future"));
    }
    Ok(())
}

fn validate_batch(limit: u32) -> Result<u32, StateError> {
    if limit == 0 || limit > MAX_BATCH_SIZE {
        return Err(StateError::InvalidInput("worker batch size is invalid"));
    }
    Ok(limit)
}

fn validate_error(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > MAX_ERROR_BYTES || value.chars().any(char::is_control) {
        return Err(StateError::InvalidInput("worker error is invalid"));
    }
    Ok(())
}

fn validate_aggregate_id(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > MAX_DEDUP_KEY_BYTES || value.chars().any(char::is_control)
    {
        return Err(StateError::InvalidInput("aggregate ID is invalid"));
    }
    Ok(())
}

fn validate_job_labels(job_type: &str, dedup_key: &str) -> Result<(), StateError> {
    if job_type.is_empty()
        || job_type.len() > MAX_JOB_TYPE_BYTES
        || job_type.chars().any(char::is_control)
        || dedup_key.is_empty()
        || dedup_key.len() > MAX_DEDUP_KEY_BYTES
        || dedup_key.chars().any(char::is_control)
    {
        return Err(StateError::InvalidInput("job label is invalid"));
    }
    Ok(())
}

fn validate_scan_label(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(StateError::InvalidInput("scan label is invalid"));
    }
    Ok(())
}

fn validate_attempts(value: u32) -> Result<(), StateError> {
    if value == 0 || value > 100 {
        return Err(StateError::InvalidInput("job max attempts are invalid"));
    }
    Ok(())
}

fn validate_payload(value: &Value) -> Result<String, StateError> {
    let encoded = serde_json::to_string(value)?;
    if encoded.len() > MAX_PAYLOAD_BYTES {
        return Err(StateError::InvalidInput("worker payload is too large"));
    }
    Ok(encoded)
}

fn to_i64(value: u64, field: &'static str) -> Result<i64, StateError> {
    i64::try_from(value).map_err(|_| StateError::InvalidInput(field))
}

fn to_u64(value: i64, field: &'static str) -> Result<u64, StateError> {
    u64::try_from(value).map_err(|_| StateError::InvalidInput(field))
}
