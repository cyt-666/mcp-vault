//! Project-owned WebDAV adapter.
//!
//! `dav-server` supplies RFC 4918 parsing, conditional headers, ranges, and
//! lock-token handling. This crate supplies authentication, Vault binding, and
//! a guarded filesystem that delegates every canonical operation to Vault
//! Core.

use std::{
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::Body as AxumBody,
    extract::{ConnectInfo, Extension, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::any,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::{Buf, Bytes};
use dav_server::{
    DavHandler,
    body::Body,
    davpath::DavPath,
    fs::{
        DavDirEntry, DavFile, DavMetaData, FsError, FsFuture, FsResult, FsStream,
        GuardedFileSystem, OpenOptions, ReadDirMeta,
    },
    memls::MemLs,
};
use futures_util::stream;
use http::{Request as HttpRequest, Response, Uri};
use mcp_vault_auth::{AuthPrincipal, AuthService, SecretString, require_secure_basic_auth};
use mcp_vault_core::{CoreMetadata, StagedWrite, VaultCore, VaultCoreRuntime, VaultError};
use mcp_vault_domain::{
    MaintenanceGate, Permission, SourcePlane, VaultContext, VaultPath, VaultPathPolicy,
};
use mcp_vault_state::{StateStore, VaultAvailability, VaultRecord};
use mcp_vault_storage_fs::{ReadFile, StorageError, StorageOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Errors returned while generating a public WebDAV connection URL.
#[derive(Debug, thiserror::Error)]
pub enum WebDavConnectionError {
    /// The configured public base URL is not a valid absolute URI.
    #[error("WebDAV base URL is invalid: {0}")]
    InvalidBaseUrl(#[from] http::uri::InvalidUri),
    /// The base URL must identify an origin.
    #[error("WebDAV base URL must include a scheme and authority")]
    MissingOrigin,
    /// Query parameters must not be copied into a generated endpoint.
    #[error("WebDAV base URL must not include a query")]
    QueryNotAllowed,
}

/// Generate the Vault-scoped WebDAV endpoint shown by the Admin plane.
///
/// This helper never includes credentials. It accepts an optional reverse
/// proxy path prefix and rejects query-bearing values so generated client URLs
/// cannot accidentally inherit unrelated request parameters.
pub fn webdav_connection_url(
    base_url: &str,
    slug: &mcp_vault_domain::VaultSlug,
) -> Result<String, WebDavConnectionError> {
    let base = Uri::from_str(base_url)?;
    let Some(scheme) = base.scheme_str() else {
        return Err(WebDavConnectionError::MissingOrigin);
    };
    let Some(authority) = base.authority() else {
        return Err(WebDavConnectionError::MissingOrigin);
    };
    if base.query().is_some() {
        return Err(WebDavConnectionError::QueryNotAllowed);
    }
    let prefix = base.path().trim_end_matches('/');
    Ok(format!(
        "{scheme}://{authority}{prefix}/dav/v1/vaults/{}/",
        slug.as_str()
    ))
}

/// Service dependencies required by the stateful WebDAV data-plane adapter.
#[derive(Clone)]
pub struct WebDavService {
    state: StateStore,
    auth: AuthService,
    history_root: PathBuf,
    storage_options: StorageOptions,
    core_runtime: VaultCoreRuntime,
    maintenance: MaintenanceGate,
    handler: DavHandler<DavCredentials>,
}

impl WebDavService {
    /// Construct a WebDAV service bound to existing application services.
    pub fn new(
        state: StateStore,
        auth: AuthService,
        history_root: PathBuf,
        storage_options: StorageOptions,
        core_runtime: VaultCoreRuntime,
    ) -> Self {
        let maintenance = core_runtime.maintenance();
        let handler = DavHandler::builder()
            .filesystem(Box::new(CoreFileSystem))
            .locksystem(MemLs::new())
            .hide_symlinks(true)
            .build_handler();
        Self {
            state,
            auth,
            history_root,
            storage_options,
            core_runtime,
            maintenance,
            handler,
        }
    }

    async fn handle(
        &self,
        request: Request,
        peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    ) -> Response<AxumBody> {
        let _request_operation = match self.maintenance.try_start_operation() {
            Some(operation) => operation,
            None => return public_error(StatusCode::SERVICE_UNAVAILABLE, false),
        };
        let (slug, relative_uri) = match mount_and_relative_uri(request.uri().path()) {
            Ok(value) => value,
            Err(status) => return public_error(status, false),
        };
        let vault = match self.state.vaults().find_by_slug(&slug).await {
            Ok(Some(vault)) => vault,
            Ok(None) => return public_error(StatusCode::NOT_FOUND, false),
            Err(_) => return public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
        };
        match self.state.vaults().availability(&vault).await {
            Ok(VaultAvailability::Disabled) => {
                return public_error(StatusCode::NOT_FOUND, false);
            }
            Ok(VaultAvailability::Initializing | VaultAvailability::Error) => {
                return public_error(StatusCode::SERVICE_UNAVAILABLE, false);
            }
            Ok(VaultAvailability::Ready | VaultAvailability::Maintenance) => {}
            Err(_) => return public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
        }
        let context = match vault.context() {
            Ok(context) => context,
            Err(_) => return public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
        };
        let (username, password) = match basic_credentials(request.headers()) {
            Ok(credentials) => credentials,
            Err(_) => return public_error(StatusCode::UNAUTHORIZED, true),
        };
        let peer_is_loopback = peer.is_some_and(|value| value.0.0.ip().is_loopback());
        let forwarded_tls = request
            .headers()
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("https"));
        if require_secure_basic_auth(forwarded_tls, peer_is_loopback).is_err() {
            return public_error(StatusCode::UNAUTHORIZED, true);
        }
        let principal = match self
            .auth
            .authenticate_webdav(&context, &username, &password, None)
            .await
        {
            Ok(principal) => principal,
            Err(error) => {
                let status = match error {
                    mcp_vault_auth::AuthError::State(_) => StatusCode::INTERNAL_SERVER_ERROR,
                    _ => StatusCode::UNAUTHORIZED,
                };
                return public_error(status, status == StatusCode::UNAUTHORIZED);
            }
        };
        if principal.vault_id != Some(context.id()) {
            return public_error(StatusCode::UNAUTHORIZED, true);
        }
        if dav_method_writes(request.method()) && !self.maintenance.allows_write() {
            return public_error(StatusCode::SERVICE_UNAVAILABLE, false);
        }
        let core = match core_for_vault(
            &self.state,
            &self.history_root,
            &vault,
            self.storage_options,
            self.core_runtime.clone(),
        ) {
            Ok(core) => core,
            Err(_) => return public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
        };
        let credentials = DavCredentials {
            context,
            principal,
            core,
        };
        let mut request = request;
        if rewrite_request_uri(&mut request, &relative_uri).is_err()
            || rewrite_destination_header(&mut request, &slug).is_err()
        {
            return public_error(StatusCode::FORBIDDEN, false);
        }
        let principal_name = credentials
            .principal
            .credential_id
            .map(|id| format!("webdav:{id}"))
            .unwrap_or_else(|| "webdav:credential".to_owned());
        let response = self
            .handler
            .handle_guarded(request, principal_name, credentials)
            .await;
        into_axum_response(response)
    }
}

fn dav_method_writes(method: &http::Method) -> bool {
    matches!(
        method.as_str(),
        "PUT"
            | "POST"
            | "PATCH"
            | "DELETE"
            | "MKCOL"
            | "COPY"
            | "MOVE"
            | "LOCK"
            | "UNLOCK"
            | "PROPPATCH"
    )
}

/// Build the stateful WebDAV mount. The server nests this router below
/// `/dav/v1/vaults`.
pub fn router(service: WebDavService) -> Router {
    Router::new()
        .route("/", any(handle_dav))
        .route("/{*path}", any(handle_dav))
        .with_state(service)
}

/// Build the explicit unconfigured fallback used by router composition tests.
pub fn unconfigured_router() -> Router {
    Router::new().fallback(any(not_configured))
}

async fn handle_dav(
    State(service): State<WebDavService>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    request: Request,
) -> Response<AxumBody> {
    service.handle(request, peer).await
}

async fn not_configured() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "WebDAV adapter is not configured\n",
    )
}

#[derive(Clone)]
struct DavCredentials {
    context: VaultContext,
    principal: AuthPrincipal,
    core: VaultCore,
}

#[derive(Clone, Copy, Debug)]
struct CoreFileSystem;

impl GuardedFileSystem<DavCredentials> for CoreFileSystem {
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        Box::pin(async move {
            let path = vault_path(path)?;
            if options.write {
                require_permission(credentials, Permission::WriteVault)?;
                let staged = credentials
                    .core
                    .begin_put(
                        &credentials.context,
                        &path,
                        options.create,
                        options.create_new,
                        credentials.principal.actor.clone(),
                        SourcePlane::WebDav,
                    )
                    .await
                    .map_err(map_vault_error)?;
                let metadata = credentials
                    .core
                    .metadata(&credentials.context, &path)
                    .await
                    .ok();
                Ok(Box::new(CoreDavFile::writer(
                    credentials.core.clone(),
                    credentials.context.clone(),
                    path,
                    staged,
                    metadata,
                )) as Box<dyn DavFile>)
            } else {
                require_permission(credentials, Permission::ReadVault)?;
                let result = credentials
                    .core
                    .read(&credentials.context, &path)
                    .await
                    .map_err(map_vault_error)?;
                let metadata = credentials
                    .core
                    .metadata(&credentials.context, &path)
                    .await
                    .map_err(map_vault_error)?;
                Ok(Box::new(CoreDavFile::reader(result.reader, metadata)) as Box<dyn DavFile>)
            }
        })
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _meta: ReadDirMeta,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        Box::pin(async move {
            require_permission(credentials, Permission::ReadVault)?;
            let path = vault_path(path)?;
            let entries = credentials
                .core
                .list_directory(&credentials.context, &path)
                .await
                .map_err(map_vault_error)?;
            let entries = entries
                .into_iter()
                .filter_map(|metadata| {
                    let name = metadata
                        .metadata
                        .path
                        .as_ref()
                        .and_then(VaultPath::file_name)
                        .map(|name| name.as_bytes().to_vec())?;
                    Some(Ok(Box::new(CoreDirEntry {
                        name,
                        metadata: CoreDavMeta::from_core(metadata),
                    }) as Box<dyn DavDirEntry>))
                })
                .collect::<Vec<_>>();
            Ok(Box::pin(stream::iter(entries)) as FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(
        &'a self,
        path: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        Box::pin(async move {
            require_permission(credentials, Permission::ReadVault)?;
            let path = vault_path(path)?;
            let metadata = credentials
                .core
                .metadata(&credentials.context, &path)
                .await
                .map_err(map_vault_error)?;
            Ok(Box::new(CoreDavMeta::from_core(metadata)) as Box<dyn DavMetaData>)
        })
    }

    fn symlink_metadata<'a>(
        &'a self,
        path: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        self.metadata(path, credentials)
    }

    fn create_dir<'a>(
        &'a self,
        path: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, ()> {
        Box::pin(async move {
            require_permission(credentials, Permission::WriteVault)?;
            let path = vault_path(path)?;
            credentials
                .core
                .create_directory(
                    &credentials.context,
                    &path,
                    credentials.principal.actor.clone(),
                    SourcePlane::WebDav,
                )
                .await
                .map_err(map_vault_error)?;
            Ok(())
        })
    }

    fn remove_dir<'a>(
        &'a self,
        path: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, ()> {
        self.remove_entry(path, credentials)
    }

    fn remove_file<'a>(
        &'a self,
        path: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, ()> {
        self.remove_entry(path, credentials)
    }

    fn rename<'a>(
        &'a self,
        from: &'a DavPath,
        to: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, ()> {
        Box::pin(async move {
            require_permission(credentials, Permission::WriteVault)?;
            require_permission(credentials, Permission::DeleteVault)?;
            let from = vault_path(from)?;
            let to = vault_path(to)?;
            let source = credentials
                .core
                .stat(&credentials.context, &from)
                .await
                .map_err(map_vault_error)?;
            credentials
                .core
                .move_entry(
                    &credentials.context,
                    &from,
                    &to,
                    source.file.current_revision,
                    credentials.principal.actor.clone(),
                    SourcePlane::WebDav,
                    None,
                )
                .await
                .map_err(map_vault_error)?;
            Ok(())
        })
    }

    fn copy<'a>(
        &'a self,
        from: &'a DavPath,
        to: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, ()> {
        Box::pin(async move {
            require_permission(credentials, Permission::ReadVault)?;
            require_permission(credentials, Permission::WriteVault)?;
            let from = vault_path(from)?;
            let to = vault_path(to)?;
            credentials
                .core
                .copy(
                    &credentials.context,
                    &from,
                    &to,
                    credentials.principal.actor.clone(),
                    SourcePlane::WebDav,
                    None,
                )
                .await
                .map_err(map_vault_error)?;
            Ok(())
        })
    }
}

impl CoreFileSystem {
    fn remove_entry<'a>(
        &'a self,
        path: &'a DavPath,
        credentials: &'a DavCredentials,
    ) -> FsFuture<'a, ()> {
        Box::pin(async move {
            require_permission(credentials, Permission::DeleteVault)?;
            let path = vault_path(path)?;
            let current = credentials
                .core
                .stat(&credentials.context, &path)
                .await
                .map_err(map_vault_error)?;
            credentials
                .core
                .delete(
                    &credentials.context,
                    &path,
                    current.file.current_revision,
                    credentials.principal.actor.clone(),
                    SourcePlane::WebDav,
                    None,
                )
                .await
                .map_err(map_vault_error)?;
            Ok(())
        })
    }
}

struct CoreDavFile {
    state: CoreDavFileState,
}

enum CoreDavFileState {
    Reader {
        reader: Box<ReadFile>,
        metadata: CoreDavMeta,
    },
    Writer(Box<CoreDavWriter>),
}

struct CoreDavWriter {
    core: VaultCore,
    context: VaultContext,
    path: VaultPath,
    staged: Option<StagedWrite>,
    metadata: Option<CoreDavMeta>,
}

impl std::fmt::Debug for CoreDavFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoreDavFile")
            .finish_non_exhaustive()
    }
}

impl CoreDavFile {
    fn reader(reader: ReadFile, metadata: CoreMetadata) -> Self {
        Self {
            state: CoreDavFileState::Reader {
                reader: Box::new(reader),
                metadata: CoreDavMeta::from_core(metadata),
            },
        }
    }

    fn writer(
        core: VaultCore,
        context: VaultContext,
        path: VaultPath,
        staged: StagedWrite,
        metadata: Option<CoreMetadata>,
    ) -> Self {
        Self {
            state: CoreDavFileState::Writer(Box::new(CoreDavWriter {
                core,
                context,
                path,
                staged: Some(staged),
                metadata: metadata.map(CoreDavMeta::from_core),
            })),
        }
    }
}

impl DavFile for CoreDavFile {
    fn metadata(&'_ mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        Box::pin(async move {
            match &mut self.state {
                CoreDavFileState::Reader { metadata, .. } => Ok(Box::new(metadata.clone()) as _),
                CoreDavFileState::Writer(writer) => {
                    if writer.metadata.is_none() {
                        let current = writer
                            .core
                            .metadata(&writer.context, &writer.path)
                            .await
                            .map_err(map_vault_error)?;
                        writer.metadata = Some(CoreDavMeta::from_core(current));
                    }
                    Ok(Box::new(
                        writer
                            .metadata
                            .as_ref()
                            .expect("metadata initialized")
                            .clone(),
                    ) as _)
                }
            }
        })
    }

    fn write_buf(&'_ mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
        Box::pin(async move {
            let bytes = buf.copy_to_bytes(buf.remaining());
            self.write_bytes(bytes).await
        })
    }

    fn write_bytes(&'_ mut self, buf: Bytes) -> FsFuture<'_, ()> {
        Box::pin(async move {
            let CoreDavFileState::Writer(writer) = &mut self.state else {
                return Err(FsError::Forbidden);
            };
            let Some(staged) = writer.staged.as_mut() else {
                return Err(FsError::GeneralFailure);
            };
            staged.write_chunk(&buf).await.map_err(map_vault_error)
        })
    }

    fn read_bytes(&'_ mut self, count: usize) -> FsFuture<'_, Bytes> {
        Box::pin(async move {
            let CoreDavFileState::Reader { reader, .. } = &mut self.state else {
                return Err(FsError::Forbidden);
            };
            let mut bytes = vec![0_u8; count];
            let read = reader
                .read(&mut bytes)
                .await
                .map_err(|_| FsError::GeneralFailure)?;
            bytes.truncate(read);
            Ok(Bytes::from(bytes))
        })
    }

    fn seek(&'_ mut self, position: std::io::SeekFrom) -> FsFuture<'_, u64> {
        Box::pin(async move {
            let CoreDavFileState::Reader { reader, .. } = &mut self.state else {
                return Err(FsError::NotImplemented);
            };
            reader
                .seek(position)
                .await
                .map_err(|_| FsError::GeneralFailure)
        })
    }

    fn flush(&'_ mut self) -> FsFuture<'_, ()> {
        Box::pin(async move {
            let CoreDavFileState::Writer(writer) = &mut self.state else {
                return Ok(());
            };
            let Some(staged_write) = writer.staged.take() else {
                return Ok(());
            };
            match staged_write.commit().await {
                Ok(_) => {
                    let current = writer
                        .core
                        .metadata(&writer.context, &writer.path)
                        .await
                        .map_err(map_vault_error)?;
                    writer.metadata = Some(CoreDavMeta::from_core(current));
                    Ok(())
                }
                Err(error) => Err(map_vault_error(error)),
            }
        })
    }
}

#[derive(Clone, Debug)]
struct CoreDirEntry {
    name: Vec<u8>,
    metadata: CoreDavMeta,
}

impl DavDirEntry for CoreDirEntry {
    fn name(&self) -> Vec<u8> {
        self.name.clone()
    }

    fn metadata(&'_ self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        Box::pin(async { Ok(Box::new(self.metadata.clone()) as Box<dyn DavMetaData>) })
    }
}

#[derive(Clone, Debug)]
struct CoreDavMeta {
    size: u64,
    modified_at: i64,
    is_dir: bool,
    etag: String,
}

impl CoreDavMeta {
    fn from_core(metadata: CoreMetadata) -> Self {
        Self {
            size: metadata.metadata.size,
            modified_at: metadata.metadata.modified_at.unwrap_or_default(),
            is_dir: metadata.metadata.kind == mcp_vault_domain::FilesystemEntryKind::Directory,
            etag: metadata.etag,
        }
    }
}

impl DavMetaData for CoreDavMeta {
    fn len(&self) -> u64 {
        self.size
    }

    fn modified(&self) -> FsResult<SystemTime> {
        if self.modified_at >= 0 {
            UNIX_EPOCH
                .checked_add(Duration::from_millis(self.modified_at as u64))
                .ok_or(FsError::GeneralFailure)
        } else {
            UNIX_EPOCH
                .checked_sub(Duration::from_millis(self.modified_at.unsigned_abs()))
                .ok_or(FsError::GeneralFailure)
        }
    }

    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn etag(&self) -> Option<String> {
        Some(self.etag.clone())
    }
}

fn core_for_vault(
    state: &StateStore,
    history_root: &std::path::Path,
    vault: &VaultRecord,
    storage_options: StorageOptions,
    core_runtime: VaultCoreRuntime,
) -> Result<VaultCore, VaultError> {
    let policy = VaultPathPolicy::new(vault.reserved_root.clone(), Default::default())
        .map_err(VaultError::Domain)?;
    Ok(VaultCore::new(
        state.clone(),
        history_root.to_path_buf(),
        policy,
        storage_options,
        core_runtime,
    ))
}

fn require_permission(credentials: &DavCredentials, permission: Permission) -> FsResult<()> {
    if credentials.principal.permissions.contains(permission) {
        Ok(())
    } else {
        Err(FsError::Forbidden)
    }
}

fn vault_path(path: &DavPath) -> FsResult<VaultPath> {
    let raw = std::str::from_utf8(path.as_bytes()).map_err(|_| FsError::Forbidden)?;
    let raw = raw.trim_start_matches('/').trim_end_matches('/');
    VaultPath::parse(raw).map_err(|_| FsError::Forbidden)
}

fn map_vault_error(error: VaultError) -> FsError {
    match error {
        VaultError::NotFound => FsError::NotFound,
        VaultError::AlreadyExists => FsError::Exists,
        VaultError::RevisionConflict { .. } | VaultError::ExternalMismatch => {
            FsError::GeneralFailure
        }
        VaultError::Maintenance => FsError::Forbidden,
        VaultError::Domain(_)
        | VaultError::InvalidPatch(_)
        | VaultError::BinaryTextOperation
        | VaultError::IdempotencyConflict
        | VaultError::InFlight => FsError::Forbidden,
        VaultError::Storage(error) => map_storage_error(error),
        VaultError::State(_)
        | VaultError::VaultNotRegistered
        | VaultError::ContextMismatch
        | VaultError::NeedsReview
        | VaultError::InjectedFailure(_) => FsError::GeneralFailure,
    }
}

fn map_storage_error(error: StorageError) -> FsError {
    match error {
        StorageError::SourceNotFound | StorageError::HistoryNotFound => FsError::NotFound,
        StorageError::DestinationExists => FsError::Exists,
        StorageError::Domain(_) | StorageError::UnsafeEntry { .. } | StorageError::RootSymlink => {
            FsError::Forbidden
        }
        StorageError::InsufficientDiskSpace { .. } => FsError::InsufficientStorage,
        StorageError::InvalidOperation(_) | StorageError::InvalidContentHash => {
            FsError::GeneralFailure
        }
        StorageError::Io { kind, .. } => match kind {
            std::io::ErrorKind::NotFound => FsError::NotFound,
            std::io::ErrorKind::PermissionDenied => FsError::Forbidden,
            _ => FsError::GeneralFailure,
        },
        StorageError::AtomicCreateUnsupported
        | StorageError::RootNotDirectory
        | StorageError::TaskCancelled => FsError::GeneralFailure,
    }
}

fn basic_credentials(headers: &HeaderMap) -> Result<(String, SecretString), ()> {
    let value = headers.get(header::AUTHORIZATION).ok_or(())?;
    let value = value.to_str().map_err(|_| ())?;
    let encoded = value.strip_prefix("Basic ").ok_or(())?;
    let decoded = STANDARD.decode(encoded).map_err(|_| ())?;
    let decoded = String::from_utf8(decoded).map_err(|_| ())?;
    let (username, password) = decoded.split_once(':').ok_or(())?;
    if username.is_empty() || username.chars().any(char::is_control) {
        return Err(());
    }
    Ok((username.to_owned(), SecretString::new(password.to_owned())))
}

fn mount_and_relative_uri(path: &str) -> Result<(mcp_vault_domain::VaultSlug, String), StatusCode> {
    let path = path.strip_prefix('/').ok_or(StatusCode::NOT_FOUND)?;
    let (slug, rest) = path.split_once('/').unwrap_or((path, ""));
    let slug = mcp_vault_domain::VaultSlug::from_str(slug).map_err(|_| StatusCode::NOT_FOUND)?;
    let relative = if rest.is_empty() {
        "/".to_owned()
    } else {
        format!("/{rest}")
    };
    Ok((slug, relative))
}

fn rewrite_request_uri(request: &mut HttpRequest<AxumBody>, path: &str) -> Result<(), ()> {
    let query = request.uri().query().map(|query| format!("?{query}"));
    let value = format!("{path}{}", query.unwrap_or_default());
    *request.uri_mut() = Uri::from_str(&value).map_err(|_| ())?;
    Ok(())
}

fn rewrite_destination_header(
    request: &mut HttpRequest<AxumBody>,
    slug: &mcp_vault_domain::VaultSlug,
) -> Result<(), ()> {
    let Some(destination) = request.headers().get("destination") else {
        return Ok(());
    };
    let destination = destination.to_str().map_err(|_| ())?;
    let uri = Uri::from_str(destination).map_err(|_| ())?;
    if uri.query().is_some() {
        return Err(());
    }
    let mount = format!("/dav/v1/vaults/{}", slug.as_str());
    let relative_reference = !destination.starts_with('/') && !destination.contains("://");
    let path = if relative_reference {
        format!("/{destination}")
    } else {
        uri.path().to_owned()
    };
    let relative = if path == mount || path == format!("{mount}/") {
        "/".to_owned()
    } else if let Some(rest) = path.strip_prefix(&format!("{mount}/")) {
        format!("/{rest}")
    } else if !path.contains("/dav/v1/vaults/") {
        path
    } else {
        return Err(());
    };
    request.headers_mut().insert(
        "destination",
        HeaderValue::from_str(&relative).map_err(|_| ())?,
    );
    Ok(())
}

fn into_axum_response(response: Response<Body>) -> Response<AxumBody> {
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, AxumBody::new(body))
}

fn public_error(status: StatusCode, challenge: bool) -> Response<AxumBody> {
    let mut response = Response::builder().status(status);
    if challenge {
        response = response.header(header::WWW_AUTHENTICATE, "Basic realm=\"mcp-vault\"");
    }
    response
        .body(AxumBody::from("WebDAV request rejected\n"))
        .expect("static WebDAV error response")
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf, sync::Arc};

    use super::{WebDavService, router, unconfigured_router};
    use axum::{
        Extension,
        body::Body,
        extract::ConnectInfo,
        http::{Request, StatusCode},
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use http_body_util::BodyExt;
    use mcp_vault_auth::{AuthService, MasterKeyRing, SecretString};
    use mcp_vault_domain::{Permission, PermissionSet, Revision, VaultContext, VaultId, VaultSlug};
    use mcp_vault_state::{StateStore, VaultStatus};
    use mcp_vault_storage_fs::StorageOptions;
    use tempfile::TempDir;
    use tower::ServiceExt;

    async fn setup() -> (
        TempDir,
        StateStore,
        WebDavService,
        String,
        mcp_vault_domain::CredentialId,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!(
            "sqlite://{}",
            directory.path().join("state.sqlite3").display()
        );
        let state = StateStore::connect_and_migrate(&database_url)
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from(directory.path()).join("content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
        );
        let issued = auth
            .issue_webdav_credential(
                &context,
                "desktop",
                "desktop",
                &SecretString::new("dav-password-123"),
                full_permissions(),
                None,
            )
            .await
            .unwrap();
        let service = WebDavService::new(
            state.clone(),
            auth,
            directory.path().join("history"),
            StorageOptions {
                minimum_free_bytes: 0,
                durability: mcp_vault_storage_fs::DurabilityPolicy::None,
                ..StorageOptions::default()
            },
            Default::default(),
        );
        let credentials = STANDARD.encode("desktop:dav-password-123");
        (directory, state, service, credentials, issued.credential_id)
    }

    fn client_router(service: WebDavService) -> axum::Router {
        client_router_with_peer(service, SocketAddr::from(([127, 0, 0, 1], 49_181)))
    }

    fn client_router_with_peer(service: WebDavService, peer: SocketAddr) -> axum::Router {
        router(service).layer(Extension(ConnectInfo(peer)))
    }

    fn full_permissions() -> PermissionSet {
        let mut permissions = PermissionSet::new();
        permissions.insert(Permission::DiscoverVault);
        permissions.insert(Permission::ReadVault);
        permissions.insert(Permission::WriteVault);
        permissions.insert(Permission::DeleteVault);
        permissions
    }

    fn request(method: &str, uri: &str, credentials: &str, body: Body) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Basic {credentials}"))
            .body(body)
            .unwrap()
    }

    async fn body(response: axum::response::Response) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    #[tokio::test]
    async fn unconfigured_router_is_explicitly_not_implemented() {
        let response = unconfigured_router()
            .oneshot(
                Request::builder()
                    .uri("/notes/today.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn managed_vault_is_unavailable_until_initialization_completes() {
        let (_directory, state, service, credentials, _) = setup().await;
        let vault = state
            .vaults()
            .find_by_slug(&VaultSlug::new("work").unwrap())
            .await
            .unwrap()
            .unwrap();
        let context = vault.context().unwrap();
        state
            .jobs()
            .enqueue(
                &context,
                "vault.initialize",
                &format!("vault:{}:initialize", context.id()),
                &serde_json::json!({}),
                20,
                3,
                0,
            )
            .await
            .unwrap();

        let response = client_router(service)
            .oneshot(request(
                "GET",
                "/work/missing.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn generated_connection_urls_are_origin_scoped_and_credential_free() {
        let slug = VaultSlug::new("work").unwrap();
        assert_eq!(
            super::webdav_connection_url("https://vault.example.test/", &slug).unwrap(),
            "https://vault.example.test/dav/v1/vaults/work/"
        );
        assert_eq!(
            super::webdav_connection_url("https://vault.example.test/proxy", &slug).unwrap(),
            "https://vault.example.test/proxy/dav/v1/vaults/work/"
        );
        assert!(matches!(
            super::webdav_connection_url("vault.example.test", &slug),
            Err(super::WebDavConnectionError::MissingOrigin)
        ));
        assert!(matches!(
            super::webdav_connection_url("https://vault.example.test/?token=bad", &slug),
            Err(super::WebDavConnectionError::QueryNotAllowed)
        ));
    }

    #[test]
    fn maintenance_classifies_dav_mutations_without_blocking_reads() {
        assert!(super::dav_method_writes(&http::Method::PUT));
        assert!(super::dav_method_writes(
            &http::Method::from_bytes(b"MOVE").unwrap()
        ));
        assert!(super::dav_method_writes(
            &http::Method::from_bytes(b"LOCK").unwrap()
        ));
        assert!(!super::dav_method_writes(&http::Method::GET));
        assert!(!super::dav_method_writes(
            &http::Method::from_bytes(b"PROPFIND").unwrap()
        ));
    }

    #[tokio::test]
    async fn concurrent_puts_commit_metadata_and_remain_readable() {
        const PUT_COUNT: usize = 32;

        let (_directory, state, service, credentials, _credential_id) = setup().await;
        let app = client_router(service);
        let barrier = Arc::new(tokio::sync::Barrier::new(PUT_COUNT));
        let requests = (0..PUT_COUNT).map(|index| {
            let app = app.clone();
            let barrier = barrier.clone();
            let credentials = credentials.clone();
            async move {
                barrier.wait().await;
                let path = format!("/work/parallel/group-{}/file-{index}.md", index % 4);
                let payload = format!("parallel payload {index}");
                let response = app
                    .oneshot(request("PUT", &path, &credentials, Body::from(payload)))
                    .await
                    .unwrap();
                (path, response.status())
            }
        });
        let statuses = futures_util::future::join_all(requests).await;

        let context = state
            .vaults()
            .find_by_slug(&VaultSlug::new("work").unwrap())
            .await
            .unwrap()
            .unwrap()
            .context()
            .unwrap();
        let incomplete = state.files().list_incomplete(&context).await.unwrap();
        let active_entries = state.files().list_active_entries(&context).await.unwrap();

        assert!(
            statuses
                .iter()
                .all(|(_, status)| matches!(*status, StatusCode::CREATED | StatusCode::NO_CONTENT)),
            "concurrent PUT statuses: {statuses:?}; incomplete journals: {}; active entries: {}",
            incomplete.len(),
            active_entries.len()
        );

        for (path, _) in &statuses {
            let response = app
                .clone()
                .oneshot(request("GET", path, &credentials, Body::empty()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "unreadable path: {path}");
        }

        assert!(
            incomplete.is_empty(),
            "successful concurrent PUTs must not leave incomplete journals"
        );
    }

    #[tokio::test]
    async fn concurrent_unconditional_puts_to_one_path_are_serialized() {
        const PUT_COUNT: usize = 16;

        let (_directory, state, service, credentials, _credential_id) = setup().await;
        let app = client_router(service);
        let path = "/work/parallel/shared.md";
        let initial = app
            .clone()
            .oneshot(request(
                "PUT",
                path,
                &credentials,
                Body::from("initial payload"),
            ))
            .await
            .unwrap();
        assert_eq!(initial.status(), StatusCode::CREATED);

        let barrier = Arc::new(tokio::sync::Barrier::new(PUT_COUNT));
        let requests = (0..PUT_COUNT).map(|index| {
            let app = app.clone();
            let barrier = barrier.clone();
            let credentials = credentials.clone();
            async move {
                barrier.wait().await;
                app.oneshot(request(
                    "PUT",
                    path,
                    &credentials,
                    Body::from(format!("replacement payload {index}")),
                ))
                .await
                .unwrap()
                .status()
            }
        });
        let statuses = futures_util::future::join_all(requests).await;

        let context = state
            .vaults()
            .find_by_slug(&VaultSlug::new("work").unwrap())
            .await
            .unwrap()
            .unwrap()
            .context()
            .unwrap();
        let incomplete = state.files().list_incomplete(&context).await.unwrap();

        assert!(
            statuses
                .iter()
                .all(|status| *status == StatusCode::NO_CONTENT),
            "same-path PUT statuses: {statuses:?}; incomplete journals: {incomplete:?}"
        );
        assert!(
            incomplete.is_empty(),
            "serialized same-path PUTs must not leave incomplete journals"
        );
        let response = app
            .oneshot(request("GET", path, &credentials, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body(response).await;
        assert!(body.starts_with(b"replacement payload "));
    }

    #[tokio::test]
    async fn authenticated_webdav_round_trips_files_directories_ranges_and_conflicts() {
        let (_directory, state, service, credentials, credential_id) = setup().await;
        let app = client_router(service);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/work/no-auth.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key("www-authenticate"));

        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/work/hello.md",
                &credentials,
                Body::from("hello WebDAV"),
            ))
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ));

        let response = app
            .clone()
            .oneshot(request(
                "HEAD",
                "/work/hello.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers().get("etag").unwrap().clone();
        assert!(etag.to_str().unwrap().contains("1-"));

        let response = app
            .clone()
            .oneshot(request(
                "OPTIONS",
                "/work/hello.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("dav"));
        assert!(
            response
                .headers()
                .get("allow")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("LOCK")
        );

        let response = app
            .clone()
            .oneshot(request(
                "GET",
                "/work/hello.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(body(response).await, b"hello WebDAV");

        let mut range = request("GET", "/work/hello.md", &credentials, Body::empty());
        range
            .headers_mut()
            .insert("range", "bytes=1-4".parse().unwrap());
        let response = app.clone().oneshot(range).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(body(response).await, b"ello");

        let mut stale = request("PUT", "/work/hello.md", &credentials, Body::from("stale"));
        stale
            .headers_mut()
            .insert("if-match", "\"stale\"".parse().unwrap());
        let response = app.clone().oneshot(stale).await.unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

        let mut propfind = request("PROPFIND", "/work/", &credentials, Body::empty());
        propfind.headers_mut().insert("depth", "1".parse().unwrap());
        let response = app.clone().oneshot(propfind).await.unwrap();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let listing = String::from_utf8(body(response).await).unwrap();
        assert!(listing.contains("hello.md"));

        let response = app
            .clone()
            .oneshot(request("MKCOL", "/work/dir/", &credentials, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/work/dir/blob.bin",
                &credentials,
                Body::from(vec![42_u8; 100_000]),
            ))
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ));

        let response = app
            .clone()
            .oneshot(request("MKCOL", "/work/tree/", &credentials, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/work/tree/nested.md",
                &credentials,
                Body::from("nested"),
            ))
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ));
        let mut move_tree = request("MOVE", "/work/tree/", &credentials, Body::empty());
        move_tree.headers_mut().insert(
            "destination",
            "/dav/v1/vaults/work/renamed-tree/".parse().unwrap(),
        );
        let response = app.clone().oneshot(move_tree).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = app
            .clone()
            .oneshot(request(
                "GET",
                "/work/renamed-tree/nested.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(body(response).await, b"nested");
        let response = app
            .clone()
            .oneshot(request(
                "DELETE",
                "/work/renamed-tree/",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = app
            .clone()
            .oneshot(request(
                "MKCOL",
                "/work/renamed-tree/",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let mut move_request = request("MOVE", "/work/dir/blob.bin", &credentials, Body::empty());
        move_request.headers_mut().insert(
            "destination",
            "/dav/v1/vaults/work/moved.bin".parse().unwrap(),
        );
        let response = app.clone().oneshot(move_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let mut copy_request = request("COPY", "/work/hello.md", &credentials, Body::empty());
        copy_request.headers_mut().insert(
            "destination",
            "/dav/v1/vaults/work/copy.md".parse().unwrap(),
        );
        let response = app.clone().oneshot(copy_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let mut overwrite_copy = request("COPY", "/work/hello.md", &credentials, Body::empty());
        overwrite_copy
            .headers_mut()
            .insert("destination", "copy.md".parse().unwrap());
        let response = app.clone().oneshot(overwrite_copy).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/work/second.md",
                &credentials,
                Body::from("second source"),
            ))
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ));
        let mut overwrite_move = request("MOVE", "/work/second.md", &credentials, Body::empty());
        overwrite_move.headers_mut().insert(
            "destination",
            "/dav/v1/vaults/work/moved.bin".parse().unwrap(),
        );
        let response = app.clone().oneshot(overwrite_move).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let lock_body = Body::from(
            r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
  <D:owner><D:href>mcp-vault-test</D:href></D:owner>
</D:lockinfo>"#,
        );
        let mut lock_request = request("LOCK", "/work/hello.md", &credentials, lock_body);
        lock_request
            .headers_mut()
            .insert("timeout", "Second-120".parse().unwrap());
        let response = app.clone().oneshot(lock_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let lock_token = response.headers().get("lock-token").unwrap().clone();

        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/work/hello.md",
                &credentials,
                Body::from("blocked while locked"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::LOCKED);

        let mut unlock_request = request("UNLOCK", "/work/hello.md", &credentials, Body::empty());
        unlock_request
            .headers_mut()
            .insert("lock-token", lock_token);
        let response = app.clone().oneshot(unlock_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(request(
                "DELETE",
                "/work/copy.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        for path in [
            "/work/../outside.md",
            "/work/%2e%2e/outside.md",
            "/work/_mcp-vault/secret.md",
        ] {
            let response = app
                .clone()
                .oneshot(request("GET", path, &credentials, Body::empty()))
                .await
                .unwrap();
            assert!(
                [
                    StatusCode::FORBIDDEN,
                    StatusCode::NOT_FOUND,
                    StatusCode::BAD_REQUEST,
                ]
                .contains(&response.status())
            );
        }

        let expiring_auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
        );
        let work_context = state
            .vaults()
            .find_by_slug(&"work".parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .context()
            .unwrap();
        expiring_auth
            .issue_webdav_credential(
                &work_context,
                "expired",
                "expired",
                &SecretString::new("expired-password-123"),
                full_permissions(),
                Some(0),
            )
            .await
            .unwrap();
        let expired_credentials = STANDARD.encode("expired:expired-password-123");
        let response = app
            .clone()
            .oneshot(request(
                "GET",
                "/work/hello.md",
                &expired_credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        state
            .auth()
            .revoke_webdav_credential(
                &state
                    .vaults()
                    .find_by_slug(&"work".parse().unwrap())
                    .await
                    .unwrap()
                    .unwrap()
                    .context()
                    .unwrap(),
                credential_id,
            )
            .await
            .unwrap();
        let response = app
            .oneshot(request(
                "GET",
                "/work/hello.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn credentials_and_mounts_are_isolated_between_vaults() {
        let (directory, state, service, credentials, _) = setup().await;
        let second_context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("private").unwrap(),
            directory.path().join("private-content"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&second_context, "Private", VaultStatus::Active)
            .await
            .unwrap();

        let second_auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
        );
        second_auth
            .issue_webdav_credential(
                &second_context,
                "private-desktop",
                "private-desktop",
                &SecretString::new("private-password-123"),
                full_permissions(),
                None,
            )
            .await
            .unwrap();
        let second_credentials = STANDARD.encode("private-desktop:private-password-123");
        let app = client_router(service);

        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/private/secret.md",
                &credentials,
                Body::from("wrong Vault"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/private/secret.md",
                &second_credentials,
                Body::from("private"),
            ))
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ));

        let response = app
            .clone()
            .oneshot(request(
                "GET",
                "/work/secret.md",
                &second_credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(request(
                "GET",
                "/private/secret.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn forwarded_https_allows_non_loopback_basic_auth() {
        let (_, _, service, credentials, _) = setup().await;
        let peer = SocketAddr::from(([192, 0, 2, 44], 49_182));
        let public_app = client_router_with_peer(service, peer);
        let response = public_app
            .clone()
            .oneshot(request(
                "GET",
                "/work/no-forwarded-scheme.md",
                &credentials,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let mut insecure_forwarded = request(
            "GET",
            "/work/insecure-forwarded-scheme.md",
            &credentials,
            Body::empty(),
        );
        insecure_forwarded
            .headers_mut()
            .insert("x-forwarded-proto", "http".parse().unwrap());
        let response = public_app
            .clone()
            .oneshot(insecure_forwarded)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let mut put = request(
            "PUT",
            "/work/proxy.md",
            &credentials,
            Body::from("trusted proxy"),
        );
        put.headers_mut()
            .insert("x-forwarded-proto", "https".parse().unwrap());
        let response = public_app.oneshot(put).await.unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ));
    }
}
