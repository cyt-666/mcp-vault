//! Authenticated control-plane Admin HTTP adapter.
//!
//! This crate owns HTTP translation and browser security middleware only.
//! State repositories and application services remain the owners of SQL,
//! canonical file operations, provider calls, and memory/index policy.

#![allow(clippy::result_large_err)]

use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Extension, Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, delete, get, patch, post, put},
};
use mcp_vault_auth::{
    AdminPrincipal, AuthError, AuthService, OAuthIssuerInput, OriginPolicy, SecretString,
    clear_csrf_cookie_header, clear_session_cookie_header, csrf_cookie_header,
    parse_session_cookie, session_cookie_header,
};
use mcp_vault_backup::{BackupError, BackupLimits, BackupService};
use mcp_vault_core::{ManagedVaultService, VaultCore, VaultCoreRuntime, VaultError};
use mcp_vault_domain::{
    Actor, ActorId, ActorType, BackupId, DomainError, MaintenanceGate, MaintenanceMode, MemoryId,
    Permission, PermissionSet, Revision, Scope, ScopeSet, SourcePlane, VaultContext, VaultId,
    VaultPathPolicy, VaultSlug,
};
use mcp_vault_indexer::IndexService;
use mcp_vault_memory::{
    ExtractionPolicy, ExtractionSourceMode, MemoryService, MemoryStatus, MemoryType,
    MemoryUpdateInput, PipelineRegenerationAdmission,
};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderError, ProviderInput, ProviderKind,
    ProviderMode, ProviderService, ProviderSettings,
};
use mcp_vault_state::{
    AuditRecord, BackupRecord, JobRecord, JobStatus, JobStatusCounts, McpTokenRecord, MemoryCounts,
    MemorySourceAuditStateRecord, MemorySourceHealthCounts, MemorySourceHealthDetailRecord,
    MemorySourceHealthState, ModelBindingRecord, ModelRecord, ProviderHealthRecord, ProviderRecord,
    StateError, StateStore, VaultRecord, VaultStatus, WebDavCredentialRecord,
};
use mcp_vault_storage_fs::StorageOptions;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use url::Url;

const SESSION_MAX_AGE: Duration = Duration::from_secs(12 * 60 * 60);
const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 200;

/// State and policy dependencies for one Admin listener.
#[derive(Clone)]
pub struct AdminApiState {
    state: StateStore,
    auth: AuthService,
    origin_policy: OriginPolicy,
    data_hosts: BTreeSet<String>,
    data_origins: Vec<String>,
    data_public_origin: Option<String>,
    data_bind: SocketAddr,
    admin_bind: SocketAddr,
    data_dir: PathBuf,
    history_root: PathBuf,
    storage_options: StorageOptions,
    core_runtime: VaultCoreRuntime,
    managed_vaults: ManagedVaultService,
    selected_vault: Option<VaultId>,
    providers: ProviderService,
    memory: MemoryService,
    backup: BackupService,
    readiness: Arc<AtomicBool>,
    version: String,
}

/// Bootstrap/runtime inputs used to compose the Admin adapter.
#[derive(Clone)]
pub struct AdminApiConfig {
    /// Exact browser Origin policy for state-changing Admin requests.
    pub origin_policy: OriginPolicy,
    /// Exact data-plane Host authorities.
    pub data_hosts: BTreeSet<String>,
    /// Configured data-plane browser origins.
    pub data_origins: Vec<String>,
    /// Canonical external data-plane origin shown in connection cards.
    pub data_public_origin: Option<String>,
    /// Data listener address shown in diagnostics.
    pub data_bind: SocketAddr,
    /// Admin listener address shown in diagnostics.
    pub admin_bind: SocketAddr,
    /// Bootstrap/application data root.
    pub data_dir: PathBuf,
    /// Content-addressed history root.
    pub history_root: PathBuf,
    /// Storage safety options used by per-Vault Core factories.
    pub storage_options: StorageOptions,
    /// Shared Core path-lock and maintenance runtime.
    pub core_runtime: VaultCoreRuntime,
    /// Shared readiness bit from the server composition root.
    pub readiness: Arc<AtomicBool>,
    /// Service version displayed by System and Dashboard.
    pub version: String,
    /// Service-owned backup artifact directory.
    pub backup_root: PathBuf,
    /// Backup/archive resource bounds.
    pub backup_limits: BackupLimits,
    /// Retained installation-key version identifiers, never key material.
    pub key_version_ids: Vec<u32>,
    /// Shared process maintenance coordinator.
    pub maintenance: MaintenanceGate,
}

impl AdminApiState {
    /// Build an Admin state boundary from already initialized services.
    pub fn new(state: StateStore, auth: AuthService, config: AdminApiConfig) -> Self {
        let managed_vaults = ManagedVaultService::new(
            state.clone(),
            config.data_dir.clone(),
            config.storage_options,
        );
        let providers = ProviderService::new(state.clone(), auth.clone());
        let memory = MemoryService::with_provider_service(state.clone(), providers.clone());
        let backup = BackupService::new(
            state.clone(),
            mcp_vault_backup::BackupConfig {
                backup_root: config.backup_root,
                history_root: config.history_root.clone(),
                storage_options: config.storage_options,
                limits: config.backup_limits,
                service_version: config.version.clone(),
                key_version_ids: config.key_version_ids,
                maintenance: config.maintenance.clone(),
                core_runtime: config.core_runtime.clone(),
                readiness: config.readiness.clone(),
            },
        );
        Self {
            state,
            auth,
            origin_policy: config.origin_policy,
            data_hosts: config.data_hosts,
            data_origins: config.data_origins,
            data_public_origin: config.data_public_origin,
            data_bind: config.data_bind,
            admin_bind: config.admin_bind,
            data_dir: config.data_dir,
            history_root: config.history_root,
            storage_options: config.storage_options,
            core_runtime: config.core_runtime,
            managed_vaults,
            selected_vault: None,
            providers,
            memory,
            backup,
            readiness: config.readiness,
            version: config.version,
        }
    }

    fn index(&self) -> IndexService {
        IndexService::with_provider_service(self.state.clone(), self.providers.clone())
    }

    fn memory(&self) -> MemoryService {
        self.memory.clone()
    }

    fn providers(&self) -> ProviderService {
        self.providers.clone()
    }

    fn with_selected_vault(&self, vault_id: VaultId) -> Self {
        let mut selected = self.clone();
        selected.selected_vault = Some(vault_id);
        selected
    }

    /// Inject process-shared provider and memory services from the composition
    /// root so Admin, MCP, and workers share transport concurrency gates.
    pub fn with_provider_services(
        mut self,
        providers: ProviderService,
        memory: MemoryService,
    ) -> Self {
        self.providers = providers;
        self.memory = memory;
        self
    }

    /// Share the composed backup application service with the worker
    /// supervisor without exposing raw state or archive paths.
    pub fn backup_service(&self) -> BackupService {
        self.backup.clone()
    }

    fn core_for_vault(&self, vault: &VaultRecord) -> Result<VaultCore, StateError> {
        let policy = VaultPathPolicy::new(vault.reserved_root.clone(), Default::default())?;
        Ok(VaultCore::new(
            self.state.clone(),
            self.history_root.clone(),
            policy,
            self.storage_options,
            self.core_runtime.clone(),
        ))
    }

    // These methods are the Admin application boundary for cross-module
    // control-plane composition. HTTP handlers below do not receive a raw
    // SQL pool and therefore cannot accidentally grow protocol-owned queries.
    async fn list_vaults(&self) -> Result<Vec<VaultRecord>, StateError> {
        self.state.vaults().list().await
    }

    async fn create_managed_vault(
        &self,
        slug: VaultSlug,
        name: &str,
    ) -> Result<mcp_vault_core::ManagedVaultCreation, VaultError> {
        self.managed_vaults.create(slug, name).await
    }

    async fn register_vault(
        &self,
        context: &VaultContext,
        name: &str,
        status: VaultStatus,
    ) -> Result<VaultRecord, StateError> {
        self.state.vaults().insert(context, name, status).await
    }

    async fn update_vault_name(
        &self,
        context: &VaultContext,
        name: &str,
    ) -> Result<(), StateError> {
        self.state.vaults().update_name(context, name).await
    }

    async fn update_vault_status(
        &self,
        context: &VaultContext,
        status: VaultStatus,
    ) -> Result<(), StateError> {
        self.state.vaults().set_status(context, status).await
    }

    async fn dashboard_files(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<mcp_vault_state::FileRecord>, StateError> {
        self.state.files().list_active_entries(context).await
    }

    async fn memory_counts(&self, context: &VaultContext) -> Result<MemoryCounts, StateError> {
        self.state.memory().counts(context).await
    }

    async fn memory_source_health_counts(
        &self,
        context: &VaultContext,
    ) -> Result<MemorySourceHealthCounts, StateError> {
        self.state.memory().source_health_counts(context).await
    }

    async fn memory_source_health_details(
        &self,
        context: &VaultContext,
        health: Option<MemorySourceHealthState>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MemorySourceHealthDetailRecord>, StateError> {
        self.state
            .memory()
            .list_source_health_details(context, health, limit, offset)
            .await
    }

    async fn memory_source_audit(
        &self,
        context: &VaultContext,
    ) -> Result<Option<MemorySourceAuditStateRecord>, StateError> {
        self.state.memory().get_source_audit(context).await
    }

    async fn memory_regeneration_pending(
        &self,
        context: &VaultContext,
    ) -> Result<bool, StateError> {
        Ok(self
            .state
            .memory()
            .get_consolidation_state(context)
            .await?
            .is_some_and(|state| state.regeneration_pending))
    }

    async fn pending_jobs_for(&self, context: &VaultContext) -> Result<u64, StateError> {
        self.state.jobs().pending_count_for(context).await
    }

    async fn enqueue_memory_source_audit(
        &self,
        context: &VaultContext,
        generation: &str,
    ) -> Result<JobRecord, StateError> {
        self.state
            .jobs()
            .enqueue(
                context,
                "memory.audit_sources",
                &format!("vault:{}:memory-source-audit:{generation}", context.id()),
                &json!({
                    "generation": generation,
                    "reason": "admin",
                    "page_size": 100,
                    "pipeline_generation": mcp_vault_memory::MEMORY_PIPELINE_GENERATION,
                }),
                6,
                10,
                now_millis(),
            )
            .await
    }

    async fn provider_health(&self) -> Result<Vec<ProviderHealthRecord>, StateError> {
        self.state.providers().list_health(1000).await
    }

    async fn integrity(&self) -> Result<mcp_vault_state::IntegrityReport, StateError> {
        self.state.integrity_check().await
    }

    async fn sqlite_diagnostics(&self) -> (Option<String>, Option<i64>) {
        (
            self.state.journal_mode().await.ok(),
            self.state.busy_timeout_millis().await.ok(),
        )
    }

    async fn global_pending(&self) -> Result<(u64, u64), StateError> {
        Ok((
            self.state.outbox().pending_count().await?,
            self.state.jobs().pending_count().await?,
        ))
    }

    async fn list_webdav(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<WebDavCredentialRecord>, StateError> {
        self.state
            .auth()
            .list_webdav_credentials(context, limit)
            .await
    }

    async fn list_tokens(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<McpTokenRecord>, StateError> {
        self.state.auth().list_mcp_tokens(context, limit).await
    }

    async fn list_oauth(
        &self,
        limit: u32,
    ) -> Result<Vec<mcp_vault_state::OAuthIssuerRecord>, StateError> {
        self.state.auth().list_oauth_issuers(limit).await
    }

    async fn list_jobs_for(
        &self,
        context: &VaultContext,
        status: Option<JobStatus>,
        job_type: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<JobRecord>, StateError> {
        self.state
            .jobs()
            .list(context, status, job_type, limit, offset)
            .await
    }

    async fn list_terminal_jobs_for(
        &self,
        context: &VaultContext,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<JobRecord>, StateError> {
        self.state
            .jobs()
            .list_terminal(context, limit, offset)
            .await
    }

    async fn job_status_counts_for(
        &self,
        context: &VaultContext,
    ) -> Result<JobStatusCounts, StateError> {
        self.state.jobs().status_counts(context).await
    }

    async fn active_job_for(
        &self,
        context: &VaultContext,
        job_type: &str,
    ) -> Result<Option<JobRecord>, StateError> {
        self.state
            .jobs()
            .find_active_by_type(context, job_type)
            .await
    }

    async fn get_job_for(
        &self,
        context: &VaultContext,
        id: mcp_vault_domain::JobId,
    ) -> Result<Option<JobRecord>, StateError> {
        self.state.jobs().get(context, id).await
    }

    async fn retry_job_for(
        &self,
        context: &VaultContext,
        id: mcp_vault_domain::JobId,
    ) -> Result<(), StateError> {
        self.state.jobs().request_retry(context, id).await
    }

    async fn restart_terminal_job_for(
        &self,
        context: &VaultContext,
        id: mcp_vault_domain::JobId,
    ) -> Result<(), StateError> {
        self.state
            .jobs()
            .request_restart_terminal(context, id)
            .await
    }

    async fn cancel_job_for(
        &self,
        context: &VaultContext,
        id: mcp_vault_domain::JobId,
    ) -> Result<(), StateError> {
        self.state.jobs().request_cancel(context, id).await
    }

    async fn cancel_jobs_for_vault(&self, context: &VaultContext) -> Result<u64, StateError> {
        self.state.jobs().request_cancel_all(context).await
    }

    async fn enqueue_vault_job(
        &self,
        context: &VaultContext,
        job_type: &str,
        dedup_key: &str,
        payload: &Value,
        priority: i32,
        max_attempts: u32,
    ) -> Result<JobRecord, StateError> {
        self.state
            .jobs()
            .enqueue(
                context,
                job_type,
                dedup_key,
                payload,
                priority,
                max_attempts,
                now_millis(),
            )
            .await
    }

    async fn audit_for(
        &self,
        context: &VaultContext,
        action: Option<&str>,
        result: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<AuditRecord>, StateError> {
        self.state
            .audit()
            .list_for_vault(context, action, result, limit, offset)
            .await
    }

    /// Append a redacted, Vault-scoped control-plane audit fact. Audit
    /// failures are logged without request bodies or secret values; the
    /// mutating application service remains the owner of its state change.
    #[allow(clippy::too_many_arguments)]
    async fn append_admin_audit(
        &self,
        context: Option<&VaultContext>,
        request_id: &str,
        actor: &Actor,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        metadata: Value,
    ) {
        if let Err(error) = self
            .state
            .audit()
            .append(
                context,
                Some(request_id),
                SourcePlane::Admin,
                actor,
                action,
                target_type,
                target_id,
                "success",
                &metadata,
            )
            .await
        {
            tracing::warn!(%error, action, "failed to append Admin audit fact");
        }
    }

    async fn secret_hint(&self, id: mcp_vault_domain::SecretId) -> Option<Value> {
        self.state
            .auth()
            .get_secret(id)
            .await
            .ok()
            .flatten()
            .map(|record| json!({"configured": true, "hint": record.hint}))
    }

    async fn provider_health_for(&self, id: mcp_vault_domain::ProviderId) -> Option<Value> {
        self.state
            .providers()
            .get_health(id)
            .await
            .ok()
            .flatten()
            .map(|health| provider_health_json(&health))
    }
}

/// Build the versioned Admin API boundary retained for bootstrap/composition
/// tests. It deliberately has no state access.
pub fn router() -> Router {
    Router::new().fallback(axum::routing::any(not_configured))
}

/// Build the authenticated, stateful Admin API.
pub fn stateful_router(state: AdminApiState) -> Router {
    let public = Router::new()
        .route("/setup", get(setup_status).post(setup))
        .route("/session", post(login));
    let protected = Router::new()
        .route("/session", get(current_session).delete(logout))
        .route("/session/password", post(change_password))
        .route("/system", get(system))
        .route("/health/details", get(health_details))
        .route("/diagnostics", get(diagnostics))
        .route("/vaults", get(list_vault_registry).post(create_vault))
        .route("/vaults/{*scoped_path}", any(scoped_vault_dispatch))
        .route(
            "/providers/{id}",
            get(get_provider)
                .patch(update_provider)
                .delete(delete_provider),
        )
        .route("/providers/{id}/test", post(test_provider))
        .route(
            "/providers/{id}/models",
            get(list_provider_models).post(register_provider_model),
        )
        .route("/providers/{id}/models/refresh", post(refresh_models))
        .route("/backups", get(list_backups).post(create_backup))
        .route("/backups/{id}/verify", post(verify_backup))
        .route("/restore/validate", post(validate_restore))
        .route("/restore", post(restore))
        .route("/maintenance/recover", post(recover_maintenance))
        .merge(vault_admin_routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authenticate_session,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            enforce_maintenance,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            capture_peer_address,
        ))
        .with_state(state)
}

fn vault_admin_routes() -> Router<AdminApiState> {
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/vault", get(get_vault).patch(patch_vault))
        .route("/vault/rescan", post(rescan_vault))
        .route(
            "/vault/initialization/retry",
            post(retry_vault_initialization),
        )
        .route(
            "/webdav/credentials",
            get(list_webdav_credentials).post(issue_webdav_credential),
        )
        .route(
            "/webdav/credentials/{id}",
            patch(update_webdav_credential).delete(revoke_webdav_credential),
        )
        .route("/mcp/tokens", get(list_mcp_tokens).post(issue_mcp_token))
        .route("/mcp/tokens/{id}", delete(revoke_mcp_token))
        .route(
            "/mcp/oauth/local",
            get(get_local_oauth)
                .put(put_local_oauth)
                .delete(disable_local_oauth),
        )
        .route("/mcp/oauth", get(get_oauth).put(put_oauth))
        .route(
            "/mcp/oauth/grants",
            get(list_oauth_grants).post(upsert_oauth_grant),
        )
        .route("/mcp/oauth/grants/{id}", delete(revoke_oauth_grant))
        .route("/mcp/connection-info", get(connection_info))
        .route(
            "/providers/mode",
            get(get_provider_mode).put(put_provider_mode),
        )
        .route("/providers", get(list_providers).post(create_provider))
        .route("/model-bindings", get(list_model_bindings))
        .route("/model-bindings/{role}", put(update_model_binding))
        .route("/index/status", get(index_status))
        .route("/index/rebuild", post(rebuild_index))
        .route("/index/nodes", get(index_nodes))
        .route("/memories", get(list_memories))
        .route("/memories/merge", post(merge_memories))
        .route(
            "/memories/{id}",
            get(get_memory).patch(update_memory).delete(delete_memory),
        )
        .route("/memories/{id}/archive", post(archive_memory))
        .route("/memories/{id}/restore", post(restore_memory))
        .route(
            "/memory/extraction",
            get(get_memory_extraction).put(put_memory_extraction),
        )
        .route("/memory/extraction/run", post(run_memory_extraction))
        .route(
            "/memory/extraction/restart",
            post(restart_memory_extraction),
        )
        .route("/memory/source-health", get(get_memory_source_health))
        .route("/memory/source-health/audit", post(run_memory_source_audit))
        .route("/jobs", get(list_jobs))
        .route("/jobs/overview", get(jobs_overview))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/retry", post(retry_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/audit", get(list_audit))
}

async fn scoped_vault_dispatch(
    State(state): State<AdminApiState>,
    Path(scoped_path): Path<String>,
    request: Request<Body>,
) -> Response {
    let scoped_path = scoped_path.trim_start_matches('/');
    let (slug_value, relative) = scoped_path
        .split_once('/')
        .map_or((scoped_path, ""), |(slug, relative)| (slug, relative));
    let slug = match VaultSlug::new(slug_value) {
        Ok(slug) => slug,
        Err(_) => {
            return api_error(
                StatusCode::NOT_FOUND,
                "vault_not_found",
                "The selected Vault was not found.",
                None,
                request_id(request.headers()),
            );
        }
    };
    let vault = match state.state.vaults().find_by_slug(&slug).await {
        Ok(Some(vault)) => vault,
        Ok(None) => {
            return api_error(
                StatusCode::NOT_FOUND,
                "vault_not_found",
                "The selected Vault was not found.",
                None,
                request_id(request.headers()),
            );
        }
        Err(error) => return state_error(error, request_id(request.headers())),
    };

    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let inner_path = match relative {
        "" => "/vault".to_owned(),
        "rescan" => "/vault/rescan".to_owned(),
        "initialization/retry" => "/vault/initialization/retry".to_owned(),
        relative => format!("/{relative}"),
    };
    let principal = request.extensions().get::<AdminPrincipal>().cloned();
    let request_id_extension = request.extensions().get::<RequestId>().cloned();
    let method = request.method().clone();
    let version = request.version();
    let (parts, body) = request.into_parts();
    let mut inner = Request::builder()
        .method(method)
        .version(version)
        .uri(format!("{inner_path}{query}"))
        .body(body)
        .expect("scoped Admin URI is valid");
    *inner.headers_mut() = parts.headers;
    if let Some(principal) = principal {
        inner.extensions_mut().insert(principal);
    }
    if let Some(request_id) = request_id_extension {
        inner.extensions_mut().insert(request_id);
    }

    vault_admin_routes()
        .with_state(state.with_selected_vault(vault.id))
        .oneshot(inner)
        .await
        .expect("scoped Admin router is infallible")
}

async fn not_configured() -> Response {
    api_error(
        StatusCode::NOT_IMPLEMENTED,
        "admin_not_configured",
        "Admin API is not configured.",
        None,
        mcp_vault_domain::EventId::new().to_string(),
    )
}

async fn enforce_maintenance(
    State(state): State<AdminApiState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let maintenance = state.backup.maintenance();
    let recovery_path = request.uri().path().ends_with("/maintenance/recover");
    let login_path = request.method() == Method::POST && request.uri().path().ends_with("/session");
    if maintenance.mode() == MaintenanceMode::Offline
        && (recovery_path || login_path)
        && !state.backup.operation_active()
    {
        return next.run(request).await;
    }
    let _request_operation = match maintenance.try_start_operation() {
        Some(operation) => operation,
        None => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "maintenance",
                "The Admin operation is temporarily paused for restore coordination.",
                None,
                request_id(request.headers()),
            );
        }
    };
    if request.method() == Method::GET
        || request.method() == Method::HEAD
        || request.method() == Method::OPTIONS
        || request.method() == Method::DELETE && request.uri().path().ends_with("/session")
        || request.uri().path().ends_with("/restore")
        || request.uri().path().ends_with("/restore/validate")
        || request.uri().path().ends_with("/maintenance/recover")
    {
        return next.run(request).await;
    }
    let Some(_write_operation) = maintenance.try_start_write() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "maintenance",
            "The Admin write operation is temporarily paused for backup or restore coordination.",
            None,
            request_id(request.headers()),
        );
    };
    next.run(request).await
}

async fn capture_peer_address(
    State(_state): State<AdminApiState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0.ip());
    request.extensions_mut().insert(AdminPeerIp(peer));
    next.run(request).await
}

#[derive(Clone, Copy, Debug)]
struct AdminPeerIp(Option<IpAddr>);

async fn authenticate_session(
    State(state): State<AdminApiState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let request_id = request_id(request.headers());
    let Some(cookie) = request.headers().get(header::COOKIE) else {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "admin_session_required",
            "An Admin session is required.",
            None,
            request_id,
        );
    };
    let Ok(cookie) = cookie.to_str() else {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "admin_session_invalid",
            "The Admin session is invalid.",
            None,
            request_id,
        );
    };
    let Ok(session_token) = parse_session_cookie(cookie) else {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "admin_session_invalid",
            "The Admin session is invalid.",
            None,
            request_id,
        );
    };
    let csrf = request
        .headers()
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok());
    match state
        .auth
        .authenticate_admin_session(
            session_token,
            csrf,
            request.headers(),
            request.method(),
            &state.origin_policy,
        )
        .await
    {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            request.extensions_mut().insert(RequestId(request_id));
            next.run(request).await
        }
        Err(error) => auth_error(error, request_id),
    }
}

fn validate_state_change_origin(
    state: &AdminApiState,
    headers: &HeaderMap,
    method: &Method,
) -> Result<(), AuthError> {
    state.origin_policy.validate_admin_request(headers, method)
}

#[derive(Clone, Debug)]
struct RequestId(String);

fn request_id(headers: &HeaderMap) -> String {
    if let Some(value) = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        && !value.is_empty()
        && value.len() <= 128
        && !value.chars().any(char::is_control)
    {
        return value.to_owned();
    }
    mcp_vault_domain::EventId::new().to_string()
}

fn auth_error(error: AuthError, request_id: String) -> Response {
    let (status, code, message) = match error {
        AuthError::OriginRejected => (
            StatusCode::FORBIDDEN,
            "origin_rejected",
            "The request Origin is not allowed.",
        ),
        AuthError::CsrfRejected => (
            StatusCode::FORBIDDEN,
            "csrf_rejected",
            "The CSRF token is invalid.",
        ),
        AuthError::RateLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many authentication attempts.",
        ),
        AuthError::SessionExpired => (
            StatusCode::UNAUTHORIZED,
            "admin_session_expired",
            "The Admin session has expired.",
        ),
        AuthError::InvalidCredential | AuthError::Revoked | AuthError::Expired => (
            StatusCode::UNAUTHORIZED,
            "admin_session_invalid",
            "The Admin session is invalid.",
        ),
        AuthError::SetupUnavailable => (
            StatusCode::CONFLICT,
            "setup_unavailable",
            "First-run setup is no longer available.",
        ),
        AuthError::PasswordPolicy => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "password_policy",
            "The password does not satisfy the configured policy.",
        ),
        AuthError::InvalidInput => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The authentication input is invalid.",
        ),
        AuthError::State(_) | AuthError::Cryptography | AuthError::MasterKeyUnavailable => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "authentication_unavailable",
            "Authentication is temporarily unavailable.",
        ),
        _ => (
            StatusCode::UNAUTHORIZED,
            "authentication_failed",
            "Authentication failed.",
        ),
    };
    api_error(status, code, message, None, request_id)
}

fn api_error(
    status: StatusCode,
    code: &str,
    message: &str,
    fields: Option<Value>,
    request_id: String,
) -> Response {
    let body = json!({
        "error": {
            "code": code,
            "message": message,
            "fields": fields.unwrap_or_else(|| json!({})),
        },
        "request_id": request_id,
    });
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

fn api_ok<T: Serialize>(status: StatusCode, data: T, request_id: String) -> Response {
    let body = json!({"data": data, "request_id": request_id});
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

fn parse_id<T>(value: &str, message: &'static str) -> Result<T, Response>
where
    T: FromStr,
{
    value.parse().map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "invalid_id",
            message,
            None,
            mcp_vault_domain::EventId::new().to_string(),
        )
    })
}

fn page_params(query: &PageQuery) -> Result<(u32, u32), Response> {
    let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    let offset = query.offset.unwrap_or(0);
    if limit == 0 || limit > MAX_PAGE_SIZE || offset > 1_000_000 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_pagination",
            "The pagination bounds are invalid.",
            None,
            mcp_vault_domain::EventId::new().to_string(),
        ));
    }
    Ok((limit, offset))
}

#[derive(Debug, Deserialize, Default)]
struct PageQuery {
    limit: Option<u32>,
    offset: Option<u32>,
}

fn parse_vault_status(value: &str) -> Result<VaultStatus, Response> {
    match value {
        "active" => Ok(VaultStatus::Active),
        "maintenance" => Ok(VaultStatus::Maintenance),
        "disabled" => Ok(VaultStatus::Disabled),
        "error" => Ok(VaultStatus::Error),
        _ => Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The Vault status is invalid.",
            Some(json!({"status": "unsupported status"})),
            mcp_vault_domain::EventId::new().to_string(),
        )),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct PasswordChangeRequest {
    current_password: String,
    new_password: String,
}

#[derive(Debug, Serialize)]
struct SessionResponse {
    user_id: String,
    username: String,
    expires_at: Option<i64>,
    csrf_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct VaultSummary {
    id: String,
    slug: String,
    name: String,
    content_root: String,
    reserved_root: String,
    status: String,
    availability: String,
    settings_revision: i64,
}

async fn setup_status(State(state): State<AdminApiState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    match state.auth.admin_setup_available().await {
        Ok(setup_available) => api_ok(
            StatusCode::OK,
            json!({"setup_available": setup_available}),
            request_id,
        ),
        Err(error) => auth_error(error, request_id),
    }
}

async fn setup(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(peer): Extension<AdminPeerIp>,
    Json(input): Json<SetupRequest>,
) -> Response {
    let request_id = request_id(&headers);
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id);
    }
    let username = input.username;
    let password = SecretString::new(input.password);
    let source_ip = peer.0.map(|address| address.to_string());
    let prepared = match state
        .auth
        .prepare_admin_setup(&username, &password, source_ip.as_deref())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => return auth_error(error, request_id),
    };

    let vault = match state.list_vaults().await {
        Ok(vaults) if vaults.is_empty() => {
            let root = state.data_dir.join("vaults").join("default");
            let context = match VaultContext::new(
                VaultId::new(),
                VaultSlug::new("default").expect("default Vault slug is valid"),
                root,
                Revision::ZERO,
            ) {
                Ok(context) => context,
                Err(_) => {
                    return api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "vault_setup_failed",
                        "The default Vault could not be initialized.",
                        None,
                        request_id,
                    );
                }
            };
            match state
                .register_vault(&context, "Default Vault", VaultStatus::Active)
                .await
            {
                Ok(vault) => vault,
                Err(_) => {
                    match state.list_vaults().await.ok().and_then(|vaults| {
                        vaults
                            .into_iter()
                            .find(|vault| vault.slug.as_str() == "default")
                    }) {
                        Some(vault) => vault,
                        None => {
                            return api_error(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "vault_setup_failed",
                                "The default Vault could not be initialized.",
                                None,
                                request_id,
                            );
                        }
                    }
                }
            }
        }
        Ok(mut vaults) => vaults.remove(0),
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "state_unavailable",
                "Operational state is temporarily unavailable.",
                None,
                request_id,
            );
        }
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "vault_setup_failed",
                "The default Vault memory pipeline could not be initialized.",
                None,
                request_id,
            );
        }
    };
    if state
        .state
        .vaults()
        .ensure_legacy_default(&context)
        .await
        .is_err()
    {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "vault_setup_failed",
            "The default Vault compatibility binding could not be initialized.",
            None,
            request_id,
        );
    }
    if state
        .state
        .memory()
        .set_pipeline_generation_state(
            &context,
            mcp_vault_memory::MEMORY_PIPELINE_GENERATION,
            false,
        )
        .await
        .is_err()
    {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "vault_setup_failed",
            "The default Vault memory pipeline could not be initialized.",
            None,
            request_id,
        );
    }
    let user = match state.auth.commit_admin_setup(prepared).await {
        Ok(user) => user,
        Err(error) => return auth_error(error, request_id),
    };
    let actor = Actor::identified(
        ActorType::Admin,
        ActorId::new(&user.id.to_string()).expect("Admin user IDs are valid actor IDs"),
    );
    state
        .append_admin_audit(
            Some(&context),
            &request_id,
            &actor,
            "admin.setup.completed",
            Some("admin_user"),
            Some(&user.id.to_string()),
            json!({"vault_initialized": true}),
        )
        .await;
    api_ok(
        StatusCode::CREATED,
        json!({
            "user": {"id": user.id.to_string(), "username": user.username},
            "vault": vault_summary(&vault),
        }),
        request_id,
    )
}

async fn login(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(peer): Extension<AdminPeerIp>,
    Json(input): Json<LoginRequest>,
) -> Response {
    let request_id = request_id(&headers);
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id);
    }
    let cookie_security = match state.origin_policy.admin_cookie_security(&headers) {
        Ok(security) => security,
        Err(error) => return auth_error(error, request_id),
    };
    let source_ip = peer.0.map(|address| address.to_string());
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok());
    let login = match state
        .auth
        .login_admin(
            &input.username,
            &SecretString::new(input.password),
            source_ip.as_deref(),
            user_agent,
        )
        .await
    {
        Ok(login) => login,
        Err(error) => {
            state
                .append_admin_audit(
                    None,
                    &request_id,
                    &Actor::system(),
                    "admin.login.failed",
                    Some("admin_session"),
                    None,
                    json!({"outcome": "rejected"}),
                )
                .await;
            return auth_error(error, request_id);
        }
    };
    let actor = Actor::identified(
        ActorType::Admin,
        ActorId::new(&login.user_id.to_string()).expect("Admin user IDs are valid actor IDs"),
    );
    state
        .append_admin_audit(
            None,
            &request_id,
            &actor,
            "admin.login.succeeded",
            Some("admin_session"),
            Some(&login.session_id.to_string()),
            json!({"outcome": "accepted"}),
        )
        .await;
    let csrf_token = login.csrf_token.expose_secret().to_owned();
    let mut response = api_ok(
        StatusCode::OK,
        SessionResponse {
            user_id: login.user_id.to_string(),
            username: login.username,
            expires_at: Some(login.expires_at),
            csrf_token: Some(csrf_token),
        },
        request_id,
    );
    if let Ok(value) = HeaderValue::from_str(&session_cookie_header(
        &login.session_token,
        SESSION_MAX_AGE,
        cookie_security,
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&csrf_cookie_header(
        &login.csrf_token,
        SESSION_MAX_AGE,
        cookie_security,
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

async fn current_session(
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let _ = headers;
    api_ok(
        StatusCode::OK,
        SessionResponse {
            user_id: principal.user_id.to_string(),
            username: principal.username,
            expires_at: None,
            csrf_token: None,
        },
        request_id.0,
    )
}

async fn logout(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let cookie_security = match state.origin_policy.admin_cookie_security(&headers) {
        Ok(security) => security,
        Err(error) => return auth_error(error, request_id.0),
    };
    let token = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_session_cookie(value).ok());
    if let Some(token) = token
        && let Err(error) = state.auth.logout_admin(token).await
    {
        return auth_error(error, request_id.0);
    }
    state
        .append_admin_audit(
            None,
            &request_id.0,
            &principal.actor,
            "admin.logout",
            Some("admin_session"),
            None,
            json!({"outcome": "accepted"}),
        )
        .await;
    let mut response = api_ok(StatusCode::OK, json!({"logged_out": true}), request_id.0);
    if let Ok(value) = HeaderValue::from_str(&clear_session_cookie_header(cookie_security)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&clear_csrf_cookie_header(cookie_security)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

async fn change_password(
    State(state): State<AdminApiState>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<PasswordChangeRequest>,
) -> Response {
    match state
        .auth
        .change_admin_password(
            principal.user_id,
            &SecretString::new(input.current_password),
            &SecretString::new(input.new_password),
        )
        .await
    {
        Ok(()) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.password.changed",
                    Some("admin_user"),
                    Some(&principal.user_id.to_string()),
                    json!({"outcome": "accepted"}),
                )
                .await;
            api_ok(StatusCode::OK, json!({"changed": true}), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn current_vault(state: &AdminApiState, request_id: &str) -> Result<VaultRecord, Response> {
    if let Some(vault_id) = state.selected_vault {
        return match state.state.vaults().find_by_id(vault_id).await {
            Ok(Some(vault)) => Ok(vault),
            Ok(None) => Err(api_error(
                StatusCode::NOT_FOUND,
                "vault_not_found",
                "The selected Vault was not found.",
                None,
                request_id.to_owned(),
            )),
            Err(error) => Err(state_error(error, request_id.to_owned())),
        };
    }
    match state.state.vaults().legacy_default().await {
        Ok(Some(vault)) => Ok(vault),
        Ok(None) => Err(api_error(
            StatusCode::CONFLICT,
            "vault_selection_required",
            "A Vault must be selected explicitly.",
            None,
            request_id.to_owned(),
        )),
        Err(error) => {
            tracing::warn!(%error, "failed to resolve legacy default Vault");
            Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "state_unavailable",
                "Operational state is temporarily unavailable.",
                None,
                request_id.to_owned(),
            ))
        }
    }
}

fn vault_summary(vault: &VaultRecord) -> VaultSummary {
    VaultSummary {
        id: vault.id.to_string(),
        slug: vault.slug.to_string(),
        name: vault.name.clone(),
        content_root: vault.content_root.display().to_string(),
        reserved_root: vault.reserved_root.to_string(),
        status: vault.status.to_string(),
        availability: match vault.status {
            VaultStatus::Active => "ready",
            VaultStatus::Maintenance => "maintenance",
            VaultStatus::Disabled => "disabled",
            VaultStatus::Error => "error",
        }
        .to_owned(),
        settings_revision: vault.settings_revision.value() as i64,
    }
}

async fn vault_summary_for(
    state: &AdminApiState,
    vault: &VaultRecord,
) -> Result<VaultSummary, StateError> {
    let mut summary = vault_summary(vault);
    summary.availability = state.state.vaults().availability(vault).await?.to_string();
    Ok(summary)
}

async fn vault_summaries_for(
    state: &AdminApiState,
    vaults: &[VaultRecord],
) -> Result<Vec<VaultSummary>, StateError> {
    let mut summaries = Vec::with_capacity(vaults.len());
    for vault in vaults {
        summaries.push(vault_summary_for(state, vault).await?);
    }
    Ok(summaries)
}

#[derive(Debug, Deserialize)]
struct VaultPatchRequest {
    name: Option<String>,
    status: Option<String>,
    expected_settings_revision: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VaultCreateRequest {
    slug: String,
    name: String,
}

async fn list_vault_registry(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vaults = match state.list_vaults().await {
        Ok(vaults) => vaults,
        Err(error) => return state_error(error, request_id.0),
    };
    let summaries = match vault_summaries_for(&state, &vaults).await {
        Ok(summaries) => summaries,
        Err(error) => return state_error(error, request_id.0),
    };
    api_ok(StatusCode::OK, json!({"vaults": summaries}), request_id.0)
}

async fn create_vault(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<VaultCreateRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let slug = match VaultSlug::new(&input.slug) {
        Ok(slug) => slug,
        Err(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The Vault slug is invalid.",
                Some(json!({
                    "slug": "use lowercase ASCII letters, digits, and hyphens"
                })),
                request_id.0,
            );
        }
    };
    match state.create_managed_vault(slug, &input.name).await {
        Ok(created) => {
            let context = match created.vault.context() {
                Ok(context) => context,
                Err(error) => return state_error(error, request_id.0),
            };
            let vault_id = created.vault.id.to_string();
            let job_id = created.initialization_job.id.to_string();
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.vault.created",
                    Some("vault"),
                    Some(&vault_id),
                    json!({
                        "slug": created.vault.slug.as_str(),
                        "job_id": job_id,
                    }),
                )
                .await;
            let summary = match vault_summary_for(&state, &created.vault).await {
                Ok(summary) => summary,
                Err(error) => return state_error(error, request_id.0),
            };
            api_ok(
                StatusCode::ACCEPTED,
                json!({
                    "vault": summary,
                    "initialization_job": job_summary(&created.initialization_job),
                }),
                request_id.0,
            )
        }
        Err(VaultError::AlreadyExists) => api_error(
            StatusCode::CONFLICT,
            "vault_already_exists",
            "A Vault with this slug already exists.",
            Some(json!({"slug": "already exists"})),
            request_id.0,
        ),
        Err(VaultError::Storage(_)) => api_error(
            StatusCode::CONFLICT,
            "vault_root_unavailable",
            "The managed Vault directory cannot be safely claimed.",
            None,
            request_id.0,
        ),
        Err(VaultError::Domain(_)) => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The Vault configuration is invalid.",
            None,
            request_id.0,
        ),
        Err(VaultError::State(error)) => state_error(error, request_id.0),
        Err(_) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "vault_create_failed",
            "The Vault could not be created.",
            None,
            request_id.0,
        ),
    }
}

async fn get_vault(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match current_vault(&state, &request_id.0).await {
        Ok(vault) => match vault_summary_for(&state, &vault).await {
            Ok(summary) => api_ok(StatusCode::OK, summary, request_id.0),
            Err(error) => state_error(error, request_id.0),
        },
        Err(response) => response,
    }
}

async fn patch_vault(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<VaultPatchRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PATCH) {
        return auth_error(error, request_id.0);
    }
    let mut vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    if input
        .expected_settings_revision
        .is_some_and(|revision| revision != vault.settings_revision.value() as i64)
    {
        return api_error(
            StatusCode::CONFLICT,
            "revision_conflict",
            "The Vault configuration changed; reload before updating it.",
            None,
            request_id.0,
        );
    }
    let changed_name = input.name.is_some();
    let changed_status = input.status.is_some();
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "vault_context_invalid",
                "The registered Vault context is invalid.",
                None,
                request_id.0,
            );
        }
    };
    if let Some(name) = input.name
        && let Err(error) = state.update_vault_name(&context, &name).await
    {
        return state_error(error, request_id.0);
    }
    if let Some(status) = input.status {
        let status = match parse_vault_status(&status) {
            Ok(status) => status,
            Err(response) => return response,
        };
        if let Err(error) = state.update_vault_status(&context, status).await {
            return state_error(error, request_id.0);
        }
        if status == VaultStatus::Disabled {
            if let Err(error) = state.cancel_jobs_for_vault(&context).await {
                tracing::warn!(%error, vault_id = %context.id(), "failed to cancel disabled Vault jobs");
            }
        } else if status == VaultStatus::Active && vault.status != VaultStatus::Active {
            let dedup = format!(
                "vault:{}:reactivated:{}",
                context.id(),
                mcp_vault_domain::EventId::new()
            );
            if let Err(error) = state
                .enqueue_vault_job(
                    &context,
                    "vault.reconcile",
                    &dedup,
                    &json!({"reason": "vault_reactivated"}),
                    10,
                    10,
                )
                .await
            {
                tracing::warn!(%error, vault_id = %context.id(), "failed to enqueue reactivated Vault reconciliation");
            }
        }
    }
    vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let target_id = context.id().to_string();
    state
        .append_admin_audit(
            Some(&context),
            &request_id.0,
            &principal.actor,
            "admin.vault.updated",
            Some("vault"),
            Some(&target_id),
            json!({"name_changed": changed_name, "status_changed": changed_status}),
        )
        .await;
    match vault_summary_for(&state, &vault).await {
        Ok(summary) => api_ok(StatusCode::OK, summary, request_id.0),
        Err(error) => state_error(error, request_id.0),
    }
}

async fn rescan_vault(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let dedup = format!(
        "vault:{}:admin-rescan:{}",
        context.id(),
        mcp_vault_domain::EventId::new()
    );
    match state
        .enqueue_vault_job(
            &context,
            "vault.reconcile",
            &dedup,
            &json!({"reason": "admin_rescan"}),
            10,
            3,
        )
        .await
    {
        Ok(job) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.vault.rescan_queued",
                    Some("job"),
                    Some(&job.id.to_string()),
                    json!({"job_type": "vault.reconcile"}),
                )
                .await;
            api_ok(StatusCode::ACCEPTED, job_summary(&job), request_id.0)
        }
        Err(error) => state_error(error, request_id.0),
    }
}

async fn retry_vault_initialization(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let mut vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    if vault.status == VaultStatus::Disabled {
        return api_error(
            StatusCode::CONFLICT,
            "vault_disabled",
            "Enable the Vault before retrying initialization.",
            None,
            request_id.0,
        );
    }
    let context = match vault.context() {
        Ok(context) => context,
        Err(error) => return state_error(error, request_id.0),
    };
    if vault.status != VaultStatus::Active {
        if let Err(error) = state
            .update_vault_status(&context, VaultStatus::Active)
            .await
        {
            return state_error(error, request_id.0);
        }
        vault = match state.state.vaults().find_by_id(context.id()).await {
            Ok(Some(vault)) => vault,
            Ok(None) => {
                return api_error(
                    StatusCode::NOT_FOUND,
                    "vault_not_found",
                    "The selected Vault was not found.",
                    None,
                    request_id.0,
                );
            }
            Err(error) => return state_error(error, request_id.0),
        };
    }
    let current_context = match vault.context() {
        Ok(context) => context,
        Err(error) => return state_error(error, request_id.0),
    };
    let dedup_key = format!("vault:{}:initialize", current_context.id());
    let existing = match state
        .state
        .jobs()
        .find_by_dedup(&current_context, &dedup_key)
        .await
    {
        Ok(existing) => existing,
        Err(error) => return state_error(error, request_id.0),
    };
    let job = match existing {
        Some(job) if matches!(job.status, JobStatus::Failed | JobStatus::Cancelled) => {
            if let Err(error) = state
                .restart_terminal_job_for(&current_context, job.id)
                .await
            {
                return state_error(error, request_id.0);
            }
            match state.get_job_for(&current_context, job.id).await {
                Ok(Some(job)) => job,
                Ok(None) => {
                    return api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "job_unavailable",
                        "The initialization job is unavailable.",
                        None,
                        request_id.0,
                    );
                }
                Err(error) => return state_error(error, request_id.0),
            }
        }
        Some(job)
            if matches!(
                job.status,
                JobStatus::Queued | JobStatus::Running | JobStatus::RetryWait
            ) =>
        {
            job
        }
        Some(job) => {
            return api_error(
                StatusCode::CONFLICT,
                "vault_already_initialized",
                "The Vault initialization has already completed.",
                Some(json!({"job_id": job.id.to_string()})),
                request_id.0,
            );
        }
        None => match state
            .enqueue_vault_job(
                &current_context,
                "vault.initialize",
                &dedup_key,
                &json!({"reason": "admin_retry"}),
                20,
                10,
            )
            .await
        {
            Ok(job) => job,
            Err(error) => return state_error(error, request_id.0),
        },
    };
    let job_id = job.id.to_string();
    state
        .append_admin_audit(
            Some(&current_context),
            &request_id.0,
            &principal.actor,
            "admin.vault.initialization_retried",
            Some("job"),
            Some(&job_id),
            json!({"job_type": "vault.initialize"}),
        )
        .await;
    api_ok(StatusCode::ACCEPTED, job_summary(&job), request_id.0)
}

async fn dashboard(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let entries = match state.dashboard_files(&context).await {
        Ok(entries) => entries,
        Err(error) => return state_error(error, request_id.0),
    };
    let core = match state.core_for_vault(&vault) {
        Ok(core) => core,
        Err(error) => return state_error(error, request_id.0),
    };
    let visible_entries = entries
        .iter()
        .filter(|entry| !core.is_managed_path(&entry.path))
        .collect::<Vec<_>>();
    let note_count = visible_entries
        .iter()
        .filter(|entry| {
            entry.entry_type.as_str() == "file"
                && entry.path.as_str().to_ascii_lowercase().ends_with(".md")
        })
        .count();
    let attachment_count = visible_entries
        .iter()
        .filter(|entry| entry.entry_type.as_str() == "file")
        .count()
        .saturating_sub(note_count);
    let total_notes = u64::try_from(note_count).unwrap_or(u64::MAX);
    let index_status = match state.index().status(&context).await {
        Ok(status) => status,
        Err(error) => return index_error(error, request_id.0),
    };
    let memory_counts = match state.memory_counts(&context).await {
        Ok(counts) => counts,
        Err(error) => return state_error(error, request_id.0),
    };
    let stage1_counts = match state.state.memory().stage1_counts(&context).await {
        Ok(counts) => counts,
        Err(error) => return state_error(error, request_id.0),
    };
    let jobs_pending = match state.pending_jobs_for(&context).await {
        Ok(count) => count,
        Err(error) => return state_error(error, request_id.0),
    };
    let providers = match state.provider_health().await {
        Ok(providers) => providers,
        Err(error) => return state_error(error, request_id.0),
    };
    let vault_summary = match vault_summary_for(&state, &vault).await {
        Ok(summary) => summary,
        Err(error) => return state_error(error, request_id.0),
    };
    api_ok(
        StatusCode::OK,
        json!({
            "version": state.version,
            "ready": state.readiness.load(Ordering::Acquire),
            "vault": vault_summary,
            "files": {"notes": note_count, "attachments": attachment_count, "entries": visible_entries.len()},
            "index": index_status.map(|status| index_status_json(status, total_notes)),
            "memory": memory_counts_json(&memory_counts, stage1_counts.pending),
            "jobs": {"pending": jobs_pending},
            "providers": providers.iter().map(provider_health_json).collect::<Vec<_>>(),
        }),
        request_id.0,
    )
}

async fn system(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let integrity = match state.integrity().await {
        Ok(integrity) => integrity,
        Err(error) => return state_error(error, request_id.0),
    };
    let (journal_mode, busy_timeout_ms) = state.sqlite_diagnostics().await;
    api_ok(
        StatusCode::OK,
        json!({
            "version": state.version,
            "listeners": {
                "data": state.data_bind.to_string(),
                "admin": state.admin_bind.to_string(),
            },
            "data_hosts": state.data_hosts,
            "data_origins": state.data_origins,
            "data_public_origin": state.data_public_origin,
            "data_dir": state.data_dir.display().to_string(),
            "history_root": state.history_root.display().to_string(),
            "ready": state.readiness.load(Ordering::Acquire),
            "maintenance": state.backup.maintenance().mode().as_str(),
            "database": {
                "migration_version": integrity.migration_version,
                "integrity_ok": integrity.integrity_ok,
                "foreign_key_violations": integrity.foreign_key_violations,
                "journal_mode": journal_mode,
                "busy_timeout_ms": busy_timeout_ms,
            },
        }),
        request_id.0,
    )
}

async fn health_details(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let integrity = match state.integrity().await {
        Ok(integrity) => integrity,
        Err(error) => return state_error(error, request_id.0),
    };
    let vaults = match state.list_vaults().await {
        Ok(vaults) => vaults,
        Err(error) => return state_error(error, request_id.0),
    };
    let (outbox_pending, jobs_pending) = match state.global_pending().await {
        Ok(counts) => counts,
        Err(error) => return state_error(error, request_id.0),
    };
    let latest_backup = state
        .backup
        .list(1, 0)
        .await
        .ok()
        .and_then(|mut records| records.pop());
    let vault_summaries = match vault_summaries_for(&state, &vaults).await {
        Ok(summaries) => summaries,
        Err(error) => return state_error(error, request_id.0),
    };
    api_ok(
        StatusCode::OK,
        json!({
            "ready": state.readiness.load(Ordering::Acquire),
            "database": {
                "migration_version": integrity.migration_version,
                "integrity_ok": integrity.integrity_ok,
                "foreign_key_violations": integrity.foreign_key_violations,
            },
            "vaults": vault_summaries,
            "outbox": {"pending": outbox_pending},
            "jobs": {"pending": jobs_pending},
            "backup": latest_backup.as_ref().map(backup_json),
            "maintenance": state.backup.maintenance().mode().as_str(),
        }),
        request_id.0,
    )
}

async fn diagnostics(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let integrity = match state.integrity().await {
        Ok(integrity) => integrity,
        Err(error) => return state_error(error, request_id.0),
    };
    let vaults = match state.list_vaults().await {
        Ok(vaults) => vaults,
        Err(error) => return state_error(error, request_id.0),
    };
    let pending = match state.global_pending().await {
        Ok(pending) => pending,
        Err(error) => return state_error(error, request_id.0),
    };
    let backups = state.backup.list(20, 0).await.unwrap_or_default();
    let vault_summaries = match vault_summaries_for(&state, &vaults).await {
        Ok(summaries) => summaries,
        Err(error) => return state_error(error, request_id.0),
    };
    api_ok(
        StatusCode::OK,
        json!({
            "format": "mcp-vault-diagnostic-v1",
            "version": state.version,
            "runtime": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "family": std::env::consts::FAMILY,
            },
            "maintenance": state.backup.maintenance().mode().as_str(),
            "database": {
                "migration_version": integrity.migration_version,
                "integrity_ok": integrity.integrity_ok,
                "foreign_key_violations": integrity.foreign_key_violations,
            },
            "listeners": {
                "data": state.data_bind.to_string(),
                "admin": state.admin_bind.to_string(),
            },
            "vaults": vault_summaries,
            "queues": {"outbox_pending": pending.0, "jobs_pending": pending.1},
            "backups": backups.iter().map(backup_json).collect::<Vec<_>>(),
            "redaction": {
                "note_bodies": false,
                "memory_bodies": false,
                "secrets": false,
                "authorization_headers": false,
            },
        }),
        request_id.0,
    )
}

fn index_status_json(status: mcp_vault_state::IndexStatusRecord, total_notes: u64) -> Value {
    let coverage_ratio = index_coverage_ratio(status.indexed_notes, total_notes);
    json!({
        "revision": status.index_revision.value(),
        "indexed_entries": status.indexed_entries,
        "indexed_notes": status.indexed_notes,
        "total_notes": total_notes,
        "indexed_bytes": status.indexed_bytes,
        "analyzer_version": status.analyzer_version,
        "coverage": status.coverage,
        "coverage_ratio": coverage_ratio,
        "last_rebuilt_at": status.last_rebuilt_at,
        "last_error": status.last_error,
    })
}

fn index_coverage_ratio(indexed_notes: u64, total_notes: u64) -> Option<f64> {
    (total_notes != 0).then(|| (indexed_notes as f64 / total_notes as f64).clamp(0.0, 1.0))
}

fn memory_counts_json(counts: &MemoryCounts, pending_consolidation: u64) -> Value {
    json!({
        "total": counts.total,
        "active": counts.active,
        "pending_consolidation": pending_consolidation,
        "stale": counts.stale,
        "superseded": counts.superseded,
        "archived": counts.archived,
        "quarantined": counts.quarantined,
    })
}

fn provider_health_json(health: &ProviderHealthRecord) -> Value {
    json!({
        "provider_id": health.provider_id.to_string(),
        "status": health.status,
        "checked_at": health.checked_at,
        "latency_ms": health.latency_ms,
        "model_count": health.model_count,
        "last_success_at": health.last_success_at,
        "last_error": health.last_error,
        "updated_at": health.updated_at,
    })
}

fn state_error(error: StateError, request_id: String) -> Response {
    let _ = error;
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "state_unavailable",
        "Operational state is temporarily unavailable.",
        None,
        request_id,
    )
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn job_summary(job: &JobRecord) -> Value {
    let progress_ratio = job_progress_ratio(job.status, job.progress.as_ref());
    let details = job_details(job);
    json!({
        "id": job.id.to_string(),
        "vault_id": job.vault_id.map(|id| id.to_string()),
        "job_type": job.job_type,
        "dedup_key": job.dedup_key,
        "status": job.status.as_str(),
        "priority": job.priority,
        "attempts": job.attempts,
        "max_attempts": job.max_attempts,
        "available_at": job.available_at,
        "progress": job.progress,
        "progress_ratio": progress_ratio,
        "details": details,
        "last_error": job.last_error,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "completed_at": job.completed_at,
        "cancel_requested": job.cancel_requested,
    })
}

fn job_admission_summary(job: &JobRecord, admission: &str) -> Value {
    let mut summary = job_summary(job);
    if let Some(object) = summary.as_object_mut() {
        object.insert("admission".to_owned(), Value::String(admission.to_owned()));
    }
    summary
}

fn job_details(job: &JobRecord) -> Option<Value> {
    if job.job_type != "memory.extract" {
        return None;
    }
    if job.payload.get("scope").and_then(Value::as_str) == Some("all") {
        return Some(json!({
            "scope": "all",
            "reason": job.payload.get("reason").and_then(Value::as_str),
            "include_evaluated": job
                .payload
                .get("include_evaluated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }));
    }
    let source_path = job
        .payload
        .get("payload")
        .and_then(|payload| payload.get("path"))
        .or_else(|| job.payload.get("path"))
        .and_then(Value::as_str);
    Some(json!({
        "scope": "note",
        "source_path": source_path,
        "event_type": job.payload.get("event_type").and_then(Value::as_str),
    }))
}

fn job_progress_ratio(status: JobStatus, progress: Option<&Value>) -> Option<f64> {
    if status == JobStatus::Completed {
        return Some(1.0);
    }
    let progress = progress?;
    let ratio = progress
        .as_f64()
        .map(|value| if value > 1.0 { value / 100.0 } else { value })
        .or_else(|| progress.get("ratio").and_then(Value::as_f64))
        .or_else(|| {
            progress
                .get("percent")
                .and_then(Value::as_f64)
                .map(|percent| percent / 100.0)
        })
        .or_else(|| {
            let completed = progress
                .get("completed")
                .or_else(|| progress.get("done"))
                .and_then(Value::as_f64)?;
            let total = progress.get("total").and_then(Value::as_f64)?;
            (total > 0.0).then_some(completed / total)
        })?;
    ratio.is_finite().then(|| ratio.clamp(0.0, 1.0))
}

#[derive(Debug, Deserialize)]
struct WebDavCredentialRequest {
    name: String,
    username: String,
    password: String,
    permissions: Vec<String>,
    expires_at: Option<i64>,
}

fn webdav_permissions(values: &[String]) -> Result<PermissionSet, Response> {
    let mut permissions = PermissionSet::new();
    for value in values {
        let permission = match value.as_str() {
            "discover" | "vault:discover" => Permission::DiscoverVault,
            "read" | "vault:read" => Permission::ReadVault,
            "write" | "vault:write" => Permission::WriteVault,
            "delete" | "vault:delete" => Permission::DeleteVault,
            _ => {
                return Err(api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "A WebDAV permission is invalid.",
                    Some(json!({"permissions": "unsupported permission"})),
                    mcp_vault_domain::EventId::new().to_string(),
                ));
            }
        };
        permissions.insert(permission);
    }
    if permissions.iter().next().is_none() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "At least one WebDAV permission is required.",
            None,
            mcp_vault_domain::EventId::new().to_string(),
        ));
    }
    Ok(permissions)
}

fn permission_name(permission: Permission) -> &'static str {
    match permission {
        Permission::DiscoverVault => "vault:discover",
        Permission::ReadVault => "vault:read",
        Permission::WriteVault => "vault:write",
        Permission::DeleteVault => "vault:delete",
        Permission::ReadHistory => "vault:history",
        Permission::ReadMemory => "memory:read",
        Permission::WriteMemory => "memory:write",
        Permission::ManageMemory => "memory:manage",
    }
}

fn webdav_credential_json(record: &WebDavCredentialRecord) -> Value {
    let permissions = serde_json::from_str::<PermissionSet>(&record.permissions_json)
        .map(|values| {
            values
                .iter()
                .map(|value| permission_name(*value))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "id": record.id.to_string(),
        "vault_id": record.vault_id.to_string(),
        "name": record.name,
        "username": record.username,
        "permissions": permissions,
        "created_at": record.created_at,
        "last_used_at": record.last_used_at,
        "expires_at": record.expires_at,
        "revoked_at": record.revoked_at,
        "password": {"configured": true},
    })
}

async fn list_webdav_credentials(
    State(state): State<AdminApiState>,
    query: Query<PageQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let (limit, _offset) = match page_params(&query) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.list_webdav(&context, limit).await {
        Ok(records) => api_ok(
            StatusCode::OK,
            json!({"credentials": records.iter().map(webdav_credential_json).collect::<Vec<_>>() }),
            request_id.0,
        ),
        Err(error) => state_error(error, request_id.0),
    }
}

async fn issue_webdav_credential(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<WebDavCredentialRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let permissions = match webdav_permissions(&input.permissions) {
        Ok(permissions) => permissions,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .auth
        .issue_webdav_credential(
            &context,
            &input.name,
            &input.username,
            &SecretString::new(input.password),
            permissions,
            input.expires_at,
        )
        .await
    {
        Ok(issue) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.webdav_credential.issued",
                    Some("webdav_credential"),
                    Some(&issue.credential_id.to_string()),
                    json!({"one_time_secret": true}),
                )
                .await;
            api_ok(
                StatusCode::CREATED,
                json!({
                    "credential": {
                        "id": issue.credential_id.to_string(),
                        "username": issue.username,
                        "permissions": issue.permissions.iter().map(|value| permission_name(*value)).collect::<Vec<_>>(),
                        "expires_at": issue.expires_at,
                    },
                    "password": issue.password.expose_secret(),
                    "show_once": true,
                }),
                request_id.0,
            )
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct WebDavCredentialPatch {
    name: String,
    permissions: Vec<String>,
    expires_at: Option<i64>,
}

async fn update_webdav_credential(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<WebDavCredentialPatch>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PATCH) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::CredentialId =
        match parse_id(&id, "The WebDAV credential ID is invalid.") {
            Ok(id) => id,
            Err(response) => return response,
        };
    let permissions = match webdav_permissions(&input.permissions) {
        Ok(permissions) => permissions,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .auth
        .update_webdav_credential(&context, id, &input.name, permissions, input.expires_at)
        .await
    {
        Ok(()) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.webdav_credential.updated",
                    Some("webdav_credential"),
                    Some(&id.to_string()),
                    json!({"outcome": "accepted"}),
                )
                .await;
            api_ok(StatusCode::OK, json!({"updated": true}), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn revoke_webdav_credential(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::DELETE) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::CredentialId =
        match parse_id(&id, "The WebDAV credential ID is invalid.") {
            Ok(id) => id,
            Err(response) => return response,
        };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.auth.revoke_webdav_credential(&context, id).await {
        Ok(()) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.webdav_credential.revoked",
                    Some("webdav_credential"),
                    Some(&id.to_string()),
                    json!({"outcome": "accepted"}),
                )
                .await;
            api_ok(StatusCode::OK, json!({"revoked": true}), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct McpTokenRequest {
    name: String,
    scopes: Vec<String>,
    expires_at: Option<i64>,
}

fn mcp_scopes(values: &[String]) -> Result<ScopeSet, Response> {
    let mut scopes = ScopeSet::new();
    for value in values {
        let scope = match value.parse::<Scope>() {
            Ok(scope) => scope,
            Err(_) => {
                return Err(api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "An MCP scope is invalid.",
                    Some(json!({"scopes": "unsupported scope"})),
                    mcp_vault_domain::EventId::new().to_string(),
                ));
            }
        };
        scopes.insert(scope);
    }
    if scopes.iter().next().is_none() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "At least one MCP scope is required.",
            None,
            mcp_vault_domain::EventId::new().to_string(),
        ));
    }
    Ok(scopes)
}

fn mcp_token_json(record: &McpTokenRecord) -> Value {
    let scopes = serde_json::from_str::<Vec<String>>(&record.scopes_json).unwrap_or_default();
    json!({
        "id": record.id.to_string(),
        "vault_id": record.vault_id.to_string(),
        "name": record.name,
        "token_prefix": record.token_prefix,
        "scopes": scopes,
        "created_at": record.created_at,
        "last_used_at": record.last_used_at,
        "expires_at": record.expires_at,
        "revoked_at": record.revoked_at,
    })
}

async fn list_mcp_tokens(
    State(state): State<AdminApiState>,
    query: Query<PageQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let (limit, _offset) = match page_params(&query) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.list_tokens(&context, limit).await {
        Ok(records) => api_ok(
            StatusCode::OK,
            json!({"tokens": records.iter().map(mcp_token_json).collect::<Vec<_>>() }),
            request_id.0,
        ),
        Err(error) => state_error(error, request_id.0),
    }
}

async fn issue_mcp_token(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<McpTokenRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let scopes = match mcp_scopes(&input.scopes) {
        Ok(scopes) => scopes,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .auth
        .issue_pat(&context, &input.name, scopes, input.expires_at)
        .await
    {
        Ok(issue) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.mcp_token.issued",
                    Some("mcp_pat"),
                    Some(&issue.credential_id.to_string()),
                    json!({"one_time_secret": true}),
                )
                .await;
            api_ok(
                StatusCode::CREATED,
                json!({
                    "token": {
                        "id": issue.credential_id.to_string(),
                        "token_prefix": issue.token_prefix,
                        "scopes": issue.scopes.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        "expires_at": issue.expires_at,
                    },
                    "secret": issue.token.expose_secret(),
                    "show_once": true,
                }),
                request_id.0,
            )
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn revoke_mcp_token(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::DELETE) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::CredentialId = match parse_id(&id, "The MCP token ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.auth.revoke_pat(&context, id).await {
        Ok(()) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.mcp_token.revoked",
                    Some("mcp_pat"),
                    Some(&id.to_string()),
                    json!({"outcome": "accepted"}),
                )
                .await;
            api_ok(StatusCode::OK, json!({"revoked": true}), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct LocalOAuthRequest {
    username: String,
    password: String,
    scopes: Vec<String>,
}

fn local_oauth_json(user: &mcp_vault_auth::LocalOAuthUser) -> Value {
    json!({
        "id": user.id.to_string(),
        "vault_id": user.vault_id.to_string(),
        "username": user.username,
        "scopes": user.scopes.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "enabled": user.enabled,
        "password_changed_at": user.password_changed_at,
        "created_at": user.created_at,
        "updated_at": user.updated_at,
    })
}

async fn get_local_oauth(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.auth.local_oauth_user(&context).await {
        Ok(user) => api_ok(
            StatusCode::OK,
            json!({
                "vault_id": context.id().to_string(),
                "configured": user.is_some(),
                "user": user.as_ref().map(local_oauth_json),
            }),
            request_id.0,
        ),
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn put_local_oauth(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<LocalOAuthRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PUT) {
        return auth_error(error, request_id.0);
    }
    let scopes = match mcp_scopes(&input.scopes) {
        Ok(scopes) => scopes,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .auth
        .configure_local_oauth_user(
            &context,
            &input.username,
            &SecretString::new(input.password),
            scopes,
        )
        .await
    {
        Ok(user) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.local_oauth_user.rotated",
                    Some("local_oauth_user"),
                    Some(&user.id.to_string()),
                    json!({"enabled": user.enabled, "all_existing_local_tokens_revoked": true}),
                )
                .await;
            api_ok(StatusCode::OK, local_oauth_json(&user), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn disable_local_oauth(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::DELETE) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.auth.disable_local_oauth_user(&context).await {
        Ok(disabled) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.local_oauth_user.disabled",
                    Some("local_oauth_user"),
                    None,
                    json!({"disabled": disabled, "all_existing_local_tokens_revoked": true}),
                )
                .await;
            api_ok(StatusCode::OK, json!({"disabled": disabled}), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct OAuthRequest {
    name: String,
    issuer_url: String,
    discovery_url: Option<String>,
    audience: String,
    resource: String,
    jwks_cache_json: String,
    enabled: bool,
}

fn oauth_issuer_json(issuer: &mcp_vault_state::OAuthIssuerRecord) -> Value {
    json!({
        "id": issuer.id.to_string(),
        "name": issuer.name,
        "issuer_url": issuer.issuer_url,
        "discovery_url": issuer.discovery_url,
        "audience": issuer.audience,
        "resource": issuer.resource,
        "has_jwks_cache": issuer.jwks_cache_json.is_some(),
        "jwks_cached_at": issuer.jwks_cached_at,
        "enabled": issuer.enabled,
        "created_at": issuer.created_at,
        "updated_at": issuer.updated_at,
    })
}

async fn get_oauth(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state.list_oauth(100).await {
        Ok(issuers) => api_ok(
            StatusCode::OK,
            json!({"issuers": issuers.iter().map(oauth_issuer_json).collect::<Vec<_>>() }),
            request_id.0,
        ),
        Err(error) => state_error(error, request_id.0),
    }
}

async fn put_oauth(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<OAuthRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PUT) {
        return auth_error(error, request_id.0);
    }
    match state
        .auth
        .configure_oauth_issuer(OAuthIssuerInput {
            name: input.name,
            issuer_url: input.issuer_url,
            discovery_url: input.discovery_url,
            audience: input.audience,
            resource: input.resource,
            jwks_cache_json: input.jwks_cache_json,
            enabled: input.enabled,
        })
        .await
    {
        Ok(issuer) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.oauth_issuer.updated",
                    Some("oauth_issuer"),
                    Some(&issuer.id.to_string()),
                    json!({"enabled": issuer.enabled, "jwks_cache_present": issuer.jwks_cache_json.is_some()}),
                )
                .await;
            api_ok(StatusCode::OK, oauth_issuer_json(&issuer), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OAuthGrantQuery {
    #[serde(default)]
    include_revoked: bool,
}

#[derive(Debug, Deserialize)]
struct OAuthGrantRequest {
    issuer_id: String,
    subject: String,
    scopes: Vec<String>,
}

fn oauth_grant_json(grant: &mcp_vault_state::OAuthGrantRecord) -> Value {
    let scopes = serde_json::from_str::<Vec<String>>(&grant.scopes_json).unwrap_or_default();
    json!({
        "id": grant.id.to_string(),
        "issuer_id": grant.issuer_id.to_string(),
        "subject": grant.subject,
        "vault_id": grant.vault_id.to_string(),
        "scopes": scopes,
        "created_at": grant.created_at,
        "updated_at": grant.updated_at,
        "revoked_at": grant.revoked_at,
    })
}

async fn list_oauth_grants(
    State(state): State<AdminApiState>,
    Query(query): Query<OAuthGrantQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .auth
        .list_oauth_grants(&context, query.include_revoked, 1000)
        .await
    {
        Ok(grants) => api_ok(
            StatusCode::OK,
            json!({
                "vault_id": context.id().to_string(),
                "grants": grants.iter().map(oauth_grant_json).collect::<Vec<_>>(),
            }),
            request_id.0,
        ),
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn upsert_oauth_grant(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<OAuthGrantRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let issuer_id: mcp_vault_domain::OAuthIssuerId =
        match parse_id(&input.issuer_id, "The OAuth issuer ID is invalid.") {
            Ok(id) => id,
            Err(response) => return response,
        };
    let scopes = match mcp_scopes(&input.scopes) {
        Ok(scopes) => scopes,
        Err(response) => return response,
    };
    let issuer_exists = match state.list_oauth(1000).await {
        Ok(issuers) => issuers.iter().any(|issuer| issuer.id == issuer_id),
        Err(error) => return state_error(error, request_id.0),
    };
    if !issuer_exists {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The OAuth issuer was not found.",
            None,
            request_id.0,
        );
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .auth
        .grant_oauth_subject(&context, issuer_id, &input.subject, scopes)
        .await
    {
        Ok(grant) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.oauth_grant.updated",
                    Some("oauth_grant"),
                    Some(&grant.id.to_string()),
                    json!({"issuer_id": issuer_id.to_string(), "revoked": false}),
                )
                .await;
            api_ok(StatusCode::CREATED, oauth_grant_json(&grant), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn revoke_oauth_grant(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::DELETE) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::OAuthGrantId = match parse_id(&id, "The OAuth grant ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.auth.revoke_oauth_grant(&context, id).await {
        Ok(()) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.oauth_grant.revoked",
                    Some("oauth_grant"),
                    Some(&id.to_string()),
                    json!({"revoked": true}),
                )
                .await;
            api_ok(StatusCode::OK, json!({"revoked": true}), request_id.0)
        }
        Err(error) => auth_error(error, request_id.0),
    }
}

async fn connection_info(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let origin = advertised_data_origin(&state);
    api_ok(
        StatusCode::OK,
        json!({
            "vault_id": vault.id.to_string(),
            "vault_slug": vault.slug.to_string(),
            "mcp_endpoint": format!("{origin}/mcp/v1/vaults/{}", vault.slug),
            "oauth_protected_resource_metadata_url": format!(
                "{origin}/.well-known/oauth-protected-resource/mcp/v1/vaults/{}",
                vault.slug
            ),
            "oauth_authorization_server_metadata_url": format!(
                "{origin}/.well-known/oauth-authorization-server"
            ),
            "webdav_endpoint": format!("{origin}/dav/v1/vaults/{}/", vault.slug),
            "supported_mcp_revisions": ["2026-07-28"],
            "authorization_modes": ["oauth_builtin", "pat", "oauth_external"],
            "instructions": "Use recall proactively for durable context; verify exact source material with search_notes/read_note.",
        }),
        request_id.0,
    )
}

fn advertised_data_origin(state: &AdminApiState) -> String {
    select_advertised_data_origin(
        state.data_public_origin.as_deref(),
        &state.data_origins,
        &state.data_hosts,
        state.data_bind,
    )
}

fn select_advertised_data_origin(
    public_origin: Option<&str>,
    data_origins: &[String],
    data_hosts: &BTreeSet<String>,
    data_bind: SocketAddr,
) -> String {
    public_origin
        .and_then(canonical_endpoint_origin)
        .or_else(|| {
            data_origins
                .first()
                .and_then(|origin| canonical_endpoint_origin(origin))
        })
        .unwrap_or_else(|| {
            let host = data_hosts
                .iter()
                .next()
                .map(String::as_str)
                .unwrap_or("127.0.0.1");
            format!("http://{}", authority_with_port(host, data_bind.port()))
        })
}

fn canonical_endpoint_origin(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let origin = url.origin().ascii_serialization();
    (origin != "null").then_some(origin)
}

fn authority_with_port(authority: &str, port: u16) -> String {
    if let Ok(address) = authority.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(_) => format!("{address}:{port}"),
            IpAddr::V6(_) => format!("[{address}]:{port}"),
        };
    }
    if authority
        .parse::<axum::http::uri::Authority>()
        .ok()
        .and_then(|value| value.port_u16())
        .is_some()
    {
        authority.to_owned()
    } else {
        format!("{authority}:{port}")
    }
}

#[derive(Debug, Deserialize)]
struct ProviderModeRequest {
    mode: ProviderMode,
    expected_revision: Option<u64>,
}

async fn get_provider_mode(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.providers().provider_mode_state(&context).await {
        Ok(mode) => api_ok(
            StatusCode::OK,
            json!({
                "vault_id": context.id().to_string(),
                "mode": mode.mode,
                "revision": mode.revision.map(Revision::value),
            }),
            request_id.0,
        ),
        Err(error) => provider_error(error, request_id.0),
    }
}

async fn put_provider_mode(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ProviderModeRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PUT) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .providers()
        .set_provider_mode(
            &context,
            input.mode,
            input.expected_revision.map(Revision::new),
        )
        .await
    {
        Ok(setting) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.provider_mode.updated",
                    Some("vault_setting"),
                    Some("provider.mode"),
                    json!({"mode": input.mode, "revision": setting.revision.value()}),
                )
                .await;
            api_ok(
                StatusCode::OK,
                json!({
                    "vault_id": context.id().to_string(),
                    "mode": input.mode,
                    "revision": setting.revision.value(),
                }),
                request_id.0,
            )
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct ProviderRequest {
    name: String,
    provider_type: String,
    base_url: String,
    #[serde(default)]
    settings: ProviderSettings,
    enabled: bool,
    secret: Option<String>,
    expected_revision: Option<i64>,
}

fn provider_kind(value: &str) -> Result<ProviderKind, Response> {
    ProviderKind::try_from(value).map_err(|_| {
        api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The provider type is unsupported.",
            Some(json!({"provider_type": "unsupported provider type"})),
            mcp_vault_domain::EventId::new().to_string(),
        )
    })
}

async fn provider_json(state: &AdminApiState, provider: &ProviderRecord) -> Value {
    let secret = match provider.secret_id {
        Some(id) => state.secret_hint(id).await,
        None => None,
    };
    let health = state.provider_health_for(provider.id).await;
    json!({
        "id": provider.id.to_string(),
        "name": provider.name,
        "provider_type": provider.provider_type,
        "base_url": provider.base_url,
        "settings": provider.settings,
        "enabled": provider.enabled,
        "revision": provider.revision.value(),
        "created_at": provider.created_at,
        "updated_at": provider.updated_at,
        "secret": secret.unwrap_or_else(|| json!({"configured": false})),
        "health": health,
    })
}

async fn list_providers(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let providers = state.providers();
    match providers.list_providers().await {
        Ok(providers) => {
            let mut values = Vec::with_capacity(providers.len());
            for provider in providers {
                values.push(provider_json(&state, &provider).await);
            }
            let mode = match state.providers().provider_mode_state(&context).await {
                Ok(mode) => mode,
                Err(error) => return provider_error(error, request_id.0),
            };
            api_ok(
                StatusCode::OK,
                json!({
                    "providers": values,
                    "provider_mode": {
                        "mode": mode.mode,
                        "revision": mode.revision.map(Revision::value),
                    },
                }),
                request_id.0,
            )
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

async fn create_provider(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ProviderRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let kind = match provider_kind(&input.provider_type) {
        Ok(kind) => kind,
        Err(response) => return response,
    };
    let base_url = match Url::parse(&input.base_url) {
        Ok(url) => url,
        Err(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The provider base URL is invalid.",
                Some(json!({"base_url": "invalid URL"})),
                request_id.0,
            );
        }
    };
    match state
        .providers()
        .create_provider(ProviderInput {
            name: input.name,
            kind,
            base_url,
            settings: input.settings,
            enabled: input.enabled,
            secret: input.secret.map(SecretString::new),
        })
        .await
    {
        Ok(provider) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.provider.created",
                    Some("provider"),
                    Some(&provider.id.to_string()),
                    json!({"enabled": provider.enabled, "secret_configured": provider.secret_id.is_some()}),
                )
                .await;
            api_ok(
                StatusCode::CREATED,
                provider_json(&state, &provider).await,
                request_id.0,
            )
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

async fn get_provider(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let id: mcp_vault_domain::ProviderId = match parse_id(&id, "The provider ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state.providers().get_provider(id).await {
        Ok(provider) => api_ok(
            StatusCode::OK,
            provider_json(&state, &provider).await,
            request_id.0,
        ),
        Err(error) => provider_error(error, request_id.0),
    }
}

async fn update_provider(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ProviderRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PATCH) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::ProviderId = match parse_id(&id, "The provider ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let current = match state.providers().get_provider(id).await {
        Ok(provider) => provider,
        Err(error) => return provider_error(error, request_id.0),
    };
    let expected_revision = match input.expected_revision {
        Some(value) => match Revision::try_from(value) {
            Ok(revision) => Some(revision),
            Err(_) => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "The expected provider revision is invalid.",
                    Some(json!({"expected_revision": "invalid revision"})),
                    request_id.0,
                );
            }
        },
        None => None,
    };
    if let Some(expected_revision) = expected_revision
        && expected_revision != current.revision
    {
        return provider_error(
            ProviderError::State(StateError::InvalidDomain(DomainError::RevisionConflict {
                expected: expected_revision,
                current: current.revision,
            })),
            request_id.0,
        );
    }
    let kind = match provider_kind(&input.provider_type) {
        Ok(kind) => kind,
        Err(response) => return response,
    };
    let base_url = match Url::parse(&input.base_url) {
        Ok(url) => url,
        Err(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The provider base URL is invalid.",
                Some(json!({"base_url": "invalid URL"})),
                request_id.0,
            );
        }
    };
    let secret_rotated = input.secret.is_some();
    match state
        .providers()
        .update_provider(
            current,
            ProviderInput {
                name: input.name,
                kind,
                base_url,
                settings: input.settings,
                enabled: input.enabled,
                secret: input.secret.map(SecretString::new),
            },
        )
        .await
    {
        Ok(provider) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.provider.updated",
                    Some("provider"),
                    Some(&provider.id.to_string()),
                    json!({
                        "enabled": provider.enabled,
                        "secret_configured": provider.secret_id.is_some(),
                        "secret_rotated": secret_rotated,
                    }),
                )
                .await;
            api_ok(
                StatusCode::OK,
                provider_json(&state, &provider).await,
                request_id.0,
            )
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

#[derive(Debug, Default, Deserialize)]
struct DeleteProviderQuery {
    expected_revision: Option<i64>,
}

async fn delete_provider(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DeleteProviderQuery>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::DELETE) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::ProviderId = match parse_id(&id, "The provider ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let expected_revision = match query.expected_revision {
        Some(value) => match Revision::try_from(value) {
            Ok(revision) => Some(revision),
            Err(_) => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "The expected provider revision is invalid.",
                    Some(json!({"expected_revision": "invalid revision"})),
                    request_id.0,
                );
            }
        },
        None => None,
    };
    match state
        .providers()
        .delete_provider(id, expected_revision)
        .await
    {
        Ok(summary) => {
            let result = json!({
                "deleted": true,
                "provider_id": id.to_string(),
                "models_deleted": summary.models_deleted,
                "bindings_deleted": summary.bindings_deleted,
                "embeddings_deleted": summary.embeddings_deleted,
                "secrets_deleted": summary.secrets_deleted,
            });
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.provider.deleted",
                    Some("provider"),
                    Some(&id.to_string()),
                    json!({
                        "models_deleted": summary.models_deleted,
                        "bindings_deleted": summary.bindings_deleted,
                        "embeddings_deleted": summary.embeddings_deleted,
                        "secrets_deleted": summary.secrets_deleted,
                    }),
                )
                .await;
            api_ok(StatusCode::OK, result, request_id.0)
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

async fn test_provider(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::ProviderId = match parse_id(&id, "The provider ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.providers().test_provider(&context, id).await {
        Ok(models) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.provider.tested",
                    Some("provider"),
                    Some(&id.to_string()),
                    json!({"model_count": models.len()}),
                )
                .await;
            api_ok(
                StatusCode::OK,
                json!({"status": "healthy", "models": models.iter().map(model_json).collect::<Vec<_>>() }),
                request_id.0,
            )
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

async fn refresh_models(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    test_provider(
        State(state),
        headers,
        Path(id),
        Extension(principal),
        Extension(request_id),
    )
    .await
}

async fn list_provider_models(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let provider_id: mcp_vault_domain::ProviderId =
        match parse_id(&id, "The provider ID is invalid.") {
            Ok(id) => id,
            Err(response) => return response,
        };
    match state.providers().get_provider(provider_id).await {
        Ok(_) => {}
        Err(error) => return provider_error(error, request_id.0),
    }
    match state.providers().list_models(Some(provider_id)).await {
        Ok(models) => api_ok(
            StatusCode::OK,
            json!({"models": models.iter().map(model_json).collect::<Vec<_>>() }),
            request_id.0,
        ),
        Err(error) => provider_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct ModelRegistrationRequest {
    external_model_id: String,
    #[serde(default)]
    capabilities: ModelCapabilities,
    #[serde(default)]
    settings: ModelSettings,
    enabled: Option<bool>,
}

async fn register_provider_model(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ModelRegistrationRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let provider_id: mcp_vault_domain::ProviderId =
        match parse_id(&id, "The provider ID is invalid.") {
            Ok(id) => id,
            Err(response) => return response,
        };
    let external_model_id = input.external_model_id.trim().to_owned();
    if external_model_id.is_empty() || external_model_id.len() > 512 {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The external model ID is invalid.",
            Some(json!({"external_model_id": "required model identifier"})),
            request_id.0,
        );
    }
    let existing = match state.providers().list_models(Some(provider_id)).await {
        Ok(models) => models,
        Err(error) => return provider_error(error, request_id.0),
    };
    if existing
        .iter()
        .any(|model| model.external_model_id == external_model_id)
    {
        return api_error(
            StatusCode::CONFLICT,
            "model_exists",
            "The provider model is already registered.",
            None,
            request_id.0,
        );
    }
    match state
        .providers()
        .register_model(ModelInput {
            provider_id,
            external_model_id,
            capabilities: input.capabilities,
            settings: input.settings,
            enabled: input.enabled.unwrap_or(true),
        })
        .await
    {
        Ok(model) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.provider_model.created",
                    Some("provider_model"),
                    Some(&model.id.to_string()),
                    json!({"provider_id": provider_id.to_string()}),
                )
                .await;
            api_ok(StatusCode::CREATED, model_json(&model), request_id.0)
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

fn model_json(model: &ModelRecord) -> Value {
    json!({
        "id": model.id.to_string(),
        "provider_id": model.provider_id.to_string(),
        "external_model_id": model.external_model_id,
        "capabilities": model.capabilities,
        "settings": model.settings,
        "enabled": model.enabled,
        "revision": model.revision.value(),
        "created_at": model.created_at,
        "updated_at": model.updated_at,
    })
}

fn binding_json(binding: &ModelBindingRecord) -> Value {
    json!({
        "id": binding.id,
        "vault_id": binding.vault_id.map(|id| id.to_string()),
        "role": binding.role,
        "model_id": binding.model_id.to_string(),
        "settings": binding.settings,
        "revision": binding.revision.value(),
        "updated_at": binding.updated_at,
    })
}

const MODEL_ROLES: [&str; 7] = [
    "memory_extraction",
    "memory_consolidation",
    "note_summary",
    "topic_enrichment",
    "embedding_note",
    "embedding_memory",
    "rerank",
];

async fn list_model_bindings(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let mut bindings = Vec::new();
    for role in MODEL_ROLES {
        match state.providers().get_binding(Some(&context), role).await {
            Ok(Some(binding)) => bindings.push(binding_json(&binding)),
            Ok(None) => match state.providers().get_binding(None, role).await {
                Ok(Some(binding)) => bindings.push(binding_json(&binding)),
                Ok(None) => {}
                Err(error) => return provider_error(error, request_id.0),
            },
            Err(error) => return provider_error(error, request_id.0),
        }
    }
    api_ok(StatusCode::OK, json!({"bindings": bindings}), request_id.0)
}

#[derive(Debug, Deserialize)]
struct ModelBindingRequest {
    model_id: String,
    #[serde(default)]
    settings: Value,
    expected_revision: Option<i64>,
    vault_override: Option<bool>,
}

async fn update_model_binding(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(role): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ModelBindingRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PUT) {
        return auth_error(error, request_id.0);
    }
    if !MODEL_ROLES.contains(&role.as_str()) {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The model role is unsupported.",
            Some(json!({"role": "unsupported role"})),
            request_id.0,
        );
    }
    let model_id: mcp_vault_domain::ModelId =
        match parse_id(&input.model_id, "The model ID is invalid.") {
            Ok(id) => id,
            Err(response) => return response,
        };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let binding_context = input.vault_override.unwrap_or(false).then_some(&context);
    let expected = match input.expected_revision {
        Some(value) => match Revision::try_from(value) {
            Ok(revision) => Some(revision),
            Err(_) => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "The expected binding revision is invalid.",
                    None,
                    request_id.0,
                );
            }
        },
        None => None,
    };
    match state
        .providers()
        .bind_model(binding_context, &role, model_id, input.settings, expected)
        .await
    {
        Ok(binding) => {
            state
                .append_admin_audit(
                    binding_context,
                    &request_id.0,
                    &principal.actor,
                    "admin.model_binding.updated",
                    Some("model_binding"),
                    Some(&binding.id.to_string()),
                    json!({"role": role}),
                )
                .await;
            if matches!(role.as_str(), "memory_extraction" | "memory_consolidation") {
                let targets = if binding.vault_id.is_some() {
                    Ok(vec![context.clone()])
                } else {
                    state.list_vaults().await.map(|vaults| {
                        vaults
                            .into_iter()
                            .filter(|vault| vault.status == VaultStatus::Active)
                            .filter_map(|vault| vault.context().ok())
                            .collect::<Vec<_>>()
                    })
                };
                let targets = match targets {
                    Ok(targets) => targets,
                    Err(error) => return state_error(error, request_id.0),
                };
                for target in targets {
                    if let Err(error) = state
                        .memory()
                        .ensure_pipeline_regeneration(&target, None)
                        .await
                    {
                        return memory_error(error, request_id.0);
                    }
                }
            }
            let mut value = binding_json(&binding);
            if role == "embedding_note"
                && let Some(object) = value.as_object_mut()
            {
                let targets = if binding.vault_id.is_some() {
                    Ok(vec![context.clone()])
                } else {
                    state.list_vaults().await.map(|vaults| {
                        vaults
                            .into_iter()
                            .filter_map(|vault| vault.context().ok())
                            .collect::<Vec<_>>()
                    })
                };
                match targets {
                    Ok(targets) => {
                        let mut source_chunks = 0_u64;
                        let mut queued_chunks = 0_u64;
                        let mut pruned_vectors = 0_u64;
                        let mut jobs = 0_u64;
                        let mut degraded = false;
                        for target in targets {
                            match state.index().schedule_note_embeddings(&target).await {
                                Ok(report) => {
                                    source_chunks =
                                        source_chunks.saturating_add(report.source_chunks);
                                    queued_chunks =
                                        queued_chunks.saturating_add(report.queued_chunks);
                                    pruned_vectors =
                                        pruned_vectors.saturating_add(report.pruned_vectors);
                                    jobs = jobs.saturating_add(report.jobs);
                                }
                                Err(_) => degraded = true,
                            }
                        }
                        object.insert(
                            "embedding_schedule".to_owned(),
                            json!({
                                "source_chunks": source_chunks,
                                "queued_chunks": queued_chunks,
                                "pruned_vectors": pruned_vectors,
                                "jobs": jobs,
                                "degraded": degraded,
                            }),
                        );
                    }
                    Err(_) => {
                        object.insert(
                            "embedding_schedule".to_owned(),
                            json!({"degraded": true, "code": "note_embedding_schedule_failed"}),
                        );
                    }
                }
            }
            api_ok(StatusCode::OK, value, request_id.0)
        }
        Err(error) => provider_error(error, request_id.0),
    }
}

async fn index_status(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let entries = match state.dashboard_files(&context).await {
        Ok(entries) => entries,
        Err(error) => return state_error(error, request_id.0),
    };
    let core = match state.core_for_vault(&vault) {
        Ok(core) => core,
        Err(error) => return state_error(error, request_id.0),
    };
    let total_notes = u64::try_from(
        entries
            .iter()
            .filter(|entry| {
                entry.entry_type.as_str() == "file"
                    && entry.path.as_str().to_ascii_lowercase().ends_with(".md")
                    && !core.is_managed_path(&entry.path)
            })
            .count(),
    )
    .unwrap_or(u64::MAX);
    let note_semantic = match state.index().note_semantic_status(&context).await {
        Ok(status) => json!({
            "configured": status.configured,
            "provider_mode_enabled": status.provider_mode_enabled,
            "model_id": status.model_id,
            "external_model_id": status.external_model_id,
            "source_chunks": status.source_chunks,
            "indexed_chunks": status.indexed_chunks,
            "stale_vectors": status.stale_vectors,
            "coverage_ratio": index_coverage_ratio(status.indexed_chunks, status.source_chunks),
            "blockers": status.blockers,
        }),
        Err(_) => json!({
            "configured": false,
            "source_chunks": 0,
            "indexed_chunks": 0,
            "stale_vectors": 0,
            "coverage_ratio": null,
            "blockers": ["semantic_status_unavailable"],
        }),
    };
    match state.index().status(&context).await {
        Ok(status) => api_ok(
            StatusCode::OK,
            json!({
                "status": status.map(|status| index_status_json(status, total_notes)),
                "note_semantic": note_semantic,
            }),
            request_id.0,
        ),
        Err(error) => index_error(error, request_id.0),
    }
}

async fn rebuild_index(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let dedup = format!(
        "vault:{}:admin-index-rebuild:{}",
        context.id(),
        mcp_vault_domain::EventId::new()
    );
    match state
        .enqueue_vault_job(
            &context,
            "index.rebuild",
            &dedup,
            &json!({"reason": "admin_index_rebuild"}),
            5,
            5,
        )
        .await
    {
        Ok(job) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.index.rebuild_queued",
                    Some("job"),
                    Some(&job.id.to_string()),
                    json!({"job_type": "index.rebuild"}),
                )
                .await;
            api_ok(StatusCode::ACCEPTED, job_summary(&job), request_id.0)
        }
        Err(error) => state_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize, Default)]
struct IndexNodesQuery {
    parent: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn index_nodes(
    State(state): State<AdminApiState>,
    query: Query<IndexNodesQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let page = PageQuery {
        limit: query.limit,
        offset: query.offset,
    };
    let (limit, offset) = match page_params(&page) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .index()
        .list_nodes(&context, query.parent.as_deref(), limit, offset)
        .await
    {
        Ok(nodes) => api_ok(
            StatusCode::OK,
            json!({"nodes": nodes.iter().map(index_node_json).collect::<Vec<_>>(), "next_offset": (nodes.len() == limit as usize).then_some(offset.saturating_add(limit))}),
            request_id.0,
        ),
        Err(error) => index_error(error, request_id.0),
    }
}

fn index_node_json(node: &mcp_vault_state::IndexNodeRecord) -> Value {
    json!({
        "stable_key": node.stable_key,
        "parent_key": node.parent_key,
        "node_type": node.node_type,
        "title": node.title,
        "summary": node.summary,
        "source_type": node.source_type,
        "sort_key": node.sort_key,
        "member_count": node.member_count,
    })
}

#[derive(Debug, Deserialize, Default)]
struct MemoryListQuery {
    statuses: Option<String>,
    types: Option<String>,
    tag: Option<String>,
    entity: Option<String>,
    source_path: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

fn parse_memory_statuses(value: Option<&str>) -> Result<Vec<MemoryStatus>, Response> {
    value
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| match value.trim() {
            "candidate" => Ok(MemoryStatus::Candidate),
            "active" => Ok(MemoryStatus::Active),
            "superseded" => Ok(MemoryStatus::Superseded),
            "stale" => Ok(MemoryStatus::Stale),
            "archived" => Ok(MemoryStatus::Archived),
            "rejected" => Ok(MemoryStatus::Rejected),
            "quarantined" => Ok(MemoryStatus::Quarantined),
            _ => Err(api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "A memory status is invalid.",
                None,
                mcp_vault_domain::EventId::new().to_string(),
            )),
        })
        .collect()
}

fn parse_memory_types(value: Option<&str>) -> Result<Vec<MemoryType>, Response> {
    value
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            MemoryType::try_from(value.trim()).map_err(|_| {
                api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "A memory type is invalid.",
                    None,
                    mcp_vault_domain::EventId::new().to_string(),
                )
            })
        })
        .collect()
}

async fn list_memories(
    State(state): State<AdminApiState>,
    query: Query<MemoryListQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let page = PageQuery {
        limit: query.limit,
        offset: query.offset,
    };
    let (limit, offset) = match page_params(&page) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let statuses = match parse_memory_statuses(query.statuses.as_deref()) {
        Ok(statuses) => statuses,
        Err(response) => return response,
    };
    let types = match parse_memory_types(query.types.as_deref()) {
        Ok(types) => types,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .memory()
        .list(
            &context,
            statuses,
            types,
            query.tag.clone(),
            query.entity.clone(),
            query.source_path.clone(),
            limit,
            offset,
        )
        .await
    {
        Ok(memories) => api_ok(
            StatusCode::OK,
            json!({"memories": memories, "next_offset": (memories.len() == limit as usize).then_some(offset.saturating_add(limit))}),
            request_id.0,
        ),
        Err(error) => memory_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct MemoryMergeRequest {
    source_id: String,
    target_id: String,
    expected_target_revision: i64,
}

async fn merge_memories(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<MemoryMergeRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let source_id: MemoryId = match parse_id(&input.source_id, "The source memory ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let target_id: MemoryId = match parse_id(&input.target_id, "The target memory ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let expected = match Revision::try_from(input.expected_target_revision) {
        Ok(revision) => revision,
        Err(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The expected target revision is invalid.",
                None,
                request_id.0,
            );
        }
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let core = match state.core_for_vault(&vault) {
        Ok(core) => core,
        Err(error) => return state_error(error, request_id.0),
    };
    match state
        .memory()
        .merge(&context, &core, source_id, target_id, expected)
        .await
    {
        Ok(memory) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.memory.merged",
                    Some("memory"),
                    Some(&target_id.to_string()),
                    json!({"source_memory_id": source_id.to_string()}),
                )
                .await;
            api_ok(StatusCode::OK, memory, request_id.0)
        }
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn get_memory(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let id: MemoryId = match parse_id(&id, "The memory ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.memory().get(&context, id).await {
        Ok(memory) => api_ok(StatusCode::OK, memory, request_id.0),
        Err(error) => memory_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize, Default)]
struct MemoryPatchRequest {
    content: Option<String>,
    memory_type: Option<String>,
    importance: Option<f64>,
    confidence: Option<f64>,
    valid_from: Option<Option<i64>>,
    valid_to: Option<Option<i64>>,
    tags: Option<Vec<String>>,
    entities: Option<Vec<String>>,
    expected_revision: i64,
}

async fn update_memory(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<MemoryPatchRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PATCH) {
        return auth_error(error, request_id.0);
    }
    let id: MemoryId = match parse_id(&id, "The memory ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let memory_type = match input.memory_type.as_deref() {
        Some(value) => match MemoryType::try_from(value) {
            Ok(value) => Some(value),
            Err(_) => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "The memory type is invalid.",
                    None,
                    request_id.0,
                );
            }
        },
        None => None,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let core = match state.core_for_vault(&vault) {
        Ok(core) => core,
        Err(error) => return state_error(error, request_id.0),
    };
    let revision = match Revision::try_from(input.expected_revision) {
        Ok(revision) => revision,
        Err(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The expected memory revision is invalid.",
                None,
                request_id.0,
            );
        }
    };
    match state
        .memory()
        .update(
            &context,
            &core,
            id,
            revision,
            MemoryUpdateInput {
                content: input.content,
                memory_type,
                importance: input.importance,
                confidence: input.confidence,
                valid_from: input.valid_from,
                valid_to: input.valid_to,
                tags: input.tags,
                entities: input.entities,
            },
        )
        .await
    {
        Ok(memory) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.memory.updated",
                    Some("memory"),
                    Some(&id.to_string()),
                    json!({"revision": memory.revision.value()}),
                )
                .await;
            api_ok(StatusCode::OK, memory, request_id.0)
        }
        Err(error) => memory_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct MemoryLifecycleRequest {
    expected_revision: i64,
}

async fn archive_memory(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<MemoryLifecycleRequest>,
) -> Response {
    lifecycle_memory(
        state,
        headers,
        principal.actor,
        id,
        input.expected_revision,
        false,
        request_id,
    )
    .await
}

#[derive(Debug, Deserialize)]
struct DeleteMemoryQuery {
    expected_revision: i64,
}

async fn delete_memory(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DeleteMemoryQuery>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    lifecycle_memory(
        state,
        headers,
        principal.actor,
        id,
        query.expected_revision,
        true,
        request_id,
    )
    .await
}

async fn restore_memory(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<MemoryLifecycleRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let id: MemoryId = match parse_id(&id, "The memory ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let revision = match Revision::try_from(input.expected_revision) {
        Ok(revision) => revision,
        Err(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The expected memory revision is invalid.",
                None,
                request_id.0,
            );
        }
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let core = match state.core_for_vault(&vault) {
        Ok(core) => core,
        Err(error) => return state_error(error, request_id.0),
    };
    match state.memory().restore(&context, &core, id, revision).await {
        Ok(memory) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.memory.restored",
                    Some("memory"),
                    Some(&id.to_string()),
                    json!({"revision": memory.revision.value()}),
                )
                .await;
            api_ok(StatusCode::OK, memory, request_id.0)
        }
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn lifecycle_memory(
    state: AdminApiState,
    headers: HeaderMap,
    actor: Actor,
    id: String,
    expected_revision: i64,
    permanent: bool,
    request_id: RequestId,
) -> Response {
    let method = if permanent {
        Method::DELETE
    } else {
        Method::POST
    };
    if let Err(error) = validate_state_change_origin(&state, &headers, &method) {
        return auth_error(error, request_id.0);
    }
    let id: MemoryId = match parse_id(&id, "The memory ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let revision = match Revision::try_from(expected_revision) {
        Ok(revision) => revision,
        Err(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The expected memory revision is invalid.",
                None,
                request_id.0,
            );
        }
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let core = match state.core_for_vault(&vault) {
        Ok(core) => core,
        Err(error) => return state_error(error, request_id.0),
    };
    match state
        .memory()
        .forget(&context, &core, id, revision, permanent)
        .await
    {
        Ok(memory) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &actor,
                    if permanent {
                        "admin.memory.deleted"
                    } else {
                        "admin.memory.archived"
                    },
                    Some("memory"),
                    Some(&id.to_string()),
                    json!({"permanent": permanent, "revision": memory.revision.value()}),
                )
                .await;
            api_ok(StatusCode::OK, memory, request_id.0)
        }
        Err(error) => memory_error(error, request_id.0),
    }
}

#[derive(Debug, Default, Deserialize)]
struct MemorySourceHealthQuery {
    health: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn get_memory_source_health(
    State(state): State<AdminApiState>,
    Query(query): Query<MemorySourceHealthQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let (limit, offset) = match page_params(&PageQuery {
        limit: query.limit,
        offset: query.offset,
    }) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let health = match query.health.as_deref() {
        Some(value) => match MemorySourceHealthState::try_from(value) {
            Ok(health) => Some(health),
            Err(_) => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "The source health filter is invalid.",
                    None,
                    request_id.0,
                );
            }
        },
        None => None,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let counts = match state.memory_source_health_counts(&context).await {
        Ok(counts) => counts,
        Err(error) => return state_error(error, request_id.0),
    };
    let details = match state
        .memory_source_health_details(&context, health, limit, offset)
        .await
    {
        Ok(details) => details,
        Err(error) => return state_error(error, request_id.0),
    };
    let audit = match state.memory_source_audit(&context).await {
        Ok(audit) => audit,
        Err(error) => return state_error(error, request_id.0),
    };
    let sources = details
        .into_iter()
        .map(|detail| {
            let health = detail.health.as_ref();
            json!({
                "source_id": detail.source.id.to_string(),
                "memory_id": detail.source.memory_id.to_string(),
                "recorded_file_id": detail.source.note_file_id.map(|id| id.to_string()),
                "recorded_path": detail.source.note_path.as_ref().map(|path| path.as_str()),
                "evidence_revision": detail.source.note_revision.map(Revision::value),
                "heading": detail.source.heading_path,
                "start_line": detail.source.start_line,
                "end_line": detail.source.end_line,
                "health": health.map(|health| health.state.as_str()).unwrap_or("unverified"),
                "health_reason": health.and_then(|health| health.reason.as_deref()),
                "resolved_file_id": health.and_then(|health| health.resolved_file_id).map(|id| id.to_string()),
                "current_path": health.and_then(|health| health.resolved_path.as_ref()).map(|path| path.as_str()),
                "checked_revision": health.and_then(|health| health.checked_revision).map(Revision::value),
                "checked_at": health.and_then(|health| health.checked_at),
            })
        })
        .collect::<Vec<_>>();
    let next_offset = (sources.len() == limit as usize).then_some(offset.saturating_add(limit));
    api_ok(
        StatusCode::OK,
        json!({
            "summary": {
                "final_sources": {
                    "total": counts.final_sources,
                    "current": counts.current,
                    "rebound": counts.rebound,
                    "changed": counts.changed,
                    "deleted": counts.deleted,
                    "missing": counts.missing,
                    "ambiguous": counts.ambiguous,
                    "unverified": counts.unverified,
                },
                "memories": {
                    "affected": counts.memories,
                    "active": counts.active_memories,
                    "stale": counts.stale_memories,
                },
                "stage1": {
                    "total": counts.stage1_sources,
                    "current": counts.stage1_current,
                    "withdrawn": counts.stage1_withdrawn,
                    "orphaned": counts.stage1_orphaned,
                },
                "distinct_file_ids": counts.distinct_file_ids,
            },
            "audit": audit.map(|audit| json!({
                "generation": audit.generation,
                "status": audit.status,
                "cursor_source_id": audit.cursor_source_id.map(|id| id.to_string()),
                "counters": audit.counters,
                "started_at": audit.started_at,
                "updated_at": audit.updated_at,
                "completed_at": audit.completed_at,
            })),
            "sources": sources,
            "next_offset": next_offset,
        }),
        request_id.0,
    )
}

async fn run_memory_source_audit(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let generation = mcp_vault_domain::EventId::new().to_string();
    match state
        .enqueue_memory_source_audit(&context, &generation)
        .await
    {
        Ok(job) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.memory_source_audit.queued",
                    Some("job"),
                    Some(&job.id.to_string()),
                    json!({"generation": generation}),
                )
                .await;
            api_ok(
                StatusCode::ACCEPTED,
                job_admission_summary(&job, "queued"),
                request_id.0,
            )
        }
        Err(error) => state_error(error, request_id.0),
    }
}

async fn memory_extraction_json(
    state: &AdminApiState,
    context: &VaultContext,
) -> Result<Value, mcp_vault_memory::MemoryError> {
    let policy = state.memory().extraction_policy(context).await?;
    let phase1_readiness = state.memory().extraction_readiness(context).await?;
    let phase2_readiness = state.memory().consolidation_readiness(context).await?;
    let stage1 = state.state.memory().stage1_counts(context).await?;
    let consolidation = state
        .state
        .memory()
        .get_consolidation_state(context)
        .await?;
    let mut blockers = phase1_readiness.blockers.clone();
    blockers.extend(phase2_readiness.blockers.clone());
    let regeneration_pending = consolidation
        .as_ref()
        .is_some_and(|value| value.regeneration_pending);
    if regeneration_pending {
        blockers.push("memory_pipeline_regeneration_pending".to_owned());
    }
    blockers.sort();
    blockers.dedup();
    Ok(json!({
        "policy": policy.policy,
        "revision": policy.revision.map(Revision::value),
        "readiness": {
            "ready": phase1_readiness.ready && phase2_readiness.ready && !regeneration_pending,
            "blockers": blockers,
        },
        "phase1_readiness": phase1_readiness,
        "phase2_readiness": phase2_readiness,
        "stage1": {
            "total": stage1.total,
            "ready": stage1.ready,
            "no_output": stage1.no_output,
            "withdrawn": stage1.withdrawn,
            "pending": stage1.pending,
        },
        "consolidation": consolidation.as_ref().map(|value| json!({
            "generation": value.generation,
            "last_success_at": value.last_success_at,
            "pipeline_generation": value.pipeline_generation,
            "regeneration_pending": value.regeneration_pending,
            "memory_summary_present": !value.memory_summary.is_empty(),
        })).unwrap_or_else(|| json!({
            "generation": 0,
            "last_success_at": null,
            "pipeline_generation": 0,
            "regeneration_pending": false,
            "memory_summary_present": false,
        })),
    }))
}

async fn get_memory_extraction(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match memory_extraction_json(&state, &context).await {
        Ok(value) => api_ok(StatusCode::OK, value, request_id.0),
        Err(error) => memory_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct ExtractionPolicyRequest {
    enabled: bool,
    source_mode: Option<ExtractionSourceMode>,
    #[serde(alias = "max_candidates_per_note")]
    max_evidence_per_note: Option<u32>,
    request_timeout_seconds: Option<u64>,
    expected_revision: Option<i64>,
}

async fn put_memory_extraction(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ExtractionPolicyRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::PUT) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let expected_revision = match input.expected_revision {
        Some(value) => match Revision::try_from(value) {
            Ok(revision) => Some(revision),
            Err(_) => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation_failed",
                    "The extraction policy revision is invalid.",
                    None,
                    request_id.0,
                );
            }
        },
        None => None,
    };
    let policy = ExtractionPolicy {
        enabled: input.enabled,
        source_mode: input.source_mode.unwrap_or_default(),
        max_evidence_per_note: input.max_evidence_per_note.unwrap_or(3),
        request_timeout_seconds: input.request_timeout_seconds.unwrap_or(300),
    };
    match state
        .memory()
        .set_extraction_policy(
            &context,
            policy,
            expected_revision,
            principal.actor.actor_id(),
        )
        .await
    {
        Ok(_) => {
            if let Err(error) = state
                .memory()
                .ensure_pipeline_regeneration(&context, None)
                .await
            {
                return memory_error(error, request_id.0);
            }
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.memory_extraction.updated",
                    Some("memory_extraction_policy"),
                    Some(&context.id().to_string()),
                    json!({"outcome": "accepted"}),
                )
                .await;
            match memory_extraction_json(&state, &context).await {
                Ok(value) => api_ok(StatusCode::OK, value, request_id.0),
                Err(error) => memory_error(error, request_id.0),
            }
        }
        Err(error) => memory_error(error, request_id.0),
    }
}

#[derive(Debug, Default, Deserialize)]
struct RunMemoryExtractionRequest {
    #[serde(default)]
    include_evaluated: bool,
}

async fn run_memory_extraction(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    input: Option<Json<RunMemoryExtractionRequest>>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let readiness = match state.memory().extraction_readiness(&context).await {
        Ok(readiness) => readiness,
        Err(error) => return memory_error(error, request_id.0),
    };
    let consolidation_readiness = match state.memory().consolidation_readiness(&context).await {
        Ok(readiness) => readiness,
        Err(error) => return memory_error(error, request_id.0),
    };
    if !readiness.ready || !consolidation_readiness.ready {
        return api_error(
            StatusCode::CONFLICT,
            "memory_extraction_not_ready",
            "The two-phase memory pipeline is not ready.",
            Some(json!({
                "phase1_blockers": readiness.blockers,
                "phase2_blockers": consolidation_readiness.blockers,
            })),
            request_id.0,
        );
    }
    let regeneration_admission = match state
        .memory()
        .ensure_pipeline_regeneration(&context, None)
        .await
    {
        Ok(admission) => admission,
        Err(error) => return memory_error(error, request_id.0),
    };
    match state.active_job_for(&context, "memory.extract").await {
        Ok(Some(job)) => {
            return api_ok(
                StatusCode::ACCEPTED,
                job_admission_summary(
                    &job,
                    if regeneration_admission == PipelineRegenerationAdmission::Admitted {
                        "queued"
                    } else {
                        "existing"
                    },
                ),
                request_id.0,
            );
        }
        Ok(None) => {}
        Err(error) => return state_error(error, request_id.0),
    }
    let dedup = format!(
        "vault:{}:admin-memory-extract:{}",
        context.id(),
        mcp_vault_domain::EventId::new()
    );
    let include_evaluated = input.is_some_and(|Json(input)| input.include_evaluated);
    match state
        .enqueue_vault_job(
            &context,
            "memory.extract",
            &dedup,
            &json!({
                "pipeline_generation": mcp_vault_memory::MEMORY_PIPELINE_GENERATION,
                "pipeline_version": mcp_vault_memory::EXTRACTION_PIPELINE_VERSION,
                "scope": "all",
                "reason": "admin_backfill",
                "include_evaluated": include_evaluated,
            }),
            4,
            5,
        )
        .await
    {
        Ok(job) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.memory_extraction.queued",
                    Some("job"),
                    Some(&job.id.to_string()),
                    json!({
                        "job_type": "memory.extract",
                        "scope": "all",
                        "include_evaluated": include_evaluated,
                    }),
                )
                .await;
            api_ok(
                StatusCode::ACCEPTED,
                job_admission_summary(&job, "queued"),
                request_id.0,
            )
        }
        Err(error) => state_error(error, request_id.0),
    }
}

async fn restart_memory_extraction(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.memory_regeneration_pending(&context).await {
        Ok(true) => {
            return api_error(
                StatusCode::CONFLICT,
                "memory_pipeline_regeneration_pending",
                "The memory pipeline is admitting its required fresh regeneration.",
                None,
                request_id.0,
            );
        }
        Ok(false) => {}
        Err(error) => return state_error(error, request_id.0),
    }
    let readiness = match state.memory().extraction_readiness(&context).await {
        Ok(readiness) => readiness,
        Err(error) => return memory_error(error, request_id.0),
    };
    let consolidation_readiness = match state.memory().consolidation_readiness(&context).await {
        Ok(readiness) => readiness,
        Err(error) => return memory_error(error, request_id.0),
    };
    if !readiness.ready || !consolidation_readiness.ready {
        return api_error(
            StatusCode::CONFLICT,
            "memory_extraction_not_ready",
            "The two-phase memory pipeline is not ready.",
            Some(json!({
                "phase1_blockers": readiness.blockers,
                "phase2_blockers": consolidation_readiness.blockers,
            })),
            request_id.0,
        );
    }
    let cancel_requested = match state
        .state
        .jobs()
        .request_cancel_type(&context, "memory.extract")
        .await
    {
        Ok(count) => count,
        Err(error) => return state_error(error, request_id.0),
    };
    let dedup = format!(
        "vault:{}:admin-memory-extract-restart:{}",
        context.id(),
        mcp_vault_domain::EventId::new()
    );
    let job = match state
        .enqueue_vault_job(
            &context,
            "memory.extract",
            &dedup,
            &json!({
                "pipeline_generation": mcp_vault_memory::MEMORY_PIPELINE_GENERATION,
                "scope": "all",
                "reason": "admin_restart",
                "pipeline_version": mcp_vault_memory::EXTRACTION_PIPELINE_VERSION,
                "include_evaluated": true,
            }),
            4,
            5,
        )
        .await
    {
        Ok(job) => job,
        Err(error) => return state_error(error, request_id.0),
    };
    state
        .append_admin_audit(
            Some(&context),
            &request_id.0,
            &principal.actor,
            "admin.memory_extraction.restarted",
            Some("job"),
            Some(&job.id.to_string()),
            json!({
                "cancel_requested": cancel_requested,
                "pipeline_version": mcp_vault_memory::EXTRACTION_PIPELINE_VERSION,
            }),
        )
        .await;
    api_ok(
        StatusCode::ACCEPTED,
        json!({
            "cancel_requested": cancel_requested,
            "job": job_admission_summary(&job, "queued"),
        }),
        request_id.0,
    )
}

#[derive(Debug, Deserialize, Default)]
struct JobQuery {
    status: Option<String>,
    job_type: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

fn parse_job_status(value: Option<&str>) -> Result<Option<JobStatus>, Response> {
    match value {
        None => Ok(None),
        Some("queued") => Ok(Some(JobStatus::Queued)),
        Some("running") => Ok(Some(JobStatus::Running)),
        Some("retry_wait") => Ok(Some(JobStatus::RetryWait)),
        Some("completed") => Ok(Some(JobStatus::Completed)),
        Some("failed") => Ok(Some(JobStatus::Failed)),
        Some("cancelled") => Ok(Some(JobStatus::Cancelled)),
        Some(_) => Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The job status is invalid.",
            None,
            mcp_vault_domain::EventId::new().to_string(),
        )),
    }
}

async fn list_jobs(
    State(state): State<AdminApiState>,
    query: Query<JobQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let page = PageQuery {
        limit: query.limit,
        offset: query.offset,
    };
    let (limit, offset) = match page_params(&page) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let status = match parse_job_status(query.status.as_deref()) {
        Ok(status) => status,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .list_jobs_for(&context, status, query.job_type.as_deref(), limit, offset)
        .await
    {
        Ok(jobs) => api_ok(
            StatusCode::OK,
            json!({"jobs": jobs.iter().map(job_summary).collect::<Vec<_>>(), "next_offset": (jobs.len() == limit as usize).then_some(offset.saturating_add(limit))}),
            request_id.0,
        ),
        Err(error) => state_error(error, request_id.0),
    }
}

async fn jobs_overview(
    State(state): State<AdminApiState>,
    query: Query<PageQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let (limit, offset) = match page_params(&query) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    let (running, queued, retry_wait, history, counts) = match tokio::try_join!(
        state.list_jobs_for(&context, Some(JobStatus::Running), None, MAX_PAGE_SIZE, 0),
        state.list_jobs_for(&context, Some(JobStatus::Queued), None, limit, 0),
        state.list_jobs_for(&context, Some(JobStatus::RetryWait), None, limit, 0),
        state.list_terminal_jobs_for(&context, limit, offset),
        state.job_status_counts_for(&context),
    ) {
        Ok(result) => result,
        Err(error) => return state_error(error, request_id.0),
    };
    let running_len = u64::try_from(running.len()).unwrap_or(u64::MAX);
    let queued_len = u64::try_from(queued.len()).unwrap_or(u64::MAX);
    let retry_wait_len = u64::try_from(retry_wait.len()).unwrap_or(u64::MAX);
    api_ok(
        StatusCode::OK,
        json!({
            "running": running.iter().map(job_summary).collect::<Vec<_>>(),
            "queued": queued.iter().map(job_summary).collect::<Vec<_>>(),
            "retry_wait": retry_wait.iter().map(job_summary).collect::<Vec<_>>(),
            "history": history.iter().map(job_summary).collect::<Vec<_>>(),
            "counts": {
                "queued": counts.queued,
                "running": counts.running,
                "retry_wait": counts.retry_wait,
                "completed": counts.completed,
                "failed": counts.failed,
                "cancelled": counts.cancelled,
                "active": counts.queued.saturating_add(counts.running).saturating_add(counts.retry_wait),
                "terminal": counts.completed.saturating_add(counts.failed).saturating_add(counts.cancelled),
            },
            "truncated": {
                "running": counts.running > running_len,
                "queued": counts.queued > queued_len,
                "retry_wait": counts.retry_wait > retry_wait_len,
            },
            "next_history_offset": (history.len() == limit as usize)
                .then_some(offset.saturating_add(limit)),
        }),
        request_id.0,
    )
}

async fn get_job(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let id: mcp_vault_domain::JobId = match parse_id(&id, "The job ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.get_job_for(&context, id).await {
        Ok(Some(job)) => api_ok(StatusCode::OK, job_summary(&job), request_id.0),
        Ok(None) => api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The job was not found.",
            None,
            request_id.0,
        ),
        Err(error) => state_error(error, request_id.0),
    }
}

async fn retry_job(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::JobId = match parse_id(&id, "The job ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.retry_job_for(&context, id).await {
        Ok(()) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.job.retry_requested",
                    Some("job"),
                    Some(&id.to_string()),
                    json!({"outcome": "queued"}),
                )
                .await;
            api_ok(StatusCode::OK, json!({"queued": true}), request_id.0)
        }
        Err(error) => state_error(error, request_id.0),
    }
}

async fn cancel_job(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let id: mcp_vault_domain::JobId = match parse_id(&id, "The job ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state.cancel_job_for(&context, id).await {
        Ok(()) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.job.cancel_requested",
                    Some("job"),
                    Some(&id.to_string()),
                    json!({"outcome": "cancel_requested"}),
                )
                .await;
            api_ok(
                StatusCode::OK,
                json!({"cancel_requested": true}),
                request_id.0,
            )
        }
        Err(error) => state_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AuditQuery {
    action: Option<String>,
    result: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

fn audit_json(record: &AuditRecord) -> Value {
    json!({
        "id": record.id.to_string(),
        "occurred_at": record.occurred_at,
        "request_id": record.request_id,
        "vault_id": record.vault_id.map(|id| id.to_string()),
        "plane": record.plane,
        "actor_type": record.actor_type,
        "actor_id": record.actor_id,
        "action": record.action,
        "target_type": record.target_type,
        "target_id": record.target_id,
        "target_path_hash": record.target_path_hash,
        "result": record.result,
        "metadata": record.metadata,
    })
}

async fn list_audit(
    State(state): State<AdminApiState>,
    query: Query<AuditQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let page = PageQuery {
        limit: query.limit,
        offset: query.offset,
    };
    let (limit, offset) = match page_params(&page) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let vault = match current_vault(&state, &request_id.0).await {
        Ok(vault) => vault,
        Err(response) => return response,
    };
    let context = match vault.context() {
        Ok(context) => context,
        Err(_) => {
            return state_error(
                StateError::InvalidInput("Vault context is invalid"),
                request_id.0,
            );
        }
    };
    match state
        .audit_for(
            &context,
            query.action.as_deref(),
            query.result.as_deref(),
            limit,
            offset,
        )
        .await
    {
        Ok(records) => api_ok(
            StatusCode::OK,
            json!({"entries": records.iter().map(audit_json).collect::<Vec<_>>(), "next_offset": (records.len() == limit as usize).then_some(offset.saturating_add(limit))}),
            request_id.0,
        ),
        Err(error) => state_error(error, request_id.0),
    }
}

fn backup_json(record: &BackupRecord) -> Value {
    json!({
        "id": record.id.to_string(),
        "status": record.status.to_string(),
        "location": record.location,
        "manifest": record.manifest.as_ref().map(backup_manifest_summary),
        "started_at": record.started_at,
        "completed_at": record.completed_at,
        "verified_at": record.verified_at,
        "error": record.error,
        "created_by": record.created_by,
    })
}

fn backup_manifest_summary(manifest: &Value) -> Value {
    json!({
        "format_version": manifest.get("format_version"),
        "service_version": manifest.get("service_version"),
        "schema_version": manifest.get("schema_version"),
        "key_version_ids": manifest.get("key_version_ids"),
        "created_at": manifest.get("created_at"),
        "completed_at": manifest.get("completed_at"),
        "file_count": manifest.get("file_count"),
        "total_bytes": manifest.get("total_bytes"),
        "vault_count": manifest
            .get("vaults")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    })
}

async fn list_backups(
    State(state): State<AdminApiState>,
    query: Query<PageQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let (limit, offset) = match page_params(&query) {
        Ok(page) => page,
        Err(response) => return response,
    };
    match state.backup.list(limit, offset).await {
        Ok(records) => api_ok(
            StatusCode::OK,
            json!({
                "backups": records.iter().map(backup_json).collect::<Vec<_>>(),
                "next_offset": (records.len() == limit as usize)
                    .then_some(offset.saturating_add(limit)),
            }),
            request_id.0,
        ),
        Err(error) => backup_error(error, request_id.0),
    }
}

async fn create_backup(
    State(state): State<AdminApiState>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state
        .backup
        .enqueue_create(Some(&principal.user_id.to_string()))
        .await
    {
        Ok(operation) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.backup.create_queued",
                    Some("backup"),
                    Some(&operation.backup.id.to_string()),
                    json!({"job_id": operation.job.id.to_string()}),
                )
                .await;
            api_ok(
                StatusCode::ACCEPTED,
                json!({"backup": backup_json(&operation.backup), "job": job_summary(&operation.job)}),
                request_id.0,
            )
        }
        Err(error) => backup_error(error, request_id.0),
    }
}

async fn verify_backup(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let id: BackupId = match parse_id(&id, "The backup ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state.backup.enqueue_verify(id).await {
        Ok(job) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.backup.verify_queued",
                    Some("backup"),
                    Some(&id.to_string()),
                    json!({"job_id": job.id.to_string()}),
                )
                .await;
            api_ok(StatusCode::ACCEPTED, job_summary(&job), request_id.0)
        }
        Err(error) => backup_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct RestoreValidateRequest {
    backup_id: String,
}

async fn validate_restore(
    State(state): State<AdminApiState>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<RestoreValidateRequest>,
) -> Response {
    let id: BackupId = match parse_id(&input.backup_id, "The backup ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state.backup.validate_restore(id).await {
        Ok(preview) => api_ok(StatusCode::OK, preview, request_id.0),
        Err(error) => backup_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct RestoreRequest {
    backup_id: String,
    confirmation: String,
    password: String,
}

async fn restore(
    State(state): State<AdminApiState>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<RestoreRequest>,
) -> Response {
    if input.confirmation != "RESTORE" {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "confirmation_required",
            "Type RESTORE to confirm the maintenance operation.",
            None,
            request_id.0,
        );
    }
    if let Err(error) = state
        .auth
        .reauthenticate_admin(principal.user_id, &SecretString::new(input.password))
        .await
    {
        return auth_error(error, request_id.0);
    }
    let id: BackupId = match parse_id(&input.backup_id, "The backup ID is invalid.") {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state.backup.enqueue_restore(id, &request_id.0).await {
        Ok(job) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.restore.queued",
                    Some("backup"),
                    Some(&id.to_string()),
                    json!({"confirmation": "RESTORE", "reauthenticated": true}),
                )
                .await;
            api_ok(StatusCode::ACCEPTED, job_summary(&job), request_id.0)
        }
        Err(error) => backup_error(error, request_id.0),
    }
}

#[derive(Debug, Deserialize)]
struct MaintenanceRecoverRequest {
    confirmation: String,
    password: String,
}

async fn recover_maintenance(
    State(state): State<AdminApiState>,
    Extension(principal): Extension<AdminPrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<MaintenanceRecoverRequest>,
) -> Response {
    if input.confirmation != "RECOVER" {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "confirmation_required",
            "Type RECOVER to confirm maintenance recovery.",
            None,
            request_id.0,
        );
    }
    if let Err(error) = state
        .auth
        .reauthenticate_admin(principal.user_id, &SecretString::new(input.password))
        .await
    {
        return auth_error(error, request_id.0);
    }
    match state.backup.recover_maintenance().await {
        Ok(()) => {
            state
                .append_admin_audit(
                    None,
                    &request_id.0,
                    &principal.actor,
                    "admin.maintenance.recovered",
                    None,
                    None,
                    json!({"confirmation": "RECOVER", "reauthenticated": true}),
                )
                .await;
            api_ok(
                StatusCode::OK,
                json!({"maintenance": "normal", "ready": true}),
                request_id.0,
            )
        }
        Err(error) => backup_error(error, request_id.0),
    }
}

fn backup_error(error: BackupError, request_id: String) -> Response {
    let (status, code, message) = match error {
        BackupError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "The backup was not found.",
        ),
        BackupError::TargetMismatch => (
            StatusCode::CONFLICT,
            "restore_target_mismatch",
            "The backup does not match the configured Vault targets.",
        ),
        BackupError::KeyVersionMismatch => (
            StatusCode::CONFLICT,
            "key_version_mismatch",
            "The backup requires an unavailable encryption-key version.",
        ),
        BackupError::Maintenance => (
            StatusCode::SERVICE_UNAVAILABLE,
            "maintenance",
            "The backup operation is unavailable during maintenance.",
        ),
        BackupError::Archive(_) | BackupError::Json(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "archive_invalid",
            "The backup archive or manifest is invalid.",
        ),
        BackupError::Limit(_) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "resource_limit",
            "The backup exceeds the configured resource limits.",
        ),
        BackupError::InconsistentSource => (
            StatusCode::CONFLICT,
            "source_changed",
            "The Vault changed while the backup was being created.",
        ),
        BackupError::Domain(_)
        | BackupError::State(_)
        | BackupError::Storage(_)
        | BackupError::Core(_)
        | BackupError::Io(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "backup_unavailable",
            "The backup service is temporarily unavailable.",
        ),
    };
    api_error(status, code, message, None, request_id)
}

fn provider_error(error: ProviderError, request_id: String) -> Response {
    let (status, code, message) = match error {
        ProviderError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "The provider was not found.",
        ),
        ProviderError::Disabled => (
            StatusCode::CONFLICT,
            "provider_disabled",
            "The provider is disabled.",
        ),
        ProviderError::State(StateError::InvalidDomain(
            DomainError::RevisionConflict { .. } | DomainError::PreconditionFailed { .. },
        )) => (
            StatusCode::CONFLICT,
            "revision_conflict",
            "The provider setting changed; reload it before saving.",
        ),
        ProviderError::CapabilityUnavailable => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "capability_unavailable",
            "The provider does not support this operation.",
        ),
        ProviderError::InvalidConfiguration(_)
        | ProviderError::PrivacyDenied
        | ProviderError::EndpointDenied
        | ProviderError::InvalidResponse(_)
        | ProviderError::SchemaValidation { .. }
        | ProviderError::DimensionMismatch
        | ProviderError::Url(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The provider configuration is invalid.",
        ),
        _ if error.retryable() => (
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_unavailable",
            "The provider is temporarily unavailable.",
        ),
        _ => (
            StatusCode::BAD_GATEWAY,
            "provider_failed",
            "The provider operation failed.",
        ),
    };
    api_error(status, code, message, None, request_id)
}

fn index_error(error: mcp_vault_indexer::IndexError, request_id: String) -> Response {
    let _ = error;
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "index_unavailable",
        "The knowledge index is temporarily unavailable.",
        None,
        request_id,
    )
}

fn memory_error(error: mcp_vault_memory::MemoryError, request_id: String) -> Response {
    let (status, code, message) = match error {
        mcp_vault_memory::MemoryError::InvalidInput(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The memory request is invalid.",
        ),
        mcp_vault_memory::MemoryError::SourceIngestion(code) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            code,
            "The source note could not be processed.",
        ),
        mcp_vault_memory::MemoryError::GeneratedOutput(code) => (
            StatusCode::BAD_GATEWAY,
            code,
            "The generated memory output failed validation.",
        ),
        mcp_vault_memory::MemoryError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "The memory resource was not found.",
        ),
        mcp_vault_memory::MemoryError::Configuration(code) => (
            StatusCode::CONFLICT,
            code,
            "Memory extraction is not fully configured.",
        ),
        mcp_vault_memory::MemoryError::State(StateError::InvalidDomain(
            DomainError::RevisionConflict { .. } | DomainError::PreconditionFailed { .. },
        )) => (
            StatusCode::CONFLICT,
            "revision_conflict",
            "The memory setting changed; reload it before saving.",
        ),
        mcp_vault_memory::MemoryError::Conflict => (
            StatusCode::CONFLICT,
            "memory_conflict",
            "Memory state changed while this operation was running; refresh and retry.",
        ),
        mcp_vault_memory::MemoryError::Quarantined | mcp_vault_memory::MemoryError::Markdown => (
            StatusCode::CONFLICT,
            "memory_quarantined",
            "The memory record is quarantined.",
        ),
        mcp_vault_memory::MemoryError::Provider(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_unavailable",
            "Optional memory provider work is unavailable.",
        ),
        mcp_vault_memory::MemoryError::State(_)
        | mcp_vault_memory::MemoryError::Core(_)
        | mcp_vault_memory::MemoryError::Index(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "memory_unavailable",
            "Memory state is temporarily unavailable.",
        ),
    };
    api_error(status, code, message, None, request_id)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::{Arc, atomic::AtomicBool},
    };

    use super::{
        AdminApiConfig, AdminApiState, BackupLimits, MaintenanceGate, MaintenanceMode,
        RunMemoryExtractionRequest, VaultCoreRuntime, index_coverage_ratio, job_progress_ratio,
        provider_kind, router, select_advertised_data_origin, stateful_router,
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        response::Response,
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use http_body_util::BodyExt;
    use mcp_vault_auth::{AuthService, MasterKeyRing, OriginPolicy};
    use mcp_vault_domain::{Actor, MemoryId, Revision, SourcePlane, VaultPath, VaultSlug};
    use mcp_vault_memory::ExtractionPolicy;
    use mcp_vault_providers::{
        ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
        ProviderSettings,
    };
    use mcp_vault_state::{JobStatus, MemoryBundle, MemoryRecord, StateStore};
    use mcp_vault_storage_fs::StorageOptions;
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tower::ServiceExt;

    #[test]
    fn first_class_provider_types_are_accepted_by_admin() {
        for provider_type in [
            "deepseek",
            "xiaomi_mimo",
            "zhipu_glm",
            "moonshot_kimi",
            "google_gemini",
            "alibaba_qwen",
        ] {
            assert_eq!(
                provider_kind(provider_type).unwrap().as_str(),
                provider_type
            );
        }
    }

    async fn fixture_with_state_for_origin(
        origin: &str,
    ) -> (axum::Router, TempDir, MaintenanceGate, AdminApiState) {
        let root = tempfile::tempdir().unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[4_u8; 32]).unwrap(),
        );
        let key_version_ids = auth.key_version_ids();
        let maintenance = MaintenanceGate::new();
        let core_runtime = VaultCoreRuntime::new(maintenance.clone());
        let admin = AdminApiState::new(
            state,
            auth,
            AdminApiConfig {
                origin_policy: OriginPolicy::new([origin]).unwrap(),
                data_hosts: ["localhost".to_owned()].into_iter().collect(),
                data_origins: Vec::new(),
                data_public_origin: None,
                data_bind: "127.0.0.1:8080".parse().unwrap(),
                admin_bind: "127.0.0.1:8081".parse().unwrap(),
                data_dir: root.path().to_owned(),
                history_root: root.path().join("history"),
                storage_options: StorageOptions::default(),
                backup_root: root.path().join("backups"),
                backup_limits: BackupLimits::default(),
                key_version_ids,
                maintenance: maintenance.clone(),
                core_runtime,
                readiness: Arc::new(AtomicBool::new(true)),
                version: "test".to_owned(),
            },
        );
        (stateful_router(admin.clone()), root, maintenance, admin)
    }

    async fn fixture_with_state() -> (axum::Router, TempDir, MaintenanceGate, AdminApiState) {
        fixture_with_state_for_origin("http://localhost:8081").await
    }

    async fn fixture() -> (axum::Router, TempDir, MaintenanceGate) {
        let (router, root, maintenance, _state) = fixture_with_state().await;
        (router, root, maintenance)
    }

    #[test]
    fn admin_progress_projections_distinguish_complete_partial_and_unknown() {
        assert_eq!(job_progress_ratio(JobStatus::Completed, None), Some(1.0));
        assert_eq!(
            job_progress_ratio(JobStatus::Running, Some(&json!({"done": 2, "total": 8}))),
            Some(0.25)
        );
        assert_eq!(
            job_progress_ratio(JobStatus::Running, Some(&json!(25))),
            Some(0.25)
        );
        assert_eq!(job_progress_ratio(JobStatus::Queued, None), None);
        assert_eq!(index_coverage_ratio(178, 178), Some(1.0));
        assert_eq!(index_coverage_ratio(89, 178), Some(0.5));
        assert_eq!(index_coverage_ratio(0, 0), None);
    }

    #[test]
    fn manual_extraction_defaults_to_incremental_and_accepts_explicit_full_re_evaluation() {
        let incremental: RunMemoryExtractionRequest = serde_json::from_value(json!({})).unwrap();
        assert!(!incremental.include_evaluated);
        let full: RunMemoryExtractionRequest =
            serde_json::from_value(json!({"include_evaluated": true})).unwrap();
        assert!(full.include_evaluated);
    }

    #[test]
    fn advertised_data_origin_preserves_ports_and_public_https() {
        let local_hosts = BTreeSet::from(["127.0.0.1".to_owned()]);
        assert_eq!(
            select_advertised_data_origin(None, &[], &local_hosts, "0.0.0.0:8080".parse().unwrap(),),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            select_advertised_data_origin(
                Some("https://vault.example.test"),
                &["https://browser.example.test".to_owned()],
                &local_hosts,
                "0.0.0.0:8080".parse().unwrap(),
            ),
            "https://vault.example.test"
        );
        assert_eq!(
            select_advertised_data_origin(
                None,
                &["https://vault.example.test:8443".to_owned()],
                &local_hosts,
                "0.0.0.0:8080".parse().unwrap(),
            ),
            "https://vault.example.test:8443"
        );
        assert_eq!(
            select_advertised_data_origin(
                None,
                &[],
                &BTreeSet::from(["::1".to_owned()]),
                "[::]:8080".parse().unwrap(),
            ),
            "http://[::1]:8080"
        );
    }

    fn request(
        method: &str,
        uri: &str,
        body: Value,
        cookie: Option<&str>,
        csrf: Option<&str>,
    ) -> Request<Body> {
        request_with_origin(method, uri, body, cookie, csrf, "http://localhost:8081")
    }

    fn request_with_origin(
        method: &str,
        uri: &str,
        body: Value,
        cookie: Option<&str>,
        csrf: Option<&str>,
        origin: &str,
    ) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("origin", origin)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        if let Some(cookie) = cookie {
            request
                .headers_mut()
                .insert("cookie", cookie.parse().unwrap());
        }
        if let Some(csrf) = csrf {
            request
                .headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
        }
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap(),
        ));
        request
    }

    async fn json_response(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap())
    }

    async fn authenticated_fixture_with_state() -> (
        axum::Router,
        TempDir,
        MaintenanceGate,
        String,
        String,
        AdminApiState,
    ) {
        let (router, root, maintenance, state) = fixture_with_state().await;
        let setup = router
            .clone()
            .oneshot(request(
                "POST",
                "/setup",
                json!({
                    "username": "owner",
                    "password": "correct horse battery staple"
                }),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(setup.status(), StatusCode::CREATED);
        let vault = state.list_vaults().await.unwrap().remove(0);
        let context = vault.context().unwrap();
        state
            .state
            .memory()
            .set_pipeline_generation_state(
                &context,
                mcp_vault_memory::MEMORY_PIPELINE_GENERATION,
                false,
            )
            .await
            .unwrap();
        let login = router
            .clone()
            .oneshot(request(
                "POST",
                "/session",
                json!({"username": "owner", "password": "correct horse battery staple"}),
                None,
                None,
            ))
            .await
            .unwrap();
        let set_cookies = login
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(set_cookies.len(), 2);
        assert!(
            set_cookies
                .iter()
                .any(|value| value.starts_with("mcp_vault_session="))
        );
        assert!(
            set_cookies.iter().any(|value| {
                value.starts_with("mcp_vault_csrf=") && !value.contains("HttpOnly")
            })
        );
        assert!(set_cookies.iter().all(|value| !value.contains("Secure")));
        let cookie = login
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let (status, body) = json_response(login).await;
        assert_eq!(status, StatusCode::OK);
        let csrf = body["data"]["csrf_token"].as_str().unwrap().to_owned();
        (router, root, maintenance, cookie, csrf, state)
    }

    #[tokio::test]
    async fn https_admin_origin_keeps_secure_session_and_csrf_cookies() {
        let origin = "https://admin.example.test:8444";
        let (router, _root, _maintenance, _state) = fixture_with_state_for_origin(origin).await;
        let setup = router
            .clone()
            .oneshot(request_with_origin(
                "POST",
                "/setup",
                json!({
                    "username": "owner",
                    "password": "correct horse battery staple"
                }),
                None,
                None,
                origin,
            ))
            .await
            .unwrap();
        assert_eq!(setup.status(), StatusCode::CREATED);

        let login = router
            .oneshot(request_with_origin(
                "POST",
                "/session",
                json!({"username": "owner", "password": "correct horse battery staple"}),
                None,
                None,
                origin,
            ))
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let set_cookies = login
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(set_cookies.len(), 2);
        assert!(set_cookies.iter().all(|value| value.contains("Secure")));
        assert!(
            set_cookies
                .iter()
                .all(|value| value.contains("SameSite=Strict"))
        );
    }

    async fn authenticated_fixture() -> (axum::Router, TempDir, MaintenanceGate, String, String) {
        let (router, root, maintenance, cookie, csrf, _state) =
            authenticated_fixture_with_state().await;
        (router, root, maintenance, cookie, csrf)
    }

    #[tokio::test]
    async fn admin_creates_lists_and_preserves_the_legacy_default_vault() {
        let (router, root, _maintenance, cookie, csrf, state) =
            authenticated_fixture_with_state().await;

        let (status, created) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/vaults",
                    json!({"slug": "work", "name": "工作资料"}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{created}");
        assert_eq!(created["data"]["vault"]["slug"], "work");
        assert_eq!(created["data"]["vault"]["availability"], "initializing");
        assert_eq!(
            created["data"]["vault"]["content_root"],
            root.path().join("vaults/work").display().to_string()
        );
        assert_eq!(
            created["data"]["initialization_job"]["job_type"],
            "vault.initialize"
        );

        let (status, selected) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/vaults/work",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{selected}");
        assert_eq!(selected["data"]["slug"], "work");

        let (status, connection) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/vaults/work/mcp/connection-info",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{connection}");
        assert_eq!(
            connection["data"]["webdav_endpoint"],
            "http://localhost:8080/dav/v1/vaults/work/"
        );
        assert_eq!(
            connection["data"]["mcp_endpoint"],
            "http://localhost:8080/mcp/v1/vaults/work"
        );

        let (status, issued) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/vaults/work/mcp/tokens",
                    json!({"name": "Work agent", "scopes": ["vault:read"]}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{issued}");

        let (status, work_tokens) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/vaults/work/mcp/tokens",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{work_tokens}");
        assert_eq!(work_tokens["data"]["tokens"].as_array().unwrap().len(), 1);

        let (status, default_tokens) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/mcp/tokens",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{default_tokens}");
        assert!(
            default_tokens["data"]["tokens"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let (status, listed) = json_response(
            router
                .clone()
                .oneshot(request("GET", "/vaults", json!({}), Some(&cookie), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed["data"]["vaults"].as_array().unwrap().len(), 2);

        let (status, legacy) = json_response(
            router
                .oneshot(request("GET", "/vault", json!({}), Some(&cookie), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{legacy}");
        assert_eq!(legacy["data"]["slug"], "default");
        assert_eq!(
            state
                .state
                .vaults()
                .legacy_default()
                .await
                .unwrap()
                .unwrap()
                .slug
                .as_str(),
            "default"
        );
    }

    #[tokio::test]
    async fn disabling_and_retrying_one_managed_vault_preserves_its_lifecycle_job() {
        let (router, _root, _maintenance, cookie, csrf, state) =
            authenticated_fixture_with_state().await;
        let (_, created) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/vaults",
                    json!({"slug": "work", "name": "Work"}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        let revision = created["data"]["vault"]["settings_revision"]
            .as_i64()
            .unwrap();
        let job_id = created["data"]["initialization_job"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let (status, disabled) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PATCH",
                    "/vaults/work",
                    json!({"status": "disabled", "expected_settings_revision": revision}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{disabled}");
        assert_eq!(disabled["data"]["availability"], "disabled");

        let work = state
            .state
            .vaults()
            .find_by_slug(&VaultSlug::new("work").unwrap())
            .await
            .unwrap()
            .unwrap();
        let context = work.context().unwrap();
        let job = state
            .state
            .jobs()
            .get(&context, job_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JobStatus::Cancelled);

        let (status, enabled) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PATCH",
                    "/vaults/work",
                    json!({
                        "status": "active",
                        "expected_settings_revision": disabled["data"]["settings_revision"]
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{enabled}");
        assert_eq!(enabled["data"]["availability"], "error");

        let (status, retried) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/vaults/work/initialization/retry",
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{retried}");
        assert_eq!(retried["data"]["status"], "queued");

        let (status, selected) = json_response(
            router
                .oneshot(request(
                    "GET",
                    "/vaults/work",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{selected}");
        assert_eq!(selected["data"]["availability"], "initializing");
    }

    #[tokio::test]
    async fn enabling_ready_memory_pipeline_immediately_admits_fresh_regeneration() {
        let (router, _root, _maintenance, cookie, csrf, state) =
            authenticated_fixture_with_state().await;
        let vault = state.list_vaults().await.unwrap().remove(0);
        let context = vault.context().unwrap();
        state
            .state
            .memory()
            .set_pipeline_generation_state(
                &context,
                mcp_vault_memory::MEMORY_PIPELINE_GENERATION,
                true,
            )
            .await
            .unwrap();
        state
            .providers()
            .set_provider_mode(&context, ProviderMode::LocalOnly, None)
            .await
            .unwrap();
        let provider = state
            .providers()
            .create_provider(ProviderInput {
                name: "Immediate memory provider".to_owned(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: url::Url::parse("http://127.0.0.1:11434/v1/").unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: None,
            })
            .await
            .unwrap();
        let model = state
            .providers()
            .register_model(ModelInput {
                provider_id: provider.id,
                external_model_id: "memory-model".to_owned(),
                capabilities: ModelCapabilities {
                    structured_output: true,
                    ..ModelCapabilities::default()
                },
                settings: ModelSettings::default(),
                enabled: true,
            })
            .await
            .unwrap();
        for role in ["memory_extraction", "memory_consolidation"] {
            state
                .providers()
                .bind_model(Some(&context), role, model.id, json!({}), None)
                .await
                .unwrap();
        }
        assert!(
            state
                .state
                .jobs()
                .find_active_by_type(&context, "memory.extract")
                .await
                .unwrap()
                .is_none()
        );

        let (status, extraction) = json_response(
            router
                .oneshot(request(
                    "PUT",
                    "/memory/extraction",
                    json!({"enabled": true}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{extraction}");
        assert_eq!(extraction["data"]["readiness"]["ready"], true);
        let job = state
            .state
            .jobs()
            .find_active_by_type(&context, "memory.extract")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.payload["reason"], "pipeline_cutover");
        assert_eq!(job.payload["fresh_start"], true);
        assert_eq!(
            job.payload["pipeline_generation"],
            mcp_vault_memory::MEMORY_PIPELINE_GENERATION
        );
        assert!(
            !state
                .state
                .memory()
                .get_consolidation_state(&context)
                .await
                .unwrap()
                .unwrap()
                .regeneration_pending
        );
    }

    #[tokio::test]
    async fn final_memory_model_binding_immediately_admits_fresh_regeneration() {
        let (router, _root, _maintenance, cookie, csrf, state) =
            authenticated_fixture_with_state().await;
        let vault = state.list_vaults().await.unwrap().remove(0);
        let context = vault.context().unwrap();
        state
            .state
            .memory()
            .set_pipeline_generation_state(
                &context,
                mcp_vault_memory::MEMORY_PIPELINE_GENERATION,
                true,
            )
            .await
            .unwrap();
        state
            .providers()
            .set_provider_mode(&context, ProviderMode::LocalOnly, None)
            .await
            .unwrap();
        let provider = state
            .providers()
            .create_provider(ProviderInput {
                name: "Binding-trigger provider".to_owned(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: url::Url::parse("http://127.0.0.1:11434/v1/").unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: None,
            })
            .await
            .unwrap();
        let model = state
            .providers()
            .register_model(ModelInput {
                provider_id: provider.id,
                external_model_id: "binding-memory-model".to_owned(),
                capabilities: ModelCapabilities {
                    structured_output: true,
                    ..ModelCapabilities::default()
                },
                settings: ModelSettings::default(),
                enabled: true,
            })
            .await
            .unwrap();
        state
            .memory()
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
        state
            .providers()
            .bind_model(
                Some(&context),
                "memory_extraction",
                model.id,
                json!({}),
                None,
            )
            .await
            .unwrap();

        let (status, binding) = json_response(
            router
                .oneshot(request(
                    "PUT",
                    "/model-bindings/memory_consolidation",
                    json!({
                        "model_id": model.id,
                        "settings": {},
                        "vault_override": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{binding}");
        let job = state
            .state
            .jobs()
            .find_active_by_type(&context, "memory.extract")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.payload["reason"], "pipeline_cutover");
        assert_eq!(job.payload["fresh_start"], true);
        assert!(
            !state
                .state
                .memory()
                .get_consolidation_state(&context)
                .await
                .unwrap()
                .unwrap()
                .regeneration_pending
        );
    }

    #[tokio::test]
    async fn jobs_overview_keeps_old_running_job_outside_recent_history_limit() {
        let (router, _root, _maintenance, cookie, _csrf, state) =
            authenticated_fixture_with_state().await;
        let vault = state.list_vaults().await.unwrap().remove(0);
        let context = vault.context().unwrap();
        let running = state
            .state
            .jobs()
            .enqueue(
                &context,
                "memory.extract",
                "admin-jobs:old-running",
                &json!({"scope": "all"}),
                0,
                5,
                0,
            )
            .await
            .unwrap();
        state
            .state
            .jobs()
            .claim_batch("admin-long-worker", 1, i64::MAX / 2, 1)
            .await
            .unwrap();
        for index in 0..55_u32 {
            state
                .state
                .jobs()
                .enqueue(
                    &context,
                    "index.note",
                    &format!("admin-jobs:terminal:{index}"),
                    &json!({"index": index}),
                    0,
                    3,
                    0,
                )
                .await
                .unwrap();
        }
        let terminal = state
            .state
            .jobs()
            .claim_batch("admin-short-worker", 2, i64::MAX / 2, 55)
            .await
            .unwrap();
        for job in terminal {
            state
                .state
                .jobs()
                .complete(job.id, "admin-short-worker")
                .await
                .unwrap();
        }

        let (status, body) = json_response(
            router
                .oneshot(request(
                    "GET",
                    "/jobs/overview?limit=50",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["counts"]["running"], 1);
        assert_eq!(body["data"]["counts"]["completed"], 55);
        assert_eq!(body["data"]["history"].as_array().unwrap().len(), 50);
        let visible_running = body["data"]["running"].as_array().unwrap();
        assert_eq!(visible_running.len(), 1);
        assert_eq!(visible_running[0]["id"], running.id.to_string());
        assert_eq!(visible_running[0]["status"], JobStatus::Running.as_str());
        assert_eq!(body["data"]["truncated"]["running"], false);
    }

    #[tokio::test]
    async fn memory_lifecycle_routes_archive_restore_and_permanently_delete() {
        let (router, _root, _maintenance, cookie, csrf, state) =
            authenticated_fixture_with_state().await;
        let vault = state.list_vaults().await.unwrap().remove(0);
        let context = vault.context().unwrap();
        let core = state.core_for_vault(&vault).unwrap();
        let memory_id = MemoryId::new();
        let canonical_path =
            VaultPath::parse(&format!("_mcp-vault/memory/records/2026/08/{memory_id}.md")).unwrap();
        let content = "The test deployment keeps Admin authentication enabled.";
        let markdown = format!(
            "---\nid: \"{memory_id}\"\ntype: \"decision\"\nstatus: \"active\"\nimportance: 0.900000\nconfidence: 1.000000\norigin: \"explicit_admin\"\nrevision: 1\ncreated_at: 1\nupdated_at: 1\nvalid_from: null\nvalid_to: null\nentities:\ntags:\nsupersedes:\nsources:\nextraction: \"{{\\\"pipeline\\\":\\\"codex_two_phase\\\"}}\"\n---\n\n{content}\n"
        );
        let mutation = core
            .create_managed_bytes(
                &context,
                &canonical_path,
                markdown.as_bytes(),
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap();
        let created = state
            .state
            .memory()
            .replace_bundle(
                &context,
                &MemoryBundle {
                    memory: MemoryRecord {
                        id: memory_id,
                        vault_id: context.id(),
                        memory_type: "decision".to_owned(),
                        status: "active".to_owned(),
                        status_reason: None,
                        status_changed_at: None,
                        content: content.to_owned(),
                        normalized_content: content.to_ascii_lowercase(),
                        content_hash: "sha256:test-admin-memory".to_owned(),
                        importance: 0.9,
                        confidence: 1.0,
                        origin: "explicit_admin".to_owned(),
                        revision: Revision::new(1),
                        canonical_file_id: Some(mutation.file.id),
                        canonical_path: Some(canonical_path.clone()),
                        canonical_revision: Some(mutation.file.current_revision),
                        valid_from: None,
                        valid_to: None,
                        extraction: json!({"pipeline": "codex_two_phase"}),
                        created_at: 1,
                        updated_at: 1,
                        last_recalled_at: None,
                        recall_count: 0,
                    },
                    sources: Vec::new(),
                    entities: Vec::new(),
                    tags: Vec::new(),
                    relations: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        let memory_id = created.memory.id.to_string();
        let canonical_path = created.memory.canonical_path.clone().unwrap();
        core.read_managed(&context, &canonical_path).await.unwrap();

        let archive_uri = format!("/memories/{memory_id}/archive");
        let (status, archived) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    &archive_uri,
                    json!({"expected_revision": created.memory.revision.value()}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(archived["data"]["status"], "archived");
        let archived_revision = archived["data"]["revision"].as_i64().unwrap();

        let restore_uri = format!("/memories/{memory_id}/restore");
        let (status, stale_restore) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    &restore_uri,
                    json!({"expected_revision": created.memory.revision.value()}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(stale_restore["error"]["code"], "memory_conflict");

        let (status, restored) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    &restore_uri,
                    json!({"expected_revision": archived_revision}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(restored["data"]["status"], "active");
        let restored_revision = restored["data"]["revision"].as_i64().unwrap();

        let delete_uri = format!("/memories/{memory_id}?expected_revision={restored_revision}");
        let (status, deleted) = json_response(
            router
                .clone()
                .oneshot(request(
                    "DELETE",
                    &delete_uri,
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(deleted["data"]["id"], memory_id);

        let get_uri = format!("/memories/{memory_id}");
        let (status, missing) = json_response(
            router
                .clone()
                .oneshot(request("GET", &get_uri, json!({}), Some(&cookie), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(missing["error"]["code"], "not_found");
        assert!(core.read_managed(&context, &canonical_path).await.is_err());

        let (status, audit) = json_response(
            router
                .oneshot(request(
                    "GET",
                    "/audit?action=admin.memory.deleted&limit=10",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            audit["data"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["action"] == "admin.memory.deleted")
        );
    }

    #[tokio::test]
    async fn memory_source_health_routes_report_separate_counts_and_admit_repeatable_audit() {
        let (router, _root, _maintenance, cookie, csrf, state) =
            authenticated_fixture_with_state().await;
        let vault = state.list_vaults().await.unwrap().remove(0);
        let context = vault.context().unwrap();

        let (status, health) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/memory/source-health?limit=10",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{health}");
        assert_eq!(health["data"]["summary"]["final_sources"]["total"], 0);
        assert_eq!(health["data"]["summary"]["memories"]["affected"], 0);
        assert_eq!(health["data"]["summary"]["stage1"]["total"], 0);
        assert_eq!(health["data"]["summary"]["distinct_file_ids"], 0);

        let (status, job) = json_response(
            router
                .oneshot(request(
                    "POST",
                    "/memory/source-health/audit",
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{job}");
        assert_eq!(job["data"]["job_type"], "memory.audit_sources");
        let queued = state
            .state
            .jobs()
            .find_active_by_type(&context, "memory.audit_sources")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            queued.payload["pipeline_generation"],
            mcp_vault_memory::MEMORY_PIPELINE_GENERATION
        );
        assert_eq!(queued.payload["reason"], "admin");
    }

    #[tokio::test]
    async fn logout_clears_session_and_csrf_cookies() {
        let (router, _root, _maintenance, cookie, csrf) = authenticated_fixture().await;
        let response = router
            .oneshot(request(
                "DELETE",
                "/session",
                json!({}),
                Some(&cookie),
                Some(&csrf),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookies = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert!(set_cookies.iter().any(|value| {
            value.starts_with("mcp_vault_session=") && value.contains("Max-Age=0")
        }));
        assert!(
            set_cookies.iter().any(|value| {
                value.starts_with("mcp_vault_csrf=") && value.contains("Max-Age=0")
            })
        );
        assert!(set_cookies.iter().all(|value| !value.contains("Secure")));
    }

    #[tokio::test]
    async fn unconfigured_router_is_not_a_data_access_path() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::NOT_IMPLEMENTED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            !body
                .windows(b"password".len())
                .any(|window| window == b"password")
        );
    }

    #[tokio::test]
    async fn setup_login_csrf_and_vault_update_round_trip() {
        let (router, _root, maintenance) = fixture().await;
        let rejected = router
            .clone()
            .oneshot(request(
                "POST",
                "/setup",
                json!({
                    "bootstrap_token": "obsolete",
                    "username": "owner",
                    "password": "correct horse battery staple"
                }),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let (status, body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/setup",
                    json!({
                        "username": "owner",
                        "password": "correct horse battery staple"
                    }),
                    None,
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["data"]["vault"]["slug"], "default");

        let login_response = router
            .clone()
            .oneshot(request(
                "POST",
                "/session",
                json!({"username": "owner", "password": "correct horse battery staple"}),
                None,
                None,
            ))
            .await
            .unwrap();
        let cookie = login_response
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let (_, login_body) = json_response(login_response).await;
        let csrf = login_body["data"]["csrf_token"]
            .as_str()
            .unwrap()
            .to_owned();

        let (status, connection) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/mcp/connection-info",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            connection["data"]["webdav_endpoint"],
            "http://localhost:8080/dav/v1/vaults/default/"
        );
        assert_eq!(
            connection["data"]["mcp_endpoint"],
            "http://localhost:8080/mcp/v1/vaults/default"
        );
        assert_eq!(
            connection["data"]["oauth_protected_resource_metadata_url"],
            "http://localhost:8080/.well-known/oauth-protected-resource/mcp/v1/vaults/default"
        );
        assert_eq!(
            connection["data"]["oauth_authorization_server_metadata_url"],
            "http://localhost:8080/.well-known/oauth-authorization-server"
        );
        assert_eq!(
            connection["data"]["authorization_modes"],
            json!(["oauth_builtin", "pat", "oauth_external"])
        );

        let (status, local_before) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/mcp/oauth/local",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(local_before["data"]["configured"], false);

        let oauth_password = "separate vault oauth password";
        let (status, local_created) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/mcp/oauth/local",
                    json!({
                        "username": "chatgpt",
                        "password": oauth_password,
                        "scopes": ["vault:discover", "vault:read", "memory:read"]
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(local_created["data"]["username"], "chatgpt");
        assert_eq!(local_created["data"]["enabled"], true);
        assert!(!local_created.to_string().contains(oauth_password));

        let (status, local_configured) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/mcp/oauth/local",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(local_configured["data"]["configured"], true);
        assert_eq!(local_configured["data"]["user"]["username"], "chatgpt");
        assert!(!local_configured.to_string().contains(oauth_password));

        let (status, disabled) = json_response(
            router
                .clone()
                .oneshot(request(
                    "DELETE",
                    "/mcp/oauth/local",
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(disabled["data"]["disabled"], true);

        let (status, webdav_body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/webdav/credentials",
                    json!({
                        "name": "Test device",
                        "username": "obsidian",
                        "password": "device-secret",
                        "permissions": ["vault:read"]
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let webdav_password = webdav_body["data"]["password"].as_str().unwrap().to_owned();
        assert_eq!(webdav_password, "device-secret");
        let (status, webdav_list) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/webdav/credentials",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!webdav_list.to_string().contains(&webdav_password));

        let (status, mcp_body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/mcp/tokens",
                    json!({"name": "Test agent", "scopes": ["vault:read"]}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let mcp_secret = mcp_body["data"]["secret"].as_str().unwrap().to_owned();
        let (status, mcp_list) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/mcp/tokens",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!mcp_list.to_string().contains(&mcp_secret));

        let (status, backup_body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/backups",
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let backup_id = backup_body["data"]["backup"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, backups_body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/backups?limit=10",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(backups_body["data"]["backups"][0]["id"], backup_id);

        let (status, body) = json_response(
            router
                .clone()
                .oneshot(request("GET", "/session", json!({}), Some(&cookie), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["username"], "owner");

        let (status, _) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PATCH",
                    "/vault",
                    json!({"name": "Renamed"}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PATCH",
                    "/vault",
                    json!({"name": "Renamed"}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["name"], "Renamed");

        maintenance.set(mcp_vault_domain::MaintenanceMode::ReadOnly);
        let (status, body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PATCH",
                    "/vault",
                    json!({"name": "Blocked"}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "maintenance");
        let (status, _) = json_response(
            router
                .clone()
                .oneshot(request("GET", "/system", json!({}), Some(&cookie), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, audit_body) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/audit?limit=50",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let entries = audit_body["data"]["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| {
            entry["action"] == "admin.vault.updated" && entry["result"] == "success"
        }));
        let serialized = audit_body.to_string();
        assert!(!serialized.contains("correct horse battery staple"));
    }

    #[tokio::test]
    async fn provider_mode_and_oauth_grants_are_manageable_and_vault_scoped() {
        let (router, _root, _maintenance, cookie, csrf) = authenticated_fixture().await;

        let (status, default_mode) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/providers/mode",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(default_mode["data"]["mode"], "disabled");
        assert!(default_mode["data"]["revision"].is_null());

        let (status, mode) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/providers/mode",
                    json!({"mode": "remote_allowed"}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(mode["data"]["revision"], 1);

        let (status, _) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/providers/mode",
                    json!({"mode": "local_only", "expected_revision": 1}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, stale) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/providers/mode",
                    json!({"mode": "disabled", "expected_revision": 1}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(stale["error"]["code"], "revision_conflict");

        let modulus = URL_SAFE_NO_PAD.encode([0xa5_u8; 256]);
        let (status, issuer) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/mcp/oauth",
                    json!({
                        "name": "Test issuer",
                        "issuer_url": "https://issuer.example.test",
                        "discovery_url": null,
                        "audience": "mcp-vault",
                        "resource": "https://vault.example.test/mcp",
                        "jwks_cache_json": format!(
                            r#"{{"keys":[{{"kty":"RSA","kid":"test","alg":"RS256","use":"sig","n":"{modulus}","e":"AQAB"}}]}}"#
                        ),
                        "enabled": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let issuer_id = issuer["data"]["id"].as_str().unwrap();

        let (status, grant) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/mcp/oauth/grants",
                    json!({
                        "issuer_id": issuer_id,
                        "subject": "agent@example.test",
                        "scopes": ["vault:discover", "vault:read"]
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let grant_id = grant["data"]["id"].as_str().unwrap().to_owned();
        let (status, grants) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/mcp/oauth/grants",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(grants["data"]["grants"].as_array().unwrap().len(), 1);

        let (status, _) = json_response(
            router
                .clone()
                .oneshot(request(
                    "DELETE",
                    &format!("/mcp/oauth/grants/{grant_id}"),
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, grants) = json_response(
            router
                .oneshot(request(
                    "GET",
                    "/mcp/oauth/grants?include_revoked=true",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(grants["data"]["grants"][0]["revoked_at"].is_number());
    }

    #[tokio::test]
    async fn provider_delete_cleans_models_bindings_secrets_and_records_audit_counts() {
        let (router, _root, _maintenance, cookie, csrf) = authenticated_fixture().await;
        let (status, provider) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/providers",
                    json!({
                        "name": "Disposable provider",
                        "provider_type": "openai_compatible",
                        "base_url": "http://127.0.0.1:11434/v1/",
                        "settings": {},
                        "enabled": true,
                        "secret": "disposable-secret"
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{provider}");
        let provider_id = provider["data"]["id"].as_str().unwrap().to_owned();
        let mut provider_revision = provider["data"]["revision"].as_i64().unwrap();
        let secret_hint = provider["data"]["secret"]["hint"]
            .as_str()
            .unwrap()
            .to_owned();

        let (status, stale_update) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PATCH",
                    &format!("/providers/{provider_id}"),
                    json!({
                        "name": "Must not overwrite",
                        "provider_type": "openai_compatible",
                        "base_url": "http://127.0.0.1:11435/v1/",
                        "settings": {},
                        "enabled": true,
                        "secret": null,
                        "expected_revision": 999
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{stale_update}");
        assert_eq!(stale_update["error"]["code"], "revision_conflict");

        let (status, updated) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PATCH",
                    &format!("/providers/{provider_id}"),
                    json!({
                        "name": "Edited provider",
                        "provider_type": "openai_compatible",
                        "base_url": "http://127.0.0.1:11435/v1/",
                        "settings": {},
                        "enabled": true,
                        "secret": null,
                        "expected_revision": provider_revision
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["data"]["name"], "Edited provider");
        assert_eq!(updated["data"]["base_url"], "http://127.0.0.1:11435/v1/");
        assert_eq!(updated["data"]["secret"]["configured"], true);
        assert_eq!(updated["data"]["secret"]["hint"], secret_hint);
        provider_revision = updated["data"]["revision"].as_i64().unwrap();

        let (status, model) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    &format!("/providers/{provider_id}/models"),
                    json!({
                        "external_model_id": "disposable-model",
                        "capabilities": {"structured_output": true},
                        "settings": {},
                        "enabled": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{model}");
        let model_id = model["data"]["id"].as_str().unwrap().to_owned();
        let (status, binding) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/model-bindings/memory_extraction",
                    json!({
                        "model_id": model_id,
                        "settings": {},
                        "vault_override": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{binding}");

        let (status, stale) = json_response(
            router
                .clone()
                .oneshot(request(
                    "DELETE",
                    &format!("/providers/{provider_id}?expected_revision=999"),
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{stale}");
        assert_eq!(stale["error"]["code"], "revision_conflict");

        let (status, deleted) = json_response(
            router
                .clone()
                .oneshot(request(
                    "DELETE",
                    &format!("/providers/{provider_id}?expected_revision={provider_revision}"),
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{deleted}");
        assert_eq!(deleted["data"]["deleted"], true);
        assert_eq!(deleted["data"]["provider_id"], provider_id);
        assert_eq!(deleted["data"]["models_deleted"], 1);
        assert_eq!(deleted["data"]["bindings_deleted"], 1);
        assert_eq!(deleted["data"]["embeddings_deleted"], 0);
        assert_eq!(deleted["data"]["secrets_deleted"], 1);

        let (status, providers) = json_response(
            router
                .clone()
                .oneshot(request("GET", "/providers", json!({}), Some(&cookie), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{providers}");
        assert!(
            providers["data"]["providers"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let (status, bindings) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    "/model-bindings",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{bindings}");
        assert!(bindings["data"]["bindings"].as_array().unwrap().is_empty());
        let (status, audit) = json_response(
            router
                .oneshot(request(
                    "GET",
                    "/audit?action=admin.provider.deleted",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{audit}");
        assert_eq!(
            audit["data"]["entries"][0]["action"],
            "admin.provider.deleted"
        );
        assert_eq!(audit["data"]["entries"][0]["metadata"]["models_deleted"], 1);
        assert_eq!(
            audit["data"]["entries"][0]["metadata"]["bindings_deleted"],
            1
        );
        assert_eq!(
            audit["data"]["entries"][0]["metadata"]["secrets_deleted"],
            1
        );
    }

    #[tokio::test]
    async fn models_bindings_and_memory_extraction_are_operable_through_admin() {
        let (router, _root, _maintenance, cookie, csrf) = authenticated_fixture().await;
        let (status, _) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/providers/mode",
                    json!({"mode": "local_only"}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, provider) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/providers",
                    json!({
                        "name": "Local fake",
                        "provider_type": "openai_compatible",
                        "base_url": "http://127.0.0.1:11434/v1/",
                        "settings": {},
                        "enabled": true,
                        "secret": null
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{provider}");
        let provider_id = provider["data"]["id"].as_str().unwrap();

        let (status, model) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    &format!("/providers/{provider_id}/models"),
                    json!({
                        "external_model_id": "fixture-memory-model",
                        "capabilities": {"structured_output": true},
                        "settings": {
                            "openai_compatibility_preset": "xiaomi_mimo",
                            "openai_structured_output_mode": "json_object",
                            "openai_token_limit_field": "max_completion_tokens",
                            "openai_thinking_mode": "enabled",
                            "generation_token_limit": 32768
                        },
                        "enabled": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{model}");
        let model_id = model["data"]["id"].as_str().unwrap();
        let (status, models) = json_response(
            router
                .clone()
                .oneshot(request(
                    "GET",
                    &format!("/providers/{provider_id}/models"),
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            models["data"]["models"][0]["external_model_id"],
            "fixture-memory-model"
        );
        assert_eq!(
            models["data"]["models"][0]["settings"]["openai_compatibility_preset"],
            "xiaomi_mimo"
        );
        assert_eq!(
            models["data"]["models"][0]["settings"]["openai_thinking_mode"],
            "enabled"
        );
        assert_eq!(
            models["data"]["models"][0]["settings"]["generation_token_limit"],
            32768
        );

        let (status, binding) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/model-bindings/memory_extraction",
                    json!({
                        "model_id": model_id,
                        "settings": {},
                        "vault_override": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{binding}");
        assert_eq!(binding["data"]["role"], "memory_extraction");
        let (status, binding) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/model-bindings/memory_extraction",
                    json!({
                        "model_id": model_id,
                        "settings": {},
                        "expected_revision": 1,
                        "vault_override": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{binding}");
        assert_eq!(binding["data"]["revision"], 2);
        let (status, stale_binding) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/model-bindings/memory_extraction",
                    json!({
                        "model_id": model_id,
                        "settings": {},
                        "expected_revision": 1,
                        "vault_override": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{stale_binding}");
        assert_eq!(stale_binding["error"]["code"], "revision_conflict");
        let (status, consolidation_binding) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/model-bindings/memory_consolidation",
                    json!({
                        "model_id": model_id,
                        "settings": {},
                        "vault_override": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{consolidation_binding}");
        assert_eq!(
            consolidation_binding["data"]["role"],
            "memory_consolidation"
        );

        let (status, extraction) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/memory/extraction",
                    json!({
                        "enabled": true
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{extraction}");
        assert_eq!(extraction["data"]["readiness"]["ready"], true);
        assert_eq!(extraction["data"]["policy"]["source_mode"], "automatic");
        assert_eq!(extraction["data"]["policy"]["request_timeout_seconds"], 300);
        assert_eq!(extraction["data"]["policy"]["max_evidence_per_note"], 3);
        assert!(extraction["data"]["policy"]["minimum_confidence"].is_null());
        assert_eq!(
            extraction["data"]["phase1_readiness"]["external_model_id"],
            "fixture-memory-model"
        );
        assert_eq!(
            extraction["data"]["phase2_readiness"]["external_model_id"],
            "fixture-memory-model"
        );
        let (status, stale_policy) = json_response(
            router
                .clone()
                .oneshot(request(
                    "PUT",
                    "/memory/extraction",
                    json!({
                        "enabled": true,
                        "expected_revision": 99
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{stale_policy}");
        assert_eq!(stale_policy["error"]["code"], "revision_conflict");

        let (status, job) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/memory/extraction/run",
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{job}");
        assert_eq!(job["data"]["job_type"], "memory.extract");
        assert_eq!(job["data"]["admission"], "queued");
        assert!(job["data"]["progress_ratio"].is_null());
        assert_eq!(job["data"]["details"]["scope"], "all");
        assert_eq!(job["data"]["details"]["reason"], "admin_backfill");
        assert_eq!(job["data"]["details"]["include_evaluated"], false);
        let first_job_id = job["data"]["id"].clone();

        let (status, existing_job) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/memory/extraction/run",
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{existing_job}");
        assert_eq!(existing_job["data"]["id"], first_job_id);
        assert_eq!(existing_job["data"]["admission"], "existing");

        let (status, restarted) = json_response(
            router
                .clone()
                .oneshot(request(
                    "POST",
                    "/memory/extraction/restart",
                    json!({}),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{restarted}");
        assert_eq!(restarted["data"]["cancel_requested"], 1);
        assert_eq!(
            restarted["data"]["job"]["details"]["reason"],
            "admin_restart"
        );
        assert_eq!(
            restarted["data"]["job"]["details"]["include_evaluated"],
            true
        );

        let (status, jobs) = json_response(
            router
                .oneshot(request(
                    "GET",
                    "/jobs?job_type=memory.extract&limit=20",
                    json!({}),
                    Some(&cookie),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(jobs["data"]["jobs"].as_array().unwrap().len(), 2);
        assert!(jobs["data"]["jobs"].as_array().unwrap().iter().all(|job| {
            job["job_type"] == "memory.extract" && job["details"]["scope"] == "all"
        }));
    }

    #[tokio::test]
    async fn operator_can_recover_after_restore_leaves_process_offline() {
        let (router, _root, maintenance, cookie, csrf) = authenticated_fixture().await;
        maintenance.set(MaintenanceMode::Offline);
        let (status, body) = json_response(
            router
                .oneshot(request(
                    "POST",
                    "/maintenance/recover",
                    json!({
                        "confirmation": "RECOVER",
                        "password": "correct horse battery staple"
                    }),
                    Some(&cookie),
                    Some(&csrf),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(maintenance.mode(), MaintenanceMode::Normal);
    }

    #[tokio::test]
    async fn concurrent_setup_requests_create_one_admin_and_one_default_vault() {
        let (router, _root, _maintenance) = fixture().await;
        let (status, body) = json_response(
            router
                .clone()
                .oneshot(request("GET", "/setup", Value::Null, None, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["setup_available"], true);

        let first = router.clone().oneshot(request(
            "POST",
            "/setup",
            json!({
                "username": "first-owner",
                "password": "correct horse battery staple"
            }),
            None,
            None,
        ));
        let second = router.clone().oneshot(request(
            "POST",
            "/setup",
            json!({
                "username": "second-owner",
                "password": "correct horse battery staple"
            }),
            None,
            None,
        ));
        let (first, second) = tokio::join!(first, second);
        let first = json_response(first.unwrap()).await;
        let second = json_response(second.unwrap()).await;
        let statuses = [first.0, second.0];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CREATED)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CONFLICT)
                .count(),
            1
        );
        let winner = if first.0 == StatusCode::CREATED {
            &first.1
        } else {
            &second.1
        };
        assert_eq!(winner["data"]["vault"]["slug"], "default");

        let (status, body) = json_response(
            router
                .oneshot(request("GET", "/setup", Value::Null, None, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["setup_available"], false);
    }

    #[tokio::test]
    async fn admin_source_filtering_is_external_and_origin_stays_strict() {
        let (router, _root, _maintenance) = fixture().await;
        let mut missing_peer = request(
            "POST",
            "/setup",
            json!({
                "username": "owner",
                "password": "correct horse battery staple"
            }),
            None,
            None,
        );
        missing_peer
            .extensions_mut()
            .remove::<axum::extract::ConnectInfo<std::net::SocketAddr>>();
        let (status, body) =
            json_response(router.clone().oneshot(missing_peer).await.unwrap()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");

        let mut wrong_origin = request(
            "POST",
            "/session",
            json!({
                "username": "owner",
                "password": "correct horse battery staple"
            }),
            None,
            None,
        );
        wrong_origin
            .headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());
        let (status, body) = json_response(router.oneshot(wrong_origin).await.unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "origin_rejected");
    }
}
