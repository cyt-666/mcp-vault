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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use mcp_vault_backup::{BackupError, BackupService};
use mcp_vault_domain::{
    BackupId, EventId, FileId, MaintenanceGate, MaintenanceOperationGuard, ModelId, VaultContext,
    VaultPath, VaultPathPolicy,
};
use mcp_vault_memory::{
    MEMORY_CONTRACT_GENERATION, MemoryError, MemoryService, NoteExtractionOptions,
};
use mcp_vault_providers::EmbeddingSourceRef;
use mcp_vault_state::{JobRecord, JobRepository, OutboxEventRecord, OutboxRepository, StateStore};
use serde_json::{Value, json};
use tokio::{
    sync::{Notify, Semaphore},
    task::{JoinError, JoinSet},
    time::sleep,
};
use tracing::{error, info, warn};

use crate::metrics::Metrics;

type BoxWorkerFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type JobTaskResult = (
    JobRecord,
    Option<JobOutcome>,
    Option<MaintenanceOperationGuard>,
);

const MAX_RECORDED_MEMORY_NOTE_FAILURES: usize = 20;
const MAX_CONSECUTIVE_MEMORY_OUTPUT_FAILURES: u32 = 3;

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
    /// Requeue without consuming an attempt because a prerequisite is active.
    Deferred { delay: Duration, code: &'static str },
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
        warn!(
            target: "mcp_vault::workers",
            event = "worker_health_error",
            error_code = code,
            "background worker health error"
        );
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
        let mut tasks = JoinSet::<JobTaskResult>::new();
        while !shutdown.is_cancelled() {
            let has_handlers = self
                .job_handlers
                .read()
                .map(|handlers| !handlers.is_empty())
                .unwrap_or(false);
            if !has_handlers {
                self.wait_for_job_capacity(&mut tasks, &repository, &worker_id, &shutdown)
                    .await;
                continue;
            }

            let free_capacity = self.config.concurrency.saturating_sub(tasks.len());
            if free_capacity > 0 {
                let Some(claim_operation) = self.maintenance.try_start_write() else {
                    self.wait_for_job_capacity(&mut tasks, &repository, &worker_id, &shutdown)
                        .await;
                    continue;
                };
                let now = now_millis();
                self.refresh_counts().await;
                let lease_until = now.saturating_add(duration_millis(self.config.lease_duration));
                let claim_limit = u32::try_from(free_capacity)
                    .unwrap_or(u32::MAX)
                    .min(self.config.batch_size);
                match repository
                    .claim_batch(&worker_id, now, lease_until, claim_limit)
                    .await
                {
                    Ok(jobs) if jobs.is_empty() => {}
                    Ok(jobs) => {
                        drop(claim_operation);
                        self.spawn_claimed_jobs(
                            &repository,
                            &worker_id,
                            jobs,
                            &shutdown,
                            &mut tasks,
                        )
                        .await;
                        continue;
                    }
                    Err(_) => {
                        self.health.error("job_claim_failed");
                    }
                }
                drop(claim_operation);
            }

            self.wait_for_job_capacity(&mut tasks, &repository, &worker_id, &shutdown)
                .await;
        }
        while let Some(result) = tasks.join_next().await {
            self.persist_job_task_result(&repository, &worker_id, result)
                .await;
        }
        let _ = repository.release_worker_leases(&worker_id).await;
    }

    async fn wait_for_job_capacity(
        &self,
        tasks: &mut JoinSet<JobTaskResult>,
        repository: &JobRepository,
        worker_id: &str,
        shutdown: &Cancellation,
    ) {
        if tasks.is_empty() {
            wait_poll(shutdown, self.config.poll_interval).await;
            return;
        }
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = sleep(self.config.poll_interval) => {}
            result = tasks.join_next() => {
                if let Some(result) = result {
                    self.persist_job_task_result(repository, worker_id, result).await;
                }
            }
        }
    }

    async fn spawn_claimed_jobs(
        &self,
        repository: &JobRepository,
        worker_id: &str,
        jobs: Vec<JobRecord>,
        shutdown: &Cancellation,
        tasks: &mut JoinSet<JobTaskResult>,
    ) {
        let monitor_interval = self
            .config
            .poll_interval
            .min(self.config.lease_duration / 3);
        let lease_duration = self.config.lease_duration;
        for job in jobs {
            if let Some(vault_id) = job.vault_id {
                let availability = match self.state.vaults().find_by_id(vault_id).await {
                    Ok(Some(vault)) => self.state.vaults().availability(&vault).await,
                    Ok(None) => {
                        let _ = repository
                            .fail_permanently(job.id, worker_id, "job_vault_missing")
                            .await;
                        continue;
                    }
                    Err(_) => {
                        let _ = repository
                            .release_claimed(job.id, worker_id, now_millis().saturating_add(5_000))
                            .await;
                        continue;
                    }
                };
                match availability {
                    Ok(mcp_vault_state::VaultAvailability::Ready) => {}
                    Ok(mcp_vault_state::VaultAvailability::Initializing)
                        if job.job_type == "vault.initialize" => {}
                    Ok(mcp_vault_state::VaultAvailability::Disabled) => {
                        let _ = repository.cancel_claimed(job.id, worker_id).await;
                        continue;
                    }
                    Ok(mcp_vault_state::VaultAvailability::Maintenance) => {
                        let _ = repository
                            .release_claimed(job.id, worker_id, now_millis().saturating_add(10_000))
                            .await;
                        continue;
                    }
                    Ok(
                        mcp_vault_state::VaultAvailability::Initializing
                        | mcp_vault_state::VaultAvailability::Error,
                    )
                    | Err(_) => {
                        let _ = repository
                            .release_claimed(job.id, worker_id, now_millis().saturating_add(60_000))
                            .await;
                        continue;
                    }
                }
            }
            if job.job_type.starts_with("memory.")
                && job
                    .payload
                    .get("memory_contract_generation")
                    .and_then(Value::as_u64)
                    != Some(u64::from(MEMORY_CONTRACT_GENERATION))
            {
                info!(
                    target: "mcp_vault::jobs",
                    event = "obsolete_memory_job_discarded",
                    job_id = %job.id,
                    job_type = %job.job_type,
                    vault_id = ?job.vault_id,
                    "obsolete prerelease memory job was discarded before handler execution"
                );
                let _ = repository.cancel_claimed(job.id, worker_id).await;
                continue;
            }
            let handler = self
                .job_handlers
                .read()
                .ok()
                .and_then(|handlers| handlers.get(&job.job_type).cloned());
            let Some(handler) = handler else {
                warn!(
                    target: "mcp_vault::jobs",
                    event = "job_handler_missing",
                    job_id = %job.id,
                    job_type = %job.job_type,
                    vault_id = ?job.vault_id,
                    "durable job has no registered handler"
                );
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
                info!(
                    target: "mcp_vault::jobs",
                    event = "job_cancelled_before_start",
                    job_id = %job.id,
                    job_type = %job.job_type,
                    vault_id = ?job.vault_id,
                    "durable job was cancelled before execution"
                );
                let _ = repository.cancel_claimed(job.id, worker_id).await;
                continue;
            }
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
                            warn!(
                                target: "mcp_vault::jobs",
                                event = "job_wait_interrupted",
                                job_id = %job.id,
                                job_type = %job.job_type,
                                vault_id = ?job.vault_id,
                                "durable job wait interrupted by shutdown"
                            );
                            return (job, None, None);
                        }
                        wait_poll(&shutdown, Duration::from_millis(25)).await;
                    }
                };
                let started_at = Instant::now();
                log_job_started(&job);
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
                    warn!(
                        target: "mcp_vault::jobs",
                        event = "job_interrupted_by_shutdown",
                        job_id = %job.id,
                        job_type = %job.job_type,
                        vault_id = ?job.vault_id,
                        "durable job result was not persisted because shutdown started"
                    );
                    return (job, None, maintenance_operation);
                }
                log_job_outcome(&job, outcome, started_at.elapsed());
                (job, Some(outcome), maintenance_operation)
            });
        }
    }

    async fn persist_job_task_result(
        &self,
        repository: &JobRepository,
        worker_id: &str,
        result: Result<JobTaskResult, JoinError>,
    ) {
        self.health.in_flight.fetch_sub(1, Ordering::AcqRel);
        let (job, outcome, _maintenance_operation) = match result {
            Ok(value) => value,
            Err(_) => {
                self.health.error("job_handler_panicked");
                error!(
                    target: "mcp_vault::jobs",
                    event = "job_handler_panicked",
                    "durable job handler panicked"
                );
                return;
            }
        };
        let Some(outcome) = outcome else {
            // Maintenance may remain offline after a failed restore. Leave
            // the durable lease to expire instead of mutating state while
            // the process is offline.
            return;
        };
        let job_id = job.id;
        match outcome {
            JobOutcome::Complete => {
                if repository.complete(job_id, worker_id).await.is_err() {
                    self.health.error("job_complete_failed");
                    error!(
                        target: "mcp_vault::jobs",
                        event = "job_state_transition_failed",
                        job_id = %job_id,
                        transition = "completed",
                        "failed to persist completed job state"
                    );
                }
            }
            JobOutcome::Deferred { delay, code } => {
                if repository
                    .release_claimed(
                        job_id,
                        worker_id,
                        now_millis().saturating_add(duration_millis(delay)),
                    )
                    .await
                    .is_err()
                {
                    self.health.error("job_defer_failed");
                    error!(
                        target: "mcp_vault::jobs",
                        event = "job_state_transition_failed",
                        job_id = %job_id,
                        transition = "deferred",
                        error_code = code,
                        "failed to defer durable job"
                    );
                }
            }
            JobOutcome::Retry { delay, code } => {
                match repository
                    .retry_or_fail(
                        job_id,
                        worker_id,
                        now_millis().saturating_add(duration_millis(delay)),
                        code,
                    )
                    .await
                {
                    Ok(mcp_vault_state::JobStatus::Failed) => {
                        self.mark_initialization_failed(&job).await;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        self.health.error("job_retry_failed");
                        error!(
                            target: "mcp_vault::jobs",
                            event = "job_state_transition_failed",
                            job_id = %job_id,
                            transition = "retry",
                            error_code = code,
                            "failed to persist retry job state"
                        );
                    }
                }
            }
            JobOutcome::Failed { code } => {
                if repository
                    .fail_permanently(job_id, worker_id, code)
                    .await
                    .is_err()
                {
                    self.health.error("job_fail_failed");
                    error!(
                        target: "mcp_vault::jobs",
                        event = "job_state_transition_failed",
                        job_id = %job_id,
                        transition = "failed",
                        error_code = code,
                        "failed to persist failed job state"
                    );
                } else {
                    self.mark_initialization_failed(&job).await;
                }
            }
            JobOutcome::Cancelled => {
                if repository.cancel_claimed(job_id, worker_id).await.is_err() {
                    self.health.error("job_cancel_failed");
                    error!(
                        target: "mcp_vault::jobs",
                        event = "job_state_transition_failed",
                        job_id = %job_id,
                        transition = "cancelled",
                        "failed to persist cancelled job state"
                    );
                }
            }
        }
    }

    async fn mark_initialization_failed(&self, job: &JobRecord) {
        if job.job_type != "vault.initialize" {
            return;
        }
        let Some(vault_id) = job.vault_id else {
            return;
        };
        let Ok(Some(vault)) = self.state.vaults().find_by_id(vault_id).await else {
            return;
        };
        if vault.status != mcp_vault_state::VaultStatus::Active {
            return;
        }
        let Ok(context) = vault.context() else {
            return;
        };
        if self
            .state
            .vaults()
            .set_status(&context, mcp_vault_state::VaultStatus::Error)
            .await
            .is_err()
        {
            self.health.error("initialize_vault_error_status_failed");
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

fn log_job_started(job: &JobRecord) {
    info!(
        target: "mcp_vault::jobs",
        event = "job_started",
        job_id = %job.id,
        job_type = %job.job_type,
        vault_id = ?job.vault_id,
        attempt = job.attempts,
        "durable job started"
    );
}

fn log_job_outcome(job: &JobRecord, outcome: JobOutcome, elapsed: Duration) {
    let elapsed_ms = elapsed_millis(elapsed);
    match outcome {
        JobOutcome::Complete => info!(
            target: "mcp_vault::jobs",
            event = "job_completed",
            job_id = %job.id,
            job_type = %job.job_type,
            vault_id = ?job.vault_id,
            elapsed_ms,
            "durable job completed"
        ),
        JobOutcome::Deferred { delay, code } => info!(
            target: "mcp_vault::jobs",
            event = "job_deferred",
            job_id = %job.id,
            job_type = %job.job_type,
            vault_id = ?job.vault_id,
            elapsed_ms,
            retry_delay_ms = duration_millis(delay),
            reason_code = code,
            "durable job is waiting for a prerequisite"
        ),
        JobOutcome::Retry { delay, code } => warn!(
            target: "mcp_vault::jobs",
            event = "job_retry_scheduled",
            job_id = %job.id,
            job_type = %job.job_type,
            vault_id = ?job.vault_id,
            elapsed_ms,
            retry_delay_ms = duration_millis(delay),
            error_code = code,
            "durable job will retry"
        ),
        JobOutcome::Failed { code } => error!(
            target: "mcp_vault::jobs",
            event = "job_failed",
            job_id = %job.id,
            job_type = %job.job_type,
            vault_id = ?job.vault_id,
            elapsed_ms,
            error_code = code,
            "durable job failed"
        ),
        JobOutcome::Cancelled => warn!(
            target: "mcp_vault::jobs",
            event = "job_cancelled",
            job_id = %job.id,
            job_type = %job.job_type,
            vault_id = ?job.vault_id,
            elapsed_ms,
            "durable job cancelled"
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn log_job_progress(
    job: &JobRecord,
    phase: &str,
    completed: u64,
    total: u64,
    current_index: Option<u64>,
    current_path: Option<&VaultPath>,
    items_published: u64,
    empty_sets_published: u64,
    source_ingestion_failures: u64,
    generated_output_failures: u64,
    notes_evaluated: u64,
    source_policy_skipped: u64,
    already_evaluated_skipped: u64,
    elapsed_ms: Option<i64>,
    error_code: Option<&str>,
) {
    let current_path_hash = current_path.map(redacted_path_hash);
    info!(
        target: "mcp_vault::jobs",
        event = "job_progress",
        job_id = %job.id,
        job_type = %job.job_type,
        vault_id = ?job.vault_id,
        phase,
        completed,
        total,
        current_index = ?current_index,
        items_published,
        empty_sets_published,
        source_ingestion_failures,
        generated_output_failures,
        notes_evaluated,
        source_policy_skipped,
        already_evaluated_skipped,
        elapsed_ms = ?elapsed_ms,
        current_path_hash = ?current_path_hash,
        error_code = ?error_code,
        "durable job progress"
    );
}

fn log_memory_note_source_failure(
    job: &JobRecord,
    current_index: u64,
    total: u64,
    path: &VaultPath,
    elapsed_ms: i64,
    error_code: &'static str,
) {
    warn!(
        target: "mcp_vault::jobs",
        event = "memory_extract_source_ingestion_failed",
        job_id = %job.id,
        job_type = %job.job_type,
        vault_id = ?job.vault_id,
        current_index,
        total,
        elapsed_ms,
        current_path_hash = redacted_path_hash(path),
        error_code,
        provider_called = false,
        "memory extraction skipped one unreadable source and will continue"
    );
}

fn log_job_progress_persist_failed(job: &JobRecord, phase: &str, error_code: &str) {
    warn!(
        target: "mcp_vault::jobs",
        event = "job_progress_persist_failed",
        job_id = %job.id,
        job_type = %job.job_type,
        vault_id = ?job.vault_id,
        phase,
        error_code,
        "durable job progress could not be persisted"
    );
}

#[allow(clippy::too_many_arguments)]
fn log_memory_note_output_failure(
    job: &JobRecord,
    current_index: u64,
    total: u64,
    path: &VaultPath,
    elapsed_ms: i64,
    error_code: &'static str,
    schema_diagnostic: Option<(&'static str, &str)>,
    consecutive_failures: u32,
) {
    let current_path_hash = redacted_path_hash(path);
    let (schema_issue, schema_path) = schema_diagnostic.unzip();
    warn!(
        target: "mcp_vault::jobs",
        event = "memory_extract_note_output_failed",
        job_id = %job.id,
        job_type = %job.job_type,
        vault_id = ?job.vault_id,
        current_index,
        total,
        elapsed_ms,
        current_path_hash,
        error_code,
        schema_issue = ?schema_issue,
        schema_path = ?schema_path,
        consecutive_failures,
        "memory extraction skipped one generated output and will continue when safe"
    );
}

fn elapsed_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn redacted_path_hash(path: &VaultPath) -> String {
    mcp_vault_indexer::content_hash(path.as_str())
}

/// Convert one transactional outbox event into a durable derived-work job.
/// File events also enqueue a Vault-scoped rebuild job. The ordinary outbox
/// event remains durable for later consumers and audit/reconciliation.
pub fn outbox_to_job_handler(state: StateStore, _memory: MemoryService) -> OutboxHandler {
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
            let mut memory_payload = payload.clone();
            memory_payload
                .as_object_mut()
                .expect("job payload is an object")
                .insert(
                    "memory_contract_generation".to_owned(),
                    json!(MEMORY_CONTRACT_GENERATION),
                );
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
                let is_memory_source_reconcile_event = is_file_event
                    && is_memory_source_reconcile_event_type(&event.event_type)
                    && !reserved_path;
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
                    if is_memory_source_reconcile_event {
                        state
                            .jobs()
                            .enqueue(
                                &context,
                                "memory.source_reconcile",
                                &format!("vault:{vault_id}:memory-source-reconcile:{event_id}"),
                                &memory_payload,
                                0,
                                10,
                                now_millis(),
                            )
                            .await
                            .map_err(|_| {
                                WorkerFailure::retryable(
                                    "memory_source_reconcile_job_admission_failed",
                                )
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

fn is_memory_extract_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "FileCreated" | "FileUpdated" | "FileRestored" | "external_change"
    )
}

fn is_memory_source_reconcile_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "FileCreated"
            | "FileUpdated"
            | "FileDeleted"
            | "FileRestored"
            | "FileMoved"
            | "external_change"
    )
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
pub fn backup_restore_job_handler(
    state: StateStore,
    service: BackupService,
    metrics: Metrics,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
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
            if result.is_ok() {
                match state.vaults().list().await {
                    Ok(vaults) => {
                        for vault in vaults {
                            let Ok(context) = vault.context() else {
                                continue;
                            };
                            let generation = EventId::new().to_string();
                            if state
                                .jobs()
                                .enqueue(
                                    &context,
                                    "vault.reconcile",
                                    &format!(
                                        "vault:{}:post-restore-reconcile:{generation}",
                                        context.id()
                                    ),
                                    &json!({"reason": "backup_restore", "generation": generation}),
                                    20,
                                    10,
                                    now_millis(),
                                )
                                .await
                                .is_err()
                            {
                                warn!(vault_id = %context.id(), "post-restore Vault reconciliation admission failed");
                            }
                        }
                    }
                    Err(_) => warn!("post-restore Vault listing failed"),
                }
            }
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
    index: mcp_vault_indexer::IndexService,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let history_root = history_root.clone();
        let core_runtime = core_runtime.clone();
        let index = index.clone();
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
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = index.rebuild_vault(&core, &context) => result,
            };
            match result {
                Ok(_) => {
                    match index.schedule_note_embeddings(&context).await {
                        Ok(report) => info!(
                            target: "mcp_vault::workers",
                            event = "note_embeddings_scheduled",
                            vault_id = %context.id(),
                            source_chunks = report.source_chunks,
                            queued_chunks = report.queued_chunks,
                            pruned_vectors = report.pruned_vectors,
                            jobs = report.jobs,
                            "note embedding projection scheduling completed"
                        ),
                        Err(_) => warn!(
                            target: "mcp_vault::workers",
                            event = "note_embedding_schedule_failed",
                            vault_id = %context.id(),
                            error_code = "note_embedding_schedule_failed",
                            "optional note embedding scheduling failed"
                        ),
                    }
                    JobOutcome::Complete
                }
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
pub fn vault_initialize_job_handler(
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
                    code: "initialize_vault_missing",
                };
            };
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "initialize_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "initialize_vault_lookup_failed",
                    };
                }
            };
            if vault.status != mcp_vault_state::VaultStatus::Active {
                return JobOutcome::Complete;
            }
            let context = match vault.context() {
                Ok(context) => context,
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "initialize_vault_context_invalid",
                    };
                }
            };
            let result = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = crate::reconcile_vault_once(
                    &state,
                    &history_root,
                    &vault,
                    "initial",
                    &core_runtime,
                ) => result,
            };
            if result.is_err() {
                return JobOutcome::Retry {
                    delay: Duration::from_secs(10),
                    code: "initialize_reconcile_failed",
                };
            }
            if mcp_vault_indexer::IndexService::new(state.clone())
                .schedule_note_embeddings(&context)
                .await
                .is_err()
            {
                warn!(
                    vault_id = %context.id(),
                    "managed Vault initialization could not schedule optional note embeddings"
                );
            }
            JobOutcome::Complete
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
            if job
                .progress
                .as_ref()
                .and_then(|progress| progress.get("phase"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|phase| matches!(phase, "completed" | "completed_with_errors"))
            {
                // The handler may have checkpointed all provider work before
                // the final job transition failed. Complete the reclaimed job
                // without submitting the same paid work again.
                return JobOutcome::Complete;
            }
            let policy = match memory.extraction_policy(&context).await {
                Ok(policy) => policy.policy,
                Err(error) if error.retryable() => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(10),
                        code: "memory_extraction_policy_unavailable",
                    };
                }
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "memory_extraction_policy_invalid",
                    };
                }
            };
            if !policy.enabled {
                return JobOutcome::Failed {
                    code: "memory_extraction_disabled",
                };
            }
            let core = match super::core_for_vault(&state, &history_root, &vault, &core_runtime) {
                Ok(core) => core,
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_extract_core_unavailable",
                    };
                }
            };
            if job.payload.get("scope").and_then(serde_json::Value::as_str) == Some("all") {
                let entries = match state.files().list_active_entries(&context).await {
                    Ok(entries) => entries,
                    Err(_) => {
                        return JobOutcome::Retry {
                            delay: Duration::from_secs(10),
                            code: "memory_extract_source_list_failed",
                        };
                    }
                };
                let entries = entries
                    .into_iter()
                    .filter(|entry| {
                        entry.entry_type.as_str() == "file"
                            && entry.path.as_str().to_ascii_lowercase().ends_with(".md")
                            && !core.is_managed_path(&entry.path)
                    })
                    .collect::<Vec<_>>();
                let Some(worker_id) = job.lease_owner.as_deref() else {
                    return JobOutcome::Failed {
                        code: "memory_extract_lease_missing",
                    };
                };
                let total = entries.len();
                let include_evaluated = job
                    .payload
                    .get("include_evaluated")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let previous_progress = job.progress.as_ref();
                let last_completed_path = previous_progress
                    .and_then(|progress| progress.get("last_completed_path"))
                    .and_then(serde_json::Value::as_str);
                let resume_index = last_completed_path.map_or(0, |last_path| {
                    entries.partition_point(|entry| entry.path.as_str() <= last_path)
                });
                let mut items_published = previous_progress
                    .and_then(|progress| progress.get("items_published"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut empty_sets_published = previous_progress
                    .and_then(|progress| progress.get("empty_sets_published"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut source_ingestion_failures = previous_progress
                    .and_then(|progress| progress.get("source_ingestion_failures"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut source_ingestion_failure_notes = previous_progress
                    .and_then(|progress| progress.get("source_ingestion_failure_notes"))
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut notes_evaluated = previous_progress
                    .and_then(|progress| progress.get("notes_evaluated"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut source_policy_skipped = previous_progress
                    .and_then(|progress| progress.get("source_policy_skipped"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut already_evaluated_skipped = previous_progress
                    .and_then(|progress| progress.get("already_evaluated_skipped"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut generated_output_failures = previous_progress
                    .and_then(|progress| progress.get("generated_output_failures"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut generated_output_failure_notes = previous_progress
                    .and_then(|progress| progress.get("generated_output_failure_notes"))
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if source_ingestion_failure_notes.len() > MAX_RECORDED_MEMORY_NOTE_FAILURES {
                    source_ingestion_failure_notes = source_ingestion_failure_notes.split_off(
                        source_ingestion_failure_notes.len() - MAX_RECORDED_MEMORY_NOTE_FAILURES,
                    );
                }
                if generated_output_failure_notes.len() > MAX_RECORDED_MEMORY_NOTE_FAILURES {
                    generated_output_failure_notes = generated_output_failure_notes.split_off(
                        generated_output_failure_notes.len() - MAX_RECORDED_MEMORY_NOTE_FAILURES,
                    );
                }
                let mut consecutive_output_failures = 0_u32;
                let mut last_note_elapsed_ms = previous_progress
                    .and_then(|progress| progress.get("last_note_elapsed_ms"))
                    .and_then(serde_json::Value::as_i64);
                let enumeration_phase = if resume_index == total
                    && (source_ingestion_failures > 0 || generated_output_failures > 0)
                {
                    "completed_with_errors"
                } else if resume_index == total {
                    "completed"
                } else {
                    "enumerated"
                };
                if state
                    .jobs()
                    .update_progress(
                        job.id,
                        worker_id,
                        &json!({
                            "phase": enumeration_phase,
                            "completed": resume_index,
                            "total": total,
                            "current_index": null,
                            "current_path": null,
                            "last_completed_path": last_completed_path,
                            "items_published": items_published,
                            "empty_sets_published": empty_sets_published,
                            "source_ingestion_failures": source_ingestion_failures,
                            "source_ingestion_failure_notes": &source_ingestion_failure_notes,
                            "generated_output_failures": generated_output_failures,
                            "generated_output_failure_notes": &generated_output_failure_notes,
                            "notes_evaluated": notes_evaluated,
                            "source_policy_skipped": source_policy_skipped,
                            "already_evaluated_skipped": already_evaluated_skipped,
                            "note_started_at": null,
                            "last_note_elapsed_ms": last_note_elapsed_ms,
                            "error_code": null,
                        }),
                    )
                    .await
                    .is_err()
                {
                    log_job_progress_persist_failed(
                        &job,
                        enumeration_phase,
                        "memory_extract_progress_failed",
                    );
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_extract_progress_failed",
                    };
                }
                log_job_progress(
                    &job,
                    enumeration_phase,
                    resume_index as u64,
                    total as u64,
                    None,
                    None,
                    items_published,
                    empty_sets_published,
                    source_ingestion_failures,
                    generated_output_failures,
                    notes_evaluated,
                    source_policy_skipped,
                    already_evaluated_skipped,
                    last_note_elapsed_ms,
                    None,
                );
                for (index, entry) in entries.iter().enumerate().skip(resume_index) {
                    let note_started_at = now_millis();
                    if state
                        .jobs()
                        .update_progress(
                            job.id,
                            worker_id,
                            &json!({
                                "phase": "extracting_note",
                                "completed": index,
                                "total": total,
                                "current_index": index.saturating_add(1),
                                "current_path": entry.path.as_str(),
                                "last_completed_path": if index == 0 {
                                    last_completed_path
                                } else {
                                    Some(entries[index - 1].path.as_str())
                                },
                                "items_published": items_published,
                                "empty_sets_published": empty_sets_published,
                                "source_ingestion_failures": source_ingestion_failures,
                                "source_ingestion_failure_notes": &source_ingestion_failure_notes,
                                "generated_output_failures": generated_output_failures,
                                "generated_output_failure_notes": &generated_output_failure_notes,
                                "notes_evaluated": notes_evaluated,
                                "source_policy_skipped": source_policy_skipped,
                                "already_evaluated_skipped": already_evaluated_skipped,
                                "note_started_at": note_started_at,
                                "last_note_elapsed_ms": last_note_elapsed_ms,
                                "error_code": null,
                            }),
                        )
                        .await
                        .is_err()
                    {
                        // No provider call has happened for this note, so this
                        // checkpoint failure is safe to retry automatically.
                        log_job_progress_persist_failed(
                            &job,
                            "extracting_note",
                            "memory_extract_progress_failed",
                        );
                        return JobOutcome::Retry {
                            delay: Duration::from_secs(5),
                            code: "memory_extract_progress_failed",
                        };
                    }
                    log_job_progress(
                        &job,
                        "extracting_note",
                        index as u64,
                        total as u64,
                        Some(index.saturating_add(1) as u64),
                        Some(&entry.path),
                        items_published,
                        empty_sets_published,
                        source_ingestion_failures,
                        generated_output_failures,
                        notes_evaluated,
                        source_policy_skipped,
                        already_evaluated_skipped,
                        None,
                        None,
                    );
                    let result = tokio::select! {
                        _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                        result = memory.extract_note_with_options(
                            &context,
                            &core,
                            &entry.path,
                            NoteExtractionOptions { include_evaluated },
                        ) => result,
                    };
                    let note_elapsed_ms = now_millis().saturating_sub(note_started_at);
                    let mut stop_after_checkpoint = None;
                    match result {
                        Ok(extraction) => {
                            consecutive_output_failures = 0;
                            if extraction.already_evaluated {
                                already_evaluated_skipped =
                                    already_evaluated_skipped.saturating_add(1);
                            } else if extraction.source_admitted {
                                notes_evaluated = notes_evaluated.saturating_add(1);
                            } else {
                                source_policy_skipped = source_policy_skipped.saturating_add(1);
                            }
                            items_published = items_published
                                .saturating_add(u64::from(extraction.items_published));
                            empty_sets_published = empty_sets_published
                                .saturating_add(u64::from(extraction.empty_set_published));
                        }
                        Err(error) => {
                            let generated_output_diagnostic = match &error {
                                MemoryError::GeneratedOutput(code) => Some((*code, None)),
                                MemoryError::Provider(provider)
                                    if provider.is_generation_output_failure() =>
                                {
                                    Some((provider.code(), provider.schema_diagnostic()))
                                }
                                _ => None,
                            };
                            if let MemoryError::SourceIngestion(code) = &error {
                                source_ingestion_failures =
                                    source_ingestion_failures.saturating_add(1);
                                source_ingestion_failure_notes.push(json!({
                                    "index": index.saturating_add(1),
                                    "path": entry.path.as_str(),
                                    "error_code": code,
                                    "elapsed_ms": note_elapsed_ms,
                                }));
                                if source_ingestion_failure_notes.len()
                                    > MAX_RECORDED_MEMORY_NOTE_FAILURES
                                {
                                    let excess = source_ingestion_failure_notes.len()
                                        - MAX_RECORDED_MEMORY_NOTE_FAILURES;
                                    source_ingestion_failure_notes.drain(..excess);
                                }
                                log_memory_note_source_failure(
                                    &job,
                                    index.saturating_add(1) as u64,
                                    total as u64,
                                    &entry.path,
                                    note_elapsed_ms,
                                    code,
                                );
                            } else if let Some((code, schema_diagnostic)) =
                                generated_output_diagnostic
                            {
                                notes_evaluated = notes_evaluated.saturating_add(1);
                                generated_output_failures =
                                    generated_output_failures.saturating_add(1);
                                consecutive_output_failures =
                                    consecutive_output_failures.saturating_add(1);
                                generated_output_failure_notes.push(json!({
                                    "index": index.saturating_add(1),
                                    "path": entry.path.as_str(),
                                    "error_code": code,
                                    "schema_issue": schema_diagnostic.map(|(issue, _)| issue),
                                    "schema_path": schema_diagnostic.map(|(_, path)| path),
                                    "elapsed_ms": note_elapsed_ms,
                                }));
                                if generated_output_failure_notes.len()
                                    > MAX_RECORDED_MEMORY_NOTE_FAILURES
                                {
                                    let excess = generated_output_failure_notes.len()
                                        - MAX_RECORDED_MEMORY_NOTE_FAILURES;
                                    generated_output_failure_notes.drain(..excess);
                                }
                                log_memory_note_output_failure(
                                    &job,
                                    index.saturating_add(1) as u64,
                                    total as u64,
                                    &entry.path,
                                    note_elapsed_ms,
                                    code,
                                    schema_diagnostic,
                                    consecutive_output_failures,
                                );
                                if memory_output_failure_limit_reached(consecutive_output_failures)
                                {
                                    stop_after_checkpoint =
                                        Some("memory_extract_output_failure_limit");
                                }
                            } else {
                                let outcome = memory_extract_error_outcome(error);
                                let (phase, code) = match outcome {
                                    JobOutcome::Deferred { code, .. } => ("waiting_retry", code),
                                    JobOutcome::Retry { code, .. } => ("waiting_retry", code),
                                    JobOutcome::Failed { code } => ("failed", code),
                                    JobOutcome::Cancelled => {
                                        ("cancelled", "memory_extract_cancelled")
                                    }
                                    JobOutcome::Complete => {
                                        ("completed", "memory_extract_completed")
                                    }
                                };
                                let progress_saved = state
                                    .jobs()
                                    .update_progress(
                                        job.id,
                                        worker_id,
                                        &json!({
                                            "phase": phase,
                                            "completed": index,
                                            "total": total,
                                            "current_index": index.saturating_add(1),
                                            "current_path": entry.path.as_str(),
                                            "last_completed_path": if index == 0 {
                                                last_completed_path
                                            } else {
                                                Some(entries[index - 1].path.as_str())
                                            },
                                            "items_published": items_published,
                                            "empty_sets_published": empty_sets_published,
                                            "source_ingestion_failures": source_ingestion_failures,
                                            "source_ingestion_failure_notes": &source_ingestion_failure_notes,
                                            "generated_output_failures": generated_output_failures,
                                            "generated_output_failure_notes": &generated_output_failure_notes,
                                            "notes_evaluated": notes_evaluated,
                                            "source_policy_skipped": source_policy_skipped,
                                            "already_evaluated_skipped": already_evaluated_skipped,
                                            "note_started_at": note_started_at,
                                            "last_note_elapsed_ms": note_elapsed_ms,
                                            "error_code": code,
                                        }),
                                    )
                                    .await
                                    .is_ok();
                                if progress_saved {
                                    log_job_progress(
                                        &job,
                                        phase,
                                        index as u64,
                                        total as u64,
                                        Some(index.saturating_add(1) as u64),
                                        Some(&entry.path),
                                        items_published,
                                        empty_sets_published,
                                        source_ingestion_failures,
                                        generated_output_failures,
                                        notes_evaluated,
                                        source_policy_skipped,
                                        already_evaluated_skipped,
                                        Some(note_elapsed_ms),
                                        Some(code),
                                    );
                                } else {
                                    log_job_progress_persist_failed(&job, phase, code);
                                }
                                return outcome;
                            }
                        }
                    }
                    let note_phase = if stop_after_checkpoint.is_some() {
                        "stopped_output_failures"
                    } else if index.saturating_add(1) == total
                        && (source_ingestion_failures > 0 || generated_output_failures > 0)
                    {
                        "completed_with_errors"
                    } else if index.saturating_add(1) == total {
                        "completed"
                    } else {
                        "note_completed"
                    };
                    if state
                        .jobs()
                        .update_progress(
                            job.id,
                            worker_id,
                            &json!({
                                "phase": note_phase,
                                "completed": index.saturating_add(1),
                                "total": total,
                                "current_index": null,
                                "current_path": null,
                                "last_completed_path": entry.path.as_str(),
                                "items_published": items_published,
                                "empty_sets_published": empty_sets_published,
                                "source_ingestion_failures": source_ingestion_failures,
                                "source_ingestion_failure_notes": &source_ingestion_failure_notes,
                                "generated_output_failures": generated_output_failures,
                                "generated_output_failure_notes": &generated_output_failure_notes,
                                "notes_evaluated": notes_evaluated,
                                "source_policy_skipped": source_policy_skipped,
                                "already_evaluated_skipped": already_evaluated_skipped,
                                "note_started_at": null,
                                "last_note_elapsed_ms": note_elapsed_ms,
                                "error_code": stop_after_checkpoint,
                            }),
                        )
                        .await
                        .is_err()
                    {
                        // The provider may already have completed billable
                        // work. Stop instead of automatically replaying it.
                        log_job_progress_persist_failed(
                            &job,
                            note_phase,
                            "memory_extract_progress_finalize_failed",
                        );
                        return JobOutcome::Failed {
                            code: "memory_extract_progress_finalize_failed",
                        };
                    }
                    log_job_progress(
                        &job,
                        note_phase,
                        index.saturating_add(1) as u64,
                        total as u64,
                        None,
                        None,
                        items_published,
                        empty_sets_published,
                        source_ingestion_failures,
                        generated_output_failures,
                        notes_evaluated,
                        source_policy_skipped,
                        already_evaluated_skipped,
                        Some(note_elapsed_ms),
                        stop_after_checkpoint,
                    );
                    last_note_elapsed_ms = Some(note_elapsed_ms);
                    if let Some(code) = stop_after_checkpoint {
                        return JobOutcome::Failed { code };
                    }
                }
                JobOutcome::Complete
            } else {
                let Some(worker_id) = job.lease_owner.as_deref() else {
                    return JobOutcome::Failed {
                        code: "memory_extract_lease_missing",
                    };
                };
                let path = job
                    .payload
                    .get("payload")
                    .and_then(|value| value.get("path"))
                    .or_else(|| job.payload.get("path"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| VaultPath::parse(value).ok());
                let Some(path) = path else {
                    return JobOutcome::Failed {
                        code: "memory_extract_path_invalid",
                    };
                };
                let include_evaluated = job
                    .payload
                    .get("include_evaluated")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let note_started_at = now_millis();
                if state
                    .jobs()
                    .update_progress(
                        job.id,
                        worker_id,
                        &json!({
                            "phase": "extracting_note",
                            "completed": 0,
                            "total": 1,
                            "current_index": 1,
                            "current_path": path.as_str(),
                            "last_completed_path": null,
                            "items_published": 0,
                            "empty_sets_published": 0,
                            "source_ingestion_failures": 0,
                            "source_ingestion_failure_notes": [],
                            "generated_output_failures": 0,
                            "generated_output_failure_notes": [],
                            "notes_evaluated": 0,
                            "source_policy_skipped": 0,
                            "already_evaluated_skipped": 0,
                            "note_started_at": note_started_at,
                            "last_note_elapsed_ms": null,
                            "error_code": null,
                        }),
                    )
                    .await
                    .is_err()
                {
                    log_job_progress_persist_failed(
                        &job,
                        "extracting_note",
                        "memory_extract_progress_failed",
                    );
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_extract_progress_failed",
                    };
                }
                log_job_progress(
                    &job,
                    "extracting_note",
                    0,
                    1,
                    Some(1),
                    Some(&path),
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                );
                let result = tokio::select! {
                    _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                    result = memory.extract_note_with_options(
                        &context,
                        &core,
                        &path,
                        NoteExtractionOptions { include_evaluated },
                    ) => result,
                };
                let note_elapsed_ms = now_millis().saturating_sub(note_started_at);
                match result {
                    Ok(extraction) => {
                        let notes_evaluated = u64::from(extraction.source_admitted);
                        let already_evaluated_skipped = u64::from(extraction.already_evaluated);
                        let source_policy_skipped =
                            u64::from(!extraction.source_admitted && !extraction.already_evaluated);
                        if state
                            .jobs()
                            .update_progress(
                                job.id,
                                worker_id,
                                &json!({
                                    "phase": "completed",
                                    "completed": 1,
                                    "total": 1,
                                    "current_index": null,
                                    "current_path": null,
                                    "last_completed_path": path.as_str(),
                                    "items_published": u64::from(extraction.items_published),
                                    "empty_sets_published": u64::from(extraction.empty_set_published),
                                    "source_ingestion_failures": 0,
                                    "source_ingestion_failure_notes": [],
                                    "generated_output_failures": 0,
                                    "generated_output_failure_notes": [],
                                    "notes_evaluated": notes_evaluated,
                                    "source_policy_skipped": source_policy_skipped,
                                    "already_evaluated_skipped": already_evaluated_skipped,
                                    "note_started_at": null,
                                    "last_note_elapsed_ms": note_elapsed_ms,
                                    "error_code": null,
                                }),
                            )
                            .await
                            .is_err()
                        {
                            log_job_progress_persist_failed(
                                &job,
                                "completed",
                                "memory_extract_progress_finalize_failed",
                            );
                            return JobOutcome::Failed {
                                code: "memory_extract_progress_finalize_failed",
                            };
                        }
                        log_job_progress(
                            &job,
                            "completed",
                            1,
                            1,
                            None,
                            None,
                            u64::from(extraction.items_published),
                            u64::from(extraction.empty_set_published),
                            0,
                            0,
                            notes_evaluated,
                            source_policy_skipped,
                            already_evaluated_skipped,
                            Some(note_elapsed_ms),
                            None,
                        );
                        JobOutcome::Complete
                    }
                    Err(error) => {
                        let source_failure_code = match &error {
                            MemoryError::SourceIngestion(code) => Some(*code),
                            _ => None,
                        };
                        let generated_output_diagnostic = match &error {
                            MemoryError::GeneratedOutput(code) => Some((*code, None)),
                            MemoryError::Provider(provider)
                                if provider.is_generation_output_failure() =>
                            {
                                Some((
                                    provider.code(),
                                    provider
                                        .schema_diagnostic()
                                        .map(|(issue, path)| (issue, path.to_owned())),
                                ))
                            }
                            _ => None,
                        };
                        let output_failure = generated_output_diagnostic.is_some();
                        let outcome = memory_extract_error_outcome(error);
                        let (phase, code) = match outcome {
                            JobOutcome::Deferred { code, .. } => ("waiting_retry", code),
                            JobOutcome::Retry { code, .. } => ("waiting_retry", code),
                            JobOutcome::Failed { code } => ("failed", code),
                            JobOutcome::Cancelled => ("cancelled", "memory_extract_cancelled"),
                            JobOutcome::Complete => ("completed", "memory_extract_completed"),
                        };
                        let source_ingestion_failure_notes =
                            source_failure_code.map_or_else(Vec::<Value>::new, |code| {
                                vec![json!({
                                    "index": 1,
                                    "path": path.as_str(),
                                    "error_code": code,
                                    "elapsed_ms": note_elapsed_ms,
                                })]
                            });
                        let generated_output_failure_notes = generated_output_diagnostic
                            .as_ref()
                            .map_or_else(Vec::<Value>::new, |(code, schema_diagnostic)| {
                            vec![json!({
                                "index": 1,
                                "path": path.as_str(),
                                "error_code": code,
                                "schema_issue": schema_diagnostic.as_ref().map(|(issue, _)| *issue),
                                "schema_path": schema_diagnostic.as_ref().map(|(_, path)| path),
                                "elapsed_ms": note_elapsed_ms,
                            })]
                        });
                        if let Some(code) = source_failure_code {
                            log_memory_note_source_failure(
                                &job,
                                1,
                                1,
                                &path,
                                note_elapsed_ms,
                                code,
                            );
                        }
                        if let Some((code, schema_diagnostic)) =
                            generated_output_diagnostic.as_ref()
                        {
                            log_memory_note_output_failure(
                                &job,
                                1,
                                1,
                                &path,
                                note_elapsed_ms,
                                code,
                                schema_diagnostic
                                    .as_ref()
                                    .map(|(issue, path)| (*issue, path.as_str())),
                                1,
                            );
                        }
                        let progress_saved = state
                            .jobs()
                            .update_progress(
                                job.id,
                                worker_id,
                                &json!({
                                    "phase": phase,
                                    "completed": 0,
                                    "total": 1,
                                    "current_index": 1,
                                    "current_path": path.as_str(),
                                    "last_completed_path": null,
                                    "items_published": 0,
                                    "empty_sets_published": 0,
                                    "source_ingestion_failures": u64::from(source_failure_code.is_some()),
                                    "source_ingestion_failure_notes": &source_ingestion_failure_notes,
                                    "generated_output_failures": u64::from(output_failure),
                                    "generated_output_failure_notes": &generated_output_failure_notes,
                                    "notes_evaluated": u64::from(output_failure),
                                    "source_policy_skipped": 0,
                                    "already_evaluated_skipped": 0,
                                    "schema_issue": generated_output_diagnostic.as_ref().and_then(|(_, diagnostic)| diagnostic.as_ref().map(|(issue, _)| *issue)),
                                    "schema_path": generated_output_diagnostic.as_ref().and_then(|(_, diagnostic)| diagnostic.as_ref().map(|(_, path)| path)),
                                    "note_started_at": note_started_at,
                                    "last_note_elapsed_ms": note_elapsed_ms,
                                    "error_code": code,
                                }),
                            )
                            .await
                            .is_ok();
                        if progress_saved {
                            log_job_progress(
                                &job,
                                phase,
                                0,
                                1,
                                Some(1),
                                Some(&path),
                                0,
                                0,
                                u64::from(source_failure_code.is_some()),
                                u64::from(output_failure),
                                u64::from(output_failure),
                                0,
                                0,
                                Some(note_elapsed_ms),
                                Some(code),
                            );
                        } else {
                            log_job_progress_persist_failed(&job, phase, code);
                        }
                        outcome
                    }
                }
            }
        })
    })
}

fn memory_extract_error_outcome(error: MemoryError) -> JobOutcome {
    match error {
        MemoryError::Configuration(code) => JobOutcome::Failed { code },
        MemoryError::SourceIngestion(code) | MemoryError::GeneratedOutput(code) => {
            JobOutcome::Failed { code }
        }
        MemoryError::Provider(error) if error.retryable() => JobOutcome::Retry {
            delay: Duration::from_secs(10),
            code: error.code(),
        },
        MemoryError::Provider(error) => JobOutcome::Failed { code: error.code() },
        error if error.retryable() => JobOutcome::Retry {
            delay: Duration::from_secs(10),
            code: "memory_extract_retryable",
        },
        MemoryError::InvalidInput(_) | MemoryError::Markdown => JobOutcome::Failed {
            code: "memory_extract_input_invalid",
        },
        MemoryError::NotFound => JobOutcome::Failed {
            code: "memory_extract_not_found",
        },
        _ => JobOutcome::Failed {
            code: "memory_extract_failed",
        },
    }
}

const fn memory_output_failure_limit_reached(consecutive_failures: u32) -> bool {
    consecutive_failures >= MAX_CONSECUTIVE_MEMORY_OUTPUT_FAILURES
}

pub(crate) async fn retire_legacy_memory_jobs(
    state: &StateStore,
    context: &VaultContext,
) -> Result<(), mcp_vault_state::StateError> {
    for job_type in [
        "memory.consolidate",
        "memory.enrich_retrieval",
        "memory.reset_pipeline",
        "memory.revalidate",
        "memory.audit_sources",
        "memory.rebuild",
        "memory.repair_sources",
    ] {
        state.jobs().request_cancel_type(context, job_type).await?;
    }
    Ok(())
}

/// Reconcile one source identity/hash change before optional extraction.
pub fn memory_source_reconcile_job_handler(
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
                    code: "memory_source_reconcile_vault_missing",
                };
            };
            let Some(file_id) = job
                .payload
                .get("aggregate_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| FileId::parse(value).ok())
            else {
                return JobOutcome::Failed {
                    code: "memory_source_reconcile_file_missing",
                };
            };
            let event_type = job
                .payload
                .get("event_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("external_change")
                .to_owned();
            let event_id = job
                .payload
                .get("event_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| EventId::parse(value).ok());
            let vault = match state.vaults().find_by_id(vault_id).await {
                Ok(Some(vault)) => vault,
                Ok(None) => {
                    return JobOutcome::Failed {
                        code: "memory_source_reconcile_vault_missing",
                    };
                }
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_source_reconcile_vault_lookup_failed",
                    };
                }
            };
            let context = match vault.context() {
                Ok(context) => context,
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "memory_source_reconcile_context_invalid",
                    };
                }
            };
            let core = match super::core_for_vault(&state, &history_root, &vault, &core_runtime) {
                Ok(core) => core,
                Err(_) => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(5),
                        code: "memory_source_reconcile_core_unavailable",
                    };
                }
            };
            let report = tokio::select! {
                _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                result = memory.reconcile_current_source_event(&context, &core, file_id) => result,
            };
            let report = match report {
                Ok(report) => report,
                Err(error) if error.retryable() => {
                    return JobOutcome::Retry {
                        delay: Duration::from_secs(10),
                        code: "memory_source_reconcile_retryable",
                    };
                }
                Err(_) => {
                    return JobOutcome::Failed {
                        code: "memory_source_reconcile_failed",
                    };
                }
            };

            let path = job
                .payload
                .get("payload")
                .and_then(|payload| payload.get("path"))
                .and_then(serde_json::Value::as_str);
            let should_extract = is_memory_extract_event_type(&event_type)
                && path.is_some_and(|path| {
                    path.to_ascii_lowercase().ends_with(".md")
                        && !path_in_reserved_namespace(&vault.reserved_root, path)
                });
            let mut extraction_followup = "not_applicable";
            if should_extract {
                let extraction_ready = memory
                    .extraction_readiness(&context)
                    .await
                    .ok()
                    .is_some_and(|readiness| readiness.ready);
                if extraction_ready {
                    let dedup_event = event_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| job.id.to_string());
                    if state
                        .jobs()
                        .enqueue(
                            &context,
                            "memory.extract",
                            &format!("vault:{vault_id}:memory-extract:{dedup_event}"),
                            &job.payload,
                            0,
                            10,
                            now_millis(),
                        )
                        .await
                        .is_err()
                    {
                        return JobOutcome::Retry {
                            delay: Duration::from_secs(5),
                            code: "memory_source_reconcile_extract_admission_failed",
                        };
                    }
                    extraction_followup = "memory.extract";
                } else {
                    extraction_followup = "disabled_or_unconfigured";
                }
            }

            if let Some(worker_id) = job.lease_owner.as_deref()
                && state
                    .jobs()
                    .update_progress(
                        job.id,
                        worker_id,
                        &json!({
                            "phase": "completed",
                            "sources_checked": report.sources_checked,
                            "current": report.current,
                            "moved": report.moved,
                            "changed": report.changed,
                            "deleted": report.deleted,
                            "memories_hidden": report.memories_hidden,
                            "memories_removed": report.memories_removed,
                            "extraction_followup": extraction_followup,
                        }),
                    )
                    .await
                    .is_err()
            {
                return JobOutcome::Retry {
                    delay: Duration::from_secs(5),
                    code: "memory_source_reconcile_progress_failed",
                };
            }
            JobOutcome::Complete
        })
    })
}

/// Rebuild current note or memory embedding chunks.
pub fn embedding_job_handler(
    state: StateStore,
    index: mcp_vault_indexer::IndexService,
    memory: MemoryService,
) -> JobHandler {
    Arc::new(move |job, shutdown| {
        let state = state.clone();
        let index = index.clone();
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
            let source_type = sources.first().map(|source| source.object_type.as_str());
            if sources
                .iter()
                .any(|source| Some(source.object_type.as_str()) != source_type)
            {
                return JobOutcome::Failed {
                    code: "embedding_payload_mixed_sources",
                };
            }
            match source_type {
                Some("note") => {
                    let result = tokio::select! {
                        _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                        result = index.reembed_note_sources(&context, model_id, &sources) => result,
                    };
                    note_embedding_error_outcome(result)
                }
                Some("memory") => {
                    let result = tokio::select! {
                        _ = shutdown.cancelled() => return JobOutcome::Cancelled,
                        result = memory.reembed_sources(&context, model_id, &sources) => result,
                    };
                    memory_embedding_error_outcome(result)
                }
                _ => JobOutcome::Failed {
                    code: "embedding_source_unsupported",
                },
            }
        })
    })
}

fn note_embedding_error_outcome(result: Result<u64, mcp_vault_indexer::IndexError>) -> JobOutcome {
    match result {
        Ok(_) => JobOutcome::Complete,
        Err(mcp_vault_indexer::IndexError::Provider(error)) if error.retryable() => {
            JobOutcome::Retry {
                delay: Duration::from_secs(10),
                code: error.code(),
            }
        }
        Err(mcp_vault_indexer::IndexError::Provider(error)) => {
            JobOutcome::Failed { code: error.code() }
        }
        Err(mcp_vault_indexer::IndexError::State(_) | mcp_vault_indexer::IndexError::Core(_)) => {
            JobOutcome::Retry {
                delay: Duration::from_secs(10),
                code: "embedding_retryable",
            }
        }
        Err(_) => JobOutcome::Failed {
            code: "embedding_rebuild_failed",
        },
    }
}

fn memory_embedding_error_outcome(result: Result<u64, MemoryError>) -> JobOutcome {
    match result {
        Ok(_) | Err(MemoryError::NotFound) => JobOutcome::Complete,
        Err(error) if error.retryable() => JobOutcome::Retry {
            delay: Duration::from_secs(10),
            code: error.code(),
        },
        Err(error) => JobOutcome::Failed { code: error.code() },
    }
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

#[cfg(test)]
fn path_is_memory_record(root: &VaultPath, raw_path: &str) -> bool {
    let Ok(path) = VaultPath::parse(raw_path) else {
        return false;
    };
    let prefix = format!("{}/memory/records/", root.as_str());
    path.as_str().starts_with(&prefix) && path.as_str().ends_with(".md")
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
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::{
        Cancellation, JobHandler, JobOutcome, OutboxHandler, WorkerConfig, WorkerFailure,
        WorkerStatus, WorkerSupervisor, index_rebuild_job_handler, memory_embedding_error_outcome,
        memory_extract_error_outcome, memory_extract_job_handler,
        memory_output_failure_limit_reached, memory_source_reconcile_job_handler,
        note_embedding_error_outcome, now_millis, outbox_event_job_handler, outbox_to_job_handler,
        path_is_memory_record, redacted_path_hash, vault_initialize_job_handler,
    };
    use axum::{Json, Router, extract::State as AxumState, routing::post};
    use mcp_vault_auth::{AuthService, MasterKeyRing};
    use mcp_vault_core::{ManagedVaultService, VaultCore, VaultCoreRuntime};
    use mcp_vault_domain::{
        Actor, EventId, FileId, MaintenanceGate, Revision, SourcePlane, VaultContext, VaultId,
        VaultPath, VaultPathPolicy, VaultSlug, WritePrecondition,
    };
    use mcp_vault_memory::{
        ExtractionPolicy, MEMORY_CONTRACT_GENERATION, MemoryError, MemoryService,
    };
    use mcp_vault_providers::{
        ModelCapabilities, ModelInput, ModelSettings, ProviderError, ProviderInput, ProviderKind,
        ProviderMode, ProviderService, ProviderSettings,
    };
    use mcp_vault_state::{JobStatus, OutboxEventRecord, StateStore, VaultStatus};
    use mcp_vault_storage_fs::StorageOptions;
    use serde_json::{Value, json};
    use tokio::{
        sync::Notify,
        time::{Duration, sleep, timeout},
    };

    fn test_memory_service(state: &StateStore) -> MemoryService {
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[8_u8; 32]).unwrap(),
        );
        MemoryService::new(state.clone(), auth)
    }

    fn test_outbox_handler(state: &StateStore) -> OutboxHandler {
        outbox_to_job_handler(state.clone(), test_memory_service(state))
    }

    #[tokio::test]
    async fn managed_vault_initialization_builds_vault_scoped_state() {
        let root = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let managed = ManagedVaultService::new(
            state.clone(),
            root.path().to_owned(),
            StorageOptions::default(),
        );
        let created = managed
            .create(VaultSlug::new("work").unwrap(), "Work")
            .await
            .unwrap();
        let context = created.vault.context().unwrap();
        let outcome = (vault_initialize_job_handler(
            state.clone(),
            root.path().join("history"),
            VaultCoreRuntime::default(),
        ))(created.initialization_job, Cancellation::default())
        .await;

        assert_eq!(outcome, JobOutcome::Complete);
        assert_eq!(
            state
                .scan_checkpoints()
                .get(&context, "initial")
                .await
                .unwrap()
                .unwrap()
                .status
                .as_str(),
            "completed"
        );
        assert!(state.index().status(&context).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn terminal_initialization_failure_marks_only_that_vault_error() {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("broken-init").unwrap(),
            "/srv/broken-init".into(),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Broken init", VaultStatus::Active)
            .await
            .unwrap();
        let job = state
            .jobs()
            .enqueue(
                &context,
                "vault.initialize",
                &format!("vault:{}:initialize", context.id()),
                &json!({}),
                20,
                1,
                0,
            )
            .await
            .unwrap();
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            test_outbox_handler(&state),
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                lease_duration: Duration::from_millis(100),
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        supervisor
            .register_job_handler(
                "vault.initialize",
                Arc::new(|_, _| {
                    Box::pin(async {
                        JobOutcome::Failed {
                            code: "init_failed",
                        }
                    })
                }),
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
                let failed = state
                    .jobs()
                    .get(&context, job.id)
                    .await
                    .unwrap()
                    .is_some_and(|job| job.status == JobStatus::Failed);
                let errored = state
                    .vaults()
                    .find_by_id(context.id())
                    .await
                    .unwrap()
                    .is_some_and(|vault| vault.status == VaultStatus::Error);
                if failed && errored {
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

    async fn mixed_extraction_response(
        AxumState(calls): AxumState<Arc<AtomicUsize>>,
        Json(request): Json<Value>,
    ) -> Json<Value> {
        let call = calls.fetch_add(1, Ordering::SeqCst);
        let content = if call == 0 {
            r#"{"memories":[{"content":"The first note records a durable decision.","kind":"decision"}],"unexpected":"reject"}"#
        } else if call == 1 {
            r#"{"memories":[{"content":"The second note records a durable decision.","kind":"decision","tags":["test"]}]}"#
        } else if call == 2 || call == 3 {
            r#"{"memories":[{"content":"The first note records a durable decision.","kind":"decision"}]}"#
        } else {
            r#"{"memories":[{"content":"The second note records a durable decision.","kind":"decision"}]}"#
        };
        assert_eq!(request["model"], "fake-extraction");
        Json(json!({
            "choices": [{
                "message": {"content": content},
                "finish_reason": "stop"
            }]
        }))
    }
    #[test]
    fn memory_extraction_jobs_preserve_redacted_provider_error_codes() {
        assert_eq!(
            memory_extract_error_outcome(MemoryError::Provider(ProviderError::Transport {
                code: "provider_connect_failed",
                retryable: true,
            })),
            JobOutcome::Retry {
                delay: Duration::from_secs(10),
                code: "provider_connect_failed",
            }
        );
        assert_eq!(
            memory_extract_error_outcome(MemoryError::Provider(ProviderError::EndpointDenied)),
            JobOutcome::Failed {
                code: "provider_endpoint_denied",
            }
        );
        assert_eq!(
            memory_extract_error_outcome(MemoryError::Provider(ProviderError::Transport {
                code: "provider_response_timeout",
                retryable: false,
            })),
            JobOutcome::Failed {
                code: "provider_response_timeout",
            }
        );
    }

    #[test]
    fn embedding_jobs_preserve_redacted_provider_error_codes() {
        assert_eq!(
            note_embedding_error_outcome(Err(mcp_vault_indexer::IndexError::Provider(
                ProviderError::HttpStatus {
                    status: 413,
                    retryable: false,
                },
            ))),
            JobOutcome::Failed {
                code: "provider_http_error",
            }
        );
        assert_eq!(
            memory_embedding_error_outcome(Err(MemoryError::Provider(
                ProviderError::DimensionMismatch,
            ))),
            JobOutcome::Failed {
                code: "embedding_dimension_mismatch",
            }
        );
    }

    #[test]
    fn one_model_output_failure_does_not_open_the_batch_circuit() {
        assert!(!memory_output_failure_limit_reached(1));
        assert!(!memory_output_failure_limit_reached(2));
        assert!(memory_output_failure_limit_reached(3));
    }

    #[test]
    fn job_progress_path_logging_is_stable_but_does_not_emit_the_path() {
        let path = VaultPath::parse("private/project/roadmap.md").unwrap();
        let hash = redacted_path_hash(&path);
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash, redacted_path_hash(&path));
        assert!(!hash.contains(path.as_str()));
    }

    #[test]
    fn only_canonical_memory_records_admit_projection_rebuilds() {
        let root = VaultPath::parse("_mcp-vault").unwrap();
        assert!(path_is_memory_record(
            &root,
            "_mcp-vault/memory/records/2026/08/memory.md"
        ));
        assert!(!path_is_memory_record(&root, "_mcp-vault/memory/MEMORY.md"));
        assert!(!path_is_memory_record(
            &root,
            "_mcp-vault/memory/source_summaries/source.md"
        ));
        assert!(!path_is_memory_record(&root, "../memory.md"));
    }

    #[tokio::test]
    async fn file_events_admit_source_reconciliation_before_optional_extraction() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("legacy-event").unwrap(),
            PathBuf::from(directory.path()).join("content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Legacy event", VaultStatus::Active)
            .await
            .unwrap();
        let memory = test_memory_service(&state);
        memory
            .set_extraction_policy(
                &context,
                ExtractionPolicy {
                    enabled: true,
                    ..ExtractionPolicy::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let event = OutboxEventRecord {
            id: EventId::new(),
            vault_id: Some(context.id()),
            event_type: "external_change".to_owned(),
            aggregate_type: "file".to_owned(),
            aggregate_id: FileId::new().to_string(),
            payload: serde_json::json!({
                "operation": "external_change",
                "path": "legacy.md",
            }),
            created_at: 1,
            available_at: 1,
            claimed_by: None,
            claimed_until: None,
            delivered_at: None,
            attempts: 0,
            last_error: None,
            dead_lettered: false,
            dead_letter_reason: None,
        };
        outbox_to_job_handler(state.clone(), memory.clone())(event)
            .await
            .unwrap();

        let extraction_jobs = state
            .jobs()
            .list(&context, None, Some("memory.extract"), 10, 0)
            .await
            .unwrap();
        assert!(extraction_jobs.is_empty());
        let reconciliation_jobs = state
            .jobs()
            .list(&context, None, Some("memory.source_reconcile"), 10, 0)
            .await
            .unwrap();
        assert_eq!(reconciliation_jobs.len(), 1);
        assert_eq!(
            reconciliation_jobs[0].payload["memory_contract_generation"],
            MEMORY_CONTRACT_GENERATION
        );

        state
            .settings()
            .set_vault(
                &context,
                "memory.extraction.policy",
                &json!({"enabled": true, "max_candidates_per_note": 11}),
                WritePrecondition::ExactRevision(Revision::new(1)),
                None,
            )
            .await
            .unwrap();
        let invalid_policy_event = OutboxEventRecord {
            id: EventId::new(),
            vault_id: Some(context.id()),
            event_type: "FileUpdated".to_owned(),
            aggregate_type: "file".to_owned(),
            aggregate_id: FileId::new().to_string(),
            payload: json!({"operation": "replace", "path": "invalid-policy.md"}),
            created_at: 2,
            available_at: 2,
            claimed_by: None,
            claimed_until: None,
            delivered_at: None,
            attempts: 0,
            last_error: None,
            dead_lettered: false,
            dead_letter_reason: None,
        };
        outbox_to_job_handler(state.clone(), memory)(invalid_policy_event)
            .await
            .unwrap();
        assert_eq!(
            state
                .jobs()
                .list(&context, None, Some("memory.source_reconcile"), 10, 0)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            state
                .jobs()
                .list(&context, None, Some("index.rebuild"), 10, 0)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn moved_note_admits_source_reconciliation_without_provider_extraction() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("move-event").unwrap(),
            PathBuf::from(directory.path()).join("content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Move event", VaultStatus::Active)
            .await
            .unwrap();
        let memory = test_memory_service(&state);
        memory
            .set_extraction_policy(
                &context,
                ExtractionPolicy {
                    enabled: true,
                    ..ExtractionPolicy::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        outbox_to_job_handler(state.clone(), memory)(OutboxEventRecord {
            id: EventId::new(),
            vault_id: Some(context.id()),
            event_type: "FileMoved".to_owned(),
            aggregate_type: "file".to_owned(),
            aggregate_id: FileId::new().to_string(),
            payload: json!({"operation": "move", "path": "renamed.md"}),
            created_at: 1,
            available_at: 1,
            claimed_by: None,
            claimed_until: None,
            delivered_at: None,
            attempts: 0,
            last_error: None,
            dead_lettered: false,
            dead_letter_reason: None,
        })
        .await
        .unwrap();

        assert!(
            state
                .jobs()
                .list(&context, None, Some("memory.extract"), 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            state
                .jobs()
                .list(&context, None, Some("memory.source_reconcile"), 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
    }

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
            test_outbox_handler(&state),
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
            test_outbox_handler(&state),
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
    async fn long_job_does_not_block_later_jobs_from_free_capacity() {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("fair-job-dispatch").unwrap(),
            "/srv/fair-job-dispatch".into(),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Fair job dispatch", VaultStatus::Active)
            .await
            .unwrap();
        let blocker = state
            .jobs()
            .enqueue(
                &context,
                "test.fair",
                "fair:blocker",
                &json!({"blocked": true}),
                0,
                3,
                0,
            )
            .await
            .unwrap();
        state
            .jobs()
            .enqueue(
                &context,
                "test.fair",
                "fair:first-short",
                &json!({"blocked": false}),
                0,
                3,
                0,
            )
            .await
            .unwrap();

        let release = Arc::new(Notify::new());
        let short_completions = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum_active = Arc::new(AtomicUsize::new(0));
        let handler: JobHandler = Arc::new({
            let release = release.clone();
            let short_completions = short_completions.clone();
            let active = active.clone();
            let maximum_active = maximum_active.clone();
            move |job, cancellation| {
                let release = release.clone();
                let short_completions = short_completions.clone();
                let active = active.clone();
                let maximum_active = maximum_active.clone();
                Box::pin(async move {
                    let running = active.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                    maximum_active.fetch_max(running, Ordering::SeqCst);
                    let outcome = if job.payload["blocked"].as_bool().unwrap_or(false) {
                        tokio::select! {
                            _ = release.notified() => JobOutcome::Complete,
                            _ = cancellation.cancelled() => JobOutcome::Cancelled,
                        }
                    } else {
                        short_completions.fetch_add(1, Ordering::SeqCst);
                        JobOutcome::Complete
                    };
                    active.fetch_sub(1, Ordering::SeqCst);
                    outcome
                })
            }
        });
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            test_outbox_handler(&state),
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                lease_duration: Duration::from_millis(100),
                batch_size: 16,
                concurrency: 2,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        supervisor
            .register_job_handler("test.fair", handler)
            .unwrap();
        let shutdown = Cancellation::default();
        let running = tokio::spawn({
            let supervisor = supervisor.clone();
            let shutdown = shutdown.clone();
            async move { supervisor.run(shutdown).await }
        });

        timeout(Duration::from_secs(2), async {
            loop {
                let blocker_running = state
                    .jobs()
                    .get(&context, blocker.id)
                    .await
                    .unwrap()
                    .is_some_and(|job| job.status == JobStatus::Running);
                if blocker_running && short_completions.load(Ordering::SeqCst) == 1 {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();

        let later = state
            .jobs()
            .enqueue(
                &context,
                "test.fair",
                "fair:later-short",
                &json!({"blocked": false}),
                0,
                3,
                now_millis(),
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(2), async {
            loop {
                let later_completed = state
                    .jobs()
                    .get(&context, later.id)
                    .await
                    .unwrap()
                    .is_some_and(|job| job.status == JobStatus::Completed);
                let blocker_still_running = state
                    .jobs()
                    .get(&context, blocker.id)
                    .await
                    .unwrap()
                    .is_some_and(|job| job.status == JobStatus::Running);
                if later_completed && blocker_still_running {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(short_completions.load(Ordering::SeqCst), 2);
        assert!(maximum_active.load(Ordering::SeqCst) <= 2);

        release.notify_one();
        timeout(Duration::from_secs(2), async {
            loop {
                if state.jobs().pending_count().await.unwrap() == 0 {
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
    async fn obsolete_memory_job_with_old_cursor_is_discarded_before_handler_call() {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("obsolete-memory-job").unwrap(),
            "/srv/obsolete-memory-job".into(),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Obsolete memory job", VaultStatus::Active)
            .await
            .unwrap();
        let old = state
            .jobs()
            .enqueue(
                &context,
                "memory.extract",
                "obsolete:memory-extract",
                &json!({
                    "scope": "all",
                    "reason": "old_pipeline",
                }),
                0,
                5,
                0,
            )
            .await
            .unwrap();
        let now = now_millis();
        state
            .jobs()
            .claim_batch("old-worker", now, now.saturating_add(60_000), 1)
            .await
            .unwrap();
        state
            .jobs()
            .update_progress(
                old.id,
                "old-worker",
                &json!({
                    "phase": "extracting_note",
                    "completed": 68,
                    "total": 178,
                    "last_completed_path": "notes/068.md",
                }),
            )
            .await
            .unwrap();
        state
            .jobs()
            .release_worker_leases("old-worker")
            .await
            .unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let handler: JobHandler = Arc::new({
            let calls = calls.clone();
            move |_job, _cancellation| {
                let calls = calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    JobOutcome::Complete
                })
            }
        });
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            test_outbox_handler(&state),
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                lease_duration: Duration::from_millis(100),
                concurrency: 1,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        supervisor
            .register_job_handler("memory.extract", handler)
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
                    .get(&context, old.id)
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
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let discarded = state.jobs().get(&context, old.id).await.unwrap().unwrap();
        assert_eq!(discarded.progress.unwrap()["completed"], 68);

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
            test_outbox_handler(&state),
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
    async fn full_vault_extraction_distinguishes_source_and_generated_output_failures() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("mixed-extraction").unwrap(),
            PathBuf::from(directory.path()).join("content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Mixed extraction", VaultStatus::Active)
            .await
            .unwrap();
        let core_runtime = mcp_vault_core::VaultCoreRuntime::default();
        let history_root = PathBuf::from(directory.path()).join("history");
        let core = VaultCore::new(
            state.clone(),
            history_root.clone(),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            core_runtime.clone(),
        );
        let mut source_files = Vec::new();
        for (path, body) in [
            ("a.md", b"First note.".as_slice()),
            ("b.md", b"Second note.".as_slice()),
            ("source-invalid.md", b"\xff\xfe".as_slice()),
        ] {
            let created = core
                .create_bytes(
                    &context,
                    &VaultPath::parse(path).unwrap(),
                    body,
                    Actor::system(),
                    SourcePlane::System,
                    None,
                )
                .await
                .unwrap();
            source_files.push(created.file);
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider_server = tokio::spawn({
            let calls = calls.clone();
            async move {
                axum::serve(
                    listener,
                    Router::new()
                        .route("/v1/chat/completions", post(mixed_extraction_response))
                        .with_state(calls),
                )
                .await
                .unwrap();
            }
        });
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[8_u8; 32]).unwrap(),
        );
        let providers = ProviderService::new(state.clone(), auth);
        providers
            .set_provider_mode(&context, ProviderMode::LocalOnly, None)
            .await
            .unwrap();
        let provider = providers
            .create_provider(ProviderInput {
                name: "mixed-extraction".to_owned(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: url::Url::parse(&format!("http://{address}/v1/")).unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: None,
            })
            .await
            .unwrap();
        let model = providers
            .register_model(ModelInput {
                provider_id: provider.id,
                external_model_id: "fake-extraction".to_owned(),
                capabilities: ModelCapabilities {
                    structured_output: true,
                    ..ModelCapabilities::default()
                },
                settings: ModelSettings::default(),
                enabled: true,
            })
            .await
            .unwrap();
        providers
            .bind_model(
                Some(&context),
                "memory_extraction",
                model.id,
                json!({}),
                None,
            )
            .await
            .unwrap();
        let memory = MemoryService::with_provider_service(state.clone(), providers);
        memory
            .set_extraction_policy(
                &context,
                ExtractionPolicy {
                    enabled: true,
                    ..ExtractionPolicy::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let job = state
            .jobs()
            .enqueue(
                &context,
                "memory.extract",
                "test:mixed-extraction:all",
                &json!({
                    "memory_contract_generation": MEMORY_CONTRACT_GENERATION,
                    "scope": "all",
                    "reason": "test",
                }),
                0,
                5,
                now_millis(),
            )
            .await
            .unwrap();
        let now = now_millis();
        let mut claimed = state
            .jobs()
            .claim_batch("mixed-worker", now, now.saturating_add(60_000), 1)
            .await
            .unwrap();
        let claimed = claimed.remove(0);
        let outcome = memory_extract_job_handler(
            state.clone(),
            history_root.clone(),
            core_runtime.clone(),
            memory.clone(),
        )(claimed, Cancellation::default())
        .await;

        assert_eq!(outcome, JobOutcome::Complete);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let progress = state
            .jobs()
            .get(&context, job.id)
            .await
            .unwrap()
            .unwrap()
            .progress
            .unwrap();
        assert_eq!(progress["phase"], "completed_with_errors");
        assert_eq!(progress["completed"], 3);
        assert_eq!(progress["notes_evaluated"], 2);
        assert_eq!(progress["generated_output_failures"], 1);
        assert_eq!(progress["source_ingestion_failures"], 1);
        assert_eq!(progress["items_published"], 1);
        assert_eq!(
            progress["generated_output_failure_notes"][0]["path"],
            "a.md"
        );
        assert_eq!(
            progress["generated_output_failure_notes"][0]["error_code"],
            "provider_schema_invalid"
        );
        assert_eq!(
            progress["generated_output_failure_notes"][0]["schema_issue"],
            "unexpected_property"
        );
        assert_eq!(
            progress["generated_output_failure_notes"][0]["schema_path"],
            "$"
        );
        assert_eq!(
            progress["source_ingestion_failure_notes"][0]["path"],
            "source-invalid.md"
        );
        assert_eq!(
            progress["source_ingestion_failure_notes"][0]["error_code"],
            "memory_source_not_utf8"
        );
        let current = memory
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(
            current[0].content,
            "The second note records a durable decision."
        );
        assert_eq!(current[0].sources[0].file_id, Some(source_files[1].id));
        state.jobs().complete(job.id, "mixed-worker").await.unwrap();
        for obsolete_job_type in ["memory.consolidate", "memory.enrich_retrieval"] {
            assert!(
                state
                    .jobs()
                    .find_active_by_type(&context, obsolete_job_type)
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        let incremental = state
            .jobs()
            .enqueue(
                &context,
                "memory.extract",
                "test:mixed-extraction:incremental",
                &json!({
                    "memory_contract_generation": MEMORY_CONTRACT_GENERATION,
                    "scope": "all",
                    "include_evaluated": false,
                }),
                0,
                5,
                now_millis(),
            )
            .await
            .unwrap();
        let now = now_millis();
        let mut claimed = state
            .jobs()
            .claim_batch("incremental-worker", now, now.saturating_add(60_000), 1)
            .await
            .unwrap();
        let incremental_outcome = memory_extract_job_handler(
            state.clone(),
            history_root.clone(),
            core_runtime.clone(),
            memory.clone(),
        )(claimed.remove(0), Cancellation::default())
        .await;
        assert_eq!(incremental_outcome, JobOutcome::Complete);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let incremental_progress = state
            .jobs()
            .get(&context, incremental.id)
            .await
            .unwrap()
            .unwrap()
            .progress
            .unwrap();
        assert_eq!(incremental_progress["phase"], "completed_with_errors");
        assert_eq!(incremental_progress["notes_evaluated"], 1);
        assert_eq!(incremental_progress["already_evaluated_skipped"], 1);
        assert_eq!(incremental_progress["source_ingestion_failures"], 1);
        state
            .jobs()
            .complete(incremental.id, "incremental-worker")
            .await
            .unwrap();
        let forced = state
            .jobs()
            .enqueue(
                &context,
                "memory.extract",
                "test:mixed-extraction:forced",
                &json!({
                    "memory_contract_generation": MEMORY_CONTRACT_GENERATION,
                    "scope": "all",
                    "include_evaluated": true,
                }),
                0,
                5,
                now_millis(),
            )
            .await
            .unwrap();
        let now = now_millis();
        let mut claimed = state
            .jobs()
            .claim_batch("forced-worker", now, now.saturating_add(60_000), 1)
            .await
            .unwrap();
        let forced_outcome =
            memory_extract_job_handler(state.clone(), history_root, core_runtime, memory)(
                claimed.remove(0),
                Cancellation::default(),
            )
            .await;
        assert_eq!(forced_outcome, JobOutcome::Complete);
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        let forced_progress = state
            .jobs()
            .get(&context, forced.id)
            .await
            .unwrap()
            .unwrap()
            .progress
            .unwrap();
        assert_eq!(forced_progress["notes_evaluated"], 2);
        assert_eq!(forced_progress["already_evaluated_skipped"], 0);
        provider_server.abort();
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
        memory
            .set_extraction_policy(
                &context,
                ExtractionPolicy {
                    enabled: true,
                    ..ExtractionPolicy::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let backfill = state
            .jobs()
            .enqueue(
                &context,
                "memory.extract",
                "test:memory-extract:all",
                &json!({
                    "memory_contract_generation": MEMORY_CONTRACT_GENERATION,
                    "scope": "all",
                    "reason": "test_backfill",
                }),
                0,
                5,
                now_millis(),
            )
            .await
            .unwrap();
        let supervisor = WorkerSupervisor::new(
            state.clone(),
            outbox_to_job_handler(state.clone(), memory.clone()),
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
                    mcp_vault_indexer::IndexService::new(state.clone()),
                ),
            )
            .unwrap();
        supervisor
            .register_job_handler(
                "memory.source_reconcile",
                memory_source_reconcile_job_handler(
                    state.clone(),
                    history_root.clone(),
                    core_runtime.clone(),
                    memory.clone(),
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
        let jobs = state
            .jobs()
            .list(&context, None, None, 10, 0)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 4);
        assert!(
            jobs.iter()
                .all(|job| matches!(job.status, JobStatus::Completed | JobStatus::Failed))
        );
        assert!(jobs.iter().all(|job| {
            !matches!(
                job.job_type.as_str(),
                "memory.consolidate"
                    | "memory.enrich_retrieval"
                    | "memory.reset_pipeline"
                    | "memory.revalidate"
                    | "memory.audit_sources"
                    | "memory.rebuild"
                    | "memory.repair_sources"
            )
        }));
        let backfill = jobs.iter().find(|job| job.id == backfill.id).unwrap();
        assert_eq!(backfill.status, JobStatus::Failed);
        let progress = backfill.progress.as_ref().unwrap();
        assert_eq!(progress["phase"], "failed");
        assert_eq!(progress["completed"], 0);
        assert_eq!(progress["total"], 1);
        assert_eq!(progress["current_index"], 1);
        assert_eq!(progress["current_path"], "note.md");
        assert!(progress["last_completed_path"].is_null());
        assert_eq!(progress["items_published"], 0);
        assert_eq!(progress["source_ingestion_failures"], 0);
        assert_eq!(progress["generated_output_failures"], 0);
        assert_eq!(progress["error_code"], "memory_extraction_model_unbound");
        assert!(progress["note_started_at"].is_number());
        assert!(progress["last_note_elapsed_ms"].is_number());
        assert!(progress.get("note_body").is_none());
        assert!(progress.get("prompt").is_none());
        assert!(progress.get("provider_response").is_none());
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
