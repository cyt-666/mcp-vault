//! Cancellation-aware bounded background-worker orchestration.
//!
//! Durable claim/ack/retry transitions remain in `mcp-vault-state`; this module
//! only coordinates handlers and process lifecycle.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mcp_vault_backup::{BackupError, BackupService};
use mcp_vault_domain::{
    BackupId, EventId, FileId, MaintenanceGate, ModelId, VaultPath, VaultPathPolicy,
};
use mcp_vault_memory::{MemoryError, MemoryService};
use mcp_vault_providers::EmbeddingSourceRef;
use mcp_vault_state::{JobRecord, JobRepository, OutboxEventRecord, OutboxRepository, StateStore};
use serde_json::json;
use tokio::{
    sync::{Notify, Semaphore},
    task::JoinSet,
    time::sleep,
};

use crate::metrics::Metrics;

type BoxWorkerFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A static, redaction-safe handler failure code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerFailure {
    /// Stable non-sensitive failure code.
    pub code: &'static str,
    /// Whether the event/job may be retried.
    pub retryable: bool,
}

impl WorkerFailure {
    /// Construct a retryable failure.
    pub const fn retryable(code: &'static str) -> Self {
        Self {
            code,
            retryable: true,
        }
    }

    /// Construct a permanent failure.
    pub const fn permanent(code: &'static str) -> Self {
        Self {
            code,
            retryable: false,
        }
    }
}

/// Outbox handlers return only safe error codes; payloads are never logged.
pub type OutboxHandler =
    Arc<dyn Fn(OutboxEventRecord) -> BoxWorkerFuture<Result<(), WorkerFailure>> + Send + Sync>;

/// Cooperative cancellation token passed to job handlers.
#[derive(Clone, Default)]
pub struct Cancellation {
    inner: Arc<CancellationInner>,
}

#[derive(Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancellation {
    /// Request cancellation and wake all waiters.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    /// Return whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Await a cancellation request.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.inner.notify.notified().await;
    }
}

/// Result returned by a durable job handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobOutcome {
    /// Job completed successfully.
    Complete,
    /// Retry after a bounded delay.
    Retry { delay: Duration, code: &'static str },
    /// Permanent failure.
    Failed { code: &'static str },
    /// Handler cooperatively stopped.
    Cancelled,
}

/// Job handler signature. Handlers must checkpoint long work using the token.
pub type JobHandler =
    Arc<dyn Fn(JobRecord, Cancellation) -> BoxWorkerFuture<JobOutcome> + Send + Sync>;

/// Worker polling and lease configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerConfig {
    /// Delay between claim cycles when no work is available.
    pub poll_interval: Duration,
    /// Lease duration for a claimed row.
    pub lease_duration: Duration,
    /// Maximum rows claimed per cycle.
    pub batch_size: u32,
    /// Maximum concurrent handler tasks.
    pub concurrency: usize,
    /// Maximum outbox attempts before dead-lettering.
    pub max_outbox_attempts: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(250),
            lease_duration: Duration::from_secs(30),
            batch_size: 16,
            concurrency: 4,
            max_outbox_attempts: 10,
        }
    }
}

/// Worker lifecycle state used by readiness/diagnostic surfaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerStatus {
    /// Supervisor has not entered its loops.
    Starting,
    /// Claim loops are active.
    Running,
    /// Shutdown was requested.
    Draining,
    /// All loops stopped.
    Stopped,
}

/// Non-sensitive worker health snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerHealthSnapshot {
    /// Current lifecycle state.
    pub status: WorkerStatus,
    /// Pending undelivered outbox count.
    pub outbox_pending: u64,
    /// Pending/running job count.
    pub jobs_pending: u64,
    /// Handler tasks currently executing.
    pub in_flight: usize,
    /// Last stable failure code, if any.
    pub last_error: Option<&'static str>,
}

struct WorkerHealth {
    status: AtomicU8,
    outbox_pending: AtomicU64,
    jobs_pending: AtomicU64,
    in_flight: AtomicUsize,
    last_error: RwLock<Option<&'static str>>,
}

impl Default for WorkerHealth {
    fn default() -> Self {
        Self {
            status: AtomicU8::new(status_code(WorkerStatus::Starting)),
            outbox_pending: AtomicU64::new(0),
            jobs_pending: AtomicU64::new(0),
            in_flight: AtomicUsize::new(0),
            last_error: RwLock::new(None),
        }
    }
}

impl WorkerHealth {
    fn set_status(&self, status: WorkerStatus) {
        self.status.store(status_code(status), Ordering::Release);
    }

    fn error(&self, code: &'static str) {
        if let Ok(mut last_error) = self.last_error.write() {
            *last_error = Some(code);
        }
    }

    fn snapshot(&self) -> WorkerHealthSnapshot {
        WorkerHealthSnapshot {
            status: status_from_code(self.status.load(Ordering::Acquire)),
            outbox_pending: self.outbox_pending.load(Ordering::Acquire),
            jobs_pending: self.jobs_pending.load(Ordering::Acquire),
            in_flight: self.in_flight.load(Ordering::Acquire),
            last_error: self.last_error.read().ok().and_then(|value| *value),
        }
    }
}

/// Durable worker supervisor for the modular-monolith process.
#[derive(Clone)]
pub struct WorkerSupervisor {
    state: StateStore,
    outbox_handler: OutboxHandler,
    job_handlers: Arc<RwLock<HashMap<String, JobHandler>>>,
    config: WorkerConfig,
    health: Arc<WorkerHealth>,
    started: Arc<Notify>,
    maintenance: MaintenanceGate,
}

impl WorkerSupervisor {
    /// Construct a supervisor with a durable outbox handler and no job
    /// handlers. Later index/provider/memory packages register job handlers.
    pub fn new(
        state: StateStore,
        outbox_handler: OutboxHandler,
        config: WorkerConfig,
    ) -> Result<Self, WorkerFailure> {
        if config.poll_interval.is_zero()
            || config.lease_duration <= config.poll_interval
            || config.batch_size == 0
            || config.batch_size > 128
            || config.concurrency == 0
            || config.concurrency > 64
            || config.max_outbox_attempts == 0
        {
            return Err(WorkerFailure::permanent("worker_config_invalid"));
        }
        Ok(Self {
            state,
            outbox_handler,
            job_handlers: Arc::new(RwLock::new(HashMap::new())),
            config,
            health: Arc::new(WorkerHealth::default()),
            started: Arc::new(Notify::new()),
            maintenance: MaintenanceGate::new(),
        })
    }

    /// Attach process maintenance coordination before the supervisor starts.
    pub fn with_maintenance_gate(mut self, maintenance: MaintenanceGate) -> Self {
        self.maintenance = maintenance;
        self
    }

    /// Register a handler before the supervisor starts claiming jobs.
    pub fn register_job_handler(
        &self,
        job_type: &str,
        handler: JobHandler,
    ) -> Result<(), WorkerFailure> {
        if job_type.is_empty() || job_type.len() > 128 || job_type.chars().any(char::is_control) {
            return Err(WorkerFailure::permanent("job_type_invalid"));
        }
        self.job_handlers
            .write()
            .map_err(|_| WorkerFailure::permanent("worker_registry_unavailable"))?
            .insert(job_type.to_owned(), handler);
        Ok(())
    }

    /// Return a non-sensitive current health snapshot.
    pub fn health(&self) -> WorkerHealthSnapshot {
        self.health.snapshot()
    }

    /// Wait until the claim loops have entered their running state.
    pub async fn wait_until_running(&self) {
        loop {
            if self.health().status == WorkerStatus::Running {
                return;
            }
            self.started.notified().await;
        }
    }

    /// Run both claim loops until cancellation; all leases remain reclaimable.
    pub async fn run(&self, shutdown: Cancellation) {
        self.health.set_status(WorkerStatus::Running);
        self.started.notify_waiters();
        let outbox = self.clone();
        let jobs = self.clone();
        let draining_health = self.health.clone();
        let draining_shutdown = shutdown.clone();
        let outbox_shutdown = shutdown.clone();
        let jobs_shutdown = shutdown.clone();
        tokio::join!(
            async move {
                draining_shutdown.cancelled().await;
                draining_health.set_status(WorkerStatus::Draining);
            },
            outbox.run_outbox_loop(outbox_shutdown),
            jobs.run_job_loop(jobs_shutdown)
        );
        self.health.set_status(WorkerStatus::Stopped);
    }

    async fn run_outbox_loop(&self, shutdown: Cancellation) {
        let repository = self.state.outbox();
        let worker_id = format!("outbox-{}", EventId::new());
        while !shutdown.is_cancelled() {
            let Some(write_operation) = self.maintenance.try_start_write() else {
                wait_poll(&shutdown, self.config.poll_interval).await;
                continue;
            };
            let now = now_millis();
            self.refresh_counts().await;
            let lease_until = now.saturating_add(duration_millis(self.config.lease_duration));
            match repository
                .claim_batch(&worker_id, now, lease_until, self.config.batch_size)
                .await
            {
                Ok(events) if events.is_empty() => {
                    drop(write_operation);
                    wait_poll(&shutdown, self.config.poll_interval).await
                }
                Ok(events) => {
                    self.dispatch_outbox_batch(&repository, &worker_id, events)
                        .await
                }
                Err(_) => {
                    drop(write_operation);
                    self.health.error("outbox_claim_failed");
                    wait_poll(&shutdown, self.config.poll_interval).await;
                }
            }
        }
        let _ = repository.release_worker_leases(&worker_id).await;
    }

    async fn dispatch_outbox_batch(
        &self,
        repository: &OutboxRepository,
        worker_id: &str,
        events: Vec<OutboxEventRecord>,
    ) {
        let semaphore = Arc::new(Semaphore::new(self.config.concurrency));
        let mut tasks = JoinSet::new();
        for event in events {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let handler = self.outbox_handler.clone();
            self.health.in_flight.fetch_add(1, Ordering::AcqRel);
            tasks.spawn(async move {
                let event_id = event.id;
                let attempts = event.attempts;
                let result = handler(event).await;
                drop(permit);
                (event_id, attempts, result)
            });
        }
        while let Some(result) = tasks.join_next().await {
            self.health.in_flight.fetch_sub(1, Ordering::AcqRel);
            let (event_id, attempts, outcome) = match result {
                Ok(value) => value,
                Err(_) => {
                    self.health.error("outbox_handler_panicked");
                    continue;
                }
            };
            let now = now_millis();
            match outcome {
                Ok(()) => {
                    if repository.ack(event_id, worker_id, now).await.is_err() {
                        self.health.error("outbox_ack_failed");
                    }
                }
                Err(failure) => {
                    let available_at = now.saturating_add(backoff_millis(attempts));
                    let max_attempts = if failure.retryable {
                        self.config.max_outbox_attempts
                    } else {
                        1
                    };
                    let dead_lettered = repository
                        .retry_or_dead_letter(
                            event_id,
                            worker_id,
                            available_at,
                            failure.code,
                            max_attempts,
                        )
                        .await;
                    if dead_lettered.is_err() {
                        self.health.error("outbox_retry_failed");
                    } else if !failure.retryable {
                        self.health.error(failure.code);
                    }
                }
            }
        }
    }

    async fn run_job_loop(&self, shutdown: Cancellation) {
        let repository = self.state.jobs();
        let worker_id = format!("job-{}", EventId::new());
        while !shutdown.is_cancelled() {
            let Some(claim_operation) = self.maintenance.try_start_write() else {
                wait_poll(&shutdown, self.config.poll_interval).await;
                continue;
            };
            let has_handlers = self
                .job_handlers
                .read()
                .map(|handlers| !handlers.is_empty())
                .unwrap_or(false);
            if !has_handlers {
                drop(claim_operation);
                wait_poll(&shutdown, self.config.poll_interval).await;
                continue;
            }
            let now = now_millis();
            self.refresh_counts().await;
            let lease_until = now.saturating_add(duration_millis(self.config.lease_duration));
            match repository
                .claim_batch(&worker_id, now, lease_until, self.config.batch_size)
                .await
            {
                Ok(jobs) if jobs.is_empty() => {
                    drop(claim_operation);
                    wait_poll(&shutdown, self.config.poll_interval).await
                }
                Ok(jobs) => {
                    drop(claim_operation);
                    self.dispatch_job_batch(&repository, &worker_id, jobs, &shutdown)
                        .await
                }
                Err(_) => {
                    drop(claim_operation);
                    self.health.error("job_claim_failed");
                    wait_poll(&shutdown, self.config.poll_interval).await;
                }
            }
        }
        let _ = repository.release_worker_leases(&worker_id).await;
    }

    async fn dispatch_job_batch(
        &self,
        repository: &JobRepository,
        worker_id: &str,
        jobs: Vec<JobRecord>,
        shutdown: &Cancellation,
    ) {
        let semaphore = Arc::new(Semaphore::new(self.config.concurrency));
        let monitor_interval = self
            .config
            .poll_interval
            .min(self.config.lease_duration / 3);
        let lease_duration = self.config.lease_duration;
        let mut tasks = JoinSet::new();
        for job in jobs {
            let handler = self
                .job_handlers
                .read()
                .ok()
                .and_then(|handlers| handlers.get(&job.job_type).cloned());
            let Some(handler) = handler else {
                let _ = repository
                    .release_claimed(
                        job.id,
                        worker_id,
                        now_millis().saturating_add(duration_millis(self.config.poll_interval)),
                    )
                    .await;
                continue;
            };
            if job.cancel_requested {
                let _ = repository.cancel_claimed(job.id, worker_id).await;
                continue;
            }
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let shutdown = shutdown.clone();
            let maintenance = self.maintenance.clone();
            let repository_for_task = repository.clone();
            let worker_id_for_task = worker_id.to_owned();
            self.health.in_flight.fetch_add(1, Ordering::AcqRel);
            tasks.spawn(async move {
                let is_backup = job.job_type.starts_with("backup.");
                let maintenance_operation = if is_backup {
                    None
                } else {
                    loop {
                        if let Some(operation) = maintenance.try_start_write() {
                            break Some(operation);
                        }
                        if shutdown.is_cancelled() {
                            drop(permit);
                            return (job.id, None, None);
                        }
                        wait_poll(&shutdown, Duration::from_millis(25)).await;
                    }
                };
                let cancellation = Cancellation::default();
                let monitor = {
                    let repository = repository_for_task.clone();
                    let worker_id = worker_id_for_task.clone();
                    let monitor_shutdown = shutdown.clone();
                    let monitor_cancellation = cancellation.clone();
                    let job_id = job.id;
                    tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                _ = monitor_shutdown.cancelled() => {
                                    monitor_cancellation.cancel();
                                    break;
                                }
                                _ = sleep(monitor_interval) => {
                                    if matches!(
                                        repository
                                            .renew_claimed(
                                                job_id,
                                                &worker_id,
                                                now_millis().saturating_add(
                                                    duration_millis(lease_duration)
                                                ),
                                            )
                                            .await,
                                        Ok(true)
                                    ) {
                                        monitor_cancellation.cancel();
                                        break;
                                    }
                                }
                            }
                        }
                    })
                };
                let mut outcome = handler(job.clone(), cancellation).await;
                monitor.abort();
                if !is_backup
                    && matches!(
                        repository_for_task
                            .should_cancel_claimed(job.id, &worker_id_for_task)
                            .await,
                        Ok(true)
                    )
                {
                    outcome = JobOutcome::Cancelled;
                }
                if shutdown.is_cancelled() {
                    drop(permit);
                    return (job.id, None, maintenance_operation);
                }
                drop(permit);
                (job.id, Some(outcome), maintenance_operation)
            });
        }
        while let Some(result) = tasks.join_next().await {
            self.health.in_flight.fetch_sub(1, Ordering::AcqRel);
            let (job_id, outcome, _maintenance_operation) = match result {
                Ok(value) => value,
                Err(_) => {
                    self.health.error("job_handler_panicked");
                    continue;
                }
            };
            let Some(outcome) = outcome else {
                // Maintenance may remain offline after a failed restore. Leave
                // the durable lease to expire instead of mutating state while
                // the process is offline.
                continue;
            };
            match outcome {
                JobOutcome::Complete => {
                    if repository.complete(job_id, worker_id).await.is_err() {
                        self.health.error("job_complete_failed");
                    }
                }
                JobOutcome::Retry { delay, code } => {
                    if repository
                        .retry_or_fail(
                            job_id,
                            worker_id,
                            now_millis().saturating_add(duration_millis(delay)),
                            code,
                        )
                        .await
                        .is_err()
                    {
                        self.health.error("job_retry_failed");
                    }
                }
                JobOutcome::Failed { code } => {
                    if repository
                        .fail_permanently(job_id, worker_id, code)
                        .await
                        .is_err()
                    {
                        self.health.error("job_fail_failed");
                    }
                }
                JobOutcome::Cancelled => {
                    if repository.cancel_claimed(job_id, worker_id).await.is_err() {
                        self.health.error("job_cancel_failed");
                    }
                }
            }
        }
    }

    async fn refresh_counts(&self) {
        if let Ok(count) = self.state.outbox().pending_count().await {
            self.health.outbox_pending.store(count, Ordering::Release);
        }
        if let Ok(count) = self.state.jobs().pending_count().await {
            self.health.jobs_pending.store(count, Ordering::Release);
        }
    }
}

/// Convert one transactional outbox event into a durable derived-work job.
/// File events also enqueue a Vault-scoped rebuild job. The ordinary outbox
/// event remains durable for later consumers and audit/reconciliation.
pub fn outbox_to_job_handler(state: StateStore) -> OutboxHandler {
    Arc::new(move |event| {
        let state = state.clone();
        Box::pin(async move {
            let event_id = event.id.to_string();
            let is_file_event = event.aggregate_type == "file";
            let payload = json!({
                "event_id": event.id,
                "event_type": event.event_type,
                "aggregate_type": event.aggregate_type,
                "aggregate_id": event.aggregate_id,
                "payload": event.payload,
            });
            let result = if let Some(vault_id) = event.vault_id {
                let vault = state
                    .vaults()
                    .find_by_id(vault_id)
                    .await
                    .map_err(|_| WorkerFailure::retryable("outbox_vault_lookup_failed"))?
                    .ok_or(WorkerFailure::permanent("outbox_vault_missing"))?;
                let context = vault
                    .context()
                    .map_err(|_| WorkerFailure::permanent("outbox_vault_context_invalid"))?;
                let reserved_path = event
                    .payload
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|path| path_in_reserved_namespace(&vault.reserved_root, path));
                let is_memory_extract_event = is_file_event
                    && matches!(event.event_type.as_str(), "FileCreated" | "FileUpdated")
                    && event
                        .payload
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|path| {
                            path.to_ascii_lowercase().ends_with(".md") && !reserved_path
                        });
                let is_memory_revalidate_event = is_file_event
                    && matches!(event.event_type.as_str(), "FileDeleted" | "FileUpdated")
                    && !reserved_path;
                let is_memory_projection_event = is_file_event
                    && matches!(
                        event.event_type.as_str(),
                        "FileCreated" | "FileUpdated" | "FileDeleted"
                    )
                    && reserved_path;
                if event.aggregate_type == "file" {
                    state
                        .jobs()
                        .enqueue(
                            &context,
                            "index.rebuild",
                            &format!("vault:{vault_id}:index:{}", event.id),
                            &payload,
                            0,
                            10,
                            now_millis(),
                        )
                        .await
                        .map_err(|_| WorkerFailure::retryable("index_job_admission_failed"))?;
                    if is_memory_extract_event {
                        state
                            .jobs()
                            .enqueue(
                                &context,
                                "memory.extract",
                                &format!("vault:{vault_id}:memory-extract:{event_id}"),
                                &payload,
                                0,
                                10,
                                now_millis(),
                            )
                            .await
                            .map_err(|_| {
                                WorkerFailure::retryable("memory_extract_job_admission_failed")
                            })?;
                    }
                    if is_memory_revalidate_event {
                        state
                            .jobs()
                            .enqueue(
                                &context,
                                "memory.revalidate",
                                &format!("vault:{vault_id}:memory-revalidate:{event_id}"),
                                &payload,
                                0,
                                10,
                                now_millis(),
                            )
                            .await
                            .map_err(|_| {
                                WorkerFailure::retryable("memory_revalidate_job_admission_failed")
                            })?;
                    }
                    if is_memory_projection_event {
                        state
                            .jobs()
                            .enqueue(
                                &context,
                                "memory.rebuild",
                                &format!("vault:{vault_id}:memory-rebuild:{event_id}"),
                                &payload,
                                0,
                                10,
                                now_millis(),
                            )
                            .await
                            .map_err(|_| {
                                WorkerFailure::retryable("memory_rebuild_job_admission_failed")
                            })?;
                    }
                }
                state
                    .jobs()
                    .enqueue(
                        &context,
                        "outbox.event",
                        &format!("vault:{vault_id}:outbox:{event_id}"),
                        &payload,
                        0,
                        10,
                        now_millis(),
                    )
                    .await
                    .map(|_| ())
            } else {
                state
                    .jobs()
                    .enqueue_global(
                        "outbox.event",
                        &format!("global:outbox:{event_id}"),
                        &payload,
                        0,
                        10,
                        now_millis(),
                    )
                    .await
                    .map(|_| ())
            };
            result.map_err(|_| WorkerFailure::retryable("outbox_job_admission_failed"))
        })
    })
}

/// Handle one durable backup creation job.
pub fn backup_create_job_handler(service: BackupService, metrics: Metrics) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let service = service.clone();
        let metrics = metrics.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(id) = backup_id_from_job(&job) else {
                return JobOutcome::Failed {
                    code: "backup_id_invalid",
                };
            };
            let result = service.create(id).await;
            metrics.observe_backup(result.is_ok());
            backup_outcome(result)
        })
    })
}

/// Handle one durable backup verification job.
pub fn backup_verify_job_handler(service: BackupService, metrics: Metrics) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let service = service.clone();
        let metrics = metrics.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(id) = backup_id_from_job(&job) else {
                return JobOutcome::Failed {
                    code: "backup_id_invalid",
                };
            };
            let result = service.verify(id).await;
            metrics.observe_backup(result.is_ok());
            backup_outcome(result)
        })
    })
}

/// Handle one explicit restore job.
pub fn backup_restore_job_handler(service: BackupService, metrics: Metrics) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let service = service.clone();
        let metrics = metrics.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(id) = backup_id_from_job(&job) else {
                return JobOutcome::Failed {
                    code: "backup_id_invalid",
                };
            };
            let result = service.restore(id).await;
            metrics.observe_backup(result.is_ok());
            backup_outcome(result)
        })
    })
}

fn backup_id_from_job(job: &JobRecord) -> Option<BackupId> {
    job.payload
        .get("backup_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.parse().ok())
}

fn backup_outcome<T>(result: Result<T, BackupError>) -> JobOutcome {
    match result {
        Ok(_) => JobOutcome::Complete,
        Err(error) if error.retryable() => JobOutcome::Retry {
            delay: Duration::from_secs(5),
            code: backup_error_code(&error),
        },
        Err(error) => JobOutcome::Failed {
            code: backup_error_code(&error),
        },
    }
}

fn backup_error_code(error: &BackupError) -> &'static str {
    match error {
        BackupError::Domain(_) => "backup_domain_invalid",
        BackupError::State(_) => "backup_state_unavailable",
        BackupError::Storage(_) => "backup_storage_unavailable",
        BackupError::Core(_) => "backup_recovery_failed",
        BackupError::Io(_) => "backup_io_failed",
        BackupError::Json(_) | BackupError::Archive(_) => "backup_archive_invalid",
        BackupError::TargetMismatch => "backup_target_mismatch",
        BackupError::KeyVersionMismatch => "backup_key_version_mismatch",
        BackupError::Limit(_) => "backup_resource_limit",
        BackupError::Maintenance => "backup_maintenance",
        BackupError::NotFound => "backup_not_found",
        BackupError::InconsistentSource => "backup_source_changed",
    }
}

/// Terminally acknowledge the durable generic-event fan-out job. The source
/// outbox row remains retained for audit/reconciliation; this handler prevents
/// the compatibility fan-out row from accumulating forever when no optional
/// downstream consumer is installed.
pub fn outbox_event_job_handler() -> JobHandler {
    Arc::new(|_job, cancellation| {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                JobOutcome::Cancelled
            } else {
                JobOutcome::Complete
            }
        })
    })
}

/// Handle one durable Vault-scoped derived-index rebuild job.
pub fn index_rebuild_job_handler(
    state: StateStore,
    history_root: std::path::PathBuf,
    core_runtime: mcp_vault_core::VaultCoreRuntime,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let history_root = history_root.clone();
        let core_runtime = core_runtime.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(vault_id) = job.vault_id else {
                return JobOutcome::Failed {
                    code: "index_vault_missing",
                };
            };
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "index_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "index_vault_lookup_failed",
                    };
                }
            };
            if matches!(
                vault.status,
                mcp_vault_state::VaultStatus::Disabled | mcp_vault_state::VaultStatus::Error
            ) {
                return JobOutcome::Complete;
            }
            let context = match vault.context() {
                Ok(context) => context,
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "index_vault_context_invalid",
                    };
                }
            };
            match state
                .jobs()
                .has_newer_active_job(&context, "index.rebuild", job.created_at, job.id)
                .await
            {
                Ok(true) => return JobOutcome::Complete,
                Ok(false) => {}
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "index_coalesce_check_failed",
                    };
                }
            }
            let core = match super::core_for_vault(&state, &history_root, &vault, &core_runtime) {
                Ok(core) => core,
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "index_core_unavailable",
                    };
                }
            };
            let index = mcp_vault_indexer::IndexService::new(state);
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = index.rebuild_vault(&core, &context) => result,
            };
            match result {
                Ok(_) => JobOutcome::Complete,
                Err(mcp_vault_indexer::IndexError::InvalidInput(_))
                | Err(mcp_vault_indexer::IndexError::TooLarge)
                | Err(mcp_vault_indexer::IndexError::Yaml) => JobOutcome::Failed {
                    code: "index_input_invalid",
                },
                Err(_) => JobOutcome::Retry {
                    delay: Duration::from_secs(5),
                    code: "index_rebuild_failed",
                },
            }
        })
    })
}

/// Handle an explicit Admin-triggered Vault reconciliation request.
pub fn vault_reconcile_job_handler(
    state: StateStore,
    history_root: std::path::PathBuf,
    core_runtime: mcp_vault_core::VaultCoreRuntime,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let history_root = history_root.clone();
        let core_runtime = core_runtime.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(vault_id) = job.vault_id else {
                return JobOutcome::Failed {
                    code: "reconcile_vault_missing",
                };
            };
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "reconcile_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "reconcile_vault_lookup_failed",
                    };
                }
            };
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = crate::reconcile_vault_once(
                    &state,
                    &history_root,
                    &vault,
                    "admin",
                    &core_runtime,
                ) => result,
            };
            match result {
                Ok(_) => JobOutcome::Complete,
                Err(_) => JobOutcome::Retry {
                    delay: Duration::from_secs(10),
                    code: "reconcile_failed",
                },
            }
        })
    })
}

/// Handle extraction work admitted from a current Markdown file event.
pub fn memory_extract_job_handler(
    state: StateStore,
    history_root: std::path::PathBuf,
    core_runtime: mcp_vault_core::VaultCoreRuntime,
    memory: MemoryService,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let history_root = history_root.clone();
        let core_runtime = core_runtime.clone();
        let memory = memory.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(vault_id) = job.vault_id else {
                return JobOutcome::Failed {
                    code: "memory_extract_vault_missing",
                };
            };
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "memory_extract_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_extract_vault_lookup_failed",
                    };
                }
            };
            let context = match vault.context() {
                Ok(context) => context,
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "memory_extract_context_invalid",
                    };
                }
            };
            let path = job
                .payload
                .get("payload")
                .and_then(|value| value.get("path"))
                .and_then(serde_json::Value::as_str)
                .and_then(|value| VaultPath::parse(value).ok());
            let Some(path) = path else {
                return JobOutcome::Failed {
                    code: "memory_extract_path_invalid",
                };
            };
            let core = match super::core_for_vault(&state, &history_root, &vault, &core_runtime) {
                Ok(core) => core,
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_extract_core_unavailable",
                    };
                }
            };
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = memory.extract_note(&context, &core, &path) => result,
            };
            match result {
                Ok(_) => JobOutcome::Complete,
                Err(MemoryError::NotFound) => JobOutcome::Complete,
                Err(error) if error.retryable() => JobOutcome::Retry {
                    delay: Duration::from_secs(10),
                    code: "memory_extract_retryable",
                },
                Err(MemoryError::InvalidInput(_)) | Err(MemoryError::Markdown) => {
                    JobOutcome::Failed {
                        code: "memory_extract_input_invalid",
                    }
                }
                Err(_) => JobOutcome::Failed {
                    code: "memory_extract_failed",
                },
            }
        })
    })
}

/// Revalidate extracted memory provenance after a note event.
pub fn memory_revalidate_job_handler(
    state: StateStore,
    history_root: std::path::PathBuf,
    core_runtime: mcp_vault_core::VaultCoreRuntime,
    memory: MemoryService,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let history_root = history_root.clone();
        let core_runtime = core_runtime.clone();
        let memory = memory.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(vault_id) = job.vault_id else {
                return JobOutcome::Failed {
                    code: "memory_revalidate_vault_missing",
                };
            };
            let file_id = job
                .payload
                .get("aggregate_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| FileId::parse(value).ok());
            let Some(file_id) = file_id else {
                return JobOutcome::Failed {
                    code: "memory_revalidate_file_missing",
                };
            };
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "memory_revalidate_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_revalidate_vault_lookup_failed",
                    };
                }
            };
            let context = match vault.context() {
                Ok(context) => context,
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "memory_revalidate_context_invalid",
                    };
                }
            };
            let core = match super::core_for_vault(&state, &history_root, &vault, &core_runtime) {
                Ok(core) => core,
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_revalidate_core_unavailable",
                    };
                }
            };
            let deleted = job
                .payload
                .get("event_type")
                .and_then(serde_json::Value::as_str)
                == Some("FileDeleted");
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = memory.invalidate_source(&context, &core, file_id, deleted) => result,
            };
            match result {
                Ok(_) => JobOutcome::Complete,
                Err(error) if error.retryable() => JobOutcome::Retry {
                    delay: Duration::from_secs(10),
                    code: "memory_revalidate_retryable",
                },
                Err(_) => JobOutcome::Failed {
                    code: "memory_revalidate_failed",
                },
            }
        })
    })
}

/// Rebuild memory projections from managed Markdown.
pub fn memory_rebuild_job_handler(
    state: StateStore,
    history_root: std::path::PathBuf,
    core_runtime: mcp_vault_core::VaultCoreRuntime,
    memory: MemoryService,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let history_root = history_root.clone();
        let core_runtime = core_runtime.clone();
        let memory = memory.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(vault_id) = job.vault_id else {
                return JobOutcome::Failed {
                    code: "memory_rebuild_vault_missing",
                };
            };
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "memory_rebuild_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_rebuild_vault_lookup_failed",
                    };
                }
            };
            let context = match vault.context() {
                Ok(context) => context,
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "memory_rebuild_context_invalid",
                    };
                }
            };
            let core = match super::core_for_vault(&state, &history_root, &vault, &core_runtime) {
                Ok(core) => core,
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_rebuild_core_unavailable",
                    };
                }
            };
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = memory.rebuild(&context, &core) => result,
            };
            match result {
                Ok(_) => JobOutcome::Complete,
                Err(error) if error.retryable() => JobOutcome::Retry {
                    delay: Duration::from_secs(10),
                    code: "memory_rebuild_retryable",
                },
                Err(_) => JobOutcome::Failed {
                    code: "memory_rebuild_failed",
                },
            }
        })
    })
}

/// Handle ProviderService reference-only embedding rebuilds for memory bodies.
pub fn memory_embedding_job_handler(state: StateStore, memory: MemoryService) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let memory = memory.clone();
        Box::pin(async move {
            if shutdown.is_cancelled() {
                return JobOutcome::Cancelled;
            }
            let Some(vault_id) = job.vault_id else {
                return JobOutcome::Failed {
                    code: "embedding_vault_missing",
                };
            };
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "embedding_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "embedding_vault_lookup_failed",
                    };
                }
            };
            let context = match vault.context() {
                Ok(context) => context,
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "embedding_context_invalid",
                    };
                }
            };
            let model_id = job
                .payload
                .get("model_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| ModelId::parse(value).ok());
            let sources =
                job.payload.get("sources").cloned().and_then(|value| {
                    serde_json::from_value::<Vec<EmbeddingSourceRef>>(value).ok()
                });
            let (Some(model_id), Some(sources)) = (model_id, sources) else {
                return JobOutcome::Failed {
                    code: "embedding_payload_invalid",
                };
            };
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = memory.reembed_sources(&context, model_id, &sources) => result,
            };
            match result {
                Ok(_) => JobOutcome::Complete,
                Err(MemoryError::NotFound) => JobOutcome::Complete,
                Err(error) if error.retryable() => JobOutcome::Retry {
                    delay: Duration::from_secs(10),
                    code: "embedding_retryable",
                },
                Err(_) => JobOutcome::Failed {
                    code: "embedding_rebuild_failed",
                },
            }
        })
    })
}

async fn wait_poll(shutdown: &Cancellation, duration: Duration) {
    tokio::select! {
        _ = sleep(duration) => {}
        _ = shutdown.cancelled() => {}
    }
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX / 2)
}

fn backoff_millis(attempt: u32) -> i64 {
    let exponent = attempt.min(8);
    1_000_i64.saturating_mul(2_i64.saturating_pow(exponent))
}

fn path_in_reserved_namespace(root: &VaultPath, raw_path: &str) -> bool {
    let Ok(path) = VaultPath::parse(raw_path) else {
        return false;
    };
    VaultPathPolicy::new(root.clone(), Default::default())
        .is_ok_and(|policy| policy.is_reserved(&path))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn status_code(status: WorkerStatus) -> u8 {
    match status {
        WorkerStatus::Starting => 0,
        WorkerStatus::Running => 1,
        WorkerStatus::Draining => 2,
        WorkerStatus::Stopped => 3,
    }
}

fn status_from_code(code: u8) -> WorkerStatus {
    match code {
        1 => WorkerStatus::Running,
        2 => WorkerStatus::Draining,
        3 => WorkerStatus::Stopped,
        _ => WorkerStatus::Starting,
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use super::{
        Cancellation, JobHandler, JobOutcome, OutboxHandler, WorkerConfig, WorkerFailure,
        WorkerStatus, WorkerSupervisor, index_rebuild_job_handler, memory_extract_job_handler,
        outbox_event_job_handler, outbox_to_job_handler,
    };
    use mcp_vault_auth::{AuthService, MasterKeyRing};
    use mcp_vault_core::VaultCore;
    use mcp_vault_domain::{
        Actor, MaintenanceGate, Revision, SourcePlane, VaultContext, VaultId, VaultPath,
        VaultPathPolicy, VaultSlug,
    };
    use mcp_vault_memory::MemoryService;
    use mcp_vault_state::{JobStatus, StateStore, VaultStatus};
    use mcp_vault_storage_fs::StorageOptions;
    use tokio::time::{Duration, sleep, timeout};

    #[tokio::test]
    async fn cancellation_wakes_waiters_and_supervisor_has_safe_start_state() {
        let cancellation = Cancellation::default();
        let waiter = cancellation.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
            true
        });
        cancellation.cancel();
        assert!(task.await.unwrap());

        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            outbox_to_job_handler(state),
            WorkerConfig::default(),
        )
        .unwrap();
        assert_eq!(supervisor.health().status, WorkerStatus::Starting);
    }

    #[tokio::test]
    async fn running_job_observes_durable_admin_cancellation() {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("cancel-test").unwrap(),
            "/srv/cancel-test".into(),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Cancel test", VaultStatus::Active)
            .await
            .unwrap();
        let job = state
            .jobs()
            .enqueue(
                &context,
                "test.blocking",
                "cancel:test",
                &serde_json::json!({}),
                0,
                3,
                0,
            )
            .await
            .unwrap();
        let handler: JobHandler = Arc::new(|_job, cancellation| {
            Box::pin(async move {
                cancellation.cancelled().await;
                JobOutcome::Cancelled
            })
        });
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            outbox_to_job_handler(state.clone()),
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                lease_duration: Duration::from_millis(100),
                concurrency: 1,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        supervisor
            .register_job_handler("test.blocking", handler)
            .unwrap();
        let shutdown = Cancellation::default();
        let running = tokio::spawn({
            let supervisor = supervisor.clone();
            let shutdown = shutdown.clone();
            async move { supervisor.run(shutdown).await }
        });

        timeout(Duration::from_secs(2), async {
            loop {
                if state
                    .jobs()
                    .get(&context, job.id)
                    .await
                    .unwrap()
                    .is_some_and(|job| job.status == JobStatus::Running)
                {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        state.jobs().request_cancel(&context, job.id).await.unwrap();
        timeout(Duration::from_secs(2), async {
            loop {
                if state
                    .jobs()
                    .get(&context, job.id)
                    .await
                    .unwrap()
                    .is_some_and(|job| job.status == JobStatus::Cancelled)
                {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        shutdown.cancel();
        timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn supervisor_admits_outbox_to_a_durable_job_before_ack() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("worker-test").unwrap(),
            PathBuf::from(directory.path()).join("content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Worker test", VaultStatus::Active)
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            PathBuf::from(directory.path()).join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        core.create_bytes(
            &context,
            &VaultPath::parse("note.md").unwrap(),
            b"durable event",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        assert_eq!(state.outbox().pending_count().await.unwrap(), 1);

        let supervisor = WorkerSupervisor::new(
            state.clone(),
            outbox_to_job_handler(state.clone()),
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                lease_duration: Duration::from_millis(100),
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let shutdown = Cancellation::default();
        let running = tokio::spawn({
            let supervisor = supervisor.clone();
            let shutdown = shutdown.clone();
            async move { supervisor.run(shutdown).await }
        });

        timeout(Duration::from_secs(2), async {
            loop {
                if state.outbox().pending_count().await.unwrap() == 0
                    && state.jobs().pending_count().await.unwrap() == 3
                {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();

        shutdown.cancel();
        timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(supervisor.health().status, WorkerStatus::Stopped);
        assert_eq!(state.outbox().pending_count().await.unwrap(), 0);
        assert_eq!(state.jobs().pending_count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn production_file_event_handlers_drain_generic_and_redundant_jobs() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("drain-test").unwrap(),
            PathBuf::from(directory.path()).join("content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Drain test", VaultStatus::Active)
            .await
            .unwrap();
        let maintenance = MaintenanceGate::new();
        let core_runtime = mcp_vault_core::VaultCoreRuntime::new(maintenance.clone());
        let history_root = PathBuf::from(directory.path()).join("history");
        let core = VaultCore::new(
            state.clone(),
            history_root.clone(),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            core_runtime.clone(),
        );
        core.create_bytes(
            &context,
            &VaultPath::parse("note.md").unwrap(),
            b"# Durable queue drain",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[8_u8; 32]).unwrap(),
        );
        let memory = MemoryService::new(state.clone(), auth);
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            outbox_to_job_handler(state.clone()),
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                lease_duration: Duration::from_millis(250),
                ..WorkerConfig::default()
            },
        )
        .unwrap()
        .with_maintenance_gate(maintenance);
        supervisor
            .register_job_handler("outbox.event", outbox_event_job_handler())
            .unwrap();
        supervisor
            .register_job_handler(
                "index.rebuild",
                index_rebuild_job_handler(
                    state.clone(),
                    history_root.clone(),
                    core_runtime.clone(),
                ),
            )
            .unwrap();
        supervisor
            .register_job_handler(
                "memory.extract",
                memory_extract_job_handler(state.clone(), history_root, core_runtime, memory),
            )
            .unwrap();
        let shutdown = Cancellation::default();
        let running = tokio::spawn({
            let supervisor = supervisor.clone();
            let shutdown = shutdown.clone();
            async move { supervisor.run(shutdown).await }
        });

        timeout(Duration::from_secs(5), async {
            loop {
                if state.outbox().pending_count().await.unwrap() == 0
                    && state.jobs().pending_count().await.unwrap() == 0
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let jobs = state.jobs().list(&context, None, 10, 0).await.unwrap();
        assert_eq!(jobs.len(), 3);
        assert!(jobs.iter().all(|job| job.status == JobStatus::Completed));
        assert_eq!(
            state
                .index()
                .status(&context)
                .await
                .unwrap()
                .unwrap()
                .indexed_notes,
            1
        );
        shutdown.cancel();
        timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn permanent_outbox_failure_is_visible_as_dead_letter() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("dead-letter-test").unwrap(),
            PathBuf::from(directory.path()).join("content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Dead letter test", VaultStatus::Active)
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            PathBuf::from(directory.path()).join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        let created = core
            .create_bytes(
                &context,
                &VaultPath::parse("note.md").unwrap(),
                b"dead letter",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap();
        let handler: OutboxHandler =
            Arc::new(|_| Box::pin(async { Err(WorkerFailure::permanent("handler_permanent")) }));
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            handler,
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                lease_duration: Duration::from_millis(100),
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let shutdown = Cancellation::default();
        let running = tokio::spawn({
            let supervisor = supervisor.clone();
            let shutdown = shutdown.clone();
            async move { supervisor.run(shutdown).await }
        });

        timeout(Duration::from_secs(2), async {
            loop {
                if state.outbox().pending_count().await.unwrap() == 0 {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        shutdown.cancel();
        timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();

        let events = state
            .outbox()
            .find_by_aggregate(&context, &created.file.id.to_string())
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].dead_lettered);
        assert_eq!(
            events[0].dead_letter_reason.as_deref(),
            Some("handler_permanent")
        );
        assert!(events[0].delivered_at.is_none());
    }
}
