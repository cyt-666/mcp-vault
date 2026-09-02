//! Stateless RMCP adapter for the Vault data plane.

use std::{path::PathBuf, sync::Arc};

use axum::{
    Router,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use mcp_vault_auth::{AuthError, AuthPrincipal, AuthService, OAuthResourceServer, OriginPolicy};
use mcp_vault_core::{
    MutationResult, ReadResult, RevisionReadResult, VaultCore, VaultCoreRuntime, VaultError,
};
use mcp_vault_domain::{
    MaintenanceGate, MemoryId, Permission, Revision, Scope, SourcePlane, VaultContext, VaultPath,
    VaultPathPolicy, VaultSlug,
};
use mcp_vault_indexer::{
    IndexError, IndexService, NoteRetrievalHit, NoteRetrievalMode, NoteRetrievalScope,
};
use mcp_vault_memory::{
    MemoryError, MemoryOrigin, MemoryService, MemorySourceInput, MemoryStatus, MemoryType,
    MemoryUpdateInput, RecallContext, RecallRequest, RememberInput,
};
use mcp_vault_state::{FileRecord, FileRevisionRecord, StateStore};
use mcp_vault_storage_fs::{ReadFile, StorageOptions};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, CompleteRequestMethod,
        CompleteRequestParams, CompleteResult, Implementation, ListPromptsRequestMethod,
        ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Resource, ResourceContents, ResourceTemplate, ServerCapabilities,
        ServerInfo, Tool,
    },
    schemars,
    service::RequestContext,
    tool, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::AsyncReadExt;
use url::Url;

mod oauth_server;

const SERVER_NAME: &str = "mcp-vault";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_TOOL_LIMIT: u32 = 100;
const MAX_READ_BYTES: u64 = 1024 * 1024;
const DEFAULT_READ_BYTES: u64 = 128 * 1024;
const LIST_CACHE_TTL_MS: u64 = 1_000;

/// Unconfigured mount retained for bootstrap and composition tests.
pub fn router() -> Router {
    Router::new().fallback(any(not_implemented))
}

/// Authenticated stateless MCP mount.
pub fn stateful_router(service: McpService) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_allowed_hosts(service.allowed_hosts.clone())
        .with_allowed_origins(
            service
                .auth_state
                .origin_policy
                .allowed_origins()
                .map(str::to_owned)
                .collect::<Vec<String>>(),
        );
    let handler = McpHandler::default();
    let rmcp = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let auth_state = service.auth_state;
    let auth_layer = middleware::from_fn(move |request: Request, next: Next| {
        let state = auth_state.clone();
        async move { authenticate_request_with_state(state, request, next).await }
    });
    Router::new().fallback_service(rmcp).layer(auth_layer)
}

/// Public RFC 9728 protected-resource metadata routes.
///
/// These routes deliberately sit outside MCP bearer middleware so an OAuth
/// client can discover the configured authorization server before it has a
/// token. The data-plane composition root mounts them at the origin root.
pub fn oauth_metadata_router(service: McpService) -> Router {
    let allowed_hosts = Arc::new(service.allowed_hosts.clone());
    let origin_policy = service.auth_state.origin_policy.clone();
    let public_origin = service.auth_state.public_origin.clone();
    let guard = middleware::from_fn(move |request: Request, next: Next| {
        let allowed_hosts = Arc::clone(&allowed_hosts);
        let origin_policy = origin_policy.clone();
        let public_origin = public_origin.clone();
        async move {
            // OAuth browser and token POSTs do not use ambient browser
            // authority. Authorization forms carry an opaque request handle
            // bound to the client, redirect, state, resource, scopes, and PKCE
            // challenge. Token requests repeat the exact client, redirect,
            // resource, and verifier while consuming a single-use code or a
            // rotating refresh token. System browsers and OpenAI hosts may
            // serialize either request with `Origin: null` or the invoking
            // application's Origin, so the MCP data-plane Origin allow-list is
            // not a security boundary for these two protocol requests. Keep
            // the configured Host check for every route and retain Origin
            // checks for metadata and DCR.
            let is_origin_independent_oauth_post = request.method() == Method::POST
                && matches!(
                    request.uri().path(),
                    oauth_server::AUTHORIZATION_PATH
                        | oauth_server::VERSIONED_V1_AUTHORIZATION_PATH
                        | oauth_server::LEGACY_AUTHORIZATION_PATH
                        | oauth_server::TOKEN_PATH
                );
            let host_allowed = request
                .headers()
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|host| {
                    allowed_hosts
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(host))
                });
            let configured_origin_allowed =
                origin_policy.validate_optional(request.headers()).is_ok();
            let mut supplied_origins = request.headers().get_all(header::ORIGIN).iter();
            let supplied_origin = supplied_origins.next();
            let public_origin_allowed = supplied_origins.next().is_none()
                && supplied_origin
                    .and_then(|value| value.to_str().ok())
                    .zip(public_origin.as_deref())
                    .is_some_and(|(supplied, configured)| supplied == configured);
            if !host_allowed
                || (!is_origin_independent_oauth_post
                    && !configured_origin_allowed
                    && !public_origin_allowed)
            {
                return public_error(StatusCode::FORBIDDEN, false);
            }
            next.run(request).await
        }
    });
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(root_protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp/v1/vaults/{vault_slug}",
            get(vault_protected_resource_metadata),
        )
        .merge(oauth_server::routes())
        .with_state(service)
        .layer(guard)
}

async fn not_implemented() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "MCP adapter is not configured\n",
    )
        .into_response()
}

/// Dependencies for one stateful MCP mount.
#[derive(Clone)]
pub struct McpService {
    auth_state: McpAuthState,
    allowed_hosts: Vec<String>,
}

impl McpService {
    /// Bind MCP to the shared state and deployment policies.
    pub fn new(
        state: StateStore,
        auth: AuthService,
        history_root: PathBuf,
        storage_options: StorageOptions,
        core_runtime: VaultCoreRuntime,
        allowed_hosts: Vec<String>,
        origin_policy: OriginPolicy,
    ) -> Self {
        let index = IndexService::new(state.clone());
        let memory = MemoryService::new(state.clone(), auth.clone());
        let maintenance = core_runtime.maintenance();
        Self {
            auth_state: McpAuthState {
                state,
                auth,
                index,
                memory,
                history_root,
                storage_options,
                core_runtime,
                origin_policy,
                maintenance,
                public_origin: None,
            },
            allowed_hosts,
        }
    }

    /// Set the canonical externally advertised data-plane origin.
    ///
    /// Configuration validation owns URL parsing. The OAuth adapter still
    /// compares the resulting resource identifier exactly with persisted
    /// issuer configuration before publishing metadata.
    pub fn with_public_origin(mut self, public_origin: Option<String>) -> Self {
        self.auth_state.public_origin = public_origin;
        self
    }

    /// Inject the process-shared memory/provider boundary assembled by the
    /// composition root.
    pub fn with_memory_service(mut self, memory: MemoryService) -> Self {
        self.auth_state.memory = memory;
        self
    }

    /// Inject the process-shared note retrieval and memory services.
    pub fn with_application_services(mut self, index: IndexService, memory: MemoryService) -> Self {
        self.auth_state.index = index;
        self.auth_state.memory = memory;
        self
    }
}

#[derive(Clone)]
struct McpAuthState {
    state: StateStore,
    auth: AuthService,
    index: IndexService,
    memory: MemoryService,
    history_root: PathBuf,
    storage_options: StorageOptions,
    core_runtime: VaultCoreRuntime,
    origin_policy: OriginPolicy,
    maintenance: MaintenanceGate,
    public_origin: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    scopes_supported: Vec<String>,
    bearer_methods_supported: Vec<&'static str>,
}

async fn vault_protected_resource_metadata(
    State(service): State<McpService>,
    Path(vault_slug): Path<String>,
) -> Response {
    let slug = match VaultSlug::new(&vault_slug) {
        Ok(slug) => slug,
        Err(_) => return public_error(StatusCode::NOT_FOUND, false),
    };
    match protected_resource_for_slug(&service, &slug).await {
        Ok(Some(metadata)) => metadata_response(metadata),
        Ok(None) => public_error(StatusCode::NOT_FOUND, false),
        Err(()) => public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
    }
}

async fn root_protected_resource_metadata(State(service): State<McpService>) -> Response {
    let vaults = match service.auth_state.state.vaults().list().await {
        Ok(vaults) => vaults,
        Err(_) => return public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
    };
    let mut candidates = Vec::new();
    for vault in vaults {
        if vault.status != mcp_vault_state::VaultStatus::Active {
            continue;
        }
        match protected_resource_for_slug(&service, &vault.slug).await {
            Ok(Some(metadata)) => candidates.push(metadata),
            Ok(None) => {}
            Err(()) => return public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
        }
    }
    candidates.sort_by(|left, right| left.resource.cmp(&right.resource));
    candidates.dedup_by(|left, right| left.resource == right.resource);
    if candidates.len() == 1 {
        metadata_response(candidates.remove(0))
    } else {
        public_error(StatusCode::NOT_FOUND, false)
    }
}

async fn protected_resource_for_slug(
    service: &McpService,
    slug: &VaultSlug,
) -> Result<Option<ProtectedResourceMetadata>, ()> {
    let vault = service
        .auth_state
        .state
        .vaults()
        .find_by_slug(slug)
        .await
        .map_err(|_| ())?;
    let Some(vault) = vault else {
        return Ok(None);
    };
    if service
        .auth_state
        .state
        .vaults()
        .availability(&vault)
        .await
        .map_err(|_| ())?
        != mcp_vault_state::VaultAvailability::Ready
    {
        return Ok(None);
    }
    let context = vault.context().map_err(|_| ())?;
    let resources = service
        .auth_state
        .auth
        .oauth_resource_servers()
        .await
        .map_err(|_| ())?;
    let selected =
        select_oauth_resource(resources, service.auth_state.public_origin.as_deref(), slug);
    let local = match oauth_server::issuer_origin(service) {
        Some(origin)
            if service
                .auth_state
                .auth
                .local_oauth_enabled(&context)
                .await
                .map_err(|_| ())? =>
        {
            let issuer = origin.trim_end_matches('/').to_owned();
            Some(OAuthResourceServer {
                resource: format!("{issuer}/mcp/v1/vaults/{slug}"),
                authorization_servers: vec![issuer],
            })
        }
        _ => None,
    };
    let resource = local
        .as_ref()
        .map(|local| local.resource.clone())
        .or_else(|| selected.as_ref().map(|selected| selected.resource.clone()));
    let Some(resource) = resource else {
        return Ok(None);
    };
    let mut authorization_servers = local
        .map(|local| local.authorization_servers)
        .unwrap_or_default();
    if let Some(external) = selected {
        authorization_servers.extend(external.authorization_servers);
    }
    let mut seen = std::collections::BTreeSet::new();
    authorization_servers.retain(|issuer| seen.insert(issuer.clone()));
    Ok(Some(ProtectedResourceMetadata {
        resource,
        authorization_servers,
        scopes_supported: Scope::ALL.map(|scope| scope.to_string()).to_vec(),
        bearer_methods_supported: vec!["header"],
    }))
}

fn select_oauth_resource(
    resources: Vec<OAuthResourceServer>,
    public_origin: Option<&str>,
    slug: &VaultSlug,
) -> Option<OAuthResourceServer> {
    let resource_path = format!("/mcp/v1/vaults/{slug}");
    if let Some(origin) = public_origin {
        let expected = format!("{}{resource_path}", origin.trim_end_matches('/'));
        return resources
            .into_iter()
            .find(|resource| resource.resource == expected);
    }

    let mut candidates = resources
        .into_iter()
        .filter(|resource| {
            Url::parse(&resource.resource)
                .ok()
                .is_some_and(|url| url.path() == resource_path && url.query().is_none())
        })
        .collect::<Vec<_>>();
    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

fn metadata_response(metadata: ProtectedResourceMetadata) -> Response {
    let mut response = axum::Json(metadata).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response
}

/// Request-scoped Vault binding passed from Axum into RMCP.
#[derive(Clone)]
pub struct McpRequestContext {
    /// Endpoint-bound Vault.
    pub vault: VaultContext,
    /// Credential-derived principal.
    pub principal: AuthPrincipal,
    /// Core bound to the Vault path policy.
    pub core: VaultCore,
    /// Operational state boundary.
    pub state: StateStore,
    /// Rebuildable lexical/index application service.
    pub index: IndexService,
    /// Durable sourced memory application service.
    pub memory: MemoryService,
    /// Shared process gate used to reject mutations during backup/restore.
    pub maintenance: MaintenanceGate,
}

async fn authenticate_request_with_state(
    state: McpAuthState,
    mut request: Request,
    next: Next,
) -> Response {
    let _request_operation = match state.maintenance.try_start_operation() {
        Some(operation) => operation,
        None => return public_error(StatusCode::SERVICE_UNAVAILABLE, false),
    };
    if state
        .origin_policy
        .validate_optional(request.headers())
        .is_err()
    {
        return public_error(StatusCode::FORBIDDEN, false);
    }
    let slug = match mounted_slug(request.uri().path()) {
        Ok(slug) => slug,
        Err(_) => return public_error(StatusCode::NOT_FOUND, false),
    };
    let token = match bearer_token(request.headers()).map(str::to_owned) {
        Ok(token) => token,
        Err(_) => {
            return oauth_public_error(
                StatusCode::UNAUTHORIZED,
                &slug,
                state.public_origin.as_deref(),
                "invalid_request",
                "A bearer access token is required",
            );
        }
    };
    match authenticate_request_context(&state, slug, token).await {
        Ok(context) => {
            request.extensions_mut().insert(context);
            next.run(request).await
        }
        Err(response) => response,
    }
}

async fn authenticate_request_context(
    state: &McpAuthState,
    slug: VaultSlug,
    token: String,
) -> Result<McpRequestContext, Response> {
    let vault = state
        .state
        .vaults()
        .find_by_slug(&slug)
        .await
        .map_err(|_| public_error(StatusCode::INTERNAL_SERVER_ERROR, false))?
        .ok_or_else(|| public_error(StatusCode::NOT_FOUND, false))?;
    match state
        .state
        .vaults()
        .availability(&vault)
        .await
        .map_err(|_| public_error(StatusCode::INTERNAL_SERVER_ERROR, false))?
    {
        mcp_vault_state::VaultAvailability::Disabled => {
            return Err(public_error(StatusCode::NOT_FOUND, false));
        }
        mcp_vault_state::VaultAvailability::Initializing
        | mcp_vault_state::VaultAvailability::Error => {
            return Err(public_error(StatusCode::SERVICE_UNAVAILABLE, false));
        }
        mcp_vault_state::VaultAvailability::Ready
        | mcp_vault_state::VaultAvailability::Maintenance => {}
    }
    let context = vault
        .context()
        .map_err(|_| public_error(StatusCode::INTERNAL_SERVER_ERROR, false))?;
    let principal_result = if token.starts_with("mcpv_pat_") {
        state
            .auth
            .authenticate_pat(&context, &token, &[], None)
            .await
    } else if token.starts_with("mcpv_oauth_") {
        match state
            .public_origin
            .as_deref()
            .filter(|origin| oauth_issuer_origin_is_secure(origin))
        {
            Some(origin) => {
                let resource = format!("{}/mcp/v1/vaults/{slug}", origin.trim_end_matches('/'));
                state
                    .auth
                    .authenticate_local_oauth(&context, &token, &resource, &[], None)
                    .await
            }
            None => Err(AuthError::OAuthConfiguration),
        }
    } else {
        state
            .auth
            .authenticate_oauth(&context, &token, &[], None)
            .await
    };
    let principal = principal_result.map_err(|error| match error {
        AuthError::State(_) => public_error(StatusCode::INTERNAL_SERVER_ERROR, false),
        _ => oauth_public_error(
            StatusCode::UNAUTHORIZED,
            &slug,
            state.public_origin.as_deref(),
            "invalid_token",
            "The bearer access token is invalid or expired",
        ),
    })?;
    if principal.vault_id != Some(context.id()) {
        return Err(oauth_public_error(
            StatusCode::UNAUTHORIZED,
            &slug,
            state.public_origin.as_deref(),
            "invalid_token",
            "The bearer access token is invalid for this resource",
        ));
    }
    let policy = VaultPathPolicy::new(vault.reserved_root.clone(), Default::default())
        .map_err(|_| public_error(StatusCode::INTERNAL_SERVER_ERROR, false))?;
    let core = VaultCore::new(
        state.state.clone(),
        state.history_root.clone(),
        policy,
        state.storage_options,
        state.core_runtime.clone(),
    );
    Ok(McpRequestContext {
        vault: context,
        principal,
        core,
        state: state.state.clone(),
        index: state.index.clone(),
        memory: state.memory.clone(),
        maintenance: state.maintenance.clone(),
    })
}

fn oauth_issuer_origin_is_secure(origin: &str) -> bool {
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn mounted_slug(path: &str) -> Result<VaultSlug, ()> {
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let relative = if let Some(index) = segments
        .windows(3)
        .position(|window| window == ["mcp", "v1", "vaults"])
    {
        &segments[index + 3..]
    } else {
        &segments
    };
    if relative.len() != 1 {
        return Err(());
    }
    VaultSlug::new(relative[0]).map_err(|_| ())
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ()> {
    let value = headers.get(header::AUTHORIZATION).ok_or(())?;
    let value = value.to_str().map_err(|_| ())?;
    let mut parts = value.split(' ');
    let scheme = parts.next().ok_or(())?;
    let token = parts.next().ok_or(())?;
    if parts.next().is_some()
        || !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.chars().any(char::is_control)
    {
        return Err(());
    }
    Ok(token)
}

fn public_error(status: StatusCode, challenge: bool) -> Response {
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "request rejected\n",
    )
        .into_response();
    if challenge {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"mcp-vault\""),
        );
    }
    response
}

fn oauth_public_error(
    status: StatusCode,
    slug: &VaultSlug,
    public_origin: Option<&str>,
    error: &'static str,
    description: &'static str,
) -> Response {
    let mut response = public_error(status, false);
    let metadata_path = format!("/.well-known/oauth-protected-resource/mcp/v1/vaults/{slug}");
    let metadata_url = public_origin
        .map(|origin| format!("{}{metadata_path}", origin.trim_end_matches('/')))
        .unwrap_or(metadata_path);
    let challenge = format!(
        "Bearer realm=\"mcp-vault\", resource_metadata=\"{metadata_url}\", error=\"{error}\", error_description=\"{description}\""
    );
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct VaultOverviewInput {
    #[serde(default)]
    include_recent: Option<bool>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct BrowseIndexInput {
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    depth: Option<u8>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    include_note_candidates: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct RecentChangesInput {
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct SearchNotesInput {
    query: String,
    #[serde(default)]
    mode: Option<SearchMode>,
    #[serde(default)]
    scope: Option<SearchScope>,
    #[serde(default)]
    result_granularity: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    include_score_breakdown: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SearchMode {
    #[default]
    Lexical,
    Semantic,
    Hybrid,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct SearchScope {
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    topic_ids: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    modified_after: Option<i64>,
    #[serde(default)]
    modified_before: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct ReadNoteInput {
    path: String,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    selection: Option<NoteSelection>,
    #[serde(default)]
    max_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NoteSelection {
    Full,
    LineRange { start_line: u32, end_line: u32 },
    Heading { heading: String },
    ByteRange { start: u64, end: u64 },
}

impl NoteSelection {
    fn is_full(&self) -> bool {
        match self {
            Self::Full => true,
            Self::LineRange {
                start_line,
                end_line,
            } => {
                let _ = (start_line, end_line);
                false
            }
            Self::Heading { heading } => {
                let _ = heading;
                false
            }
            Self::ByteRange { start, end } => {
                let _ = (start, end);
                false
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct CreateNoteInput {
    path: String,
    content: String,
    #[serde(default)]
    if_absent: Option<bool>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct EditNoteInput {
    path: String,
    expected_revision: u64,
    operation: EditOperation,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum EditOperation {
    ReplaceAll {
        content: String,
    },
    ApplyUnifiedDiff {
        patch: String,
    },
    Append {
        content: String,
    },
    InsertAfterHeading {
        heading: String,
        insertion: String,
    },
    ReplaceHeadingSection {
        heading: String,
        replacement: String,
    },
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct MoveNoteInput {
    source: String,
    destination: String,
    expected_revision: u64,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct DeleteNoteInput {
    path: String,
    expected_revision: u64,
    #[serde(default)]
    mode: DeleteMode,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DeleteMode {
    #[default]
    Trash,
    Permanent,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct NoteHistoryInput {
    path: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct RestoreNoteRevisionInput {
    path: String,
    revision: u64,
    expected_current_revision: u64,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct RecallMemoryInput {
    query: String,
    #[serde(default)]
    context: Option<RecallMemoryContextInput>,
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    valid_at: Option<i64>,
    #[serde(default)]
    min_importance: Option<f64>,
    #[serde(default)]
    include_historical: Option<bool>,
    #[serde(default)]
    include_sources: Option<bool>,
    #[serde(default)]
    include_score_breakdown: Option<bool>,
    #[serde(default)]
    max_results: Option<u32>,
    #[serde(default)]
    max_related_notes: Option<u32>,
    #[serde(default)]
    max_tokens: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct RecallMemoryContextInput {
    #[serde(default)]
    active_project: Option<String>,
    #[serde(default)]
    entities: Vec<String>,
    #[serde(default)]
    recent_topics: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct MemoryIdInput {
    id: String,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct ListMemoryInput {
    #[serde(default)]
    statuses: Vec<String>,
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct RememberMemoryInput {
    content: String,
    memory_type: String,
    importance: f64,
    confidence: f64,
    #[serde(default)]
    valid_from: Option<i64>,
    #[serde(default)]
    valid_to: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    entities: Vec<String>,
    #[serde(default)]
    source_note: Option<MemorySourceInputDto>,
    #[serde(default)]
    supersedes: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct MemorySourceInputDto {
    path: String,
    #[serde(default)]
    file_id: Option<String>,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    heading: Vec<String>,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
    #[serde(default)]
    excerpt_hash: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct UpdateMemoryInput {
    id: String,
    expected_revision: u64,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    memory_type: Option<String>,
    #[serde(default)]
    importance: Option<f64>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    valid_from: Option<Option<i64>>,
    #[serde(default)]
    valid_to: Option<Option<i64>>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    entities: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct ForgetMemoryInput {
    id: String,
    expected_revision: u64,
    #[serde(default)]
    permanent: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
struct ToolErrorBody {
    code: String,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl ToolErrorBody {
    fn new(code: &'static str, message: &'static str, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
            details: None,
        }
    }

    fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
struct ToolEnvelope {
    request_id: String,
    ok: bool,
    /// All MCP Vault tool payloads are JSON objects. Keeping this explicit in
    /// the advertised schema also preserves compatibility with the dated
    /// MCP schemas, which reject an unconstrained `true` value schema here.
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ToolErrorBody>,
}

#[derive(Clone)]
struct McpHandler {
    tool_router: ToolRouter<Self>,
}

impl Default for McpHandler {
    fn default() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

enum ReadSource {
    Current(ReadResult),
    Historical(RevisionReadResult),
}

#[tool_router]
impl McpHandler {
    #[tool(
        name = "vault_overview",
        description = "Return bounded Vault identity, statistics, direct entries, and recent change metadata.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn vault_overview(
        &self,
        Parameters(input): Parameters<VaultOverviewInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::DiscoverVault) {
            return Ok(error_result(&context, error));
        }
        let limit = match bounded_limit(input.limit, 25) {
            Ok(limit) => limit,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match overview_data(&request, input.include_recent.unwrap_or(false), limit).await {
            Ok(data) => Ok(success_result(&context, data)),
            Err(error) => Ok(error_result(&context, error)),
        }
    }

    #[tool(
        name = "browse_index",
        description = "Browse a bounded deterministic Vault tree through Vault Core.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn browse_index(
        &self,
        Parameters(input): Parameters<BrowseIndexInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::DiscoverVault) {
            return Ok(error_result(&context, error));
        }
        let limit = match bounded_limit(input.limit, 50) {
            Ok(limit) => limit,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let depth = input.depth.unwrap_or(1);
        if depth > 2 {
            return Ok(error_result(
                &context,
                ToolErrorBody::new("invalid_argument", "depth must not exceed 2", false),
            ));
        }
        let node = match parse_node_id(input.node_id.as_deref()) {
            Ok(node) => node,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let offset = match parse_cursor(input.cursor.as_deref()) {
            Ok(offset) => offset,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match browse_data(
            &request,
            &node,
            depth,
            limit,
            offset,
            input.include_note_candidates.unwrap_or(false),
        )
        .await
        {
            Ok(data) => Ok(success_result(&context, data)),
            Err(error) => Ok(error_result(&context, error)),
        }
    }

    #[tool(
        name = "recent_changes",
        description = "Return bounded Vault-scoped immutable revision metadata in newest-first order.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn recent_changes(
        &self,
        Parameters(input): Parameters<RecentChangesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::DiscoverVault) {
            return Ok(error_result(&context, error));
        }
        let limit = match bounded_limit(input.limit, 50) {
            Ok(limit) => limit,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let revisions = match request
            .state
            .files()
            .list_recent_revisions(&request.vault, limit)
            .await
        {
            Ok(revisions) => revisions,
            Err(_) => {
                return Ok(error_result(
                    &context,
                    ToolErrorBody::new(
                        "temporarily_unavailable",
                        "recent changes are temporarily unavailable",
                        true,
                    ),
                ));
            }
        };
        Ok(success_result(
            &context,
            json!({
                "changes": revisions.iter().map(revision_json).collect::<Vec<_>>(),
                "limit": limit,
            }),
        ))
    }

    #[tool(
        name = "search_notes",
        description = "Search indexed Markdown source material with bounded lexical filters.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn search_notes(
        &self,
        Parameters(input): Parameters<SearchNotesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::ReadVault) {
            return Ok(error_result(&context, error));
        }
        let limit = match bounded_limit(input.limit, 12) {
            Ok(limit) => limit,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let offset = match parse_cursor(input.cursor.as_deref()) {
            Ok(offset) => offset,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let scope = input.scope.clone().unwrap_or_default();
        if scope.topic_ids.len() > 20 || scope.tags.len() > 20 {
            return Ok(error_result(
                &context,
                ToolErrorBody::new("invalid_argument", "too many search filters", false),
            ));
        }
        let mode = input.mode.unwrap_or_default();
        match search_data(&request, &input, &scope, mode, limit, offset).await {
            Ok(data) => Ok(success_result(&context, data)),
            Err(error) => Ok(error_result(&context, error)),
        }
    }

    #[tool(
        name = "read_note",
        description = "Read bounded UTF-8 note content or safe binary metadata through Vault Core.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn read_note(
        &self,
        Parameters(input): Parameters<ReadNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::ReadVault) {
            return Ok(error_result(&context, error));
        }
        if input
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_full())
        {
            return Ok(error_result(
                &context,
                ToolErrorBody::new(
                    "unsupported_selection",
                    "only full note reads are available until indexed selection support is enabled",
                    false,
                ),
            ));
        }
        let max_bytes = match bounded_read_bytes(input.max_bytes) {
            Ok(max_bytes) => max_bytes,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let path = match parse_tool_path(&input.path) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let read = match input.revision.map(Revision::new) {
            Some(revision) => match request
                .core
                .read_revision(&request.vault, &path, revision)
                .await
            {
                Ok(read) => ReadSource::Historical(read),
                Err(error) => return Ok(error_result(&context, vault_error(error))),
            },
            None => match request.core.read(&request.vault, &path).await {
                Ok(read) => ReadSource::Current(read),
                Err(error) => return Ok(error_result(&context, vault_error(error))),
            },
        };
        let (file, revision, reader) = match read {
            ReadSource::Current(read) => {
                (read.file.clone(), read.file.current_revision, read.reader)
            }
            ReadSource::Historical(read) => (read.file, read.revision.revision, read.reader),
        };
        let (bytes, truncated) = match read_bounded(reader, max_bytes).await {
            Ok(value) => value,
            Err(_) => {
                return Ok(error_result(
                    &context,
                    ToolErrorBody::new("internal_error", "note content could not be read", true),
                ));
            }
        };
        let text = String::from_utf8(bytes).ok();
        let binary = text.is_none();
        Ok(success_result(
            &context,
            json!({
                "path": path.as_str(),
                "revision": revision.value(),
                "selection": {"kind": "full"},
                "size": file.size,
                "content_hash": file.content_hash,
                "truncated": truncated,
                "content": text,
                "binary": binary,
                "resource_uri": note_resource_uri(&path),
            }),
        ))
    }

    #[tool(
        name = "recall",
        description = "Recall durable sourced context plus related ordinary-note cues for the current task without a query-time generative LLM; read returned note sources for exact details.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn recall(
        &self,
        Parameters(input): Parameters<RecallMemoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::ReadMemory) {
            return Ok(error_result(&context, error));
        }
        let types = match input
            .types
            .iter()
            .map(|value| parse_memory_type(value))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(types) => types,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let continuity = input.context.unwrap_or_default();
        let include_related_notes = request
            .principal
            .permissions
            .contains(Permission::ReadVault);
        let result = request
            .memory
            .recall(
                &request.vault,
                RecallRequest {
                    query: input.query,
                    context: RecallContext {
                        active_project: continuity.active_project,
                        entities: continuity.entities,
                        recent_topics: continuity.recent_topics,
                    },
                    types,
                    valid_at: input.valid_at,
                    min_importance: input.min_importance.unwrap_or(0.0),
                    include_historical: input.include_historical.unwrap_or(false),
                    include_sources: input.include_sources.unwrap_or(false),
                    include_score_breakdown: input.include_score_breakdown.unwrap_or(false),
                    include_related_notes,
                    max_results: input.max_results.unwrap_or(12),
                    max_related_notes: if include_related_notes {
                        input.max_related_notes.unwrap_or(8)
                    } else {
                        0
                    },
                    max_tokens: input.max_tokens.unwrap_or(1800),
                },
            )
            .await;
        match result {
            Ok(result) => Ok(success_result(&context, recall_json(result))),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "get_memory",
        description = "Inspect one durable memory, its lifecycle, canonical Markdown path, and provenance.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn get_memory(
        &self,
        Parameters(input): Parameters<MemoryIdInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::ReadMemory) {
            return Ok(error_result(&context, error));
        }
        let id = match parse_memory_id(&input.id) {
            Ok(id) => id,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request.memory.get(&request.vault, id).await {
            Ok(memory) => Ok(success_result(
                &context,
                serde_json::to_value(memory).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "list_memories",
        description = "Browse durable memories deliberately with bounded lifecycle, type, tag, entity, and source filters.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn list_memories(
        &self,
        Parameters(input): Parameters<ListMemoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::ReadMemory) {
            return Ok(error_result(&context, error));
        }
        let statuses = match input
            .statuses
            .iter()
            .map(|value| parse_memory_status(value))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(statuses) => statuses,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let types = match input
            .types
            .iter()
            .map(|value| parse_memory_type(value))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(types) => types,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let limit = match bounded_limit(input.limit, 50) {
            Ok(limit) => limit,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let offset = match parse_cursor(input.cursor.as_deref()) {
            Ok(offset) => offset,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request
            .memory
            .list(
                &request.vault,
                statuses,
                types,
                input.tag,
                input.entity,
                input.source_path,
                limit,
                offset,
            )
            .await
        {
            Ok(memories) => Ok(success_result(
                &context,
                json!({
                    "memories": memories,
                    "next_cursor": (memories.len() == limit as usize).then(|| format!("offset:{}", offset.saturating_add(limit))),
                    "truncated": memories.len() == limit as usize
                }),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "remember",
        description = "Stage one explicit sourced memory input for durable background consolidation. Returns the raw input and consolidation job IDs; final recall changes after Phase 2 commits.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn remember(
        &self,
        Parameters(input): Parameters<RememberMemoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::WriteMemory) {
            return Ok(error_result(&context, error));
        }
        let memory_type = match parse_memory_type(&input.memory_type) {
            Ok(memory_type) => memory_type,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let supersedes = match input.supersedes.as_deref() {
            Some(value) => match parse_memory_id(value) {
                Ok(id) => Some(id),
                Err(error) => return Ok(error_result(&context, error)),
            },
            None => None,
        };
        let source = match input.source_note {
            Some(source) => {
                let path = match parse_tool_path(&source.path) {
                    Ok(path) => path,
                    Err(error) => return Ok(error_result(&context, error)),
                };
                let file_id = match source.file_id.as_deref() {
                    Some(value) => match value.parse() {
                        Ok(id) => Some(id),
                        Err(_) => {
                            return Ok(error_result(
                                &context,
                                ToolErrorBody::new(
                                    "invalid_argument",
                                    "source file_id is invalid",
                                    false,
                                ),
                            ));
                        }
                    },
                    None => None,
                };
                vec![MemorySourceInput {
                    source_type: "note".to_owned(),
                    note_file_id: file_id,
                    note_path: Some(path),
                    note_revision: source.revision.map(Revision::new),
                    heading_path: source.heading,
                    start_line: source.start_line,
                    end_line: source.end_line,
                    excerpt_hash: source.excerpt_hash,
                    actor_id: request
                        .principal
                        .actor
                        .actor_id()
                        .map(|value| value.as_str().to_owned()),
                }]
            }
            None => Vec::new(),
        };
        match request
            .memory
            .remember_as(
                &request.vault,
                &request.core,
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                RememberInput {
                    content: input.content,
                    memory_type,
                    importance: input.importance,
                    confidence: input.confidence,
                    valid_from: input.valid_from,
                    valid_to: input.valid_to,
                    tags: input.tags,
                    entities: input.entities,
                    sources: source,
                    supersedes,
                    idempotency_key: input.idempotency_key,
                    origin: MemoryOrigin::ExplicitAgent,
                    extraction: json!({}),
                },
            )
            .await
        {
            Ok(result) => Ok(success_result(
                &context,
                json!({
                    "outcome": result.outcome,
                    "memory": result.memory,
                    "raw_memory_id": result.raw_memory_id,
                    "consolidation_job_id": result.consolidation_job_id,
                }),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "update_memory",
        description = "Update one durable memory under its expected metadata revision.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn update_memory(
        &self,
        Parameters(input): Parameters<UpdateMemoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::ManageMemory) {
            return Ok(error_result(&context, error));
        }
        let id = match parse_memory_id(&input.id) {
            Ok(id) => id,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let memory_type = match input.memory_type.as_deref() {
            Some(value) => match parse_memory_type(value) {
                Ok(value) => Some(value),
                Err(error) => return Ok(error_result(&context, error)),
            },
            None => None,
        };
        match request
            .memory
            .update(
                &request.vault,
                &request.core,
                id,
                Revision::new(input.expected_revision),
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
            Ok(memory) => Ok(success_result(
                &context,
                serde_json::to_value(memory).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "forget_memory",
        description = "Archive or explicitly permanently delete one durable memory under an expected revision.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn forget_memory(
        &self,
        Parameters(input): Parameters<ForgetMemoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::ManageMemory) {
            return Ok(error_result(&context, error));
        }
        let id = match parse_memory_id(&input.id) {
            Ok(id) => id,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request
            .memory
            .forget(
                &request.vault,
                &request.core,
                id,
                Revision::new(input.expected_revision),
                input.permanent.unwrap_or(false),
            )
            .await
        {
            Ok(memory) => Ok(success_result(
                &context,
                serde_json::to_value(memory).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "create_note",
        description = "Create a new canonical note with an absent-path precondition and idempotency key.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn create_note(
        &self,
        Parameters(input): Parameters<CreateNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::WriteVault) {
            return Ok(error_result(&context, error));
        }
        if input.if_absent == Some(false) {
            return Ok(error_result(
                &context,
                ToolErrorBody::new(
                    "invalid_argument",
                    "create_note requires if_absent=true",
                    false,
                ),
            ));
        }
        if input.content.len() > MAX_READ_BYTES as usize {
            return Ok(error_result(
                &context,
                ToolErrorBody::new(
                    "payload_too_large",
                    "note content exceeds the MCP limit",
                    false,
                ),
            ));
        }
        let path = match parse_tool_path(&input.path) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request
            .core
            .create_bytes(
                &request.vault,
                &path,
                input.content.as_bytes(),
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                input.idempotency_key.as_deref(),
            )
            .await
        {
            Ok(result) => Ok(success_result(&context, mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "edit_note",
        description = "Apply one exact revision-checked note edit through Vault Core.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn edit_note(
        &self,
        Parameters(input): Parameters<EditNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::WriteVault) {
            return Ok(error_result(&context, error));
        }
        let path = match parse_tool_path(&input.path) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let expected = Revision::new(input.expected_revision);
        let actor = request.principal.actor.clone();
        let result = match input.operation {
            EditOperation::ReplaceAll { content } => {
                if content.len() > MAX_READ_BYTES as usize {
                    return Ok(error_result(
                        &context,
                        ToolErrorBody::new(
                            "payload_too_large",
                            "note content exceeds the MCP limit",
                            false,
                        ),
                    ));
                }
                request
                    .core
                    .replace_bytes(
                        &request.vault,
                        &path,
                        expected,
                        content.as_bytes(),
                        actor,
                        SourcePlane::Mcp,
                        input.idempotency_key.as_deref(),
                    )
                    .await
            }
            EditOperation::ApplyUnifiedDiff { patch } => {
                request
                    .core
                    .patch_unified_diff(
                        &request.vault,
                        &path,
                        expected,
                        &patch,
                        actor,
                        SourcePlane::Mcp,
                        input.idempotency_key.as_deref(),
                    )
                    .await
            }
            EditOperation::Append { content } => {
                request
                    .core
                    .append_bytes(
                        &request.vault,
                        &path,
                        expected,
                        content.as_bytes(),
                        actor,
                        SourcePlane::Mcp,
                        input.idempotency_key.as_deref(),
                    )
                    .await
            }
            EditOperation::InsertAfterHeading { heading, insertion } => {
                request
                    .core
                    .insert_after_heading(
                        &request.vault,
                        &path,
                        expected,
                        &heading,
                        &insertion,
                        actor,
                        SourcePlane::Mcp,
                        input.idempotency_key.as_deref(),
                    )
                    .await
            }
            EditOperation::ReplaceHeadingSection {
                heading,
                replacement,
            } => {
                request
                    .core
                    .replace_heading_section(
                        &request.vault,
                        &path,
                        expected,
                        &heading,
                        &replacement,
                        actor,
                        SourcePlane::Mcp,
                        input.idempotency_key.as_deref(),
                    )
                    .await
            }
        };
        match result {
            Ok(result) => Ok(success_result(&context, mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "move_note",
        description = "Move a note or directory with an exact source revision precondition.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn move_note(
        &self,
        Parameters(input): Parameters<MoveNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::WriteVault) {
            return Ok(error_result(&context, error));
        }
        let source = match parse_tool_path(&input.source) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let destination = match parse_tool_path(&input.destination) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request
            .core
            .move_entry(
                &request.vault,
                &source,
                &destination,
                Revision::new(input.expected_revision),
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                input.idempotency_key.as_deref(),
            )
            .await
        {
            Ok(result) => Ok(success_result(&context, mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "delete_note",
        description = "Tombstone a note after an exact revision check; permanent deletion is not exposed.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn delete_note(
        &self,
        Parameters(input): Parameters<DeleteNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::DeleteVault) {
            return Ok(error_result(&context, error));
        }
        if matches!(input.mode, DeleteMode::Permanent) {
            return Ok(error_result(
                &context,
                ToolErrorBody::new(
                    "unsupported_mode",
                    "permanent deletion is not enabled for this Vault",
                    false,
                ),
            ));
        }
        let path = match parse_tool_path(&input.path) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request
            .core
            .delete(
                &request.vault,
                &path,
                Revision::new(input.expected_revision),
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                input.idempotency_key.as_deref(),
            )
            .await
        {
            Ok(result) => Ok(success_result(&context, mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "note_history",
        description = "Return immutable revision metadata for one Vault-relative note path.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn note_history(
        &self,
        Parameters(input): Parameters<NoteHistoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::ReadHistory) {
            return Ok(error_result(&context, error));
        }
        let path = match parse_tool_path(&input.path) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request.core.history(&request.vault, &path).await {
            Ok(history) => Ok(success_result(
                &context,
                json!({
                    "path": path.as_str(),
                    "revisions": history.iter().map(revision_json).collect::<Vec<_>>(),
                }),
            )),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "restore_note_revision",
        description = "Restore one retained revision as a new current revision after two precondition checks.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn restore_note_revision(
        &self,
        Parameters(input): Parameters<RestoreNoteRevisionInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        for permission in [Permission::ReadHistory, Permission::WriteVault] {
            if let Err(error) = require_permission(&request.principal, permission) {
                return Ok(error_result(&context, error));
            }
        }
        let path = match parse_tool_path(&input.path) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request
            .core
            .restore(
                &request.vault,
                &path,
                Revision::new(input.revision),
                Revision::new(input.expected_current_revision),
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                input.idempotency_key.as_deref(),
            )
            .await
        {
            Ok(result) => Ok(success_result(&context, mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
        .with_server_info(Implementation::new(SERVER_NAME, SERVER_VERSION))
        .with_instructions(
            "This server is the user's persistent Markdown knowledge Vault.\n\
             Use vault_overview or browse_index when you need to understand the available knowledge.\n\
             Use recall proactively when the task may depend on prior decisions, preferences, constraints, project state, past work, or knowledge that may already exist in the Vault. Treat related_notes as retrieval cues, then use read_note to verify exact source material.\n\
             Use mutation tools only when the user requests or clearly authorizes a persistent change. Preserve revisions and never overwrite a revision conflict.\n\
             MCP Vault binds each request to the Vault slug in the endpoint and the bearer credential. Recall is projection-based and does not require a query-time LLM; semantic providers are optional and report degradation.",
        )
    }

    async fn complete(
        &self,
        _request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        Err(ErrorData::method_not_found::<CompleteRequestMethod>())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Err(ErrorData::method_not_found::<ListPromptsRequestMethod>())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let tool_context = ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_context).await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let request = request_context(&context)?;
        let mut tools = self.tool_router.list_all();
        tools.retain(|tool| tool_allowed(&request.principal, tool.name.as_ref()));
        tools.sort_by_key(|tool| tool_order(tool.name.as_ref()));
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(LIST_CACHE_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let request = request_context(&context)?;
        let can_discover = request
            .principal
            .permissions
            .contains(Permission::DiscoverVault);
        let can_read_memory = request
            .principal
            .permissions
            .contains(Permission::ReadMemory);
        if !can_discover && !can_read_memory {
            return Ok(ListResourcesResult::with_all_items(Vec::new())
                .with_ttl_ms(LIST_CACHE_TTL_MS)
                .with_cache_scope(CacheScope::Private));
        }
        let mut resources = Vec::new();
        if can_discover {
            resources.extend([
                Resource::new("vault://overview", "vault-overview")
                    .with_description("Bounded Vault identity and statistics")
                    .with_mime_type("application/json"),
                Resource::new("vault://index/root", "vault-index-root")
                    .with_description("Bounded deterministic Vault tree")
                    .with_mime_type("application/json"),
                Resource::new("vault://recent", "vault-recent-changes")
                    .with_description("Recent immutable revision metadata")
                    .with_mime_type("application/json"),
            ]);
        }
        if can_read_memory {
            resources.push(
                Resource::new("vault://memory/context", "vault-memory-context")
                    .with_description("Compact high-value active memory context")
                    .with_mime_type("application/json"),
            );
        }
        Ok(ListResourcesResult::with_all_items(resources)
            .with_ttl_ms(LIST_CACHE_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let request = request_context(&context)?;
        let can_read_vault = request
            .principal
            .permissions
            .contains(Permission::ReadVault);
        let can_read_memory = request
            .principal
            .permissions
            .contains(Permission::ReadMemory);
        if !can_read_vault && !can_read_memory {
            return Ok(ListResourceTemplatesResult::with_all_items(Vec::new())
                .with_ttl_ms(LIST_CACHE_TTL_MS)
                .with_cache_scope(CacheScope::Private));
        }
        let mut templates = Vec::new();
        if can_read_vault {
            templates.push(
                ResourceTemplate::new("vault://note/{+path}", "vault-note")
                    .with_description("UTF-8 canonical note content")
                    .with_mime_type("text/markdown"),
            );
        }
        if can_read_memory {
            templates.push(
                ResourceTemplate::new("vault://memory/{memory_id}", "vault-memory")
                    .with_description("Durable memory and provenance")
                    .with_mime_type("application/json"),
            );
        }
        Ok(ListResourceTemplatesResult::with_all_items(templates)
            .with_ttl_ms(LIST_CACHE_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let request_context = request_context(&context)?;
        let url = Url::parse(&request.uri)
            .map_err(|_| ErrorData::invalid_params("resource URI is invalid", None))?;
        if url.scheme() != "vault"
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ErrorData::invalid_params(
                "resource URI is not supported",
                None,
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| ErrorData::invalid_params("resource URI is not supported", None))?;
        let contents = match host {
            "overview" | "recent" | "index" => {
                AuthService::require_permission(
                    &request_context.principal,
                    Permission::DiscoverVault,
                )
                .map_err(|_| ErrorData::invalid_params("resource is not available", None))?;
                let payload = match host {
                    "overview" => overview_data(&request_context, false, 25)
                        .await
                        .map_err(|_| ErrorData::internal_error("resource is unavailable", None))?,
                    "recent" => {
                        let rows = request_context
                            .state
                            .files()
                            .list_recent_revisions(&request_context.vault, 50)
                            .await
                            .map_err(|_| {
                                ErrorData::internal_error("resource is unavailable", None)
                            })?;
                        json!({"changes": rows.iter().map(revision_json).collect::<Vec<_>>()})
                    }
                    "index" => {
                        let node = parse_resource_index_path(url.path())?;
                        browse_data(&request_context, &node, 1, 50, 0, true)
                            .await
                            .map_err(|_| {
                                ErrorData::internal_error("resource is unavailable", None)
                            })?
                    }
                    _ => unreachable!(),
                };
                ResourceContents::text(payload.to_string(), request.uri.clone())
                    .with_mime_type("application/json")
            }
            "note" => {
                AuthService::require_permission(&request_context.principal, Permission::ReadVault)
                    .map_err(|_| ErrorData::invalid_params("resource is not available", None))?;
                let path = VaultPath::from_url_path(url.path())
                    .map_err(|_| ErrorData::invalid_params("resource path is invalid", None))?;
                let ReadResult { reader, .. } = request_context
                    .core
                    .read(&request_context.vault, &path)
                    .await
                    .map_err(|_| ErrorData::invalid_params("resource is not available", None))?;
                let (bytes, truncated) = read_bounded(reader, DEFAULT_READ_BYTES)
                    .await
                    .map_err(|_| ErrorData::internal_error("resource is unavailable", None))?;
                if truncated {
                    return Err(ErrorData::invalid_params(
                        "resource exceeds the read limit",
                        None,
                    ));
                }
                let text = String::from_utf8(bytes)
                    .map_err(|_| ErrorData::invalid_params("resource is not UTF-8 text", None))?;
                ResourceContents::text(text, request.uri.clone()).with_mime_type("text/markdown")
            }
            "memory" => {
                AuthService::require_permission(&request_context.principal, Permission::ReadMemory)
                    .map_err(|_| ErrorData::invalid_params("resource is not available", None))?;
                let value = url.path().trim_matches('/');
                if value == "context" {
                    let memories = request_context
                        .memory
                        .list(
                            &request_context.vault,
                            vec![MemoryStatus::Active],
                            Vec::new(),
                            None,
                            None,
                            None,
                            12,
                            0,
                        )
                        .await
                        .map_err(|_| {
                            ErrorData::internal_error("memory resource is unavailable", None)
                        })?;
                    ResourceContents::text(
                        serde_json::to_string(&json!({"memories": memories}))
                            .unwrap_or_else(|_| "{}".to_owned()),
                        request.uri.clone(),
                    )
                    .with_mime_type("application/json")
                } else {
                    let memory_id = MemoryId::parse(value).map_err(|_| {
                        ErrorData::invalid_params("memory resource is invalid", None)
                    })?;
                    let memory = request_context
                        .memory
                        .get(&request_context.vault, memory_id)
                        .await
                        .map_err(|_| ErrorData::invalid_params("memory is not available", None))?;
                    ResourceContents::text(
                        serde_json::to_string(&memory).unwrap_or_else(|_| "{}".to_owned()),
                        request.uri.clone(),
                    )
                    .with_mime_type("application/json")
                }
            }
            _ => return Err(ErrorData::invalid_params("resource is not supported", None)),
        };
        Ok(ReadResourceResult::new(vec![contents])
            .with_cache_scope(CacheScope::Private)
            .with_ttl_ms(LIST_CACHE_TTL_MS)
            .into())
    }
}

fn request_context(context: &RequestContext<RoleServer>) -> Result<McpRequestContext, ErrorData> {
    let parts = context
        .extensions
        .get::<Parts>()
        .ok_or_else(|| ErrorData::internal_error("MCP request context is unavailable", None))?;
    parts
        .extensions
        .get::<McpRequestContext>()
        .cloned()
        .ok_or_else(|| ErrorData::internal_error("MCP request context is unavailable", None))
}

fn require_permission(
    principal: &AuthPrincipal,
    permission: Permission,
) -> Result<(), ToolErrorBody> {
    AuthService::require_permission(principal, permission).map_err(|_| {
        ToolErrorBody::new(
            "permission_denied",
            "the credential does not grant this operation",
            false,
        )
    })
}

fn require_writable(request: &McpRequestContext) -> Result<(), ToolErrorBody> {
    if request.maintenance.allows_write() {
        Ok(())
    } else {
        Err(ToolErrorBody::new(
            "maintenance",
            "the Vault is temporarily read-only for backup or restore coordination",
            true,
        ))
    }
}

fn parse_memory_id(value: &str) -> Result<MemoryId, ToolErrorBody> {
    MemoryId::parse(value)
        .map_err(|_| ToolErrorBody::new("invalid_argument", "memory id is invalid", false))
}

fn parse_memory_type(value: &str) -> Result<MemoryType, ToolErrorBody> {
    MemoryType::try_from(value)
        .map_err(|_| ToolErrorBody::new("invalid_argument", "memory type is invalid", false))
}

fn parse_memory_status(value: &str) -> Result<MemoryStatus, ToolErrorBody> {
    MemoryStatus::try_from(value)
        .map_err(|_| ToolErrorBody::new("invalid_argument", "memory status is invalid", false))
}

fn memory_error(error: MemoryError) -> ToolErrorBody {
    match error {
        MemoryError::InvalidInput(_) | MemoryError::Markdown => {
            ToolErrorBody::new("invalid_argument", "the memory request is invalid", false)
        }
        MemoryError::SourceIngestion(code) => {
            ToolErrorBody::new(code, "the source note could not be processed", false)
        }
        MemoryError::GeneratedOutput(code) => {
            ToolErrorBody::new(code, "the generated memory output failed validation", false)
        }
        MemoryError::NotFound => ToolErrorBody::new("not_found", "the memory was not found", false),
        MemoryError::Configuration(code) => {
            ToolErrorBody::new(code, "memory extraction is not fully configured", false)
        }
        MemoryError::Conflict => ToolErrorBody::new(
            "memory_conflict",
            "memory state changed while the operation was running; retry with current state",
            true,
        ),
        MemoryError::Quarantined => ToolErrorBody::new(
            "memory_quarantined",
            "the memory record is quarantined",
            false,
        ),
        MemoryError::Provider(error) => ToolErrorBody::new(
            if error.retryable() {
                "temporarily_unavailable"
            } else {
                "provider_unavailable"
            },
            "optional memory provider work is unavailable",
            error.retryable(),
        ),
        MemoryError::State(_) | MemoryError::Core(_) | MemoryError::Index(_) => ToolErrorBody::new(
            "temporarily_unavailable",
            "memory is temporarily unavailable",
            true,
        ),
    }
}

fn tool_allowed(principal: &AuthPrincipal, name: &str) -> bool {
    let required = match name {
        "vault_overview" | "browse_index" | "recent_changes" => &[Permission::DiscoverVault][..],
        "search_notes" => &[Permission::ReadVault][..],
        "read_note" => &[Permission::ReadVault][..],
        "recall" | "get_memory" | "list_memories" => &[Permission::ReadMemory][..],
        "create_note" | "edit_note" | "move_note" => &[Permission::WriteVault][..],
        "delete_note" => &[Permission::DeleteVault][..],
        "note_history" => &[Permission::ReadHistory][..],
        "restore_note_revision" => &[Permission::ReadHistory, Permission::WriteVault][..],
        "remember" => &[Permission::WriteMemory][..],
        "update_memory" | "forget_memory" => &[Permission::ManageMemory][..],
        _ => return false,
    };
    required
        .iter()
        .all(|permission| principal.permissions.contains(*permission))
}

fn tool_order(name: &str) -> usize {
    match name {
        "vault_overview" => 0,
        "browse_index" => 1,
        "recent_changes" => 2,
        "search_notes" => 3,
        "read_note" => 4,
        "recall" => 5,
        "get_memory" => 6,
        "list_memories" => 7,
        "create_note" => 8,
        "edit_note" => 9,
        "move_note" => 10,
        "delete_note" => 11,
        "note_history" => 12,
        "restore_note_revision" => 13,
        "remember" => 14,
        "update_memory" => 15,
        "forget_memory" => 16,
        _ => usize::MAX,
    }
}

fn success_result(context: &RequestContext<RoleServer>, data: Value) -> CallToolResult {
    let data = match data {
        Value::Object(data) => data,
        value => Map::from_iter([(String::from("value"), value)]),
    };
    CallToolResult::structured(
        serde_json::to_value(ToolEnvelope {
            request_id: context.id.to_string(),
            ok: true,
            data: Some(data),
            error: None,
        })
        .expect("ToolEnvelope is serializable"),
    )
}

fn error_result(context: &RequestContext<RoleServer>, error: ToolErrorBody) -> CallToolResult {
    CallToolResult::structured_error(
        serde_json::to_value(ToolEnvelope {
            request_id: context.id.to_string(),
            ok: false,
            data: None,
            error: Some(error),
        })
        .expect("ToolEnvelope is serializable"),
    )
}

fn bounded_limit(value: Option<u32>, default: u32) -> Result<u32, ToolErrorBody> {
    let value = value.unwrap_or(default);
    if value == 0 || value > MAX_TOOL_LIMIT {
        return Err(ToolErrorBody::new(
            "invalid_argument",
            "limit must be between 1 and 100",
            false,
        ));
    }
    Ok(value)
}

fn parse_cursor(value: Option<&str>) -> Result<u32, ToolErrorBody> {
    let Some(value) = value else {
        return Ok(0);
    };
    let offset = value
        .strip_prefix("offset:")
        .ok_or_else(|| ToolErrorBody::new("invalid_argument", "cursor is invalid", false))?
        .parse::<u32>()
        .map_err(|_| ToolErrorBody::new("invalid_argument", "cursor is invalid", false))?;
    if offset > 1_000_000 {
        return Err(ToolErrorBody::new(
            "invalid_argument",
            "cursor is out of bounds",
            false,
        ));
    }
    Ok(offset)
}

fn bounded_read_bytes(value: Option<u64>) -> Result<u64, ToolErrorBody> {
    let value = value.unwrap_or(DEFAULT_READ_BYTES);
    if value == 0 || value > MAX_READ_BYTES {
        return Err(ToolErrorBody::new(
            "invalid_argument",
            "max_bytes must be between 1 and 1048576",
            false,
        ));
    }
    Ok(value)
}

fn parse_tool_path(value: &str) -> Result<VaultPath, ToolErrorBody> {
    VaultPath::parse(value).map_err(|_| {
        ToolErrorBody::new(
            "invalid_path",
            "path must be a normalized Vault-relative path",
            false,
        )
    })
}

fn parse_node_id(value: Option<&str>) -> Result<String, ToolErrorBody> {
    let value = value.unwrap_or("root");
    if value == "root" || value.is_empty() {
        return Ok("root".to_owned());
    }
    let value = if let Some(path) = value.strip_prefix("path:") {
        let path = parse_tool_path(path)?;
        format!("folder:{}", path.as_str())
    } else {
        value.to_owned()
    };
    validate_index_key(&value).map_err(|_| {
        ToolErrorBody::new(
            "invalid_argument",
            "node_id is not a valid indexed node identifier",
            false,
        )
    })?;
    Ok(value)
}

fn parse_resource_index_path(value: &str) -> Result<String, ErrorData> {
    let value = value.strip_prefix('/').unwrap_or(value);
    let value = percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| ErrorData::invalid_params("resource path is invalid", None))?;
    if value.is_empty() || value == "root" {
        return Ok("root".to_owned());
    }
    validate_index_key(&value)
        .map(|()| value.into_owned())
        .map_err(|_| ErrorData::invalid_params("resource path is invalid", None))
}

fn validate_index_key(value: &str) -> Result<(), ()> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(())
}

fn index_error(error: IndexError) -> ToolErrorBody {
    match error {
        IndexError::TooLarge => ToolErrorBody::new(
            "invalid_argument",
            "the indexed source exceeds the configured bound",
            false,
        ),
        IndexError::InvalidInput(_) | IndexError::Yaml => {
            ToolErrorBody::new("invalid_argument", "the index request is invalid", false)
        }
        IndexError::Core(VaultError::NotFound) => ToolErrorBody::new(
            "temporarily_unavailable",
            "the indexed source is not available",
            true,
        ),
        IndexError::Core(_) | IndexError::State(_) => {
            ToolErrorBody::new("temporarily_unavailable", "the index is unavailable", true)
        }
        IndexError::Provider(error) => ToolErrorBody::new(
            error.code(),
            "the semantic note index is unavailable",
            error.retryable(),
        ),
    }
}

fn note_resource_uri(path: &VaultPath) -> String {
    let encoded = path
        .segments()
        .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
        .collect::<Vec<_>>()
        .join("/");
    format!("vault://note/{encoded}")
}

fn file_json(file: &FileRecord) -> Value {
    json!({
        "file_id": file.id.to_string(),
        "vault_id": file.vault_id.to_string(),
        "path": file.path.as_str(),
        "entry_type": file.entry_type.as_str(),
        "revision": file.current_revision.value(),
        "content_hash": file.content_hash,
        "size": file.size,
        "modified_at": file.modified_at,
        "active": file.is_active(),
    })
}

fn mutation_json(result: &MutationResult) -> Value {
    json!({
        "file": file_json(&result.file),
        "revision": revision_json(&result.revision),
        "etag": result.etag,
    })
}

fn revision_json(revision: &FileRevisionRecord) -> Value {
    json!({
        "revision_id": revision.id.to_string(),
        "file_id": revision.file_id.to_string(),
        "vault_id": revision.vault_id.to_string(),
        "revision": revision.revision.value(),
        "operation": revision.operation.as_str(),
        "path_before": revision.path_before.as_ref().map(|path| path.as_str()),
        "path_after": revision.path_after.as_ref().map(|path| path.as_str()),
        "content_hash": revision.content_hash,
        "size": revision.size,
        "actor_type": revision.actor_type,
        "actor_id": revision.actor_id.as_ref().map(ToString::to_string),
        "source_plane": revision.source_plane.to_string(),
        "created_at": revision.created_at,
    })
}

async fn overview_data(
    request: &McpRequestContext,
    include_recent: bool,
    limit: u32,
) -> Result<Value, ToolErrorBody> {
    let status = indexed_status(request).await?;
    let topics = request
        .index
        .list_nodes(&request.vault, Some("root"), limit, 0)
        .await
        .map_err(index_error)?;
    let recent = if include_recent {
        request
            .state
            .files()
            .list_recent_revisions(&request.vault, limit.min(50))
            .await
            .map_err(|_| {
                ToolErrorBody::new(
                    "temporarily_unavailable",
                    "recent changes are temporarily unavailable",
                    true,
                )
            })?
            .iter()
            .map(revision_json)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    Ok(json!({
        "vault": {
            "id": request.vault.id().to_string(),
            "slug": request.vault.slug().as_str(),
            "settings_revision": request.vault.settings_revision().value(),
            "index_revision": status.index_revision.value(),
        },
        "statistics": {
            "indexed_entries": status.indexed_entries,
            "notes": status.indexed_notes,
            "indexed_bytes": status.indexed_bytes,
            "topics": topics.len(),
        },
        "topics": topics.iter().map(index_node_json).collect::<Vec<_>>(),
        "index": {
            "revision": status.index_revision.value(),
            "coverage": status.coverage,
            "last_error": status.last_error,
        },
        "recent": recent,
        "truncated": topics.len() >= limit as usize,
    }))
}

async fn browse_data(
    request: &McpRequestContext,
    node: &str,
    depth: u8,
    limit: u32,
    offset: u32,
    include_note_candidates: bool,
) -> Result<Value, ToolErrorBody> {
    let status = indexed_status(request).await?;
    let parent = (node != "root").then_some(node);
    let children = request
        .index
        .list_nodes(&request.vault, parent, limit, offset)
        .await
        .map_err(index_error)?;
    let mut children_json = Vec::with_capacity(children.len());
    for child in &children {
        let mut value = index_node_json(child);
        if include_note_candidates {
            let notes = request
                .index
                .list_node_notes(&request.vault, &child.stable_key, limit.min(5), 0)
                .await
                .map_err(index_error)?;
            value["note_candidates"] =
                Value::Array(notes.iter().map(note_search_json).collect::<Vec<_>>());
        }
        if depth > 1 {
            let grandchildren = request
                .index
                .list_nodes(&request.vault, Some(&child.stable_key), limit.min(25), 0)
                .await
                .map_err(index_error)?;
            value["children"] = Value::Array(
                grandchildren
                    .iter()
                    .map(index_node_json)
                    .collect::<Vec<_>>(),
            );
        }
        children_json.push(value);
    }
    let node_notes = if include_note_candidates {
        request
            .index
            .list_node_notes(&request.vault, node, limit.min(10), 0)
            .await
            .map_err(index_error)?
            .iter()
            .map(note_search_json)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    Ok(json!({
        "node": {"id": node},
        "depth": depth,
        "children": children_json,
        "note_candidates": node_notes,
        "index_revision": status.index_revision.value(),
        "coverage": status.coverage,
        "next_cursor": (children.len() == limit as usize).then(|| format!("offset:{}", offset.saturating_add(children.len() as u32))),
        "truncated": children.len() == limit as usize,
    }))
}

async fn search_data(
    request: &McpRequestContext,
    input: &SearchNotesInput,
    scope: &SearchScope,
    mode: SearchMode,
    limit: u32,
    offset: u32,
) -> Result<Value, ToolErrorBody> {
    let result_granularity = input.result_granularity.as_deref().unwrap_or("note");
    if !matches!(result_granularity, "note" | "section") {
        return Err(ToolErrorBody::new(
            "invalid_argument",
            "result granularity must be note or section",
            false,
        ));
    }
    let path_prefix = match scope.path_prefix.as_deref() {
        None => None,
        Some(value) => {
            let path = parse_tool_path(value)?;
            (!path.is_root()).then(|| path.as_str().to_owned())
        }
    };
    if scope
        .topic_ids
        .iter()
        .any(|topic| validate_index_key(topic).is_err())
    {
        return Err(ToolErrorBody::new(
            "invalid_argument",
            "topic filter is invalid",
            false,
        ));
    }
    if let (Some(after), Some(before)) = (scope.modified_after, scope.modified_before)
        && after > before
    {
        return Err(ToolErrorBody::new(
            "invalid_argument",
            "modified time range is invalid",
            false,
        ));
    }
    let status = indexed_status(request).await?;
    let retrieval_mode = match mode {
        SearchMode::Lexical => NoteRetrievalMode::Lexical,
        SearchMode::Semantic => NoteRetrievalMode::Semantic,
        SearchMode::Hybrid => NoteRetrievalMode::Hybrid,
    };
    let result = request
        .index
        .retrieve_notes(
            &request.vault,
            &input.query,
            retrieval_mode,
            &NoteRetrievalScope {
                path_prefix,
                tags: scope.tags.clone(),
                topic_ids: scope.topic_ids.clone(),
                modified_after: scope.modified_after,
                modified_before: scope.modified_before,
            },
            limit,
            offset,
            input.include_score_breakdown.unwrap_or(false),
        )
        .await
        .map_err(index_error)?;
    let result_count = result.hits.len();
    let truncated = offset.saturating_add(result_count as u32) < result.available_result_count;
    Ok(json!({
        "mode": match mode {
            SearchMode::Lexical => "lexical",
            SearchMode::Semantic => "semantic",
            SearchMode::Hybrid => "hybrid",
        },
        "degraded": !result.degraded.is_empty(),
        "degradation_reasons": result.degraded,
        "results": result.hits.iter().map(note_retrieval_json).collect::<Vec<_>>(),
        "available_result_count": result.available_result_count,
        "index_revision": status.index_revision.value(),
        "coverage": status.coverage,
        "next_cursor": truncated.then(|| format!("offset:{}", offset.saturating_add(result_count as u32))),
        "truncated": truncated,
        "result_granularity": result_granularity,
        "include_score_breakdown": input.include_score_breakdown.unwrap_or(false),
    }))
}

async fn indexed_status(
    request: &McpRequestContext,
) -> Result<mcp_vault_state::IndexStatusRecord, ToolErrorBody> {
    request
        .index
        .status(&request.vault)
        .await
        .map_err(index_error)?
        .ok_or_else(|| {
            ToolErrorBody::new(
                "temporarily_unavailable",
                "the Vault index is not ready",
                true,
            )
        })
}

fn index_node_json(node: &mcp_vault_state::IndexNodeRecord) -> Value {
    json!({
        "id": node.stable_key,
        "parent_id": node.parent_key,
        "type": node.node_type,
        "title": node.title,
        "summary": node.summary,
        "source_type": node.source_type,
        "sort_key": node.sort_key,
        "note_count": node.member_count,
    })
}

fn note_search_json(note: &mcp_vault_state::NoteSearchRecord) -> Value {
    json!({
        "file_id": note.file_id.to_string(),
        "path": note.path.as_str(),
        "revision": note.revision.value(),
        "title": note.title,
        "modified_at": note.updated_at,
        "snippet": note.snippet,
        "score": note.score,
        "tags": note.tags,
        "topic_ids": note.topic_ids,
        "headings": note.headings,
        "outgoing_links": note.outgoing_links.iter().map(|link| json!({
            "id": link.id,
            "target_text": link.target_text,
            "target_file_id": link.target_file_id.map(|id| id.to_string()),
            "target_heading": link.target_heading,
            "link_type": link.link_type,
            "ordinal": link.ordinal,
        })).collect::<Vec<_>>(),
        "backlink_count": note.backlink_count,
        "resource_uri": note_resource_uri(&note.path),
    })
}

fn note_retrieval_json(hit: &NoteRetrievalHit) -> Value {
    let mut value = note_search_json(&hit.note);
    if let Some(object) = value.as_object_mut() {
        object.insert("score".to_owned(), json!(hit.score));
        if let Some(breakdown) = hit.score_breakdown.as_ref() {
            object.insert("score_breakdown".to_owned(), json!(breakdown));
        }
    }
    value
}

fn recall_json(result: mcp_vault_memory::RecallResult) -> Value {
    let mut value = serde_json::to_value(result).unwrap_or_else(|_| json!({}));
    if let Some(notes) = value.get_mut("related_notes").and_then(Value::as_array_mut) {
        for note in notes {
            let path = note
                .get("path")
                .and_then(Value::as_str)
                .and_then(|path| VaultPath::parse(path).ok());
            if let (Some(object), Some(path)) = (note.as_object_mut(), path) {
                object.insert("resource_uri".to_owned(), json!(note_resource_uri(&path)));
            }
        }
    }
    value
}

async fn read_bounded(
    mut reader: ReadFile,
    max_bytes: u64,
) -> Result<(Vec<u8>, bool), std::io::Error> {
    let mut bytes = Vec::with_capacity(max_bytes.min(MAX_READ_BYTES) as usize);
    let mut limited = (&mut reader).take(max_bytes.saturating_add(1));
    limited.read_to_end(&mut bytes).await?;
    let truncated = bytes.len() as u64 > max_bytes;
    if truncated {
        bytes.truncate(max_bytes as usize);
    }
    Ok((bytes, truncated))
}

fn vault_error(error: VaultError) -> ToolErrorBody {
    match error {
        VaultError::AlreadyExists => {
            ToolErrorBody::new("already_exists", "the target already exists", false)
        }
        VaultError::NotFound => ToolErrorBody::new("not_found", "the target was not found", false),
        VaultError::RevisionConflict {
            expected,
            current,
            current_hash,
        } => ToolErrorBody::new("revision_conflict", "the current revision changed", true)
            .with_details(json!({
                "expected_revision": expected.value(),
                "current_revision": current.value(),
                "current_hash": current_hash,
            })),
        VaultError::InvalidPatch(_) => ToolErrorBody::new(
            "invalid_patch",
            "the exact patch could not be applied",
            false,
        ),
        VaultError::BinaryTextOperation => ToolErrorBody::new(
            "unsupported_media_type",
            "the operation requires UTF-8 text",
            false,
        ),
        VaultError::Maintenance | VaultError::NeedsReview => ToolErrorBody::new(
            "temporarily_unavailable",
            "the Vault is temporarily unavailable for this operation",
            true,
        ),
        VaultError::ExternalMismatch => ToolErrorBody::new(
            "external_mismatch",
            "the canonical file changed outside the expected revision",
            true,
        ),
        VaultError::Domain(_) => ToolErrorBody::new(
            "precondition_failed",
            "the Vault precondition failed",
            false,
        ),
        VaultError::InFlight => ToolErrorBody::new(
            "operation_in_flight",
            "an idempotent operation is still in progress",
            true,
        ),
        VaultError::IdempotencyConflict => ToolErrorBody::new(
            "idempotency_conflict",
            "the idempotency key was reused for another operation",
            false,
        ),
        VaultError::State(_) => ToolErrorBody::new(
            "internal_error",
            "the Vault operational state transaction failed",
            true,
        )
        .with_details(json!({"component": "state"})),
        VaultError::Storage(error) => ToolErrorBody::new(
            "internal_error",
            "the Vault filesystem operation failed",
            true,
        )
        .with_details(json!({
            "component": "storage",
            "diagnostic": error.to_string(),
        })),
        VaultError::VaultNotRegistered => ToolErrorBody::new(
            "internal_error",
            "the Vault is not registered for this operation",
            false,
        )
        .with_details(json!({"component": "vault_registry", "reason": "not_registered"})),
        VaultError::ContextMismatch => ToolErrorBody::new(
            "internal_error",
            "the Vault context does not match registered state",
            false,
        )
        .with_details(json!({"component": "vault_registry", "reason": "context_mismatch"})),
        VaultError::InjectedFailure(_) => {
            ToolErrorBody::new("internal_error", "the Vault operation failed", true)
                .with_details(json!({"component": "core"}))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{io::ErrorKind, path::PathBuf};

    use super::{
        McpService, bearer_token, mounted_slug, oauth_metadata_router, router, stateful_router,
        vault_error,
    };
    use axum::{Router, body::Body, http::Request};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use http_body_util::BodyExt;
    use mcp_vault_auth::{
        AuthService, MasterKeyRing, OAuthIssuerInput, OAuthResourceServer, OriginPolicy,
        SecretString,
    };
    use mcp_vault_core::{VaultCore, VaultError};
    use mcp_vault_domain::{
        Actor, MemoryId, Revision, Scope, ScopeSet, VaultContext, VaultId, VaultPath,
        VaultPathPolicy, VaultSlug,
    };
    use mcp_vault_indexer::IndexService;
    use mcp_vault_memory::{MEMORY_PIPELINE_GENERATION, MemoryOrigin, MemoryStatus, MemoryType};
    use mcp_vault_state::{MemoryBundle, MemoryRecord, StateStore, VaultStatus};
    use mcp_vault_storage_fs::{StorageError, StorageOptions};
    use rand::rngs::OsRng;
    use rsa::{
        RsaPrivateKey, RsaPublicKey,
        pkcs1v15::SigningKey,
        signature::{SignatureEncoding, Signer},
        traits::PublicKeyParts,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;
    use tower::ServiceExt;
    use url::Url;

    #[test]
    fn endpoint_slug_is_taken_from_the_mount() {
        assert_eq!(mounted_slug("/default").unwrap().as_str(), "default");
        assert_eq!(
            mounted_slug("/mcp/v1/vaults/work").unwrap().as_str(),
            "work"
        );
        assert!(mounted_slug("/mcp/v1/vaults/work/extra").is_err());
        assert!(mounted_slug("/mcp/v1/vaults/../work").is_err());
    }

    #[test]
    fn storage_internal_errors_keep_only_redacted_component_diagnostics() {
        let error = vault_error(VaultError::Storage(StorageError::Io {
            operation: "create_parent",
            kind: ErrorKind::PermissionDenied,
        }));
        assert_eq!(error.code, "internal_error");
        assert_eq!(error.details.as_ref().unwrap()["component"], "storage");
        assert_eq!(
            error.details.as_ref().unwrap()["diagnostic"],
            "filesystem operation create_parent failed (PermissionDenied)"
        );
    }

    #[test]
    fn bearer_parser_rejects_malformed_headers() {
        let mut headers = axum::http::HeaderMap::new();
        assert!(bearer_token(&headers).is_err());
        headers.insert("authorization", "Basic secret".parse().unwrap());
        assert!(bearer_token(&headers).is_err());
        headers.insert("authorization", "Bearer token extra".parse().unwrap());
        assert!(bearer_token(&headers).is_err());
        headers.insert("authorization", "Bearer mcpv_pat_example".parse().unwrap());
        assert_eq!(bearer_token(&headers).unwrap(), "mcpv_pat_example");
    }

    #[test]
    fn oauth_resource_selection_is_exact_and_ambiguous_fallback_fails_closed() {
        let slug = VaultSlug::new("work").unwrap();
        let resources = vec![
            OAuthResourceServer {
                resource: "https://one.example.test/mcp/v1/vaults/work".to_owned(),
                authorization_servers: vec!["https://issuer-one.example.test".to_owned()],
            },
            OAuthResourceServer {
                resource: "https://two.example.test/mcp/v1/vaults/work".to_owned(),
                authorization_servers: vec!["https://issuer-two.example.test".to_owned()],
            },
        ];

        assert!(super::select_oauth_resource(resources.clone(), None, &slug).is_none());
        assert!(
            super::select_oauth_resource(
                resources.clone(),
                Some("https://missing.example.test"),
                &slug,
            )
            .is_none()
        );
        assert_eq!(
            super::select_oauth_resource(resources, Some("https://two.example.test/"), &slug)
                .unwrap()
                .authorization_servers,
            vec!["https://issuer-two.example.test"]
        );
    }

    #[tokio::test]
    async fn unconfigured_router_is_an_explicit_boundary() {
        let response = router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    fn full_scopes() -> ScopeSet {
        [
            Scope::VaultDiscover,
            Scope::VaultRead,
            Scope::VaultWrite,
            Scope::VaultDelete,
            Scope::VaultHistory,
        ]
        .into_iter()
        .collect()
    }

    fn mounted_service_router(service: McpService) -> Router {
        Router::new()
            .merge(oauth_metadata_router(service.clone()))
            .nest("/mcp/v1/vaults", stateful_router(service))
    }

    async fn configured_router() -> (axum::Router, String, tempfile::TempDir) {
        configured_router_with_scopes(full_scopes()).await
    }

    async fn configured_memory_router() -> (axum::Router, String, tempfile::TempDir) {
        let scopes: ScopeSet = [
            Scope::VaultDiscover,
            Scope::VaultRead,
            Scope::VaultWrite,
            Scope::VaultDelete,
            Scope::VaultHistory,
            Scope::MemoryRead,
            Scope::MemoryWrite,
            Scope::MemoryManage,
        ]
        .into_iter()
        .collect();
        configured_router_with_scopes(scopes).await
    }

    async fn configured_router_with_scopes(
        scopes: ScopeSet,
    ) -> (axum::Router, String, tempfile::TempDir) {
        configured_router_with_availability(scopes, false).await
    }

    async fn configured_router_with_availability(
        scopes: ScopeSet,
        initializing: bool,
    ) -> (axum::Router, String, tempfile::TempDir) {
        let root = tempdir().unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from(root.path()),
            Revision::new(1),
        )
        .unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        state
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        let other_context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("other").unwrap(),
            root.path().join("other"),
            Revision::new(1),
        )
        .unwrap();
        state
            .vaults()
            .insert(&other_context, "Other", VaultStatus::Active)
            .await
            .unwrap();
        for vault_context in [&context, &other_context] {
            state
                .memory()
                .set_pipeline_generation_state(vault_context, MEMORY_PIPELINE_GENERATION, false)
                .await
                .unwrap();
        }
        if initializing {
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
        }
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
        );
        let pat = auth
            .issue_pat(&context, "test-agent", scopes, None)
            .await
            .unwrap();
        let service = McpService::new(
            state,
            auth,
            root.path().join("history"),
            StorageOptions::default(),
            Default::default(),
            vec!["localhost".to_owned()],
            OriginPolicy::new(std::iter::empty::<&str>()).unwrap(),
        );
        (
            mounted_service_router(service),
            pat.token.expose_secret().to_owned(),
            root,
        )
    }

    async fn configured_indexed_router() -> (axum::Router, String, tempfile::TempDir) {
        configured_indexed_router_with_scopes(full_scopes()).await
    }

    async fn configured_indexed_memory_router() -> (axum::Router, String, tempfile::TempDir) {
        let scopes: ScopeSet = [Scope::VaultDiscover, Scope::VaultRead, Scope::MemoryRead]
            .into_iter()
            .collect();
        configured_indexed_router_with_scopes(scopes).await
    }

    async fn configured_indexed_router_with_scopes(
        scopes: ScopeSet,
    ) -> (axum::Router, String, tempfile::TempDir) {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("notes")).unwrap();
        std::fs::write(
            root.path().join("notes/search.md"),
            "---\ntags: [Rust]\n---\n# Search\n\nWebDAV conflict handling.\n",
        )
        .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from(root.path()),
            Revision::new(1),
        )
        .unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        state
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        state
            .memory()
            .set_pipeline_generation_state(&context, MEMORY_PIPELINE_GENERATION, false)
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            root.path().join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        core.reconcile(&context, Actor::system()).await.unwrap();
        IndexService::new(state.clone())
            .rebuild_vault(&core, &context)
            .await
            .unwrap();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[9_u8; 32]).unwrap(),
        );
        let pat = auth
            .issue_pat(&context, "test-agent", scopes, None)
            .await
            .unwrap();
        let service = McpService::new(
            state,
            auth,
            root.path().join("history"),
            StorageOptions::default(),
            Default::default(),
            vec!["localhost".to_owned()],
            OriginPolicy::new(std::iter::empty::<&str>()).unwrap(),
        );
        (
            mounted_service_router(service),
            pat.token.expose_secret().to_owned(),
            root,
        )
    }

    async fn configured_oauth_router() -> (axum::Router, String, tempfile::TempDir) {
        let root = tempdir().unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from(root.path()),
            Revision::new(1),
        )
        .unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        state
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[8_u8; 32]).unwrap(),
        );
        let resource = "https://vault.example.test/mcp/v1/vaults/work";
        let issuer_url = "https://issuer.example.test";
        let private = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let public = RsaPublicKey::from(&private);
        let modulus = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let exponent = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
        let issuer = auth
            .configure_oauth_issuer(OAuthIssuerInput {
                name: "test issuer".to_owned(),
                issuer_url: issuer_url.to_owned(),
                discovery_url: None,
                audience: resource.to_owned(),
                resource: resource.to_owned(),
                jwks_cache_json: format!(
                    r#"{{"keys":[{{"kty":"RSA","kid":"test","alg":"RS256","use":"sig","n":"{modulus}","e":"{exponent}"}}]}}"#
                ),
                enabled: true,
            })
            .await
            .unwrap();
        auth.grant_oauth_subject(
            &context,
            issuer.id,
            "agent",
            [Scope::VaultDiscover, Scope::VaultRead]
                .into_iter()
                .collect(),
        )
        .await
        .unwrap();
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","kid":"test"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"iss":"{issuer_url}","sub":"agent","aud":"{resource}","exp":4102444800,"scope":"vault:discover vault:read"}}"#
        ));
        let signing = format!("{header}.{payload}");
        let signature = SigningKey::<Sha256>::new(private).sign(signing.as_bytes());
        let token = format!("{signing}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()));
        let service = McpService::new(
            state,
            auth,
            root.path().join("history"),
            StorageOptions::default(),
            Default::default(),
            vec!["localhost".to_owned()],
            OriginPolicy::new(std::iter::empty::<&str>()).unwrap(),
        )
        .with_public_origin(Some("https://vault.example.test".to_owned()));
        (mounted_service_router(service), token, root)
    }

    async fn configured_builtin_oauth_router() -> (axum::Router, tempfile::TempDir, MemoryId) {
        let root = tempdir().unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from(root.path()),
            Revision::new(1),
        )
        .unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        state
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        state
            .memory()
            .set_pipeline_generation_state(&context, MEMORY_PIPELINE_GENERATION, false)
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            root.path().join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        let memory_id = MemoryId::new();
        let canonical_path = core
            .managed_root()
            .join(&VaultPath::parse(&format!("memory/records/2026/08/{memory_id}.md")).unwrap())
            .unwrap();
        state
            .memory()
            .replace_bundle(
                &context,
                &MemoryBundle {
                    memory: MemoryRecord {
                        id: memory_id,
                        vault_id: context.id(),
                        memory_type: MemoryType::Decision.as_str().to_owned(),
                        status: MemoryStatus::Active.as_str().to_owned(),
                        status_reason: None,
                        status_changed_at: None,
                        content: "OAuth memory operations are available.".to_owned(),
                        normalized_content: "oauth memory operations are available.".to_owned(),
                        content_hash: "oauth-all-tools-fixture".to_owned(),
                        importance: 0.9,
                        confidence: 1.0,
                        origin: MemoryOrigin::ExplicitAdmin.as_str().to_owned(),
                        revision: Revision::new(1),
                        canonical_file_id: None,
                        canonical_path: Some(canonical_path),
                        canonical_revision: None,
                        valid_from: None,
                        valid_to: None,
                        extraction: json!({"fixture": "oauth_all_tools"}),
                        created_at: 1_777_593_600_000,
                        updated_at: 1_777_593_600_000,
                        last_recalled_at: None,
                        recall_count: 0,
                    },
                    sources: Vec::new(),
                    entities: Vec::new(),
                    tags: vec!["oauth".to_owned()],
                    relations: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        core.reconcile(&context, Actor::system()).await.unwrap();
        IndexService::new(state.clone())
            .rebuild_vault(&core, &context)
            .await
            .unwrap();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[19_u8; 32]).unwrap(),
        );
        auth.configure_local_oauth_user(
            &context,
            "chatgpt",
            &SecretString::new("correct horse battery staple"),
            Scope::ALL.into_iter().collect(),
        )
        .await
        .unwrap();
        let service = McpService::new(
            state,
            auth,
            root.path().join("history"),
            StorageOptions::default(),
            Default::default(),
            vec!["localhost".to_owned()],
            OriginPolicy::new(std::iter::empty::<&str>()).unwrap(),
        )
        .with_public_origin(Some("https://vault.example.test".to_owned()));
        (mounted_service_router(service), root, memory_id)
    }

    fn discover_request(token: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/mcp/v1/vaults/work")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {token}"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "server/discover")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#,
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn built_in_oauth_http_flow_exposes_and_routes_every_tool() {
        let (router, _root, memory_id) = configured_builtin_oauth_router().await;
        let metadata = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metadata.status(), axum::http::StatusCode::OK);
        let metadata: serde_json::Value =
            serde_json::from_slice(&metadata.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(metadata["issuer"], "https://vault.example.test");
        assert_eq!(
            metadata["authorization_endpoint"],
            "https://vault.example.test/oauth/v2/authorize"
        );
        assert_eq!(
            metadata["code_challenge_methods_supported"],
            json!(["S256"])
        );
        assert_eq!(
            metadata["token_endpoint_auth_methods_supported"],
            json!(["none"])
        );
        assert!(
            metadata["scopes_supported"]
                .as_array()
                .unwrap()
                .iter()
                .any(|scope| scope == "offline_access")
        );

        let rejected_metadata = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .header("host", "localhost")
                    .header("origin", "null")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            rejected_metadata.status(),
            axum::http::StatusCode::FORBIDDEN
        );

        let registration = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .extension(axum::extract::ConnectInfo(
                        "127.0.0.1:54321".parse::<std::net::SocketAddr>().unwrap(),
                    ))
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"client_name":"ChatGPT","redirect_uris":["https://chatgpt.com/connector_platform_oauth_redirect"],"grant_types":["authorization_code","refresh_token"],"response_types":["code"],"token_endpoint_auth_method":"none"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(registration.status(), axum::http::StatusCode::CREATED);
        let registration: serde_json::Value =
            serde_json::from_slice(&registration.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let client_id = registration["client_id"].as_str().unwrap().to_owned();
        assert!(registration.get("client_secret").is_none());

        let verifier = "p".repeat(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let resource = "https://vault.example.test/mcp/v1/vaults/work";
        let mut authorize = Url::parse("https://vault.example.test/oauth/v2/authorize").unwrap();
        authorize
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair(
                "redirect_uri",
                "https://chatgpt.com/connector_platform_oauth_redirect",
            )
            .append_pair(
                "scope",
                "vault:discover vault:read vault:write vault:delete vault:history memory:read memory:write memory:manage offline_access",
            )
            .append_pair("state", "state-123")
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", resource);
        let authorize_path = format!(
            "{}?{}",
            authorize.path(),
            authorize.query().expect("authorization query exists")
        );
        let legacy_authorize_path =
            authorize_path.replacen("/oauth/v2/authorize", "/oauth/authorize", 1);
        let legacy_redirect = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&legacy_authorize_path)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            legacy_redirect.status(),
            axum::http::StatusCode::TEMPORARY_REDIRECT
        );
        assert_eq!(
            legacy_redirect.headers().get("location").unwrap(),
            authorize_path.as_str()
        );
        assert_eq!(legacy_redirect.headers().get("vary").unwrap(), "*");
        assert_eq!(
            legacy_redirect.headers().get("cdn-cache-control").unwrap(),
            "no-store"
        );

        let login = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(authorize_path)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), axum::http::StatusCode::OK);
        assert_eq!(
            login.headers().get("cache-control").unwrap(),
            "private, no-cache, no-store, max-age=0, must-revalidate"
        );
        assert_eq!(
            login.headers().get("cdn-cache-control").unwrap(),
            "no-store"
        );
        assert_eq!(
            login.headers().get("surrogate-control").unwrap(),
            "no-store"
        );
        assert_eq!(login.headers().get("vary").unwrap(), "*");
        assert_eq!(login.headers().get("x-frame-options").unwrap(), "DENY");
        assert_eq!(
            login.headers().get("content-security-policy").unwrap(),
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'"
        );
        let login = String::from_utf8(
            login
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        let marker = "name=\"request_handle\" value=\"";
        let start = login.find(marker).unwrap() + marker.len();
        let end = login[start..].find('"').unwrap() + start;
        let request_handle = &login[start..end];
        assert!(request_handle.starts_with("mcpv_oauth_req_"));
        assert!(login.contains("action=\"https://vault.example.test/oauth/v2/authorize\""));
        assert!(login.contains("autocomplete=\"username\""));
        assert!(login.contains("autocomplete=\"current-password\""));
        assert!(login.contains("<code>offline_access</code>（保持长期连接）"));
        assert!(!login.contains("data-1p-ignore"));

        let invalid_authorization = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/oauth/v2/authorize")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            invalid_authorization.status(),
            axum::http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            invalid_authorization
                .headers()
                .get("content-security-policy")
                .unwrap(),
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'"
        );

        let authorize_form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("request_handle", request_handle)
            .append_pair("resource", resource)
            .append_pair("username", "chatgpt")
            .append_pair("password", "correct horse battery staple")
            .finish();
        let redirect = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("host", "localhost")
                    // System OAuth browsers can submit an opaque Origin. The
                    // authorization transaction, not Origin, binds this form.
                    .header("origin", "null")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(authorize_form.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(redirect.status(), axum::http::StatusCode::FOUND);
        let location = redirect
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        let location = Url::parse(location).unwrap();
        let parameters = location
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        let first_code = parameters.get("code").unwrap().to_string();
        assert_eq!(parameters.get("state").unwrap(), "state-123");
        assert_eq!(parameters.get("iss").unwrap(), "https://vault.example.test");

        // Browser engines, password managers, and edge proxies can replay a
        // form POST after the first response commits. A valid retry must get a
        // fresh code instead of replacing the browser navigation with the
        // misleading "authorization request expired" page.
        let retried_redirect = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/v2/authorize")
                    .header("host", "localhost")
                    .header("origin", "null")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(authorize_form))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(retried_redirect.status(), axum::http::StatusCode::FOUND);
        let retried_location = Url::parse(
            retried_redirect
                .headers()
                .get("location")
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let retried_parameters = retried_location
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        let code = retried_parameters.get("code").unwrap().to_string();
        assert_ne!(code, first_code);
        assert_eq!(retried_parameters.get("state").unwrap(), "state-123");
        assert_eq!(
            retried_parameters.get("iss").unwrap(),
            "https://vault.example.test"
        );

        let token_form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "authorization_code")
            .append_pair("code", &code)
            .append_pair("client_id", &client_id)
            .append_pair(
                "redirect_uri",
                "https://chatgpt.com/connector_platform_oauth_redirect",
            )
            .append_pair("code_verifier", &verifier)
            .append_pair("resource", resource)
            .finish();
        let token = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/token")
                    .header("host", "localhost")
                    .header("origin", "https://chatgpt.com")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(token_form))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(token.status(), axum::http::StatusCode::OK);
        assert_eq!(
            token.headers().get("cache-control").unwrap(),
            "private, no-cache, no-store, max-age=0, must-revalidate"
        );
        let token: serde_json::Value =
            serde_json::from_slice(&token.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let access_token = token["access_token"].as_str().unwrap();
        assert!(access_token.starts_with("mcpv_oauth_"));
        assert!(
            token["refresh_token"]
                .as_str()
                .unwrap()
                .starts_with("mcpv_refresh_")
        );
        assert!(
            token["scope"]
                .as_str()
                .unwrap()
                .split_ascii_whitespace()
                .any(|scope| scope == "offline_access")
        );

        let response = router
            .clone()
            .oneshot(discover_request(access_token))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let tools = router
            .clone()
            .oneshot(list_tools_request(access_token))
            .await
            .unwrap();
        let tools_status = tools.status();
        let tools_body = tools.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            tools_status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&tools_body)
        );
        let tools_body: serde_json::Value = serde_json::from_slice(&tools_body).unwrap();
        let tool_names = tools_body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            tool_names,
            vec![
                "vault_overview",
                "browse_index",
                "recent_changes",
                "search_notes",
                "read_note",
                "recall",
                "get_memory",
                "list_memories",
                "create_note",
                "edit_note",
                "move_note",
                "delete_note",
                "note_history",
                "restore_note_revision",
                "remember",
                "update_memory",
                "forget_memory",
            ]
        );

        let create = call_tool_json(
            &router,
            access_token,
            10,
            "create_note",
            json!({
                "path": "notes/oauth-created.md",
                "content": "# Created through built-in OAuth\n"
            }),
        )
        .await;
        assert_tool_ok(&create, "create_note");
        assert_eq!(mutation_revision(&create), 1);

        for (id, name, arguments) in [
            (11, "vault_overview", json!({})),
            (12, "browse_index", json!({})),
            (13, "recent_changes", json!({})),
            (
                14,
                "search_notes",
                json!({"query": "OAuth", "mode": "lexical"}),
            ),
            (15, "read_note", json!({"path": "notes/oauth-created.md"})),
            (
                16,
                "recall",
                json!({"query": "OAuth", "max_results": 5, "max_tokens": 500}),
            ),
            (17, "list_memories", json!({})),
        ] {
            let body = call_tool_json(&router, access_token, id, name, arguments).await;
            assert_tool_ok(&body, name);
        }

        let edit = call_tool_json(
            &router,
            access_token,
            18,
            "edit_note",
            json!({
                "path": "notes/oauth-created.md",
                "expected_revision": 1,
                "operation": {"kind": "append", "content": "OAuth edit\n"}
            }),
        )
        .await;
        assert_tool_ok(&edit, "edit_note");
        assert_eq!(mutation_revision(&edit), 2);

        let history = call_tool_json(
            &router,
            access_token,
            19,
            "note_history",
            json!({"path": "notes/oauth-created.md"}),
        )
        .await;
        assert_tool_ok(&history, "note_history");

        let restore = call_tool_json(
            &router,
            access_token,
            20,
            "restore_note_revision",
            json!({
                "path": "notes/oauth-created.md",
                "revision": 1,
                "expected_current_revision": 2
            }),
        )
        .await;
        assert_tool_ok(&restore, "restore_note_revision");
        assert_eq!(mutation_revision(&restore), 3);

        let moved = call_tool_json(
            &router,
            access_token,
            21,
            "move_note",
            json!({
                "source": "notes/oauth-created.md",
                "destination": "notes/oauth-moved.md",
                "expected_revision": 3
            }),
        )
        .await;
        assert_tool_ok(&moved, "move_note");
        assert_eq!(mutation_revision(&moved), 4);

        let deleted = call_tool_json(
            &router,
            access_token,
            22,
            "delete_note",
            json!({"path": "notes/oauth-moved.md", "expected_revision": 4}),
        )
        .await;
        assert_tool_ok(&deleted, "delete_note");
        assert_eq!(mutation_revision(&deleted), 5);

        let get_memory = call_tool_json(
            &router,
            access_token,
            23,
            "get_memory",
            json!({"id": memory_id}),
        )
        .await;
        assert_tool_ok(&get_memory, "get_memory");

        let remember = call_tool_json(
            &router,
            access_token,
            24,
            "remember",
            json!({
                "content": "Built-in OAuth can invoke every MCP Vault tool.",
                "memory_type": "decision",
                "importance": 0.9,
                "confidence": 0.99,
                "idempotency_key": "oauth-all-tools-memory"
            }),
        )
        .await;
        assert_tool_ok(&remember, "remember");
        assert_eq!(
            remember["result"]["structuredContent"]["data"]["outcome"],
            "staged"
        );

        let update_memory = call_tool_json(
            &router,
            access_token,
            25,
            "update_memory",
            json!({
                "id": memory_id,
                "expected_revision": 1,
                "content": "OAuth memory operations remain available."
            }),
        )
        .await;
        assert_tool_ok(&update_memory, "update_memory");
        assert_eq!(
            update_memory["result"]["structuredContent"]["data"]["revision"],
            2
        );

        let forget_memory = call_tool_json(
            &router,
            access_token,
            26,
            "forget_memory",
            json!({"id": memory_id, "expected_revision": 2}),
        )
        .await;
        assert_tool_ok(&forget_memory, "forget_memory");
        assert_eq!(
            forget_memory["result"]["structuredContent"]["data"]["status"],
            "archived"
        );
    }

    fn tool_request(
        token: &str,
        id: u64,
        name: &str,
        arguments: serde_json::Value,
    ) -> Request<Body> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        Request::builder()
            .method("POST")
            .uri("/mcp/v1/vaults/work")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {token}"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", name)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn call_tool_json(
        router: &Router,
        token: &str,
        id: u64,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let response = router
            .clone()
            .oneshot(tool_request(token, id, name, arguments))
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{name}: {}",
            String::from_utf8_lossy(&body)
        );
        serde_json::from_slice(&body).unwrap()
    }

    fn assert_tool_ok(body: &serde_json::Value, name: &str) {
        assert_eq!(body["result"]["isError"], false, "{name}: {body}");
        assert_eq!(
            body["result"]["structuredContent"]["ok"], true,
            "{name}: {body}"
        );
    }

    fn mutation_revision(body: &serde_json::Value) -> u64 {
        body["result"]["structuredContent"]["data"]["revision"]["revision"]
            .as_u64()
            .unwrap()
    }

    fn list_tools_request(token: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/mcp/v1/vaults/work")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {token}"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/list")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#,
            ))
            .unwrap()
    }

    fn resource_read_request(token: &str, uri: &str) -> Request<Body> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "resources/read",
            "params": {
                "uri": uri,
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        Request::builder()
            .method("POST")
            .uri("/mcp/v1/vaults/work")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {token}"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "resources/read")
            .header("mcp-name", uri)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn stateless_discovery_is_authenticated_and_advertises_current_protocol() {
        let (router, token, _root) = configured_router().await;
        let response = router.oneshot(discover_request(&token)).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "mcp-vault"
        );
        assert_eq!(body["result"]["supportedVersions"][4], "2026-07-28");
        assert_eq!(body["result"]["cacheScope"], "private");
    }

    #[tokio::test]
    async fn list_tools_advertises_private_cache_ttl_for_2026_transport() {
        let (router, token, _root) = configured_router().await;
        let response = router.oneshot(list_tools_request(&token)).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["ttlMs"], super::LIST_CACHE_TTL_MS);
        assert_eq!(body["result"]["cacheScope"], "private");
    }

    #[tokio::test]
    async fn oauth_resource_server_token_is_accepted_for_its_granted_vault() {
        let (router, token, _root) = configured_oauth_router().await;
        let response = router.oneshot(discover_request(&token)).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["result"]["capabilities"]["tools"],
            serde_json::json!({})
        );
    }

    #[tokio::test]
    async fn oauth_metadata_is_public_vault_specific_and_redaction_safe() {
        let (router, _token, _root) = configured_oauth_router().await;
        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/mcp/v1/vaults/work",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .header("host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            assert_eq!(
                response.headers().get("cache-control").unwrap(),
                "no-store, max-age=0"
            );
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                body["resource"],
                "https://vault.example.test/mcp/v1/vaults/work"
            );
            assert_eq!(
                body["authorization_servers"],
                serde_json::json!(["https://issuer.example.test"])
            );
            assert_eq!(
                body["bearer_methods_supported"],
                serde_json::json!(["header"])
            );
            assert_eq!(body["scopes_supported"].as_array().unwrap().len(), 8);
            assert!(body.get("jwks_cache_json").is_none());
            assert!(body.get("subjects").is_none());
        }
    }

    #[tokio::test]
    async fn configured_public_origin_produces_an_absolute_oauth_challenge() {
        let (router, _token, _root) = configured_oauth_router().await;
        let mut request = discover_request("unused");
        request.headers_mut().remove("authorization");
        let response = router.oneshot(request).await.unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        let challenge = response
            .headers()
            .get("www-authenticate")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(challenge.contains(
            "resource_metadata=\"https://vault.example.test/.well-known/oauth-protected-resource/mcp/v1/vaults/work\""
        ));
    }

    #[tokio::test]
    async fn pat_cannot_use_the_other_vault_endpoint() {
        let (router, token, _root) = configured_router().await;
        let mut request = discover_request(&token);
        *request.uri_mut() = "/mcp/v1/vaults/other".parse().unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn managed_vault_mcp_is_unavailable_during_initialization() {
        let (router, token, _root) = configured_router_with_availability(full_scopes(), true).await;
        let response = router.oneshot(discover_request(&token)).await.unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn stateful_mount_rejects_missing_credentials_before_rmcp() {
        let (router, _token, _root) = configured_router().await;
        let mut request = discover_request("unused");
        request.headers_mut().remove("authorization");
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        let challenge = response
            .headers()
            .get("www-authenticate")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(challenge.contains("Bearer realm=\"mcp-vault\""));
        assert!(challenge.contains(
            "resource_metadata=\"/.well-known/oauth-protected-resource/mcp/v1/vaults/work\""
        ));
        assert!(challenge.contains("error=\"invalid_request\""));
    }

    #[tokio::test]
    async fn controlled_tools_round_trip_through_core_and_keep_structured_results() {
        let (router, token, _root) = configured_router().await;
        let create = router
            .clone()
            .oneshot(tool_request(
                &token,
                2,
                "create_note",
                serde_json::json!({"path": "notes/today.md", "content": "hello"}),
            ))
            .await
            .unwrap();
        let create_status = create.status();
        let create_body = create.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            create_status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&create_body)
        );
        let create_body: serde_json::Value = serde_json::from_slice(&create_body).unwrap();
        assert_eq!(create_body["result"]["isError"], false);
        assert_eq!(create_body["result"]["structuredContent"]["ok"], true);

        let conflict = router
            .clone()
            .oneshot(tool_request(
                &token,
                7,
                "edit_note",
                serde_json::json!({
                    "path": "notes/today.md",
                    "expected_revision": 99,
                    "operation": {"kind": "replace_all", "content": "must not win"}
                }),
            ))
            .await
            .unwrap();
        assert_eq!(conflict.status(), axum::http::StatusCode::OK);
        let conflict_body = conflict.into_body().collect().await.unwrap().to_bytes();
        let conflict_body: serde_json::Value = serde_json::from_slice(&conflict_body).unwrap();
        assert_eq!(conflict_body["result"]["isError"], true);
        assert_eq!(
            conflict_body["result"]["structuredContent"]["error"]["code"],
            "revision_conflict"
        );

        let read = router
            .oneshot(tool_request(
                &token,
                3,
                "read_note",
                serde_json::json!({"path": "notes/today.md"}),
            ))
            .await
            .unwrap();
        assert_eq!(read.status(), axum::http::StatusCode::OK);
        let read_body = read.into_body().collect().await.unwrap().to_bytes();
        let read_body: serde_json::Value = serde_json::from_slice(&read_body).unwrap();
        assert_eq!(
            read_body["result"]["structuredContent"]["data"]["content"],
            "hello"
        );
    }

    #[tokio::test]
    async fn indexed_search_round_trips_through_public_mcp() {
        let (router, token, _root) = configured_indexed_router().await;
        let moved = router
            .clone()
            .oneshot(tool_request(
                &token,
                10,
                "move_note",
                serde_json::json!({
                    "source": "notes/search.md",
                    "destination": "archive/search.md",
                    "expected_revision": 1
                }),
            ))
            .await
            .unwrap();
        let moved = moved.into_body().collect().await.unwrap().to_bytes();
        let moved: serde_json::Value = serde_json::from_slice(&moved).unwrap();
        assert_eq!(moved["result"]["isError"], false, "{moved}");

        let response = router
            .clone()
            .oneshot(tool_request(
                &token,
                11,
                "search_notes",
                serde_json::json!({
                    "query": "conflict",
                    "mode": "lexical",
                    "scope": {"tags": ["rust"]},
                    "limit": 10
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["isError"], false);
        assert_eq!(
            body["result"]["structuredContent"]["data"]["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            body["result"]["structuredContent"]["data"]["results"][0]["path"],
            "archive/search.md"
        );

        let read = router
            .oneshot(tool_request(
                &token,
                12,
                "read_note",
                serde_json::json!({"path": "archive/search.md"}),
            ))
            .await
            .unwrap();
        let read = read.into_body().collect().await.unwrap().to_bytes();
        let read: serde_json::Value = serde_json::from_slice(&read).unwrap();
        assert_eq!(read["result"]["isError"], false, "{read}");
    }

    #[tokio::test]
    async fn recall_returns_related_ordinary_notes_without_memory_promotion() {
        let (router, token, _root) = configured_indexed_memory_router().await;
        let response = router
            .oneshot(tool_request(
                &token,
                12,
                "recall",
                serde_json::json!({
                    "query": "WebDAV conflict handling",
                    "max_results": 5,
                    "max_related_notes": 5,
                    "max_tokens": 500
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["isError"], false, "{body}");
        let data = &body["result"]["structuredContent"]["data"];
        assert!(data["memories"].as_array().unwrap().is_empty());
        assert_eq!(data["related_notes"].as_array().unwrap().len(), 1);
        assert_eq!(data["related_notes"][0]["path"], "notes/search.md");
        assert_eq!(
            data["related_notes"][0]["resource_uri"],
            "vault://note/notes/search%2Emd"
        );
        assert_eq!(data["available_related_note_count"], 1);
    }

    #[tokio::test]
    async fn memory_only_scope_cannot_receive_ordinary_note_cues() {
        let scopes: ScopeSet = [Scope::MemoryRead].into_iter().collect();
        let (router, token, _root) = configured_indexed_router_with_scopes(scopes).await;
        let response = router
            .oneshot(tool_request(
                &token,
                13,
                "recall",
                serde_json::json!({
                    "query": "WebDAV conflict handling",
                    "max_results": 5,
                    "max_related_notes": 5,
                    "max_tokens": 500
                }),
            ))
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["isError"], false, "{body}");
        let data = &body["result"]["structuredContent"]["data"];
        assert!(data["related_notes"].as_array().unwrap().is_empty());
        assert_eq!(data["available_related_note_count"], 0);
    }

    #[tokio::test]
    async fn tool_list_is_scope_filtered_in_documented_order() {
        let scopes: ScopeSet = [Scope::VaultDiscover, Scope::VaultRead]
            .into_iter()
            .collect();
        let (router, token, _root) = configured_router_with_scopes(scopes).await;
        let response = router.oneshot(list_tools_request(&token)).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let names = body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "vault_overview",
                "browse_index",
                "recent_changes",
                "search_notes",
                "read_note"
            ]
        );
    }

    #[tokio::test]
    async fn note_resource_reads_are_vault_scoped_and_bounded() {
        let (router, token, _root) = configured_router().await;
        let create = router
            .clone()
            .oneshot(tool_request(
                &token,
                6,
                "create_note",
                serde_json::json!({"path": "notes/resource.md", "content": "resource text"}),
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), axum::http::StatusCode::OK);

        let response = router
            .oneshot(resource_read_request(
                &token,
                "vault://note/notes/resource.md",
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["contents"][0]["text"], "resource text");
    }

    #[tokio::test]
    async fn remember_stages_input_and_context_changes_only_after_consolidation() {
        let (router, token, _root) = configured_memory_router().await;
        let remember = router
            .clone()
            .oneshot(tool_request(
                &token,
                21,
                "remember",
                serde_json::json!({
                    "content": "The memory subsystem keeps canonical Markdown.",
                    "memory_type": "decision",
                    "importance": 0.95,
                    "confidence": 0.99,
                    "tags": ["architecture"],
                    "entities": ["MCP Vault"],
                    "idempotency_key": "mcp-memory-1"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(remember.status(), axum::http::StatusCode::OK);
        let body = remember.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["isError"], false);
        assert_eq!(
            body["result"]["structuredContent"]["data"]["outcome"],
            "staged"
        );
        assert!(body["result"]["structuredContent"]["data"]["memory"].is_null());
        let raw_memory_id = body["result"]["structuredContent"]["data"]["raw_memory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            body["result"]["structuredContent"]["data"]["consolidation_job_id"]
                .as_str()
                .is_some()
        );

        let recall = router
            .clone()
            .oneshot(tool_request(
                &token,
                22,
                "recall",
                serde_json::json!({"query": "canonical Markdown", "max_results": 5, "max_tokens": 500}),
            ))
            .await
            .unwrap();
        let body = recall.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["isError"], false);
        assert!(
            body["result"]["structuredContent"]["data"]["memories"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let resource = router
            .oneshot(resource_read_request(&token, "vault://memory/context"))
            .await
            .unwrap();
        assert_eq!(resource.status(), axum::http::StatusCode::OK);
        let body = resource.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["result"]["contents"][0]["mimeType"],
            "application/json"
        );
        assert!(
            !body["result"]["contents"][0]["text"]
                .as_str()
                .unwrap()
                .contains(&raw_memory_id)
        );
    }
}
