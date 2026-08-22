//! MCP Vault process composition root.
//!
//! This crate owns bootstrap configuration, listener separation, health state,
//! tracing initialization, graceful shutdown, and static Admin assets. It does
//! not own canonical Vault file behavior.

mod assets;
pub mod config;
pub mod health;
pub mod metrics;
mod router;
pub mod workers;

use std::{io, net::SocketAddr, path::Path, path::PathBuf, sync::Arc};

use axum::Router;
use config::AppConfig;
use mcp_vault_domain::MaintenanceGate;
use thiserror::Error;
use tokio::{net::TcpListener, sync::Notify, time::timeout};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

pub use router::{
    control_router, control_router_with_admin, data_router, data_router_with_webdav,
    data_router_with_webdav_and_mcp, data_router_with_webdav_and_mcp_and_metrics,
};

/// Errors that prevent the server from starting or serving.
#[derive(Debug, Error)]
pub enum ServerError {
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
        return init_otlp_tracing(config, filter, endpoint);
    }

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
    }
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
    let state = mcp_vault_state::StateStore::connect_and_migrate(&config.database_url).await?;
    let integrity = state.integrity_check().await?;
    if !integrity.integrity_ok || integrity.foreign_key_violations != 0 {
        return Err(ServerError::State(
            mcp_vault_state::StateError::IntegrityFailure,
        ));
    }
    let maintenance = MaintenanceGate::new();
    let core_runtime = mcp_vault_core::VaultCoreRuntime::new(maintenance.clone());
    let auth_keys = load_master_key_ring(&config, &state).await?;
    remove_obsolete_managed_bootstrap_token(&config).await;
    let data_root = resolve_runtime_path(&config.data_dir)?;
    let history_root = data_root.join("history");
    let backup_root = resolve_runtime_path(&config.backup_root)?;
    for vault in state.vaults().list().await? {
        let context = vault.context()?;
        let recovery_core = core_for_vault(&state, &history_root, &vault, &core_runtime)
            .map_err(ServerError::Recovery)?;
        let report = recovery_core.recover(&context).await?;
        if report.needs_review != 0 {
            return Err(ServerError::Recovery(
                mcp_vault_core::VaultError::NeedsReview,
            ));
        }
        if report.rolled_back != 0 || report.finalized != 0 {
            info!(
                vault_id = %context.id(),
                rolled_back = report.rolled_back,
                finalized = report.finalized,
                "recovered Vault journal operations"
            );
        }
    }

    run_initial_scans(&state, &history_root, &core_runtime).await?;

    let auth_service = mcp_vault_auth::AuthService::new(state.auth(), auth_keys);
    let provider_service =
        mcp_vault_providers::ProviderService::new(state.clone(), auth_service.clone());
    let memory_service = mcp_vault_memory::MemoryService::with_provider_service(
        state.clone(),
        provider_service.clone(),
    );
    let webdav_service = mcp_vault_webdav::WebDavService::new(
        state.clone(),
        auth_service.clone(),
        history_root.clone(),
        mcp_vault_storage_fs::StorageOptions::default(),
        core_runtime.clone(),
        config.trusted_proxy_ips.clone(),
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
    .with_memory_service(memory_service.clone());
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

    let supervisor = workers::WorkerSupervisor::new(
        state.clone(),
        workers::outbox_to_job_handler(state.clone()),
        workers::WorkerConfig::default(),
    )
    .map_err(|failure| ServerError::Workers(failure.code))?
    .with_maintenance_gate(maintenance.clone());
    supervisor
        .register_job_handler("outbox.event", workers::outbox_event_job_handler())
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "backup.create",
            workers::backup_create_job_handler(backup_service.clone(), metrics.clone()),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "backup.verify",
            workers::backup_verify_job_handler(backup_service.clone(), metrics.clone()),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "backup.restore",
            workers::backup_restore_job_handler(backup_service, metrics.clone()),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "vault.reconcile",
            workers::vault_reconcile_job_handler(
                state.clone(),
                history_root.clone(),
                core_runtime.clone(),
            ),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "index.rebuild",
            workers::index_rebuild_job_handler(
                state.clone(),
                history_root.clone(),
                core_runtime.clone(),
            ),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "memory.extract",
            workers::memory_extract_job_handler(
                state.clone(),
                history_root.clone(),
                core_runtime.clone(),
                memory_service.clone(),
            ),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "memory.revalidate",
            workers::memory_revalidate_job_handler(
                state.clone(),
                history_root.clone(),
                core_runtime.clone(),
                memory_service.clone(),
            ),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "memory.rebuild",
            workers::memory_rebuild_job_handler(
                state.clone(),
                history_root.clone(),
                core_runtime.clone(),
                memory_service.clone(),
            ),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    supervisor
        .register_job_handler(
            "embedding.rebuild",
            workers::memory_embedding_job_handler(state.clone(), memory_service),
        )
        .map_err(|failure| ServerError::Workers(failure.code))?;
    let worker_shutdown = workers::Cancellation::default();
    let worker_task = tokio::spawn({
        let supervisor = supervisor.clone();
        let shutdown = worker_shutdown.clone();
        async move { supervisor.run(shutdown).await }
    });
    supervisor.wait_until_running().await;

    let reconciliation_shutdown = workers::Cancellation::default();
    let reconciliation_task = tokio::spawn(run_reconciliation_loop(
        state.clone(),
        history_root.clone(),
        config.reconciliation_interval,
        maintenance.clone(),
        core_runtime.clone(),
        reconciliation_shutdown.clone(),
    ));

    readiness.mark_ready();
    info!(
        data_bind = %config.data_bind,
        admin_bind = %config.admin_bind,
        data_dir = %config.data_dir.display(),
        database_migration_version = integrity.migration_version,
        shutdown_timeout_seconds = config.shutdown_timeout.as_secs(),
        reconciliation_interval_seconds = config.reconciliation_interval.as_secs(),
        "mcp vault listeners ready"
    );

    let shutdown = Arc::new(Notify::new());
    let signal_shutdown = Arc::clone(&shutdown);
    let signal_worker_shutdown = worker_shutdown.clone();
    let signal_reconciliation_shutdown = reconciliation_shutdown.clone();
    let signal_readiness = readiness.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_readiness.mark_not_ready();
        signal_worker_shutdown.cancel();
        signal_reconciliation_shutdown.cancel();
        signal_shutdown.notify_waiters();
    });

    let data_shutdown = wait_for_notification(Arc::clone(&shutdown));
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
    worker_shutdown.cancel();
    reconciliation_shutdown.cancel();

    let mut worker_task = worker_task;
    let mut reconciliation_task = reconciliation_task;
    if timeout(config.shutdown_timeout, async {
        let _ = tokio::join!(&mut worker_task, &mut reconciliation_task);
    })
    .await
    .is_err()
    {
        warn!("background workers did not stop within the shutdown timeout");
        worker_task.abort();
        reconciliation_task.abort();
        let _ = worker_task.await;
        let _ = reconciliation_task.await;
    }

    serve_result?;

    Ok(())
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

async fn run_initial_scans(
    state: &mcp_vault_state::StateStore,
    history_root: &Path,
    core_runtime: &mcp_vault_core::VaultCoreRuntime,
) -> Result<(), ServerError> {
    for vault in state.vaults().list().await? {
        if vault.status != mcp_vault_state::VaultStatus::Active {
            continue;
        }
        let context = vault.context()?;
        let report =
            reconcile_vault_once(state, history_root, &vault, "initial", core_runtime).await?;
        info!(
            vault_id = %context.id(),
            entries_seen = report.entries_seen,
            imported = report.imported,
            deleted = report.deleted,
            "initial Vault scan completed"
        );
    }
    Ok(())
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
    history_root: PathBuf,
    interval: std::time::Duration,
    maintenance: MaintenanceGate,
    core_runtime: mcp_vault_core::VaultCoreRuntime,
    shutdown: workers::Cancellation,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                if !maintenance.allows_write() {
                    continue;
                }
                let vaults = match state.vaults().list().await {
                    Ok(vaults) => vaults,
                    Err(_) => {
                        warn!("periodic reconciliation could not list Vaults");
                        continue;
                    }
                };
                for vault in vaults {
                    if vault.status != mcp_vault_state::VaultStatus::Active {
                        continue;
                    }
                    let Some(_write_operation) = maintenance.try_start_write() else {
                        break;
                    };
                    let vault_id = vault.id;
                    match reconcile_vault_once(&state, &history_root, &vault, "reconciliation", &core_runtime).await {
                        Ok(report) => info!(
                            vault_id = %vault_id,
                            entries_seen = report.entries_seen,
                            imported = report.imported,
                            deleted = report.deleted,
                            "periodic Vault reconciliation completed"
                        ),
                        Err(_) => warn!(vault_id = %vault_id, "periodic Vault reconciliation failed"),
                    }
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

async fn wait_for_notification(notify: Arc<Notify>) {
    notify.notified().await;
}

async fn wait_for_shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for shutdown signal");
    }
}

/// Return both routers as a useful composition smoke-test seam.
pub fn routers_for_test(readiness: health::Readiness) -> (Router, Router) {
    (data_router(readiness), control_router())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::workers;
    use axum::{body::Body, http::Request};
    use mcp_vault_domain::{CredentialId, Revision, SecretId, VaultContext, VaultId, VaultSlug};
    use mcp_vault_state::VaultStatus;
    use tower::ServiceExt;

    use super::{
        config::AppConfig, load_master_key_ring, reconcile_vault_once,
        remove_obsolete_managed_bootstrap_token, resolve_runtime_path, routers_for_test,
        run_initial_scans, validate_bootstrap_material,
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

    #[test]
    fn default_configuration_validates_without_filesystem_access() {
        assert!(AppConfig::default().validate().is_ok());
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
            .list(&context, None, 20, 0)
            .await
            .unwrap()
            .into_iter()
            .find(|job| job.job_type == "index.rebuild")
            .unwrap();
        let outcome = (workers::index_rebuild_job_handler(
            state.clone(),
            directory.path().join("history"),
            core_runtime,
        ))(index_job, workers::Cancellation::default())
        .await;
        assert_eq!(outcome, workers::JobOutcome::Complete);
        let second_index = state.index().status(&context).await.unwrap().unwrap();
        assert!(second_index.index_revision > first_index.index_revision);
    }
}
