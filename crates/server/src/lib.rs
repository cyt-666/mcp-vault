//! MCP Vault process composition root.
//!
//! This crate owns bootstrap configuration, listener separation, health state,
//! tracing initialization, graceful shutdown, and static Admin assets. It does
//! not own canonical Vault file behavior.

mod assets;
pub mod config;
mod initialize_memory;
pub use initialize_memory::{initialize_memory, inspect_memory_initialization};
pub mod health;
pub mod metrics;
mod router;
pub mod workers;

use std::{io, net::SocketAddr, path::Path, path::PathBuf};

use axum::Router;
use config::AppConfig;
use mcp_vault_domain::MaintenanceGate;
use thiserror::Error;
use tokio::{net::TcpListener, time::timeout};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

pub use router::{
    control_router, control_router_with_admin, data_router, data_router_with_webdav,
    data_router_with_webdav_and_mcp, data_router_with_webdav_and_mcp_and_metrics,
};

/// Errors that prevent the server from starting or serving.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Redacted offline memory cutover failure.
    #[error("memory initialization failed: {0}")]
    MemoryInitialization(&'static str),
    /// Configuration was invalid before listener binding.
    #[error("invalid configuration: {0}")]
    Configuration(#[from] config::ConfigError),
    /// A listener could not be bound.
    #[error("failed to bind {plane} listener at {address}: {source}")]
    Bind {
        plane: &'static str,
        address: std::net::SocketAddr,
        #[source]
        source: io::Error,
    },
    /// Axum stopped with a serving error.
    #[error("{plane} listener failed: {source}")]
    Serve {
        plane: &'static str,
        #[source]
        source: io::Error,
    },
    /// The global tracing subscriber could not be installed.
    #[error("failed to initialize tracing: {0}")]
    Logging(String),
    /// Operational state could not be opened, migrated, or checked.
    #[error("operational state is unavailable: {0}")]
    State(#[from] mcp_vault_state::StateError),
    /// A Vault journal could not be safely recovered before readiness.
    #[error("Vault recovery requires maintenance: {0}")]
    Recovery(#[from] mcp_vault_core::VaultError),
    /// A startup or periodic Vault reconciliation failed.
    #[error("Vault reconciliation failed: {0}")]
    Reconciliation(mcp_vault_core::VaultError),
    /// A rebuildable Markdown/index projection could not be refreshed.
    #[error("Vault index rebuild failed: {0}")]
    Index(#[from] mcp_vault_indexer::IndexError),
    /// Bootstrap authentication/secret material could not be loaded.
    #[error("authentication bootstrap is unavailable: {0}")]
    Authentication(#[from] mcp_vault_auth::AuthError),
    /// The process could not resolve its deployment root for recovery.
    #[error("deployment root is unavailable: {0}")]
    BootstrapFilesystem(String),
    /// The background worker supervisor could not be configured.
    #[error("background workers are unavailable: {0}")]
    Workers(&'static str),
}

/// Initialize the process-wide structured tracing subscriber.
pub fn init_tracing(config: &AppConfig) -> Result<(), ServerError> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if let Some(endpoint) = config.otlp_endpoint.as_deref() {
        init_otlp_tracing(config, filter, endpoint)?;
    } else {
        match config.log_format {
            config::LogFormat::Json => fmt()
                .with_env_filter(filter)
                .json()
                .with_target(true)
                .try_init()
                .map_err(|error| ServerError::Logging(error.to_string())),
            config::LogFormat::Pretty => fmt()
                .with_env_filter(filter)
                .compact()
                .with_target(true)
                .try_init()
                .map_err(|error| ServerError::Logging(error.to_string())),
        }?;
    }
    // Rust's default panic hook can print a source body when a UTF-8 slice or
    // assertion fails. Production diagnostics keep only trusted code location.
    std::panic::set_hook(Box::new(|info| {
        let location = info.location();
        tracing::error!(target:"mcp_vault::panic",event="internal_panic",source_file=location.map(|location|location.file()),source_line=location.map(|location|location.line()),"internal panic; payload omitted");
    }));
    Ok(())
}

fn init_otlp_tracing(
    config: &AppConfig,
    filter: EnvFilter,
    endpoint: &str,
) -> Result<(), ServerError> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|error| ServerError::Logging(format!("invalid OTLP exporter: {error}")))?;
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();
    let tracer = provider.tracer("mcp-vault");
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
    // Keep the provider alive for the process lifetime. Export is explicitly
    // opt-in through MCP_VAULT_OTEL_ENDPOINT and still uses redacted spans.
    let _provider: &'static SdkTracerProvider = Box::leak(Box::new(provider));
    let result = match config.log_format {
        config::LogFormat::Json => tracing_subscriber::registry()
            .with(filter)
            .with(telemetry)
            .with(tracing_subscriber::fmt::layer().json().with_target(true))
            .try_init(),
        config::LogFormat::Pretty => tracing_subscriber::registry()
            .with(filter)
            .with(telemetry)
            .with(tracing_subscriber::fmt::layer().compact().with_target(true))
            .try_init(),
    };
    result.map_err(|error| ServerError::Logging(error.to_string()))
}

/// Build both listeners, transition readiness, and serve until interrupted.
pub async fn run(config: AppConfig) -> Result<(), ServerError> {
    let config = config.validate()?;
    let _database_lock = mcp_vault_state::StateStore::acquire_process_lock(&config.database_url)?;
    let state = mcp_vault_state::StateStore::connect_and_migrate(&config.database_url).await?;
    let integrity = state.integrity_check().await?;
    if !integrity.integrity_ok || integrity.foreign_key_violations != 0 {
        return Err(ServerError::State(
            mcp_vault_state::StateError::IntegrityFailure,
        ));
    }
    let initialization_offline = mark_interrupted_memory_initializations(&state).await?;
    let maintenance = MaintenanceGate::new();
    let core_runtime = mcp_vault_core::VaultCoreRuntime::new(maintenance.clone());
    let auth_keys = load_master_key_ring(&config, &state).await?;
    remove_obsolete_managed_bootstrap_token(&config).await;
    let data_root = resolve_runtime_path(&config.data_dir)?;
    let history_root = data_root.join("history");
    let backup_root = resolve_runtime_path(&config.backup_root)?;
    let semantic_memory_service = mcp_vault_memory::SemanticMemoryService::new(state.clone());
    let semantic_organization_service =
        mcp_vault_memory::semantic::organize::SemanticOrganizationService::new(state.clone());
    recover_registered_vaults(
        &state,
        &history_root,
        &core_runtime,
        &semantic_memory_service,
        &semantic_organization_service,
    )
    .await?;

    if config.workers_enabled {
        run_initial_scans(&state, &history_root, &core_runtime).await?;
    }

    let auth_service = mcp_vault_auth::AuthService::new(state.auth(), auth_keys);
    let provider_service =
        mcp_vault_providers::ProviderService::new(state.clone(), auth_service.clone());
    let memory_service = mcp_vault_memory::MemoryService::with_provider_service(
        state.clone(),
        provider_service.clone(),
    );
    // Semantic-memory workers are intentionally independent from the legacy
    // v3 MemoryService pipeline. Their extraction handler remains a safe
    // provider-free terminal until the separately authorized Provider slice.
    if config.workers_enabled {
        for vault in state.vaults().list().await? {
            let context = vault.context()?;
            if vault.status == mcp_vault_state::VaultStatus::Active
                && !state
                    .memory_units()
                    .initialization_required(&context)
                    .await?
            {
                let core = core_for_vault(&state, &history_root, &vault, &core_runtime)?;
                let report = memory_service
                    .rebuild(&context, &core)
                    .await
                    .map_err(|error| ServerError::MemoryInitialization(error.diagnostic_code()))?;
                tracing::info!(vault_id=%context.id(),projected=report.projected,quarantined=report.quarantined,"current memory projections rebuilt");
            }
        }
    }

    let index_service = mcp_vault_indexer::IndexService::with_provider_service(
        state.clone(),
        provider_service.clone(),
    );
    if config.workers_enabled {
        for vault in state.vaults().list().await? {
            if state.vaults().availability(&vault).await?
                != mcp_vault_state::VaultAvailability::Ready
            {
                continue;
            }
            let context = vault.context()?;
            if index_service
                .schedule_note_embeddings(&context)
                .await
                .is_err()
            {
                warn!(
                    vault_id = %context.id(),
                    error_code = "note_embedding_schedule_failed",
                    "optional startup note embedding scheduling failed"
                );
            }
        }
    }
    let webdav_service = mcp_vault_webdav::WebDavService::new(
        state.clone(),
        auth_service.clone(),
        history_root.clone(),
        mcp_vault_storage_fs::StorageOptions::default(),
        core_runtime.clone(),
    );
    let mcp_service = mcp_vault_mcp::McpService::new(
        state.clone(),
        auth_service.clone(),
        history_root.clone(),
        mcp_vault_storage_fs::StorageOptions::default(),
        core_runtime.clone(),
        config.data_hosts.iter().cloned().collect(),
        config.data_origins.clone(),
    )
    .with_public_origin(config.data_public_origin.clone())
    .with_application_services(index_service.clone(), memory_service.clone());
    let readiness = health::Readiness::new();
    let metrics = metrics::Metrics::new(config.metrics_enabled);
    let key_version_ids = auth_service.key_version_ids();
    let data_router = data_router_with_webdav_and_mcp_and_metrics(
        readiness.clone(),
        webdav_service,
        mcp_service,
        metrics.clone(),
    );
    let admin_state = mcp_vault_admin_api::AdminApiState::new(
        state.clone(),
        auth_service,
        mcp_vault_admin_api::AdminApiConfig {
            origin_policy: config.admin_origins.clone(),
            data_hosts: config.data_hosts.clone(),
            data_origins: config
                .data_origins
                .allowed_origins()
                .map(str::to_owned)
                .collect(),
            data_public_origin: config.data_public_origin.clone(),
            data_bind: config.data_bind,
            admin_bind: config.admin_bind,
            data_dir: data_root,
            history_root: history_root.clone(),
            storage_options: mcp_vault_storage_fs::StorageOptions::default(),
            core_runtime: core_runtime.clone(),
            backup_root,
            backup_limits: config.backup_limits,
            key_version_ids,
            maintenance: maintenance.clone(),
            readiness: readiness.shared_flag(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    )
    .with_provider_services(provider_service, memory_service.clone());
    let admin_state_shutdown = admin_state.clone();
    let backup_service = admin_state.backup_service();
    let control_router = control_router_with_admin(admin_state)
        .layer(axum::middleware::from_fn(metrics::observe_control))
        .layer(axum::Extension(metrics.clone()));

    let data_listener = TcpListener::bind(config.data_bind)
        .await
        .map_err(|source| ServerError::Bind {
            plane: "data",
            address: config.data_bind,
            source,
        })?;
    let control_listener = TcpListener::bind(config.admin_bind)
        .await
        .map_err(|source| ServerError::Bind {
            plane: "control",
            address: config.admin_bind,
            source,
        })?;

    // Allow recovery and scans for unrelated ready Vaults to complete before
    // preserving Offline for an interrupted cleanup Vault.
    if initialization_offline {
        maintenance.set(mcp_vault_domain::MaintenanceMode::Offline);
    }
    let worker_state = state.clone();
    let worker_memory = memory_service.clone();
    let worker_backup = backup_service.clone();
    let worker_metrics = metrics.clone();
    let worker_history = history_root.clone();
    let worker_core = core_runtime.clone();
    let worker_index = index_service.clone();
    let worker_semantic_memory = semantic_memory_service.clone();
    let worker_maintenance = maintenance.clone();
    let background_tasks = start_background_tasks(
        config.workers_enabled,
        state.clone(),
        config.reconciliation_interval,
        maintenance.clone(),
        move || {
            let supervisor = workers::WorkerSupervisor::new(
                worker_state.clone(),
                workers::outbox_to_job_handler(worker_state.clone(), worker_memory.clone()),
                workers::WorkerConfig::default(),
            )
            .map_err(|failure| ServerError::Workers(failure.code))?
            .with_maintenance_gate(worker_maintenance.clone());
            supervisor
                .register_job_handler("outbox.event", workers::outbox_event_job_handler())
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "backup.create",
                    workers::backup_create_job_handler(
                        worker_backup.clone(),
                        worker_metrics.clone(),
                    ),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "backup.verify",
                    workers::backup_verify_job_handler(
                        worker_backup.clone(),
                        worker_metrics.clone(),
                    ),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "backup.restore",
                    workers::backup_restore_job_handler(
                        worker_state.clone(),
                        worker_backup.clone(),
                        worker_metrics.clone(),
                    ),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "vault.initialize",
                    workers::vault_initialize_job_handler(
                        worker_state.clone(),
                        worker_history.clone(),
                        worker_core.clone(),
                        worker_memory.clone(),
                    ),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "vault.reconcile",
                    workers::vault_reconcile_job_handler(
                        worker_state.clone(),
                        worker_history.clone(),
                        worker_core.clone(),
                    ),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "index.rebuild",
                    workers::index_rebuild_job_handler(
                        worker_state.clone(),
                        worker_history.clone(),
                        worker_core.clone(),
                        worker_index.clone(),
                    ),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "semantic.source_reconcile",
                    workers::semantic_source_reconcile_job_handler(
                        worker_state.clone(),
                        worker_history,
                        worker_core,
                        worker_semantic_memory,
                    ),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "semantic.extract",
                    workers::semantic_extract_job_handler(worker_state.clone()),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            supervisor
                .register_job_handler(
                    "embedding.rebuild",
                    workers::embedding_job_handler(worker_state, worker_index, worker_memory),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
            Ok(supervisor)
        },
    )
    .await?;

    readiness.mark_ready();
    info!(
        data_bind = %config.data_bind,
        admin_bind = %config.admin_bind,
        data_dir = %config.data_dir.display(),
        database_migration_version = integrity.migration_version,
        workers_enabled = config.workers_enabled,
        shutdown_timeout_seconds = config.shutdown_timeout.as_secs(),
        reconciliation_interval_seconds = config.reconciliation_interval.as_secs(),
        "mcp vault listeners ready"
    );

    let shutdown = workers::Cancellation::default();
    let signal_shutdown = shutdown.clone();
    let signal_background_shutdown = background_tasks
        .as_ref()
        .map(|tasks| tasks.shutdown.clone());
    let signal_readiness = readiness.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_readiness.mark_not_ready();
        if let Some(shutdown) = signal_background_shutdown {
            shutdown.cancel();
        }
        signal_shutdown.cancel();
    });

    let data_shutdown = wait_for_notification(shutdown.clone());
    let control_shutdown = wait_for_notification(shutdown);

    let data_server = async move {
        axum::serve(
            data_listener,
            data_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(data_shutdown)
        .await
        .map_err(|source| ServerError::Serve {
            plane: "data",
            source,
        })
    };
    let control_server = async move {
        axum::serve(
            control_listener,
            control_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(control_shutdown)
        .await
        .map_err(|source| ServerError::Serve {
            plane: "control",
            source,
        })
    };

    let serve_result = tokio::try_join!(data_server, control_server);
    admin_state_shutdown.shutdown_initialization().await;
    if let Some(tasks) = background_tasks {
        stop_background_tasks(tasks, config.shutdown_timeout).await;
    }

    serve_result?;

    Ok(())
}

struct BackgroundTasks {
    shutdown: workers::Cancellation,
    worker_task: tokio::task::JoinHandle<()>,
    reconciliation_task: tokio::task::JoinHandle<()>,
}

async fn start_background_tasks<F>(
    enabled: bool,
    state: mcp_vault_state::StateStore,
    reconciliation_interval: std::time::Duration,
    maintenance: MaintenanceGate,
    initialize_supervisor: F,
) -> Result<Option<BackgroundTasks>, ServerError>
where
    F: FnOnce() -> Result<workers::WorkerSupervisor, ServerError>,
{
    if !enabled {
        return Ok(None);
    }

    let supervisor = initialize_supervisor()?;
    let shutdown = workers::Cancellation::default();
    let worker_task = tokio::spawn({
        let supervisor = supervisor.clone();
        let shutdown = shutdown.clone();
        async move { supervisor.run(shutdown).await }
    });
    supervisor.wait_until_running().await;

    let reconciliation_task = tokio::spawn(run_reconciliation_loop(
        state,
        reconciliation_interval,
        maintenance,
        shutdown.clone(),
    ));

    Ok(Some(BackgroundTasks {
        shutdown,
        worker_task,
        reconciliation_task,
    }))
}

async fn stop_background_tasks(mut tasks: BackgroundTasks, shutdown_timeout: std::time::Duration) {
    tasks.shutdown.cancel();
    if timeout(shutdown_timeout, async {
        let _ = tokio::join!(&mut tasks.worker_task, &mut tasks.reconciliation_task);
    })
    .await
    .is_err()
    {
        warn!("background workers did not stop within the shutdown timeout");
        tasks.worker_task.abort();
        tasks.reconciliation_task.abort();
        let _ = tasks.worker_task.await;
        let _ = tasks.reconciliation_task.await;
    }
}

fn core_for_vault(
    state: &mcp_vault_state::StateStore,
    history_root: &Path,
    vault: &mcp_vault_state::VaultRecord,
    core_runtime: &mcp_vault_core::VaultCoreRuntime,
) -> Result<mcp_vault_core::VaultCore, mcp_vault_core::VaultError> {
    let path_policy =
        mcp_vault_domain::VaultPathPolicy::new(vault.reserved_root.clone(), Default::default())
            .map_err(mcp_vault_core::VaultError::Domain)?;
    Ok(mcp_vault_core::VaultCore::new(
        state.clone(),
        history_root.to_owned(),
        path_policy,
        mcp_vault_storage_fs::StorageOptions::default(),
        core_runtime.clone(),
    ))
}

async fn recover_registered_vaults(
    state: &mcp_vault_state::StateStore,
    history_root: &Path,
    core_runtime: &mcp_vault_core::VaultCoreRuntime,
    semantic_memory: &mcp_vault_memory::SemanticMemoryService,
    semantic_organization: &mcp_vault_memory::semantic::organize::SemanticOrganizationService,
) -> Result<(), ServerError> {
    let permit = core_runtime.maintenance_recovery_permit();
    for vault in state.vaults().list().await? {
        let context = match vault.context() {
            Ok(context) => context,
            Err(error) => {
                warn!(vault_id = %vault.id, %error, "registered Vault context is invalid");
                continue;
            }
        };
        if let Err(error) = semantic_memory
            .preflight_all_semantic_recovery(&context)
            .await
        {
            state
                .vaults()
                .set_status(&context, mcp_vault_state::VaultStatus::Error)
                .await?;
            warn!(
                vault_id = %context.id(),
                error_code = error.code(),
                "semantic publication recovery preflight requires operator review"
            );
            continue;
        }
        // The legacy v3 initialization gate is intentionally checked only
        // after the semantic barrier. Pending v3 initialization may defer
        // generic Core recovery, but it must not let a stale semantic journal
        // bypass this preflight on startup.
        if state
            .memory_units()
            .initialization_required(&context)
            .await
            .unwrap_or(false)
        {
            warn!(vault_id = %context.id(), "skipping startup journal recovery while memory initialization is pending");
            continue;
        }
        let recovery_core = match core_for_vault(state, history_root, &vault, core_runtime) {
            Ok(core) => core,
            Err(error) => {
                state
                    .vaults()
                    .set_status(&context, mcp_vault_state::VaultStatus::Error)
                    .await?;
                warn!(vault_id = %context.id(), %error, "Vault recovery configuration is invalid");
                continue;
            }
        };
        match recovery_core
            .recover_during_maintenance(&context, &permit)
            .await
        {
            Ok(report) if report.needs_review == 0 => {
                if report.rolled_back != 0 || report.finalized != 0 || report.superseded != 0 {
                    info!(
                        vault_id = %context.id(),
                        rolled_back = report.rolled_back,
                        finalized = report.finalized,
                        superseded = report.superseded,
                        "recovered Vault journal operations"
                    );
                }
                if let Err(error) = semantic_memory
                    .recover_pending_publications_after_core(&context, &recovery_core)
                    .await
                {
                    state
                        .vaults()
                        .set_status(&context, mcp_vault_state::VaultStatus::Error)
                        .await?;
                    warn!(
                        vault_id = %context.id(),
                        error_code = error.code(),
                        "semantic publication recovery requires operator review"
                    );
                    continue;
                }
                if let Err(error) = semantic_organization
                    .recover_pending_organizations_after_core(&context, &recovery_core)
                    .await
                {
                    state
                        .vaults()
                        .set_status(&context, mcp_vault_state::VaultStatus::Error)
                        .await?;
                    warn!(
                        vault_id = %context.id(),
                        error_code = error.code(),
                        "semantic organization recovery requires operator review"
                    );
                }
            }
            Ok(_) | Err(mcp_vault_core::VaultError::NeedsReview) => {
                state
                    .vaults()
                    .set_status(&context, mcp_vault_state::VaultStatus::Error)
                    .await?;
                warn!(vault_id = %context.id(), "Vault recovery requires operator review");
            }
            Err(mcp_vault_core::VaultError::State(error)) => {
                return Err(ServerError::State(error));
            }
            Err(error) => {
                state
                    .vaults()
                    .set_status(&context, mcp_vault_state::VaultStatus::Error)
                    .await?;
                warn!(vault_id = %context.id(), %error, "Vault recovery failed in isolation");
            }
        }
    }
    Ok(())
}

async fn run_initial_scans(
    state: &mcp_vault_state::StateStore,
    history_root: &Path,
    core_runtime: &mcp_vault_core::VaultCoreRuntime,
) -> Result<(), ServerError> {
    for vault in state.vaults().list().await? {
        if state.vaults().availability(&vault).await? != mcp_vault_state::VaultAvailability::Ready {
            continue;
        }
        let context = vault.context()?;
        if state
            .memory_units()
            .initialization_required(&context)
            .await?
        {
            info!(vault_id = %context.id(), "skipping startup Vault scan while memory initialization is pending");
            continue;
        }
        match reconcile_vault_once(state, history_root, &vault, "initial", core_runtime).await {
            Ok(report) => info!(
                vault_id = %context.id(),
                entries_seen = report.entries_seen,
                imported = report.imported,
                deleted = report.deleted,
                "initial Vault scan completed"
            ),
            Err(ServerError::State(error)) => return Err(ServerError::State(error)),
            Err(error) => {
                state
                    .vaults()
                    .set_status(&context, mcp_vault_state::VaultStatus::Error)
                    .await?;
                warn!(vault_id = %context.id(), %error, "initial Vault scan failed in isolation");
            }
        }
    }
    Ok(())
}

async fn mark_interrupted_memory_initializations(
    state: &mcp_vault_state::StateStore,
) -> Result<bool, ServerError> {
    let mut offline = false;
    for vault in state.vaults().list().await? {
        let context = vault.context()?;
        let initialization = state.memory_units().initialization(&context).await?;
        if initialization
            .as_ref()
            .is_some_and(|row| row.phase == "clearing")
            && state
                .memory_units()
                .initialization_task(&context)
                .await?
                .is_none()
        {
            let total_files = initialization
                .as_ref()
                .and_then(|row| row.manifest.get("files"))
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len) as u64;
            state
                .memory_units()
                .queue_initialization_task(
                    &context,
                    &mcp_vault_domain::OperationId::new().to_string(),
                    total_files,
                    false,
                    "normal",
                )
                .await?;
        }
        state
            .memory_units()
            .mark_interrupted_initialization_task(&context)
            .await?;
        if initialization
            .as_ref()
            .is_some_and(|row| row.phase == "ready")
        {
            let completed_files = initialization
                .as_ref()
                .and_then(|row| row.manifest.get("files"))
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len) as u64;
            state
                .memory_units()
                .reconcile_ready_initialization_task(&context, completed_files)
                .await?;
        }
        if initialization
            .as_ref()
            .is_some_and(|row| row.phase == "clearing")
        {
            offline = true;
        }
    }
    Ok(offline)
}

/// Reconcile one Vault and refresh its rebuildable Markdown index.
///
/// This is public so the disposable interoperability fixture can prepare a
/// realistic seed Vault through the same startup path as the production
/// composition root. It remains an application-service boundary; callers do
/// not receive raw SQL or filesystem mutation helpers.
pub async fn reconcile_vault_once(
    state: &mcp_vault_state::StateStore,
    history_root: &Path,
    vault: &mcp_vault_state::VaultRecord,
    scan_type: &str,
    core_runtime: &mcp_vault_core::VaultCoreRuntime,
) -> Result<mcp_vault_core::ReconciliationReport, ServerError> {
    let context = vault.context()?;
    let core = core_for_vault(state, history_root, vault, core_runtime)
        .map_err(ServerError::Reconciliation)?;
    let generation = mcp_vault_domain::EventId::new().to_string();
    state
        .scan_checkpoints()
        .start(&context, scan_type, &generation)
        .await?;

    let actor = mcp_vault_domain::Actor::new(mcp_vault_domain::ActorType::Reconciler, None);
    let report = match core.reconcile(&context, actor).await {
        Ok(report) => report,
        Err(error) => {
            let _ = state
                .scan_checkpoints()
                .finish(
                    &context,
                    scan_type,
                    &generation,
                    mcp_vault_state::ScanStatus::Failed,
                    Some("reconciliation_failed"),
                )
                .await;
            return Err(ServerError::Reconciliation(error));
        }
    };

    let changes_imported = report
        .imported
        .saturating_add(report.moved)
        .saturating_add(report.deleted);
    if let Err(error) = state
        .scan_checkpoints()
        .update_progress(
            &context,
            scan_type,
            &generation,
            None,
            report.entries_seen,
            report.files_seen,
            report.directories_seen,
            changes_imported,
            report.unsafe_entries_skipped,
            report.missing_deletes_skipped,
        )
        .await
    {
        let _ = state
            .scan_checkpoints()
            .finish(
                &context,
                scan_type,
                &generation,
                mcp_vault_state::ScanStatus::Failed,
                Some("checkpoint_update_failed"),
            )
            .await;
        return Err(ServerError::State(error));
    }
    state
        .scan_checkpoints()
        .finish(
            &context,
            scan_type,
            &generation,
            mcp_vault_state::ScanStatus::Completed,
            None,
        )
        .await?;
    let index = mcp_vault_indexer::IndexService::new(state.clone());
    let status = index.status(&context).await?;
    if scan_type == "initial" || changes_imported != 0 || status.is_none() {
        if scan_type == "initial" {
            let rebuilt = index.rebuild_vault(&core, &context).await?;
            tracing::info!(
                vault_id = %context.id(),
                index_revision = rebuilt.index_revision.value(),
                indexed_notes = rebuilt.indexed_notes,
                skipped_notes = rebuilt.skipped_notes,
                "Vault Markdown index refreshed"
            );
        } else {
            state
                .jobs()
                .enqueue(
                    &context,
                    "index.rebuild",
                    &format!("vault:{}:scan-index:{generation}", context.id()),
                    &serde_json::json!({
                        "scan_type": scan_type,
                        "generation": generation,
                        "changes_imported": changes_imported,
                    }),
                    0,
                    10,
                    0,
                )
                .await?;
        }
    }
    Ok(report)
}

async fn run_reconciliation_loop(
    state: mcp_vault_state::StateStore,
    interval: std::time::Duration,
    maintenance: MaintenanceGate,
    shutdown: workers::Cancellation,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Tokio's first tick is immediate: existing configured deployments are
    // admitted after recovery/worker registration, without an Admin request.
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                if !maintenance.allows_write() {
                    continue;
                }
                let mut offset = 0_u32;
                loop {
                let vaults = match state.vaults().list_page(64, offset).await {
                    Ok(vaults) => vaults,
                    Err(_) => {
                        warn!("periodic reconciliation could not list Vaults");
                        break;
                    }
                };
                let more = vaults.len() == 64;
                for vault in vaults {
                    let availability = match state.vaults().availability(&vault).await {
                        Ok(availability) => availability,
                        Err(_) => {
                            warn!(vault_id = %vault.id, "periodic reconciliation could not resolve Vault availability");
                            continue;
                        }
                    };
                    if availability != mcp_vault_state::VaultAvailability::Ready {
                        continue;
                    }
                    let vault_id = vault.id;
                    let context = match vault.context() {
                        Ok(context) => context,
                        Err(_) => {
                            warn!(vault_id = %vault_id, "periodic memory admission could not resolve Vault context");
                            continue;
                        }
                    };
                    match state.jobs().find_active_by_type(&context, "vault.reconcile").await {
                        Ok(None) => {
                            let dedup = format!(
                                "vault:{}:periodic-reconcile:{}",
                                context.id(),
                                mcp_vault_domain::EventId::new()
                            );
                            if state
                                .jobs()
                                .enqueue(
                                    &context,
                                    "vault.reconcile",
                                    &dedup,
                                    &serde_json::json!({"reason": "periodic"}),
                                    0,
                                    10,
                                    0,
                                )
                                .await
                                .is_err()
                            {
                                warn!(vault_id = %vault_id, "periodic Vault reconciliation admission failed");
                            }
                        }
                        Ok(Some(_)) => {}
                        Err(_) => warn!(vault_id = %vault_id, "periodic Vault reconciliation lookup failed"),
                    }
                }
                if !more || shutdown.is_cancelled() { break; }
                offset = offset.saturating_add(64);
                }
            }
        }
    }
}

async fn load_master_key_ring(
    config: &AppConfig,
    state: &mcp_vault_state::StateStore,
) -> Result<mcp_vault_auth::MasterKeyRing, ServerError> {
    let repository = state.auth();
    let dependency_count = repository.count_master_key_dependencies().await?;
    let check_count = repository.count_installation_key_checks().await?;
    let keys = if let Some(path) = config.master_key_file.as_deref() {
        mcp_vault_auth::MasterKeyRing::load_file(path).await?
    } else {
        let path = config.managed_master_key_file();
        let exists = tokio::fs::try_exists(&path).await.map_err(|_| {
            ServerError::Authentication(mcp_vault_auth::AuthError::MasterKeyUnavailable)
        })?;
        if !exists && (dependency_count != 0 || check_count != 0) {
            return Err(ServerError::Authentication(
                mcp_vault_auth::AuthError::MasterKeyUnavailable,
            ));
        }
        mcp_vault_auth::load_or_create_master_key(&path).await?
    };

    let version = keys.current_version();
    if let Some(stored) = repository.get_installation_key_check(version).await? {
        if !keys.matches_installation_key_check(&stored) {
            return Err(ServerError::Authentication(
                mcp_vault_auth::AuthError::MasterKeyUnavailable,
            ));
        }
    } else if check_count != 0 {
        return Err(ServerError::Authentication(
            mcp_vault_auth::AuthError::MasterKeyUnavailable,
        ));
    } else {
        repository
            .insert_installation_key_check_if_absent(version, &keys.installation_key_check())
            .await?;
        let stored = repository
            .get_installation_key_check(version)
            .await?
            .ok_or(ServerError::Authentication(
                mcp_vault_auth::AuthError::MasterKeyUnavailable,
            ))?;
        if !keys.matches_installation_key_check(&stored) {
            return Err(ServerError::Authentication(
                mcp_vault_auth::AuthError::MasterKeyUnavailable,
            ));
        }
    }
    Ok(keys)
}

fn resolve_runtime_path(path: &Path) -> Result<PathBuf, ServerError> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| ServerError::BootstrapFilesystem(error.to_string()))
}

async fn remove_obsolete_managed_bootstrap_token(config: &AppConfig) {
    let path = config.secrets_dir.join("bootstrap-token");
    if let Err(error) = tokio::fs::remove_file(&path).await
        && error.kind() != io::ErrorKind::NotFound
    {
        warn!(
            path = %path.display(),
            %error,
            "obsolete managed bootstrap-token file could not be removed"
        );
    }
}

#[cfg(test)]
async fn validate_bootstrap_material(
    config: &AppConfig,
    state: &mcp_vault_state::StateStore,
) -> Result<(), ServerError> {
    let _keys = load_master_key_ring(config, state).await?;
    Ok(())
}

async fn wait_for_notification(shutdown: workers::Cancellation) {
    shutdown.cancelled().await;
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    result=tokio::signal::ctrl_c()=>{if result.is_err(){tracing::error!(event="shutdown_signal_failed","failed to listen for shutdown signal");}}
                    _=terminate.recv()=>{}
                }
            }
            Err(_) => {
                tracing::error!(
                    event = "shutdown_signal_failed",
                    "failed to listen for termination signal"
                );
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    if tokio::signal::ctrl_c().await.is_err() {
        tracing::error!(
            event = "shutdown_signal_failed",
            "failed to listen for shutdown signal"
        );
    }
}

/// Return both routers as a useful composition smoke-test seam.
pub fn routers_for_test(readiness: health::Readiness) -> (Router, Router) {
    (data_router(readiness), control_router())
}

#[cfg(test)]
mod tests {
    #[test]
    fn panic_diagnostics_never_print_an_untrusted_payload() {
        const MARKER: &str = "MCP_VAULT_TEST_PANIC_CHILD";
        const PAYLOAD: &str = "PRIVATE_NOTE_BODY_SENTINEL_7e5024";
        if std::env::var_os(MARKER).is_some() {
            super::init_tracing(&super::AppConfig::default()).unwrap();
            panic!("{PAYLOAD}");
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::panic_diagnostics_never_print_an_untrusted_payload",
                "--nocapture",
            ])
            .env(MARKER, "1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(log.contains("internal_panic"));
        assert!(
            !log.contains(PAYLOAD),
            "panic logging must omit untrusted payloads"
        );
    }

    use super::{ServerError, start_background_tasks, stop_background_tasks};
    use crate::workers;
    use axum::{body::Body, http::Request};
    use mcp_vault_backup::{BackupConfig, BackupLimits, BackupService};
    use mcp_vault_core::{CommitPhase, FailureInjector, VaultCore, VaultCoreRuntime};
    use mcp_vault_domain::{
        Actor, CredentialId, MaintenanceGate, MaintenanceMode, Revision, SecretId, SourcePlane,
        VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug, WritePrecondition,
    };
    use mcp_vault_memory::SemanticMemoryService;
    use mcp_vault_state::{StateStore, VaultStatus};
    use mcp_vault_storage_fs::StorageOptions;
    use std::path::{Path, PathBuf};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::time::{sleep, timeout};
    use tower::ServiceExt;

    struct FailOnce {
        phase: CommitPhase,
        fired: AtomicBool,
    }

    impl FailureInjector for FailOnce {
        fn fail(&self, phase: CommitPhase) -> Result<(), &'static str> {
            if phase == self.phase && !self.fired.swap(true, Ordering::SeqCst) {
                Err("startup semantic recovery fixture fault")
            } else {
                Ok(())
            }
        }
    }

    async fn seed_pending_semantic_publication(
        state: &StateStore,
        context: &VaultContext,
        history_root: &Path,
        invalidate: bool,
    ) -> VaultCore {
        state
            .settings()
            .set_vault(
                context,
                "memory.units.policy",
                &serde_json::json!({"enabled":true,"request_timeout_seconds":300}),
                WritePrecondition::Unconditional,
                None,
            )
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            history_root.to_owned(),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            VaultCoreRuntime::default(),
        );
        let path = VaultPath::parse("notes/startup.md").unwrap();
        core.create_bytes(
            context,
            &path,
            b"# Startup\nKeep only the current source.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let service = SemanticMemoryService::new(state.clone());
        let input = service.prepare_source(context, &core, &path).await.unwrap();
        let body = input.blocks.last().unwrap();
        let proposal = serde_json::json!({
            "outcome":"success_nonempty",
            "observations":[{"kind":"decision","statement":"Keep only the current source.","scope":"project","assertion_status":"source_asserted","admission_reason":"startup recovery","value_for_future_work":"preserve source","body_block_ids":[body.local_id.clone()]}],
            "cards":[{"title":"Startup source","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
        });
        let failing_core = core.clone().with_failure_injector(Arc::new(FailOnce {
            phase: CommitPhase::RenameCommitted,
            fired: AtomicBool::new(false),
        }));
        assert!(
            service
                .submit_proposal_json(context, &failing_core, &path, &proposal.to_string())
                .await
                .is_err()
        );
        if invalidate {
            let file = core.read(context, &path).await.unwrap().file;
            state
                .semantic_memory()
                .invalidate_source(context, file.id, "source_changed", None)
                .await
                .unwrap();
        }
        core
    }

    use super::{
        config::AppConfig, load_master_key_ring, mark_interrupted_memory_initializations,
        reconcile_vault_once, recover_registered_vaults, remove_obsolete_managed_bootstrap_token,
        resolve_runtime_path, routers_for_test, run_initial_scans, validate_bootstrap_material,
    };

    #[tokio::test]
    async fn public_health_is_on_data_plane_only() {
        let (data, control) = routers_for_test(super::health::Readiness::new());

        let response = data
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let response = control
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn startup_marks_interrupted_initialization_task_resumable() {
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("interrupted").unwrap(),
            PathBuf::from("/srv/interrupted"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Interrupted", VaultStatus::Active)
            .await
            .unwrap();
        state
            .memory_units()
            .queue_initialization_task(&context, "task-startup", 1, false, "normal")
            .await
            .unwrap();
        state
            .memory_units()
            .start_initialization_task(&context, "task-startup")
            .await
            .unwrap();
        assert!(
            !mark_interrupted_memory_initializations(&state)
                .await
                .unwrap()
        );
        let task = state
            .memory_units()
            .initialization_task(&context)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task.state, "failed");
        assert_eq!(
            task.error_code.as_deref(),
            Some("initialization_interrupted")
        );
        assert!(task.resumable);
    }

    #[test]
    fn default_configuration_validates_without_filesystem_access() {
        assert!(AppConfig::default().validate().is_ok());
    }

    #[tokio::test]
    async fn worker_flag_gates_supervisor_initialization_and_job_consumption() {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        state
            .jobs()
            .enqueue_global(
                "worker-gate.test",
                "worker-gate:test-job",
                &serde_json::json!({}),
                0,
                3,
                0,
            )
            .await
            .unwrap();
        let initialized = Arc::new(AtomicBool::new(false));
        let initialize_flag = initialized.clone();
        let disabled_state = state.clone();
        let disabled = start_background_tasks(
            false,
            state.clone(),
            Duration::from_secs(3600),
            MaintenanceGate::new(),
            move || {
                initialize_flag.store(true, Ordering::SeqCst);
                let supervisor = workers::WorkerSupervisor::new(
                    disabled_state.clone(),
                    Arc::new(|_| Box::pin(async { Ok(()) })),
                    workers::WorkerConfig::default(),
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
                Ok(supervisor)
            },
        )
        .await
        .unwrap();
        assert!(disabled.is_none());
        assert!(!initialized.load(Ordering::SeqCst));
        assert_eq!(state.jobs().pending_count().await.unwrap(), 1);

        let default_config = AppConfig::default();
        assert!(default_config.workers_enabled);
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let calls = handler_calls.clone();
        let enabled_state = state.clone();
        let enabled_initialized = initialized.clone();
        let tasks = start_background_tasks(
            default_config.workers_enabled,
            state.clone(),
            Duration::from_secs(3600),
            MaintenanceGate::new(),
            move || {
                enabled_initialized.store(true, Ordering::SeqCst);
                let supervisor = workers::WorkerSupervisor::new(
                    enabled_state.clone(),
                    Arc::new(|_| Box::pin(async { Ok(()) })),
                    workers::WorkerConfig {
                        poll_interval: Duration::from_millis(5),
                        lease_duration: Duration::from_millis(100),
                        ..workers::WorkerConfig::default()
                    },
                )
                .map_err(|failure| ServerError::Workers(failure.code))?;
                supervisor
                    .register_job_handler(
                        "worker-gate.test",
                        Arc::new(move |_, _| {
                            let calls = calls.clone();
                            Box::pin(async move {
                                calls.fetch_add(1, Ordering::SeqCst);
                                workers::JobOutcome::Complete
                            })
                        }),
                    )
                    .map_err(|failure| ServerError::Workers(failure.code))?;
                Ok(supervisor)
            },
        )
        .await
        .unwrap()
        .unwrap();
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
        assert!(initialized.load(Ordering::SeqCst));
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);
        stop_background_tasks(tasks, Duration::from_secs(1)).await;
    }

    #[test]
    fn relative_default_data_root_becomes_an_absolute_vault_context() {
        let root = resolve_runtime_path(Path::new("./data")).unwrap();
        assert!(root.is_absolute());
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("default").unwrap(),
            root.join("vaults/default"),
            Revision::ZERO,
        )
        .unwrap();
        assert!(context.content_root().is_absolute());
    }

    #[tokio::test]
    async fn startup_removes_only_the_obsolete_managed_token_path() {
        let directory = tempfile::tempdir().unwrap();
        let obsolete = directory.path().join("bootstrap-token");
        let unrelated = directory.path().join("master-key");
        tokio::fs::write(&obsolete, b"obsolete").await.unwrap();
        tokio::fs::write(&unrelated, b"keep").await.unwrap();
        let config = AppConfig {
            secrets_dir: directory.path().to_owned(),
            ..AppConfig::default()
        };

        remove_obsolete_managed_bootstrap_token(&config).await;

        assert!(!tokio::fs::try_exists(obsolete).await.unwrap());
        assert!(tokio::fs::try_exists(unrelated).await.unwrap());
    }

    #[tokio::test]
    async fn encrypted_state_without_master_key_blocks_bootstrap() {
        let directory = tempfile::tempdir().unwrap();
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        state
            .auth()
            .insert_secret(
                SecretId::new(),
                "provider",
                "system",
                None,
                1,
                &[0; 24],
                b"ciphertext",
                Some("masked"),
            )
            .await
            .unwrap();
        let config = AppConfig::from_lookup(|key| match key {
            "MCP_VAULT_DATABASE_URL" => Some("sqlite::memory:".to_owned()),
            "MCP_VAULT_DATA_DIR" => Some(directory.path().display().to_string()),
            _ => None,
        })
        .unwrap();
        let error = validate_bootstrap_material(&config, &state)
            .await
            .unwrap_err();
        assert!(matches!(error, super::ServerError::Authentication(_)));
    }

    #[tokio::test]
    async fn pat_state_without_master_key_blocks_bootstrap() {
        let directory = tempfile::tempdir().unwrap();
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("pat-only").unwrap(),
            "/srv/pat-only".into(),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "PAT only", VaultStatus::Active)
            .await
            .unwrap();
        state
            .auth()
            .insert_mcp_token(
                &context,
                CredentialId::new(),
                "agent",
                "mcpv_pat_test",
                &[7_u8; 32],
                1,
                r#"["vault:read"]"#,
                None,
            )
            .await
            .unwrap();

        let config = AppConfig {
            data_dir: directory.path().to_owned(),
            secrets_dir: directory.path().join("secrets"),
            ..AppConfig::default()
        };
        let error = load_master_key_ring(&config, &state).await.unwrap_err();
        assert!(matches!(error, super::ServerError::Authentication(_)));
        assert!(!config.managed_master_key_file().exists());
    }

    #[tokio::test]
    async fn persisted_key_verifier_rejects_a_different_master_key() {
        let directory = tempfile::tempdir().unwrap();
        let key_path = directory.path().join("master-key");
        tokio::fs::write(&key_path, [1_u8; 32]).await.unwrap();
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let config = AppConfig {
            master_key_file: Some(key_path.clone()),
            ..AppConfig::default()
        };
        load_master_key_ring(&config, &state).await.unwrap();

        tokio::fs::write(&key_path, [2_u8; 32]).await.unwrap();
        let error = load_master_key_ring(&config, &state).await.unwrap_err();
        assert!(matches!(error, super::ServerError::Authentication(_)));
    }

    #[tokio::test]
    async fn managed_master_key_is_created_once_reused_and_never_replaced_after_loss() {
        let directory = tempfile::tempdir().unwrap();
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let config = AppConfig {
            data_dir: directory.path().to_owned(),
            secrets_dir: directory.path().join("secrets"),
            ..AppConfig::default()
        };

        let first = load_master_key_ring(&config, &state).await.unwrap();
        let second = load_master_key_ring(&config, &state).await.unwrap();
        assert!(first.is_persistent());
        assert_eq!(
            first.installation_key_check(),
            second.installation_key_check()
        );
        let key_path = config.managed_master_key_file();
        assert!(tokio::fs::try_exists(&key_path).await.unwrap());

        tokio::fs::remove_file(&key_path).await.unwrap();
        let error = load_master_key_ring(&config, &state).await.unwrap_err();
        assert!(matches!(error, super::ServerError::Authentication(_)));
        assert!(!tokio::fs::try_exists(&key_path).await.unwrap());
    }

    #[tokio::test]
    async fn fresh_install_provisions_only_the_managed_master_key() {
        let directory = tempfile::tempdir().unwrap();
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let config = AppConfig {
            data_dir: directory.path().to_owned(),
            secrets_dir: directory.path().join("secrets"),
            ..AppConfig::default()
        };
        validate_bootstrap_material(&config, &state).await.unwrap();
        assert!(
            tokio::fs::try_exists(config.managed_master_key_file())
                .await
                .unwrap()
        );
        let mut entries = tokio::fs::read_dir(&config.secrets_dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name());
        }
        assert_eq!(names, [std::ffi::OsString::from("master-key")]);

        let explicit_directory = tempfile::tempdir().unwrap();
        let explicit_state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let explicit_path = explicit_directory.path().join("missing-master-key");
        let explicit = AppConfig {
            data_dir: explicit_directory.path().to_owned(),
            secrets_dir: explicit_directory.path().join("secrets"),
            master_key_file: Some(explicit_path.clone()),
            ..AppConfig::default()
        };
        assert!(
            load_master_key_ring(&explicit, &explicit_state)
                .await
                .is_err()
        );
        assert!(!tokio::fs::try_exists(&explicit_path).await.unwrap());
        assert!(
            !tokio::fs::try_exists(explicit.managed_master_key_file())
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn initial_scan_imports_external_files_and_completes_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let content_root = directory.path().join("content");
        std::fs::create_dir_all(&content_root).unwrap();
        std::fs::write(content_root.join("outside.md"), b"created before startup").unwrap();
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("startup-test").unwrap(),
            PathBuf::from(&content_root),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Startup test", VaultStatus::Active)
            .await
            .unwrap();

        run_initial_scans(
            &state,
            &directory.path().join("history"),
            &Default::default(),
        )
        .await
        .unwrap();

        let checkpoint = state
            .scan_checkpoints()
            .get(&context, "initial")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.status, mcp_vault_state::ScanStatus::Completed);
        assert_eq!(checkpoint.files_seen, 1);
        assert_eq!(checkpoint.changes_imported, 1);
        assert_eq!(checkpoint.unsafe_entries_skipped, 0);
        assert!(!checkpoint.missing_deletes_skipped);
        assert!(
            state
                .files()
                .get_active(&context, &"outside.md".parse().unwrap())
                .await
                .unwrap()
                .is_some()
        );
        let first_index = state.index().status(&context).await.unwrap().unwrap();
        assert_eq!(first_index.indexed_notes, 1);

        std::fs::write(
            content_root.join("outside.md"),
            b"edited outside startup WebDAV conflict",
        )
        .unwrap();
        let vault = state
            .vaults()
            .find_by_id(context.id())
            .await
            .unwrap()
            .unwrap();
        let core_runtime = mcp_vault_core::VaultCoreRuntime::default();
        reconcile_vault_once(
            &state,
            &directory.path().join("history"),
            &vault,
            "reconciliation",
            &core_runtime,
        )
        .await
        .unwrap();
        let index_job = state
            .jobs()
            .list(&context, None, None, 20, 0)
            .await
            .unwrap()
            .into_iter()
            .find(|job| job.job_type == "index.rebuild")
            .unwrap();
        let outcome = (workers::index_rebuild_job_handler(
            state.clone(),
            directory.path().join("history"),
            core_runtime,
            mcp_vault_indexer::IndexService::new(state.clone()),
        ))(index_job, workers::Cancellation::default())
        .await;
        assert_eq!(outcome, workers::JobOutcome::Complete);
        let second_index = state.index().status(&context).await.unwrap().unwrap();
        assert!(second_index.index_revision > first_index.index_revision);
    }

    #[tokio::test]
    async fn disabled_vault_does_not_block_startup_recovery_for_other_vaults() {
        let root = tempfile::tempdir().unwrap();
        let state = mcp_vault_state::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let active = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("active").unwrap(),
            root.path().join("active"),
            Revision::ZERO,
        )
        .unwrap();
        let disabled = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("disabled").unwrap(),
            root.path().join("disabled"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&active, "Active", VaultStatus::Active)
            .await
            .unwrap();
        state
            .vaults()
            .insert(&disabled, "Disabled", VaultStatus::Disabled)
            .await
            .unwrap();

        recover_registered_vaults(
            &state,
            &root.path().join("history"),
            &mcp_vault_core::VaultCoreRuntime::default(),
            &mcp_vault_memory::SemanticMemoryService::new(state.clone()),
            &mcp_vault_memory::semantic::organize::SemanticOrganizationService::new(state.clone()),
        )
        .await
        .unwrap();

        assert_eq!(
            state
                .vaults()
                .find_by_id(active.id())
                .await
                .unwrap()
                .unwrap()
                .status,
            VaultStatus::Active
        );
        assert_eq!(
            state
                .vaults()
                .find_by_id(disabled.id())
                .await
                .unwrap()
                .unwrap()
                .status,
            VaultStatus::Disabled
        );
    }

    #[tokio::test]
    async fn startup_semantic_preflight_blocks_stale_core_recovery_but_recovers_other_vaults() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("startup-semantic.sqlite3");
        let database_url = format!("sqlite://{}", database.display());
        let state = StateStore::connect_and_migrate(&database_url)
            .await
            .unwrap();
        let stale = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("startup-stale-semantic").unwrap(),
            root.path().join("stale"),
            Revision::ZERO,
        )
        .unwrap();
        let ready = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("startup-ready-semantic").unwrap(),
            root.path().join("ready"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&stale, "Startup stale", VaultStatus::Active)
            .await
            .unwrap();
        state
            .vaults()
            .insert(&ready, "Startup ready", VaultStatus::Active)
            .await
            .unwrap();
        let stale_core =
            seed_pending_semantic_publication(&state, &stale, &root.path().join("history"), true)
                .await;
        let ready_core =
            seed_pending_semantic_publication(&state, &ready, &root.path().join("history"), false)
                .await;

        recover_registered_vaults(
            &state,
            &root.path().join("history"),
            &VaultCoreRuntime::default(),
            &SemanticMemoryService::new(state.clone()),
            &mcp_vault_memory::semantic::organize::SemanticOrganizationService::new(state.clone()),
        )
        .await
        .unwrap();

        // Reopen a file-backed StateStore to model a real second process
        // startup. The barrier must come from durable rows, not this pool.
        drop(stale_core);
        drop(ready_core);
        state.close().await;
        let reopened = StateStore::connect_and_migrate(&database_url)
            .await
            .unwrap();

        // A blocked semantic snapshot still owns an unproven Core journal.
        // Re-running startup recovery must keep the barrier in place rather
        // than letting the generic Core pass finalize the RenameCommitted
        // operation on the second boot.
        recover_registered_vaults(
            &reopened,
            &root.path().join("history"),
            &VaultCoreRuntime::default(),
            &SemanticMemoryService::new(reopened.clone()),
            &mcp_vault_memory::semantic::organize::SemanticOrganizationService::new(
                reopened.clone(),
            ),
        )
        .await
        .unwrap();

        assert_eq!(
            reopened
                .vaults()
                .find_by_id(stale.id())
                .await
                .unwrap()
                .unwrap()
                .status,
            VaultStatus::Error
        );
        assert_eq!(
            reopened
                .vaults()
                .find_by_id(ready.id())
                .await
                .unwrap()
                .unwrap()
                .status,
            VaultStatus::Active
        );
        assert_eq!(
            reopened
                .files()
                .list_incomplete(&stale)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            reopened
                .semantic_memory()
                .list_cards(&stale, 20)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !reopened
                .files()
                .list_active_entries(&stale)
                .await
                .unwrap()
                .iter()
                .any(|file| file.path.as_str().starts_with("_mcp-vault/")),
            "stale semantic recovery must not publish a managed card file"
        );
        assert!(
            reopened
                .files()
                .list_incomplete(&ready)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            SemanticMemoryService::new(reopened.clone())
                .list_cards(
                    &ready,
                    &VaultCore::new(
                        reopened.clone(),
                        root.path().join("history"),
                        VaultPathPolicy::default(),
                        StorageOptions::default(),
                        VaultCoreRuntime::default(),
                    ),
                    20,
                )
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn backup_maintenance_recovery_preflights_stale_semantic_journal() {
        let root = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("backup-semantic-barrier").unwrap(),
            root.path().join("backup-barrier"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Backup semantic barrier", VaultStatus::Active)
            .await
            .unwrap();
        let core =
            seed_pending_semantic_publication(&state, &context, &root.path().join("history"), true)
                .await;
        let gate = MaintenanceGate::new();
        gate.set(MaintenanceMode::Offline);
        let readiness = Arc::new(AtomicBool::new(false));
        let service = BackupService::new(
            state.clone(),
            BackupConfig {
                backup_root: root.path().join("backups"),
                history_root: root.path().join("history"),
                storage_options: StorageOptions::default(),
                limits: BackupLimits::default(),
                service_version: "test".to_owned(),
                key_version_ids: vec![1],
                maintenance: gate.clone(),
                core_runtime: VaultCoreRuntime::new(gate.clone()),
                readiness: readiness.clone(),
            },
        );
        assert!(service.recover_maintenance().await.is_err());
        assert_eq!(gate.mode(), MaintenanceMode::Offline);
        assert!(!readiness.load(Ordering::Acquire));
        assert_eq!(
            state.files().list_incomplete(&context).await.unwrap().len(),
            1
        );
        drop(core);
    }
}
