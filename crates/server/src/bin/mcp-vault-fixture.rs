//! Disposable real-HTTP deployment used by protocol conformance and
//! interoperability checks.
//!
//! This binary deliberately creates its own temporary Vault, state database,
//! and credentials. It composes the same protocol routers as the production
//! server. The only test-only behavior is an outer loopback middleware that
//! injects the generated MCP PAT so an external conformance client, which does
//! not know MCP Vault's credential issuance flow, can exercise the real MCP
//! transport without weakening production authentication.

use std::{
    collections::BTreeSet,
    error::Error,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
};
use mcp_vault_admin_api::AdminApiConfig;
use mcp_vault_auth::{AuthService, MasterKeyRing, OriginPolicy, SecretString};
use mcp_vault_domain::{MaintenanceGate, Permission, PermissionSet, Revision, Scope, ScopeSet};
use mcp_vault_server::{
    control_router_with_admin, data_router_with_webdav_and_mcp_and_metrics, health::Readiness,
};
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use mcp_vault_webdav::WebDavService;
use serde::Serialize;
use tokio::{net::TcpListener, sync::Notify};

const VAULT_SLUG: &str = "interop";
const WEB_DAV_USERNAME: &str = "interop-desktop";
const WEB_DAV_PASSWORD: &str = "interop-dav-password-123";

#[derive(Debug, Serialize)]
struct FixtureManifest {
    schema_version: u32,
    mcp_url: String,
    webdav_url: String,
    health_url: String,
    admin_url: String,
    vault_slug: &'static str,
    /// The PAT is confined to this temporary fixture and is not uploaded as
    /// conformance evidence. The fixture middleware consumes it locally.
    mcp_token: String,
    webdav_username: &'static str,
    /// The WebDAV password is required by the external Litmus/smoke runner.
    /// The manifest is created with mode 0600 on Unix and deleted with the
    /// fixture's temporary directory.
    webdav_password: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let content_root = directory.path().join("vault");
    let history_root = directory.path().join("history");
    let backup_root = directory.path().join("backups");
    tokio::fs::create_dir_all(&content_root).await?;
    tokio::fs::create_dir_all(&history_root).await?;
    tokio::fs::create_dir_all(&backup_root).await?;
    tokio::fs::write(
        content_root.join("welcome.md"),
        "# Interop fixture\n\nThis note belongs to the disposable conformance Vault.\n",
    )
    .await?;

    let database_path = directory.path().join("state.sqlite3");
    let database_url = format!("sqlite://{}", database_path.display());
    let state = StateStore::connect_and_migrate(&database_url).await?;
    let context = mcp_vault_domain::VaultContext::new(
        mcp_vault_domain::VaultId::new(),
        mcp_vault_domain::VaultSlug::new(VAULT_SLUG)?,
        content_root,
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(&context, "Interop fixture", VaultStatus::Active)
        .await?;

    let auth = AuthService::new(state.auth(), MasterKeyRing::from_bytes(1, &[0x42; 32])?);
    let scopes: ScopeSet = Scope::ALL.into_iter().collect();
    let pat = auth
        .issue_pat(&context, "interop-client", scopes, None)
        .await?;
    auth.issue_webdav_credential(
        &context,
        WEB_DAV_USERNAME,
        WEB_DAV_USERNAME,
        &SecretString::new(WEB_DAV_PASSWORD),
        webdav_permissions(),
        None,
    )
    .await?;
    let maintenance = MaintenanceGate::new();
    let core_runtime = mcp_vault_core::VaultCoreRuntime::new(maintenance.clone());

    // Import the seed note through the same reconciliation/index path used at
    // startup. This keeps public search/resource scenarios meaningful while
    // leaving all canonical content in the temporary Vault filesystem.
    mcp_vault_server::reconcile_vault_once(
        &state,
        &history_root,
        &state
            .vaults()
            .find_by_slug(&mcp_vault_domain::VaultSlug::new(VAULT_SLUG)?)
            .await?
            .ok_or("fixture Vault was not registered")?,
        "initial",
        &core_runtime,
    )
    .await?;

    let data_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let control_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let data_bind = data_listener.local_addr()?;
    let admin_bind = control_listener.local_addr()?;
    let storage_options = StorageOptions {
        durability: DurabilityPolicy::None,
        minimum_free_bytes: 0,
        ..StorageOptions::default()
    };
    let readiness = Readiness::new();
    let data_hosts = BTreeSet::from([
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ]);
    let data_origins = OriginPolicy::new([format!("http://{data_bind}")])?;
    let webdav_service = WebDavService::new(
        state.clone(),
        auth.clone(),
        history_root.clone(),
        storage_options,
        core_runtime.clone(),
        BTreeSet::new(),
    );
    let provider_service = mcp_vault_providers::ProviderService::new(state.clone(), auth.clone());
    let memory_service = mcp_vault_memory::MemoryService::with_provider_service(
        state.clone(),
        provider_service.clone(),
    );
    let index_service = mcp_vault_indexer::IndexService::with_provider_service(
        state.clone(),
        provider_service.clone(),
    );
    let mcp_service = mcp_vault_mcp::McpService::new(
        state.clone(),
        auth.clone(),
        history_root.clone(),
        storage_options,
        core_runtime.clone(),
        data_hosts.iter().cloned().collect(),
        data_origins.clone(),
    )
    .with_application_services(index_service, memory_service.clone());
    let data_router = data_router_with_webdav_and_mcp_and_metrics(
        readiness.clone(),
        webdav_service,
        mcp_service,
        mcp_vault_server::metrics::Metrics::new(false),
    )
    .layer(middleware::from_fn_with_state(
        FixtureAuth {
            authorization: HeaderValue::from_str(&format!("Bearer {}", pat.token.expose_secret()))?,
        },
        test_mcp_auth,
    ));

    let admin_state = mcp_vault_admin_api::AdminApiState::new(
        state.clone(),
        auth.clone(),
        AdminApiConfig {
            origin_policy: OriginPolicy::new([format!("http://{admin_bind}")])?,
            data_hosts,
            data_origins: data_origins.allowed_origins().map(str::to_owned).collect(),
            data_public_origin: None,
            data_bind,
            admin_bind,
            data_dir: directory.path().to_owned(),
            history_root,
            storage_options,
            core_runtime,
            readiness: readiness.shared_flag(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            backup_root,
            backup_limits: mcp_vault_backup::BackupLimits {
                max_entry_bytes: 16 * 1024 * 1024,
                max_total_bytes: 64 * 1024 * 1024,
                max_archive_bytes: 64 * 1024 * 1024,
                max_entries: 10_000,
                keep_count: 1,
            },
            key_version_ids: auth.key_version_ids(),
            maintenance: maintenance.clone(),
        },
    )
    .with_provider_services(provider_service, memory_service);
    let control_router = control_router_with_admin(admin_state);
    readiness.mark_ready();

    let manifest = FixtureManifest {
        schema_version: 1,
        mcp_url: format!("http://{data_bind}/mcp/v1/vaults/{VAULT_SLUG}"),
        webdav_url: format!("http://{data_bind}/dav/v1/vaults/{VAULT_SLUG}/"),
        health_url: format!("http://{data_bind}/health/ready"),
        admin_url: format!("http://{admin_bind}/api/v1/system"),
        vault_slug: VAULT_SLUG,
        mcp_token: pat.token.expose_secret().to_owned(),
        webdav_username: WEB_DAV_USERNAME,
        webdav_password: WEB_DAV_PASSWORD,
    };
    let manifest_path = std::env::var_os("MCP_VAULT_FIXTURE_MANIFEST")
        .map(PathBuf::from)
        .unwrap_or_else(|| directory.path().join("fixture-manifest.json"));
    write_manifest(&manifest_path, &manifest).await?;
    println!("MCP_VAULT_FIXTURE_MANIFEST={}", manifest_path.display());
    println!("MCP_VAULT_FIXTURE_MCP_URL={}", manifest.mcp_url);
    println!("MCP_VAULT_FIXTURE_WEBDAV_URL={}", manifest.webdav_url);
    println!("MCP_VAULT_FIXTURE_HEALTH_URL={}", manifest.health_url);
    println!("MCP_VAULT_FIXTURE_ADMIN_URL={}", manifest.admin_url);

    serve(data_listener, control_listener, data_router, control_router).await?;
    drop(directory);
    Ok(())
}

fn webdav_permissions() -> PermissionSet {
    PermissionSet::from_iter([
        Permission::DiscoverVault,
        Permission::ReadVault,
        Permission::WriteVault,
        Permission::DeleteVault,
    ])
}

#[derive(Clone)]
struct FixtureAuth {
    authorization: HeaderValue,
}

async fn test_mcp_auth(
    State(state): State<FixtureAuth>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.uri().path().starts_with("/mcp/") {
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, state.authorization);
    }
    next.run(request).await
}

async fn write_manifest(path: &Path, manifest: &FixtureManifest) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(path, bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

async fn serve(
    data_listener: TcpListener,
    control_listener: TcpListener,
    data_router: Router,
    control_router: Router,
) -> Result<(), Box<dyn Error>> {
    let shutdown = Arc::new(Notify::new());
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_shutdown.notify_waiters();
    });

    let data_shutdown = shutdown.clone();
    let control_shutdown = shutdown;
    let data_server = axum::serve(
        data_listener,
        data_router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        data_shutdown.notified().await;
    });
    let control_server = axum::serve(
        control_listener,
        control_router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        control_shutdown.notified().await;
    });
    let (data_result, control_result) = tokio::join!(data_server, control_server);
    data_result?;
    control_result?;
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
