//! Stateless RMCP adapter for the Vault data plane.

mod presentation;

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
    MemoryError, MemoryOrigin, MemoryReadAccess, MemoryService, OverviewRequest, SemanticAccess,
    SemanticActor, SemanticCardRequest, SemanticEvidenceAccess, SemanticEvidenceRequest,
    SemanticExplicitDeleteRequest, SemanticExplicitFacade, SemanticExplicitListRequest,
    SemanticExplicitUpdatePatch, SemanticExplicitUpdateRequest, SemanticListCardsRequest,
    SemanticMutation, SemanticProcessingStatusRequest, SemanticPublicFacade,
    SemanticRememberExplicitRequest, SemanticRuleCommand,
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

/// RMCP deserializes `Parameters<T>` before invoking a tool. Keep the shared
/// card DTO schema while retaining a small validation shim so an invalid enum
/// is returned as the normal semantic `invalid_argument` envelope.
struct SemanticCardParameters(Value);

impl<'de> Deserialize<'de> for SemanticCardParameters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self)
    }
}

impl rmcp::schemars::JsonSchema for SemanticCardParameters {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <SemanticCardRequest as rmcp::schemars::JsonSchema>::schema_name()
    }

    fn json_schema(generator: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
        <SemanticCardRequest as rmcp::schemars::JsonSchema>::json_schema(generator)
    }
}

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
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Include newest-first revision metadata in the overview. Defaults to false.
    #[serde(default)]
    include_recent: Option<bool>,
    /// Maximum top-level topics and, separately, recent revisions. Range 1-100; default 25.
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct BrowseIndexInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Stable index node ID to expand. Omit to browse the root; reuse IDs returned by this tool.
    #[serde(default)]
    node_id: Option<String>,
    /// Child levels: 0 returns only the selected node and optional notes; 1 lists children; 2 also lists grandchildren. Default 1.
    #[serde(default)]
    depth: Option<u8>,
    /// Maximum number of children to return. Range 1-100; default 50.
    #[serde(default)]
    limit: Option<u32>,
    /// Opaque pagination cursor returned by a previous browse_index call.
    #[serde(default)]
    cursor: Option<String>,
    /// Include bounded candidate-note metadata for the selected node. Defaults to false.
    #[serde(default)]
    include_note_candidates: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct RecentChangesInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Maximum number of newest-first revisions to return. Range 1-100; default 50.
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct SearchNotesInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Natural-language concept or exact keywords to find in canonical Markdown notes.
    query: String,
    /// Retrieval strategy. Defaults to lexical; semantic and hybrid may report provider degradation.
    #[serde(default)]
    mode: Option<SearchMode>,
    /// Optional path, topic, tag, and modification-time filters applied within this Vault.
    #[serde(default)]
    scope: Option<SearchScope>,
    /// Result unit: `note` or `section`. Defaults to `note`.
    #[serde(default)]
    result_granularity: Option<String>,
    /// Maximum number of matches to return. Range 1-100; default 12.
    #[serde(default)]
    limit: Option<u32>,
    /// Opaque pagination cursor returned by a previous search_notes call.
    #[serde(default)]
    cursor: Option<String>,
    /// Include component ranking scores for diagnostics. Defaults to false.
    #[serde(default)]
    include_score_breakdown: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SearchMode {
    /// Exact-token full-text retrieval that works without an embedding provider.
    #[default]
    Lexical,
    /// Vector similarity retrieval; requires the note-embedding role and may degrade.
    Semantic,
    /// Rank-fused lexical and vector retrieval; falls back safely when vectors are unavailable.
    Hybrid,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct SearchScope {
    /// Vault-relative folder or file prefix, for example `projects/alpha`.
    #[serde(default)]
    path_prefix: Option<String>,
    /// Stable topic IDs returned by vault_overview or browse_index. Maximum 20.
    #[serde(default)]
    topic_ids: Vec<String>,
    /// Exact note tags that matches must carry. Maximum 20.
    #[serde(default)]
    tags: Vec<String>,
    /// Inclusive lower bound for note modification time as Unix milliseconds.
    #[serde(default)]
    modified_after: Option<i64>,
    /// Inclusive upper bound for note modification time as Unix milliseconds.
    #[serde(default)]
    modified_before: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct ReadNoteInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Current Vault-relative path from search, recall, browse, or another read result.
    path: String,
    /// Retained historical revision to read. Omit to read the current revision.
    #[serde(default)]
    revision: Option<u64>,
    /// Omit or use {"kind":"full"}. Only full-note reads are supported; increase max_bytes if truncated. Search section offsets are not Markdown byte offsets.
    #[serde(default)]
    selection: Option<NoteSelection>,
    /// Maximum bytes to return. Defaults to 131072 and cannot exceed 1048576.
    #[serde(default)]
    max_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NoteSelection {
    /// Read the complete file subject to max_bytes.
    Full,
    /// Reserved one-based Markdown line range; the current handler rejects it.
    #[schemars(skip)]
    LineRange {
        /// First one-based line to return.
        start_line: u32,
        /// Last one-based line to return; must not precede start_line.
        end_line: u32,
    },
    /// Reserved Markdown heading selection; the current handler rejects it.
    #[schemars(skip)]
    Heading {
        /// Heading text to select, without Markdown `#` markers.
        heading: String,
    },
    /// Reserved half-open byte range; the current handler rejects it.
    #[schemars(skip)]
    ByteRange {
        /// Zero-based first byte offset.
        start: u64,
        /// Exclusive end byte offset; must exceed start.
        end: u64,
    },
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
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// New Vault-relative Markdown path, for example `projects/alpha/status.md`.
    path: String,
    /// Complete UTF-8 Markdown body for the new note, up to 1048576 bytes.
    content: String,
    /// Absent-path precondition. Omit or set true; false is rejected.
    #[serde(default)]
    if_absent: Option<bool>,
    /// Stable retry key for this logical creation; reuse only with the identical request.
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct EditNoteInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Existing Vault-relative note path obtained from a current read or search result.
    path: String,
    /// Current revision returned by read_note or search_notes; conflicts must be reread, not overwritten.
    expected_revision: u64,
    /// One exact edit operation. Prefer the narrowest operation that preserves unrelated content.
    operation: EditOperation,
    /// Stable retry key for this logical edit when the selected operation supports safe retry.
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum EditOperation {
    /// Replace the entire note; use only when the complete desired body is known.
    ReplaceAll {
        /// Complete replacement Markdown body, up to 1048576 bytes.
        content: String,
    },
    /// Apply an exact unified diff; context must match and fuzzy patching is never attempted.
    ApplyUnifiedDiff {
        /// Unified diff against the expected current revision.
        patch: String,
    },
    /// Append Markdown to the end of the note without changing existing text.
    Append {
        /// Markdown content to append.
        content: String,
    },
    /// Insert Markdown immediately after a uniquely matched heading.
    InsertAfterHeading {
        /// Existing heading text, without Markdown `#` markers.
        heading: String,
        /// Markdown to insert after the heading.
        insertion: String,
    },
    /// Replace one heading and its section while preserving the rest of the note.
    ReplaceHeadingSection {
        /// Existing heading text, without Markdown `#` markers.
        heading: String,
        /// Complete replacement Markdown for that heading section.
        replacement: String,
    },
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct MoveNoteInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Existing Vault-relative source note or directory path.
    source: String,
    /// New Vault-relative destination path, which must not already exist.
    destination: String,
    /// Current source revision returned by a read, search, or metadata result.
    expected_revision: u64,
    /// Stable retry key for this logical move; reuse only with the identical request.
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct DeleteNoteInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Existing Vault-relative note path to remove.
    path: String,
    /// Current revision returned by read_note or search_notes.
    expected_revision: u64,
    /// Deletion mode. Only `trash` is currently supported; `permanent` is rejected.
    #[serde(default)]
    mode: DeleteMode,
    /// Stable retry key for this logical deletion; reuse only with the identical request.
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DeleteMode {
    /// Create a recoverable tombstone and preserve required history.
    #[default]
    Trash,
    /// Reserved value; the current MCP surface rejects permanent note deletion.
    #[schemars(skip)]
    Permanent,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct NoteHistoryInput {
    /// Maximum retained revisions per page. Range 1-100; default 25, newest first.
    #[serde(default)]
    limit: Option<u32>,
    /// Opaque cursor from note_history. Reuse it with the same path and limit.
    #[serde(default)]
    cursor: Option<String>,
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Vault-relative path whose retained revision metadata should be listed.
    path: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
struct RestoreNoteRevisionInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Current Vault-relative note path.
    path: String,
    /// Retained historical revision whose content should become current.
    revision: u64,
    /// Current live revision checked before restoration to prevent overwriting newer work.
    expected_current_revision: u64,
    /// Stable retry key for this logical restore; reuse only with the identical request.
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryOverviewInput {
    /// Return scoped counts and directory grouping details; default false. Current entries and warnings are always retained.
    #[serde(default)]
    include_details: Option<bool>,
    /// Optional exact source path to browse.
    source_path: Option<String>,
    /// Directory path prefix, matched at a path-segment boundary.
    path_prefix: Option<String>,
    /// Existing topic keys obtained from browse_index; all must match.
    #[serde(default)]
    topic_ids: Vec<String>,
    /// Last unit ID returned by next_after_id, with the same source filter.
    after_id: Option<String>,
    /// Maximum navigation entries. Range 1-100; default 40.
    #[schemars(range(min = 1, max = 100))]
    limit: Option<u32>,
    /// Complete response token estimate budget. Range 256-32000; default 4096.
    #[schemars(range(min = 256, max = 32000))]
    max_tokens: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawMemoryIdInput {
    /// Stable raw explicit-memory ID returned by list_raw_memories or remember.
    memory_id: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawUpdateInput {
    /// Stable raw explicit-memory ID returned by get_raw_memory or list_raw_memories.
    #[schemars(
        description = "Stable raw explicit-memory ID returned by get_raw_memory or list_raw_memories."
    )]
    memory_id: String,
    /// Current raw memory revision; reread after a conflict.
    #[schemars(description = "Current raw memory revision; reread after a conflict.")]
    expected_revision: u64,
    /// Revision-fenced replacement fields; omitted values remain unchanged.
    #[schemars(
        description = "Revision-fenced replacement fields; omitted values remain unchanged."
    )]
    patch: SemanticExplicitUpdatePatch,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawDeleteInput {
    /// Stable raw explicit-memory ID returned by get_raw_memory or list_raw_memories.
    #[schemars(
        description = "Stable raw explicit-memory ID returned by get_raw_memory or list_raw_memories."
    )]
    memory_id: String,
    /// Current raw memory revision; reread after a conflict.
    #[schemars(description = "Current raw memory revision; reread after a conflict.")]
    expected_revision: u64,
    /// Non-empty retry key for this logical deletion.
    #[schemars(description = "Non-empty retry key for this logical deletion.")]
    idempotency_key: String,
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
struct UpdateMemoryInput {
    /// Return extended metadata. Default false; essential paths, revisions and warnings are always included.
    #[serde(default)]
    include_details: Option<bool>,
    /// Stable durable-memory ID returned by get_raw_memory, recall, or list_raw_memories.
    id: String,
    /// Current memory revision returned by get_raw_memory, not canonical_revision or a source-note revision. On conflict, get_raw_memory again.
    expected_revision: u64,
    /// Replacement durable proposition. Omit to preserve the current content.
    #[serde(default)]
    content: Option<String>,
    /// Replacement type from the supported memory-type list. Omit to preserve; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    memory_type: Option<Option<String>>,
    /// Replacement importance in the inclusive range 0-1. Null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    importance: Option<Option<f64>>,
    /// Replacement confidence in the inclusive range 0-1. Null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    confidence: Option<Option<f64>>,
    /// Replacement validity start as Unix milliseconds. Omit to preserve; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    valid_from: Option<Option<i64>>,
    /// Replacement exclusive validity end as Unix milliseconds. Omit to preserve; null clears it.
    #[serde(default, deserialize_with = "deserialize_patch_field")]
    valid_to: Option<Option<i64>>,
    /// Complete replacement tag set. Omit to preserve; [] clears all tags.
    #[serde(default)]
    tags: Option<Vec<String>>,
    /// Complete replacement entity set. Omit to preserve; [] clears all entities.
    #[serde(default)]
    entities: Option<Vec<String>>,
}

#[cfg(test)]
fn deserialize_patch_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
struct ToolErrorBody {
    /// Stable machine-readable reason for the failure, such as `invalid_argument` or `conflict`.
    code: String,
    /// Sanitized human-readable explanation of what failed.
    message: String,
    /// True only when retrying the same logical operation later may succeed without changing its intent.
    retryable: bool,
    /// Optional sanitized structured context that may help correct the next call.
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
    /// Request identifier for correlating this result with logs or support diagnostics.
    request_id: String,
    /// True when the call succeeded and `data` is present; false when `error` is present.
    ok: bool,
    /// Tool-specific success object described by the selected tool. Omitted on failure.
    /// Keeping this object-shaped also preserves compatibility with dated MCP
    /// schemas, which reject an unconstrained `true` value schema here.
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Map<String, Value>>,
    /// Failure information. Inspect `code` and `retryable`; omitted on success.
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
        name = "build_memory_pack",
        title = "Build semantic memory pack",
        description = "Use this when the task needs sourced semantic background. Build a Vault-scoped semantic MemoryPack using current verified M1/M2 cards. On success, `data` contains the semantic pack and its bounded evidence gaps.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn build_memory_pack(
        &self,
        Parameters(input): Parameters<mcp_vault_memory::MemoryPackRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let facade = SemanticPublicFacade::new(request.state.clone());
        match facade
            .build_pack(
                &request.vault,
                &request.core,
                &SemanticAccess::new(request.principal.permissions.clone()),
                &input,
            )
            .await
        {
            Ok(data) => Ok(success_result(
                &context,
                serde_json::to_value(data).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "get_memory_card",
        title = "Get semantic memory card",
        description = "Use this when you know a semantic card reference and need its sourced semantic fields. Read one current verified semantic card without ordinary note正文. On success, `data.card` contains the card fields and source bindings.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn get_memory_card(
        &self,
        Parameters(raw): Parameters<SemanticCardParameters>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let input: SemanticCardRequest = match serde_json::from_value(raw.0) {
            Ok(input) => input,
            Err(_) => {
                return Ok(error_result(
                    &context,
                    ToolErrorBody::new(
                        "invalid_argument",
                        "card_kind must be card or composed_card",
                        false,
                    ),
                ));
            }
        };
        let request = request_context(&context)?;
        let facade = SemanticPublicFacade::new(request.state.clone());
        let result = if matches!(
            input.card_kind,
            Some(mcp_vault_memory::SemanticCardKind::ComposedCard)
        ) {
            facade
                .get_composed_card(
                    &request.vault,
                    &request.core,
                    &SemanticAccess::new(request.principal.permissions.clone()),
                    &input.card_id,
                )
                .await
                .map(|data| data.map(|card| json!({"composed_card": card})))
        } else {
            facade
                .get_card(
                    &request.vault,
                    &request.core,
                    &SemanticAccess::new(request.principal.permissions.clone()),
                    &input.card_id,
                )
                .await
                .map(|data| data.map(|card| json!({"card": card})))
        };
        match result {
            Ok(Some(data)) => Ok(success_result(&context, data)),
            Ok(None) => Ok(error_result(
                &context,
                ToolErrorBody::new("not_found", "the semantic card was not found", false),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "list_memory_cards",
        title = "List semantic memory cards",
        description = "Use this when you need to discover current sourced semantic cards. List current verified M1 and M2 semantic cards. On success, `data.cards` and `data.composed_cards` contain the results.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn list_memory_cards(
        &self,
        Parameters(input): Parameters<SemanticListCardsRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let facade = SemanticPublicFacade::new(request.state.clone());
        let limit = match input.limit.unwrap_or(50) {
            1..=200 => input.limit.unwrap_or(50),
            _ => {
                return Ok(error_result(
                    &context,
                    ToolErrorBody::new("invalid_argument", "semantic card limit is invalid", false),
                ));
            }
        };
        match facade
            .list_cards(
                &request.vault,
                &request.core,
                &SemanticAccess::new(request.principal.permissions.clone()),
                limit,
            )
            .await
        {
            Ok((cards, composed_cards)) => Ok(success_result(
                &context,
                json!({"cards":cards,"composed_cards":composed_cards}),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "get_memory_evidence",
        title = "Get semantic evidence",
        description = "Use this when a semantic card cites evidence and you have its source binding. Read one current evidence reference with explicit source and optional parent binding. On success, `data.evidence` contains exact spans and coordinates.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn get_memory_evidence(
        &self,
        Parameters(input): Parameters<SemanticEvidenceRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let facade = SemanticPublicFacade::new(request.state.clone());
        let access = SemanticEvidenceAccess {
            source_id: input.source_id,
            source_revision_id: input.source_revision_id,
            parent_ref: input.parent_ref,
        };
        match facade
            .read_evidence(
                &request.vault,
                &request.core,
                &SemanticAccess::new(request.principal.permissions.clone()),
                &access,
                &input.evidence_ref_id,
            )
            .await
        {
            Ok(Some(data)) => Ok(success_result(&context, json!({"evidence":data}))),
            Ok(None) => Ok(error_result(
                &context,
                ToolErrorBody::new("not_found", "the semantic evidence was not found", false),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "correct_memory",
        title = "Correct semantic memory",
        description = "Use this only when the user explicitly authorizes a semantic correction. Apply a typed persistent correction to a resolved semantic card target. On success, `data` contains the rule id and revision.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn correct_memory(
        &self,
        Parameters(input): Parameters<SemanticRuleCommand>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if !matches!(input.mutation, SemanticMutation::Correction) {
            return Ok(error_result(
                &context,
                ToolErrorBody::new("invalid_input", "correction mutation is required", false),
            ));
        }
        semantic_rule_call(&context, input).await
    }

    #[tool(
        name = "forget_memory",
        title = "Forget semantic memory",
        description = "Use this only when the user explicitly asks to forget semantic memory. Apply an explicit safe semantic forget/suppression action without deleting source notes. On success, `data` contains the rule id and revision.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn forget_memory(
        &self,
        Parameters(input): Parameters<SemanticRuleCommand>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if !matches!(
            input.mutation,
            SemanticMutation::SuppressRead
                | SemanticMutation::ForgetCurrent
                | SemanticMutation::SuppressRegeneration
        ) {
            return Ok(error_result(
                &context,
                ToolErrorBody::new("invalid_input", "forget mutation is required", false),
            ));
        }
        semantic_rule_call(&context, input).await
    }

    #[tool(
        name = "get_processing_status",
        title = "Get semantic processing status",
        description = "Use this when you need to inspect semantic processing progress. Read bounded safe semantic extraction and organization processing status. On success, `data` contains safe counts and states.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn get_processing_status(
        &self,
        Parameters(input): Parameters<SemanticProcessingStatusRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let facade = SemanticPublicFacade::new(request.state.clone());
        match facade
            .processing_status(
                &request.vault,
                &SemanticAccess::new(request.principal.permissions.clone()),
                input.limit.unwrap_or(50),
            )
            .await
        {
            Ok(data) => Ok(success_result(
                &context,
                serde_json::to_value(data).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "vault_overview",
        title = "Inspect Vault overview",
        description = "Use this when you need an overview of the Vault. Optionally include recent changes. On success, `data` contains statistics, topics, index coverage and optional recent revisions. Pass a topic id to browse_index; read a known path directly with read_note. Do not repeatedly request the overview for a known note.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn vault_overview(
        &self,
        Parameters(input): Parameters<VaultOverviewInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("vault_overview", data, include_details),
            )
        };
        if let Err(error) = require_permission(&request.principal, Permission::DiscoverVault) {
            return Ok(error_result(&context, error));
        }
        let limit = match bounded_limit(input.limit, 25) {
            Ok(limit) => limit,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match overview_data(&request, input.include_recent.unwrap_or(false), limit).await {
            Ok(data) => Ok(success(data)),
            Err(error) => Ok(error_result(&context, error)),
        }
    }

    #[tool(
        name = "browse_index",
        title = "Browse knowledge index",
        description = "Use this when exploring topics without known keywords. Omit node_id for root; reuse returned IDs. On success, `data.children` contains expandable topics and optional note_candidates have paths. Pass a child id back here or a note path to read_note. Continue with next_cursor and unchanged filters; use search_notes only when content matching is needed.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn browse_index(
        &self,
        Parameters(input): Parameters<BrowseIndexInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("browse_index", data, include_details),
            )
        };
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
            Ok(data) => Ok(success(data)),
            Err(error) => Ok(error_result(&context, error)),
        }
    }

    #[tool(
        name = "recent_changes",
        title = "View recent Vault changes",
        description = "Use this when recent edits matter. Set limit for newest-first revisions. On success, `data.changes` contains operation, revision, paths and timestamp. Read path_after with read_note for an active note; for a deletion use path_before with note_history. No bodies or pagination are included; note_history gives the retained versions of a known path.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn recent_changes(
        &self,
        Parameters(input): Parameters<RecentChangesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("recent_changes", data, include_details),
            )
        };
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
        Ok(success(json!({
            "changes": revisions.iter().map(revision_json).collect::<Vec<_>>(),
            "limit": limit,
        })))
    }

    #[tool(
        name = "search_notes",
        title = "Search Vault notes",
        description = "Use this when the note path is unknown and you need keyword or semantic matching. mode defaults to lexical; hybrid combines lexical and semantic matches. On success, `data.results` contains path, file_id, revision, title, snippet and resource_uri. Read a returned path directly with read_note instead of searching again. next_cursor continues with unchanged query and filters. Section matches are indexed plain-text coordinates, not Markdown byte offsets; read_note currently reads full Markdown. degradation_reasons explains degraded results; truncated means incomplete. include_details or include_score_breakdown adds diagnostic metadata.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn search_notes(
        &self,
        Parameters(input): Parameters<SearchNotesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false)
            || input.include_score_breakdown.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("search_notes", data, include_details),
            )
        };
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
            Ok(data) => Ok(success(data)),
            Err(error) => Ok(error_result(&context, error)),
        }
    }

    #[tool(
        name = "read_note",
        title = "Read a Vault note",
        description = "Use this when you know a note path, including sources[].path from a memory. Pass the path directly; no search is required. Omit revision for current content or pass one from note_history. On success, `data` contains content, file_id, path, selected revision, size, content_hash and truncated. If truncated, raise max_bytes up to 1048576; do not claim the excerpt is complete. Binary content is not text. Only full selection is supported. Before a write use a current read revision, never a historical one; on not_found search for a moved note.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn read_note(
        &self,
        Parameters(input): Parameters<ReadNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("read_note", data, include_details),
            )
        };
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
        let path = match parse_user_tool_path(&request.core, &input.path) {
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
        let (file, revision, size, hash, reader) = match read {
            ReadSource::Current(read) => (
                read.file.clone(),
                read.file.current_revision,
                Some(read.file.size),
                read.file.content_hash.clone(),
                read.reader,
            ),
            ReadSource::Historical(read) => (
                read.file,
                read.revision.revision,
                read.revision.size,
                read.revision.content_hash.clone(),
                read.reader,
            ),
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
        Ok(success(json!({
            "file_id": file.id.to_string(),
            "path": path.as_str(),
            "revision": revision.value(),
            "selection": {"kind": "full"},
            "size": size,
            "content_hash": hash,
            "truncated": truncated,
            "content": text,
            "binary": binary,
            "resource_uri": note_resource_uri(&path),
        })))
    }

    #[tool(
        name = "get_raw_memory_overview",
        title = "Browse raw memory overview",
        description = "Use this when you need bounded navigation of current raw explicit memories before reading one. On success, `data.entries` contains raw memory IDs, revisions and raw-memory resource URIs; labels are navigation only. Use the returned cursor with the same limit, then call get_raw_memory for a complete raw body. This view is explicit-memory-only and never returns semantic cards, evidence or task packs.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn get_raw_memory_overview(
        &self,
        Parameters(input): Parameters<MemoryOverviewInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_permission(&request.principal, Permission::ReadMemory) {
            return Ok(error_result(&context, error));
        }
        let can_read_vault = request
            .principal
            .permissions
            .contains(Permission::ReadVault);
        let after_id = match input.after_id.as_deref().map(parse_memory_id).transpose() {
            Ok(id) => id,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request
            .memory
            .get_memory_overview(
                &request.vault,
                OverviewRequest {
                    access: MemoryReadAccess::ExplicitOnly,
                    source_path: can_read_vault.then_some(input.source_path).flatten(),
                    path_prefix: can_read_vault.then_some(input.path_prefix).flatten(),
                    topic_ids: if can_read_vault {
                        input.topic_ids
                    } else {
                        Vec::new()
                    },
                    after_id,
                    limit: input.limit.unwrap_or(40),
                    max_tokens: input.max_tokens.unwrap_or(4096),
                },
            )
            .await
        {
            Ok(value) => Ok(success_result(
                &context,
                raw_tool_data(
                    "get_raw_memory_overview",
                    json!(value),
                    input.include_details.unwrap_or(false),
                    can_read_vault,
                ),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "get_raw_memory",
        title = "Read a raw explicit memory",
        description = "Use this when you know a raw explicit memory ID and need its complete user-owned body. On success, `data` contains raw ownership, content, revision and source bindings. This is an explicit-memory read only; it never returns a semantic card, evidence record or MemoryPack. Use the returned revision for update_raw_memory or forget_raw_memory; conflicts require a fresh read.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn get_raw_memory(
        &self,
        Parameters(input): Parameters<RawMemoryIdInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = false;
        let success = |data| {
            success_result(
                &context,
                raw_tool_data(
                    "get_raw_memory",
                    data,
                    include_details,
                    request
                        .principal
                        .permissions
                        .contains(Permission::ReadVault),
                ),
            )
        };
        if let Err(error) = require_permission(&request.principal, Permission::ReadMemory) {
            return Ok(error_result(&context, error));
        }
        match SemanticExplicitFacade::new(request.memory.clone())
            .get(&request.vault, &input.memory_id)
            .await
        {
            Ok(Some(memory)) => Ok(success(
                serde_json::to_value(memory).unwrap_or_else(|_| json!({})),
            )),
            Ok(None) => Ok(error_result(
                &context,
                ToolErrorBody::new("not_found", "the raw explicit memory was not found", false),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "list_raw_memories",
        title = "List raw explicit memories",
        description = "Use this when browsing current raw explicit memories in bounded pages. On success, `data.memories` contains raw ownership, complete user-owned bodies and revisions; `next_cursor` continues the same page size. Use get_raw_memory for one known ID. This tool never returns semantic cards, evidence or MemoryPacks.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn list_raw_memories(
        &self,
        Parameters(input): Parameters<SemanticExplicitListRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = false;
        let success = |data| {
            success_result(
                &context,
                raw_tool_data(
                    "list_raw_memories",
                    data,
                    include_details,
                    request
                        .principal
                        .permissions
                        .contains(Permission::ReadVault),
                ),
            )
        };
        if let Err(error) = require_permission(&request.principal, Permission::ReadMemory) {
            return Ok(error_result(&context, error));
        }
        match SemanticExplicitFacade::new(request.memory.clone())
            .list(&request.vault, &input)
            .await
        {
            Ok(result) => Ok(success(
                serde_json::to_value(result).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "remember",
        title = "Save a durable memory",
        description = "Use this only when the user explicitly authorizes saving a durable explicit memory. Provide the exact content and a new idempotency_key for each logical save; reuse that key only for an identical retry. Optional source bindings require read access and path, file_id and current revision from read_note. On success, `data.explicit.memory` contains the saved memory, its file path and revision, source bindings and embedding_eligible; use the returned identity for later memory operations. This does not create a semantic card or evidence record.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn remember(
        &self,
        Parameters(input): Parameters<SemanticRememberExplicitRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::WriteMemory) {
            return Ok(error_result(&context, error));
        }
        if !input.sources.is_empty()
            && let Err(error) = require_permission(&request.principal, Permission::ReadVault)
        {
            return Ok(error_result(&context, error));
        }
        match SemanticExplicitFacade::new(request.memory.clone())
            .remember(
                &request.vault,
                &request.core,
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                MemoryOrigin::ExplicitAgent,
                input,
            )
            .await
        {
            Ok(result) => Ok(success_result(
                &context,
                raw_tool_data(
                    "remember",
                    json!({"explicit": result}),
                    false,
                    request
                        .principal
                        .permissions
                        .contains(Permission::ReadVault),
                ),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "update_raw_memory",
        title = "Update a raw explicit memory",
        description = "Use this only when the user authorizes changing a raw explicit memory. First call get_raw_memory and use its revision as expected_revision; omitted fields stay unchanged, null clears nullable metadata and [] clears tags/entities. On success, `data` contains raw ownership, the updated body and revision. This does not edit a source note or semantic card; conflicts require a fresh raw read. Use forget_raw_memory for deletion.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn update_raw_memory(
        &self,
        Parameters(input): Parameters<RawUpdateInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = false;
        let success = |data| {
            success_result(
                &context,
                raw_tool_data(
                    "update_raw_memory",
                    data,
                    include_details,
                    request
                        .principal
                        .permissions
                        .contains(Permission::ReadVault),
                ),
            )
        };
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::ManageMemory) {
            return Ok(error_result(&context, error));
        }
        let input = SemanticExplicitUpdateRequest {
            memory_id: input.memory_id,
            expected_revision: input.expected_revision,
            patch: input.patch,
        };
        match SemanticExplicitFacade::new(request.memory.clone())
            .update(
                &request.vault,
                &request.core,
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                &input,
            )
            .await
        {
            Ok(memory) => Ok(success(
                serde_json::to_value(memory).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "forget_raw_memory",
        title = "Forget a raw explicit memory",
        description = "Use this only when the user explicitly asks to delete a raw explicit memory. First call get_raw_memory and use its revision as expected_revision. `idempotency_key` is required: retry the identical request with the same key, but use a new key for a different request because reusing a key with different input conflicts. On success, `data` contains the raw deletion receipt without the deleted body. This deletes only the raw explicit memory and never deletes source notes, semantic cards, evidence or task packs; revision conflicts require a fresh raw read.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn forget_raw_memory(
        &self,
        Parameters(input): Parameters<RawDeleteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = false;
        let success = |data| {
            success_result(
                &context,
                raw_tool_data("forget_raw_memory", data, include_details, false),
            )
        };
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::ManageMemory) {
            return Ok(error_result(&context, error));
        }
        let input = SemanticExplicitDeleteRequest {
            memory_id: input.memory_id,
            expected_revision: input.expected_revision,
            idempotency_key: input.idempotency_key,
        };
        match SemanticExplicitFacade::new(request.memory.clone())
            .delete(
                &request.vault,
                &request.core,
                request.principal.actor.clone(),
                SourcePlane::Mcp,
                &input,
            )
            .await
        {
            Ok(memory) => Ok(success(
                serde_json::to_value(memory).unwrap_or_else(|_| json!({})),
            )),
            Err(error) => Ok(error_result(&context, memory_error(error))),
        }
    }

    #[tool(
        name = "create_note",
        title = "Create a Vault note",
        description = "Use this when the user authorizes a new Markdown note and no known existing note should be updated. Supply path and full content; if_absent must be true or omitted. On success, `data.file` gives path, file_id and revision; data.revision records the operation and data.etag the result. Reuse the returned path directly for read_note or an authorized edit. If already_exists, read the existing note and reconsider. Reuse idempotency_key only for the identical create.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn create_note(
        &self,
        Parameters(input): Parameters<CreateNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("create_note", data, include_details),
            )
        };
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
        let path = match parse_user_tool_path(&request.core, &input.path) {
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
            Ok(result) => Ok(success(mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "edit_note",
        title = "Edit a Vault note",
        description = "Use this when the user authorizes a note change. First call read_note for current content and revision. Choose replace_all, apply_unified_diff, append, insert_after_heading or replace_heading_section. On success, `data.file` contains the path and new revision, with operation receipt in data.revision and data.etag. Use the new revision for a subsequent authorized edit. On conflict reread and reconsider, never overwrite blindly. Heading text must match uniquely; replacement sections must contain the desired heading and body.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn edit_note(
        &self,
        Parameters(input): Parameters<EditNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("edit_note", data, include_details),
            )
        };
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::WriteVault) {
            return Ok(error_result(&context, error));
        }
        let path = match parse_user_tool_path(&request.core, &input.path) {
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
            Ok(result) => Ok(success(mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "move_note",
        title = "Move or rename a Vault note",
        description = "Use this only when the user explicitly requests a move or rename. Supply source, an absent destination, and the current source revision. On success, `data.file.path` is the new path and data.file.revision is current; data.revision records both paths. Use the new path for subsequent reads/writes. On conflict reread and reconsider; never reorganize the Vault without authorization. Reuse idempotency_key only for the identical move.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn move_note(
        &self,
        Parameters(input): Parameters<MoveNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("move_note", data, include_details),
            )
        };
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        if let Err(error) = require_permission(&request.principal, Permission::WriteVault) {
            return Ok(error_result(&context, error));
        }
        let source = match parse_user_tool_path(&request.core, &input.source) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let destination = match parse_user_tool_path(&request.core, &input.destination) {
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
            Ok(result) => Ok(success(mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "delete_note",
        title = "Delete a Vault note",
        description = "Use this only when the user explicitly requests note deletion. First read_note; pass its current revision. mode must be trash or omitted. On success, `data.file.active` is false and data.revision records the deletion. To inspect retained content use note_history and read_note with a historical revision. Restoration requires explicit authorization and restore_note_revision. On conflict reread and reconsider; this does not mean permanent erasure of retained history.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn delete_note(
        &self,
        Parameters(input): Parameters<DeleteNoteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("delete_note", data, include_details),
            )
        };
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
        let path = match parse_user_tool_path(&request.core, &input.path) {
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
            Ok(result) => Ok(success(mutation_json(&result))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "note_history",
        title = "View note revision history",
        description = "Use this when inspecting edits or selecting a retained version to restore, including a deleted note. Supply its current or last path. On success, `data.revisions` lists revision, operation, paths and timestamp. Read a chosen version with read_note(path, revision) before considering restoration. next_cursor continues with the same path and limit. Default 25 revisions, newest first. Use only the latest live/tombstone revision as a write precondition; older revisions are for historical reads. No bodies are included.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn note_history(
        &self,
        Parameters(input): Parameters<NoteHistoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("note_history", data, include_details),
            )
        };
        if let Err(error) = require_permission(&request.principal, Permission::ReadHistory) {
            return Ok(error_result(&context, error));
        }
        let path = match parse_user_tool_path(&request.core, &input.path) {
            Ok(path) => path,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let limit = match bounded_limit(input.limit, 25) {
            Ok(value) => value,
            Err(error) => return Ok(error_result(&context, error)),
        };
        let offset = match parse_cursor(input.cursor.as_deref()) {
            Ok(value) => value,
            Err(error) => return Ok(error_result(&context, error)),
        };
        match request.core.history(&request.vault, &path).await {
            Ok(history) => Ok(success(json!({
                "path": path.as_str(),
                "next_cursor": (offset.saturating_add(limit) < history.len() as u32).then(|| format!("offset:{}",offset.saturating_add(limit))),
                "truncated": offset.saturating_add(limit) < history.len() as u32,
                "revisions": history.iter().rev().skip(offset as usize).take(limit as usize).map(revision_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(error_result(&context, vault_error(error))),
        }
    }

    #[tool(
        name = "restore_note_revision",
        title = "Restore a note revision",
        description = "Use this only when the user authorizes restoration. Inspect note_history then read_note with the target revision. Supply revision for the target and expected_current_revision for the latest live/tombstone state; for a deleted note use its deletion receipt or latest history. On success, `data.file` contains the new current revision and path, and data.revision records restoration. Read the restored path directly if verification is needed. On conflict refresh state and reconsider.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ToolEnvelope>()
    )]
    async fn restore_note_revision(
        &self,
        Parameters(input): Parameters<RestoreNoteRevisionInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = request_context(&context)?;
        let include_details = input.include_details.unwrap_or(false);
        let success = |data| {
            success_result(
                &context,
                presentation::tool_data("restore_note_revision", data, include_details),
            )
        };
        if let Err(error) = require_writable(&request) {
            return Ok(error_result(&context, error));
        }
        for permission in [Permission::ReadHistory, Permission::WriteVault] {
            if let Err(error) = require_permission(&request.principal, permission) {
                return Ok(error_result(&context, error));
            }
        }
        let path = match parse_user_tool_path(&request.core, &input.path) {
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
            Ok(result) => Ok(success(mutation_json(&result))),
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
             Use build_memory_pack proactively when the task may depend on prior decisions, preferences, constraints, project state, past work, or knowledge that may already exist in the Vault. Pass the task in its natural language and consume only the returned sourced semantic cards, qualifiers, support and evidence gaps. Use get_memory_card or list_memory_cards to inspect semantic cards, and get_memory_evidence only when a card's support must be verified. Semantic tools never return complete raw memory bodies. Use get_raw_memory_overview, get_raw_memory and list_raw_memories only for explicitly owned raw memory; raw tools never return semantic cards, evidence or task packs.\n\
             When the user requests or clearly authorizes a persistent note change, use a known source path directly; search only if the path is unknown. Read its current revision, and use the narrowest mutation; create a note only when no existing note should be updated. Never overwrite a revision conflict.\n\
             Every result has request_id and ok. On success consume data; on failure inspect error.code and error.retryable, and retry the same logical operation only when retryable is true. Treat degraded or truncated results as incomplete coverage.",
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
                Resource::new("vault://raw-memory/context", "vault-raw-memory-context")
                    .with_description("Compact current raw explicit-memory navigation")
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
                ResourceTemplate::new("vault://raw-memory/{memory_id}", "vault-raw-memory")
                    .with_description("Complete current raw explicit memory and source bindings")
                    .with_mime_type("application/json"),
            );
        }
        if can_read_vault && can_read_memory {
            templates.push(
                ResourceTemplate::new("vault://memory-card/{card_id}", "vault-memory-card")
                    .with_description("Current semantic memory card with support bindings")
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
                if request_context.core.is_managed_path(&path) {
                    return Err(ErrorData::invalid_params("resource is not available", None));
                }
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
            "raw-memory" => {
                AuthService::require_permission(&request_context.principal, Permission::ReadMemory)
                    .map_err(|_| ErrorData::invalid_params("resource is not available", None))?;
                let value = url.path().trim_matches('/');
                if value == "context" {
                    let memories = request_context
                        .memory
                        .get_memory_overview(
                            &request_context.vault,
                            OverviewRequest {
                                access: MemoryReadAccess::ExplicitOnly,
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(|_| {
                            ErrorData::internal_error("memory resource is unavailable", None)
                        })?;
                    ResourceContents::text(
                        raw_tool_data(
                            "get_raw_memory_overview",
                            serde_json::to_value(memories).unwrap_or_else(|_| json!({})),
                            true,
                            request_context
                                .principal
                                .permissions
                                .contains(Permission::ReadVault),
                        )
                        .to_string(),
                        request.uri.clone(),
                    )
                    .with_mime_type("application/json")
                } else {
                    let memory_id = MemoryId::parse(value).map_err(|_| {
                        ErrorData::invalid_params("memory resource is invalid", None)
                    })?;
                    let memory = request_context
                        .memory
                        .get_with_access(
                            &request_context.vault,
                            memory_id,
                            MemoryReadAccess::ExplicitOnly,
                        )
                        .await
                        .map_err(|_| ErrorData::invalid_params("memory is not available", None))?;
                    ResourceContents::text(
                        raw_tool_data(
                            "get_raw_memory",
                            serde_json::to_value(memory).unwrap_or_else(|_| json!({})),
                            true,
                            request_context
                                .principal
                                .permissions
                                .contains(Permission::ReadVault),
                        )
                        .to_string(),
                        request.uri.clone(),
                    )
                    .with_mime_type("application/json")
                }
            }
            "memory-card" => {
                AuthService::require_permission(&request_context.principal, Permission::ReadMemory)
                    .and(AuthService::require_permission(
                        &request_context.principal,
                        Permission::ReadVault,
                    ))
                    .map_err(|_| ErrorData::invalid_params("resource is not available", None))?;
                let card_id = url.path().trim_matches('/');
                if card_id.is_empty() || card_id.contains('/') {
                    return Err(ErrorData::invalid_params(
                        "semantic card resource is invalid",
                        None,
                    ));
                }
                let facade = SemanticPublicFacade::new(request_context.state.clone());
                let access = SemanticAccess::new(request_context.principal.permissions.clone());
                let card = facade
                    .get_card(
                        &request_context.vault,
                        &request_context.core,
                        &access,
                        card_id,
                    )
                    .await
                    .map_err(|_| {
                        ErrorData::invalid_params("semantic card is not available", None)
                    })?;
                let value = if let Some(card) = card {
                    json!({"card": card})
                } else {
                    let composed = facade
                        .get_composed_card(
                            &request_context.vault,
                            &request_context.core,
                            &access,
                            card_id,
                        )
                        .await
                        .map_err(|_| {
                            ErrorData::invalid_params("semantic card is not available", None)
                        })?
                        .ok_or_else(|| {
                            ErrorData::invalid_params("semantic card is not available", None)
                        })?;
                    json!({"composed_card": composed})
                };
                ResourceContents::text(value.to_string(), request.uri.clone())
                    .with_mime_type("application/json")
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

async fn semantic_rule_call(
    context: &RequestContext<RoleServer>,
    input: SemanticRuleCommand,
) -> Result<CallToolResult, ErrorData> {
    let request = request_context(context)?;
    if let Err(error) = require_writable(&request) {
        return Ok(error_result(context, error));
    }
    let facade = SemanticPublicFacade::new(request.state.clone());
    let actor = SemanticActor::trusted(
        request
            .principal
            .actor
            .actor_id()
            .map(|id| id.as_str().to_owned())
            .unwrap_or_else(|| format!("{:?}", request.principal.actor.actor_type())),
    );
    match facade
        .apply_rule(
            &request.vault,
            &SemanticAccess::new(request.principal.permissions.clone()),
            &actor,
            &input,
        )
        .await
    {
        Ok(data) => Ok(success_result(
            context,
            serde_json::to_value(data).unwrap_or_else(|_| json!({})),
        )),
        Err(error) => Ok(error_result(context, memory_error(error))),
    }
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

fn memory_error(error: MemoryError) -> ToolErrorBody {
    match error {
        MemoryError::AccessDenied => {
            ToolErrorBody::new("permission_denied", "the operation is not available", false)
        }
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
        MemoryError::State(_)
        | MemoryError::Core(_)
        | MemoryError::Index(_)
        | MemoryError::InitializationFailure { .. } => ToolErrorBody::new(
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
        "build_memory_pack"
        | "get_memory_card"
        | "list_memory_cards"
        | "get_memory_evidence"
        | "get_processing_status" => &[Permission::ReadMemory, Permission::ReadVault][..],
        "create_note" | "edit_note" | "move_note" => &[Permission::WriteVault][..],
        "delete_note" => &[Permission::DeleteVault][..],
        "note_history" => &[Permission::ReadHistory][..],
        "restore_note_revision" => &[Permission::ReadHistory, Permission::WriteVault][..],
        "remember" => &[Permission::WriteMemory][..],
        "update_raw_memory" | "forget_raw_memory" => &[Permission::ManageMemory][..],
        "correct_memory" | "forget_memory" => &[Permission::ManageMemory][..],
        "get_raw_memory" | "list_raw_memories" | "get_raw_memory_overview" => {
            &[Permission::ReadMemory][..]
        }
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
        "build_memory_pack" => 5,
        "get_memory_card" => 6,
        "list_memory_cards" => 7,
        "get_memory_evidence" => 8,
        "correct_memory" => 9,
        "forget_memory" => 10,
        "get_processing_status" => 11,
        "create_note" => 12,
        "edit_note" => 13,
        "move_note" => 14,
        "delete_note" => 15,
        "note_history" => 16,
        "restore_note_revision" => 17,
        "remember" => 18,
        "get_raw_memory" => 19,
        "list_raw_memories" => 20,
        "get_raw_memory_overview" => 21,
        "update_raw_memory" => 22,
        "forget_raw_memory" => 23,
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

/// Isolated MCP shaping seam for the v3 explicit-memory facade. The semantic
/// facade agent may replace this with protocol-neutral raw DTOs; until then,
/// keep the public raw namespace explicit and never expose it as a card,
/// evidence reference, or MemoryPack.
fn raw_tool_data(tool: &str, mut data: Value, _details: bool, expose_sources: bool) -> Value {
    if tool == "get_raw_memory_overview"
        && let Some(entries) = data.get_mut("entries").and_then(Value::as_array_mut)
    {
        for entry in entries {
            if let Some(uri) = entry.get("resource_uri").and_then(Value::as_str) {
                *entry.get_mut("resource_uri").expect("resource_uri exists") =
                    Value::String(uri.replace("vault://memory/", "vault://raw-memory/"));
            }
        }
    }
    if let Some(object) = data.as_object_mut() {
        object.insert(
            "representation".to_owned(),
            Value::String("raw_explicit_memory".to_owned()),
        );
        object.insert(
            "raw_ownership".to_owned(),
            Value::String("explicit".to_owned()),
        );
        if tool == "list_raw_memories"
            && let Some(items) = object.get_mut("memories").and_then(Value::as_array_mut)
        {
            for item in items {
                if let Some(item) = item.as_object_mut() {
                    item.insert(
                        "representation".to_owned(),
                        Value::String("raw_explicit_memory".to_owned()),
                    );
                    item.insert(
                        "raw_ownership".to_owned(),
                        Value::String("explicit".to_owned()),
                    );
                }
            }
        }
        if !expose_sources {
            object.remove("sources");
            object.remove("source_bindings");
            object.remove("directory_groups");
            object.remove("generated_sections");
            if let Some(entries) = object.get_mut("entries").and_then(Value::as_array_mut) {
                for entry in entries {
                    if let Some(entry) = entry.as_object_mut() {
                        entry.remove("sources");
                        entry.insert(
                            "label".to_owned(),
                            Value::String("raw explicit memory".to_owned()),
                        );
                    }
                }
            }
            if let Some(items) = object.get_mut("memories").and_then(Value::as_array_mut) {
                for item in items {
                    if let Some(item) = item.as_object_mut() {
                        item.remove("sources");
                        item.remove("source_bindings");
                    }
                }
            }
            if let Some(memory) = object
                .get_mut("explicit")
                .and_then(Value::as_object_mut)
                .and_then(|explicit| explicit.get_mut("memory"))
                .and_then(Value::as_object_mut)
            {
                memory.remove("sources");
                memory.remove("source_bindings");
            }
        }
    }
    data
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

fn parse_user_tool_path(core: &VaultCore, value: &str) -> Result<VaultPath, ToolErrorBody> {
    let path = parse_tool_path(value)?;
    if core.is_managed_path(&path) {
        return Err(ToolErrorBody::new(
            "invalid_path",
            "path must identify ordinary Vault content",
            false,
        ));
    }
    Ok(path)
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
    let children = if depth == 0 {
        Vec::new()
    } else {
        request
            .index
            .list_nodes(&request.vault, parent, limit, offset)
            .await
            .map_err(index_error)?
    };
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
                source_path: None,
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
        "results": result.hits.iter().map(|hit| {
            let mut value = note_retrieval_json(hit);
            value["result_granularity"] = json!(if result_granularity == "section" && hit.matched_section.is_some() { "section" } else { "note" });
            value
        }).collect::<Vec<_>>(),
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
        object.insert("matched_section".to_owned(), json!(hit.matched_section));
        object.insert(
            "result_granularity".to_owned(),
            json!(if hit.matched_section.is_some() {
                "section"
            } else {
                "note"
            }),
        );
        if let Some(breakdown) = hit.score_breakdown.as_ref() {
            object.insert("score_breakdown".to_owned(), json!(breakdown));
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
    use std::{collections::BTreeSet, io::ErrorKind, path::PathBuf};

    use super::{
        McpHandler, McpService, UpdateMemoryInput, bearer_token, mounted_slug,
        oauth_metadata_router, raw_tool_data, router, stateful_router, vault_error,
    };
    use axum::{Router, body::Body, http::Request};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use http_body_util::BodyExt;
    use mcp_vault_auth::{
        AuthService, MasterKeyRing, OAuthIssuerInput, OAuthResourceServer, OriginPolicy,
        SecretString,
    };
    use mcp_vault_core::{VaultCore, VaultCoreRuntime, VaultError};
    use mcp_vault_domain::{
        Actor, MaintenanceGate, MaintenanceMode, MemoryId, Revision, Scope, ScopeSet, SourcePlane,
        VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug, WritePrecondition,
    };
    use mcp_vault_indexer::IndexService;
    use mcp_vault_memory::{
        MemoryOrigin, MemoryService, MemoryType, RememberInput, SemanticMemoryService,
    };
    use mcp_vault_state::{StateStore, VaultStatus};
    use mcp_vault_storage_fs::{StorageError, StorageOptions};
    use rand::rngs::OsRng;
    use rsa::{
        RsaPrivateKey, RsaPublicKey,
        pkcs1v15::SigningKey,
        signature::{SignatureEncoding, Signer},
        traits::PublicKeyParts,
    };
    use serde_json::{Value, json};
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
    fn update_raw_memory_json_distinguishes_omitted_set_and_clear_fields() {
        let omitted: UpdateMemoryInput = serde_json::from_value(json!({
            "id": MemoryId::new().to_string(),
            "expected_revision": 1
        }))
        .unwrap();
        assert_eq!(omitted.memory_type, None);
        assert_eq!(omitted.confidence, None);

        let cleared: UpdateMemoryInput = serde_json::from_value(json!({
            "id": MemoryId::new().to_string(),
            "expected_revision": 1,
            "memory_type": null,
            "confidence": null
        }))
        .unwrap();
        assert_eq!(cleared.memory_type, Some(None));
        assert_eq!(cleared.confidence, Some(None));

        let set: UpdateMemoryInput = serde_json::from_value(json!({
            "id": MemoryId::new().to_string(),
            "expected_revision": 1,
            "memory_type": "preference",
            "confidence": 0.8
        }))
        .unwrap();
        assert_eq!(
            set.memory_type.as_ref().and_then(|value| value.as_deref()),
            Some("preference")
        );
        assert_eq!(set.confidence, Some(Some(0.8)));
    }

    #[tokio::test]
    async fn default_recall_sources_read_directly_and_details_are_opt_in() {
        let (router, token, _root) = configured_memory_router().await;
        let response = router
            .oneshot(tool_request(
                &token,
                103,
                "recall",
                json!({"query":"retired"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn historical_read_reports_selected_content_metadata() {
        let (router, token, _root) = configured_router().await;
        let created = call_tool_json(
            &router,
            &token,
            201,
            "create_note",
            json!({"path":"past.md","content":"old"}),
        )
        .await;
        assert_tool_ok(&created, "create_note");
        let old =
            call_tool_json(&router, &token, 202, "read_note", json!({"path":"past.md"})).await;
        let edited=call_tool_json(&router,&token,203,"edit_note",json!({"path":"past.md","expected_revision":1,"operation":{"kind":"replace_all","content":"a much longer replacement"}})).await;
        assert_tool_ok(&edited, "edit_note");
        let historical = call_tool_json(
            &router,
            &token,
            204,
            "read_note",
            json!({"path":"past.md","revision":1}),
        )
        .await;
        assert_tool_ok(&historical, "read_note");
        let actual = &historical["result"]["structuredContent"]["data"];
        assert_eq!(actual["size"], 3);
        assert_eq!(
            actual["content_hash"],
            old["result"]["structuredContent"]["data"]["content_hash"]
        );
        assert_eq!(actual["content"], "old");
        let first = call_tool_json(
            &router,
            &token,
            205,
            "note_history",
            json!({"path":"past.md","limit":1}),
        )
        .await;
        let page = &first["result"]["structuredContent"]["data"];
        assert_eq!(page["revisions"][0]["revision"], 2);
        assert_eq!(page["truncated"], true);
        let second = call_tool_json(
            &router,
            &token,
            206,
            "note_history",
            json!({"path":"past.md","limit":1,"cursor":page["next_cursor"]}),
        )
        .await;
        assert_eq!(
            second["result"]["structuredContent"]["data"]["revisions"][0]["revision"],
            1
        );
        assert_eq!(
            second["result"]["structuredContent"]["data"]["truncated"],
            false
        );
    }

    #[test]
    fn tool_metadata_is_model_facing_selection_and_result_guidance() {
        let tools = McpHandler::default().tool_router.list_all();
        assert_eq!(tools.len(), 24);

        let mut titles = BTreeSet::new();
        for tool in &tools {
            let title = tool
                .title
                .as_deref()
                .unwrap_or_else(|| panic!("{} is missing a title", tool.name));
            assert!(!title.trim().is_empty(), "{} has an empty title", tool.name);
            assert!(titles.insert(title), "tool title must be unique: {title}");

            let description = tool
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("{} is missing a description", tool.name));
            assert!(
                description.starts_with("Use this when")
                    || description.starts_with("Use this proactively")
                    || description.starts_with("Use this only when"),
                "{} does not state its selection condition: {description}",
                tool.name
            );
            assert!(
                description.contains("On success, `data"),
                "{} does not explain its result fields: {description}",
                tool.name
            );
            assert!(
                description.len() <= 1_000,
                "{} has an overly long description",
                tool.name
            );
            for implementation_term in [
                "Vault Core",
                "projection-based",
                "query-time",
                "durable sourced",
                "ordinary-note cues",
                "bounded deterministic",
            ] {
                assert!(
                    !description.contains(implementation_term),
                    "{} exposes implementation terminology {implementation_term}: {description}",
                    tool.name
                );
            }

            let annotations = tool
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("{} is missing annotations", tool.name));
            assert_eq!(
                annotations.open_world_hint,
                Some(false),
                "{} must declare the Vault as a closed world",
                tool.name
            );

            let properties = tool
                .input_schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .unwrap_or_else(|| panic!("{} has no object input properties", tool.name));
            assert!(
                !properties.is_empty(),
                "{} has no input properties",
                tool.name
            );
            if !tool.name.starts_with("semantic_")
                && !matches!(
                    tool.name.as_ref(),
                    "remember"
                        | "build_memory_pack"
                        | "get_memory_card"
                        | "list_memory_cards"
                        | "get_memory_evidence"
                        | "correct_memory"
                        | "forget_memory"
                        | "get_processing_status"
                        | "get_raw_memory"
                        | "list_raw_memories"
                        | "get_raw_memory_overview"
                        | "update_raw_memory"
                        | "forget_raw_memory"
                )
            {
                assert!(
                    properties.contains_key("include_details"),
                    "{} must support detailed output",
                    tool.name
                );
            }
            for (property, schema) in properties {
                assert!(
                    schema
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|description| !description.trim().is_empty()),
                    "{}.{} is missing a parameter description",
                    tool.name,
                    property
                );
            }
        }

        let description = |name: &str| {
            tools
                .iter()
                .find(|tool| tool.name.as_ref() == name)
                .and_then(|tool| tool.description.as_deref())
                .unwrap()
        };
        assert!(description("search_notes").contains("`data.results`"));
        assert!(description("search_notes").contains("degradation_reasons"));
        assert!(description("build_memory_pack").contains("semantic MemoryPack"));
        assert!(description("build_memory_pack").contains("`data`"));
        assert!(description("get_memory_card").contains("`data.card`"));
        assert!(description("get_memory_evidence").contains("`data.evidence`"));
        assert!(description("update_raw_memory").contains("raw explicit memory"));
        assert!(description("update_raw_memory").contains("expected_revision"));
        assert!(description("forget_raw_memory").contains("`idempotency_key` is required"));
        assert!(description("forget_raw_memory").contains("same key"));
        assert!(description("forget_raw_memory").contains("different input conflicts"));
        assert!(!description("build_memory_pack").contains("implementation"));
        assert!(description("remember").contains("`data.explicit.memory`"));
        assert!(description("remember").contains("idempotency_key"));
        assert!(description("remember").contains("read access"));
        assert!(description("edit_note").contains("First call read_note"));
        assert!(description("edit_note").contains("replace_heading_section"));
    }

    #[test]
    fn raw_overview_shaping_redacts_source_navigation_for_memory_only_callers() {
        let shaped = raw_tool_data(
            "get_raw_memory_overview",
            json!({
                "entries": [{
                    "resource_uri": "vault://memory/raw-1",
                    "label": "notes/private.md#Heading",
                    "sources": [{"path": "notes/private.md"}]
                }],
                "generated_sections": [{"label": "notes/private.md", "description": "private"}],
                "directory_groups": [{"path": "notes", "returned_units": 1}]
            }),
            false,
            false,
        );
        assert_eq!(
            shaped["entries"][0]["resource_uri"],
            "vault://raw-memory/raw-1"
        );
        assert_eq!(shaped["entries"][0]["label"], "raw explicit memory");
        assert!(shaped["entries"][0].get("sources").is_none());
        assert!(shaped.get("generated_sections").is_none());
        assert!(shaped.get("directory_groups").is_none());
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

    async fn configured_memory_router_with_memory_only_pair() -> (
        axum::Router,
        String,
        axum::Router,
        String,
        tempfile::TempDir,
    ) {
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
            MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
        );
        let full_scopes: ScopeSet = Scope::ALL.into_iter().collect();
        let full_pat = auth
            .issue_pat(&context, "full-agent", full_scopes, None)
            .await
            .unwrap();
        let memory_only_scopes: ScopeSet = [Scope::MemoryRead].into_iter().collect();
        let memory_only_pat = auth
            .issue_pat(&context, "memory-only-agent", memory_only_scopes, None)
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
            mounted_service_router(service.clone()),
            full_pat.token.expose_secret().to_owned(),
            mounted_service_router(service),
            memory_only_pat.token.expose_secret().to_owned(),
            root,
        )
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
            state.clone(),
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

    async fn configured_semantic_router() -> (
        axum::Router,
        String,
        tempfile::TempDir,
        mcp_vault_state::SemanticCardRecord,
        String,
        String,
        String,
        String,
        String,
    ) {
        let full_scopes: ScopeSet = Scope::ALL.into_iter().collect();
        let (
            router,
            token,
            root,
            card,
            source_id,
            source_revision_id,
            evidence,
            source_file_id,
            source_file_revision,
            _maintenance,
        ) = configured_semantic_router_with_scopes(full_scopes).await;
        (
            router,
            token,
            root,
            card,
            source_id,
            source_revision_id,
            evidence,
            source_file_id,
            source_file_revision,
        )
    }

    async fn configured_semantic_router_with_scopes(
        scopes: ScopeSet,
    ) -> (
        axum::Router,
        String,
        tempfile::TempDir,
        mcp_vault_state::SemanticCardRecord,
        String,
        String,
        String,
        String,
        String,
        MaintenanceGate,
    ) {
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
            .settings()
            .set_vault(
                &context,
                "memory.units.policy",
                &json!({"enabled":true,"request_timeout_seconds":300}),
                WritePrecondition::Unconditional,
                None,
            )
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            root.path().join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        let path = VaultPath::parse("notes/semantic.md").unwrap();
        core.create_bytes(
            &context,
            &path,
            b"# Semantic\nThe semantic MCP path is sourced.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let memory = SemanticMemoryService::new(state.clone());
        let input = memory.prepare_source(&context, &core, &path).await.unwrap();
        let body = input.blocks.last().unwrap();
        let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"The semantic MCP path is sourced.","scope":"project","assertion_status":"source_asserted","admission_reason":"http fixture","value_for_future_work":"retain","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Semantic HTTP","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
        memory
            .submit_proposal_json(&context, &core, &path, &proposal.to_string())
            .await
            .unwrap();
        let card = state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let file = core.read(&context, &path).await.unwrap().file;
        let source = state
            .semantic_memory()
            .get_source_by_file(&context, file.id)
            .await
            .unwrap()
            .unwrap();
        let evidence = card.items[0].evidence_ref_ids[0].to_string();
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[13_u8; 32]).unwrap(),
        );
        let pat = auth
            .issue_pat(&context, "semantic-http", scopes, None)
            .await
            .unwrap();
        let token = pat.token.expose_secret().to_owned();
        let maintenance = MaintenanceGate::new();
        let core_runtime = VaultCoreRuntime::new(maintenance.clone());
        let service = McpService::new(
            state,
            auth,
            root.path().join("history"),
            StorageOptions::default(),
            core_runtime,
            vec!["localhost".to_owned()],
            OriginPolicy::new(std::iter::empty::<&str>()).unwrap(),
        );
        (
            mounted_service_router(service),
            token,
            root,
            card,
            source.source_id.to_string(),
            source.current_revision_id.unwrap().to_string(),
            evidence,
            file.id.to_string(),
            file.current_revision.value().to_string(),
            maintenance,
        )
    }

    struct SemanticHttpFixture {
        router: Router,
        token: String,
        _root: tempfile::TempDir,
        slug: String,
        card: mcp_vault_state::SemanticCardRecord,
        source_id: String,
        source_revision_id: String,
        evidence_id: String,
    }

    async fn seed_semantic_vault(
        state: &StateStore,
        root: &tempfile::TempDir,
        slug: &str,
        name: &str,
    ) -> (
        VaultContext,
        mcp_vault_state::SemanticCardRecord,
        String,
        String,
        String,
    ) {
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new(slug).unwrap(),
            PathBuf::from(root.path()),
            Revision::new(1),
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, name, VaultStatus::Active)
            .await
            .unwrap();
        state
            .settings()
            .set_vault(
                &context,
                "memory.units.policy",
                &json!({"enabled":true,"request_timeout_seconds":300}),
                WritePrecondition::Unconditional,
                None,
            )
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            root.path().join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        let path = VaultPath::parse("notes/semantic.md").unwrap();
        core.create_bytes(
            &context,
            &path,
            format!("# Semantic {name}\nThe semantic MCP path is sourced.\n").as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let memory = SemanticMemoryService::new(state.clone());
        let input = memory.prepare_source(&context, &core, &path).await.unwrap();
        let body = input.blocks.last().unwrap();
        let proposal = json!({
            "outcome":"success_nonempty",
            "observations":[{"kind":"decision","statement":"The semantic MCP path is sourced.","scope":"project","assertion_status":"source_asserted","admission_reason":"dual-vault fixture","value_for_future_work":"retain","body_block_ids":[body.local_id.clone()]}],
            "cards":[{"title":format!("Semantic HTTP {name}"),"kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
        });
        memory
            .submit_proposal_json(&context, &core, &path, &proposal.to_string())
            .await
            .unwrap();
        let card = state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let file = core.read(&context, &path).await.unwrap().file;
        let source = state
            .semantic_memory()
            .get_source_by_file(&context, file.id)
            .await
            .unwrap()
            .unwrap();
        (
            context,
            card.clone(),
            source.source_id.to_string(),
            source.current_revision_id.unwrap().to_string(),
            card.items[0].evidence_ref_ids[0].to_string(),
        )
    }

    async fn configured_semantic_dual_vault_routers() -> (SemanticHttpFixture, SemanticHttpFixture)
    {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let root_a = tempdir().unwrap();
        let root_b = tempdir().unwrap();
        let (context_a, card_a, source_a, revision_a, evidence_a) =
            seed_semantic_vault(&state, &root_a, "alpha", "Alpha").await;
        let (context_b, card_b, source_b, revision_b, evidence_b) =
            seed_semantic_vault(&state, &root_b, "bravo", "Bravo").await;
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[17_u8; 32]).unwrap(),
        );
        let scopes: ScopeSet = Scope::ALL.into_iter().collect();
        let pat_a = auth
            .issue_pat(&context_a, "alpha-agent", scopes.clone(), None)
            .await
            .unwrap();
        let pat_b = auth
            .issue_pat(&context_b, "bravo-agent", scopes, None)
            .await
            .unwrap();
        let service_a = McpService::new(
            state.clone(),
            auth.clone(),
            root_a.path().join("history"),
            StorageOptions::default(),
            Default::default(),
            vec!["localhost".to_owned()],
            OriginPolicy::new(std::iter::empty::<&str>()).unwrap(),
        );
        let service_b = McpService::new(
            state,
            auth,
            root_b.path().join("history"),
            StorageOptions::default(),
            Default::default(),
            vec!["localhost".to_owned()],
            OriginPolicy::new(std::iter::empty::<&str>()).unwrap(),
        );
        (
            SemanticHttpFixture {
                router: mounted_service_router(service_a),
                token: pat_a.token.expose_secret().to_owned(),
                _root: root_a,
                slug: "alpha".to_owned(),
                card: card_a,
                source_id: source_a,
                source_revision_id: revision_a,
                evidence_id: evidence_a,
            },
            SemanticHttpFixture {
                router: mounted_service_router(service_b),
                token: pat_b.token.expose_secret().to_owned(),
                _root: root_b,
                slug: "bravo".to_owned(),
                card: card_b,
                source_id: source_b,
                source_revision_id: revision_b,
                evidence_id: evidence_b,
            },
        )
    }

    async fn configured_indexed_router() -> (
        axum::Router,
        String,
        tempfile::TempDir,
        StateStore,
        VaultCore,
        VaultContext,
    ) {
        configured_indexed_router_fixture_with_scopes(full_scopes()).await
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
        let (router, token, root, _, _, _) =
            configured_indexed_router_fixture_with_scopes(scopes).await;
        (router, token, root)
    }

    async fn configured_indexed_router_fixture_with_scopes(
        scopes: ScopeSet,
    ) -> (
        axum::Router,
        String,
        tempfile::TempDir,
        StateStore,
        VaultCore,
        VaultContext,
    ) {
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
            state.clone(),
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
            state,
            core,
            context,
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
        let core = VaultCore::new(
            state.clone(),
            root.path().join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        let auth = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[19_u8; 32]).unwrap(),
        );
        let memory_id = MemoryService::new(state.clone(), auth.clone())
            .remember(
                &context,
                &core,
                RememberInput {
                    content: "OAuth memory operations are available.".to_owned(),
                    memory_type: Some(MemoryType::Decision),
                    importance: Some(0.9),
                    confidence: Some(1.0),
                    tags: vec!["oauth".to_owned()],
                    idempotency_key: Some("oauth-current-fixture".to_owned()),
                    origin: MemoryOrigin::ExplicitAdmin,
                    extraction: json!({"fixture": "oauth_all_tools"}),
                    ..RememberInput::default()
                },
            )
            .await
            .unwrap()
            .memory
            .unwrap()
            .id;
        core.reconcile(&context, Actor::system()).await.unwrap();
        IndexService::new(state.clone())
            .rebuild_vault(&core, &context)
            .await
            .unwrap();
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
    async fn semantic_v1_tools_round_trip_through_real_http_harness() {
        let (
            router,
            token,
            _root,
            card,
            source_id,
            source_revision_id,
            evidence_id,
            source_file_id,
            source_file_revision,
        ) = configured_semantic_router().await;
        let tools = router
            .clone()
            .oneshot(list_tools_request(&token))
            .await
            .unwrap();
        let tools: serde_json::Value =
            serde_json::from_slice(&tools.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let names = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        for name in [
            "remember",
            "build_memory_pack",
            "get_memory_card",
            "list_memory_cards",
            "get_memory_evidence",
            "correct_memory",
            "forget_memory",
            "get_processing_status",
        ] {
            assert!(names.contains(&name), "missing {name}: {names:?}");
        }
        let explicit_tool = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "remember")
            .unwrap();
        assert_eq!(
            explicit_tool["inputSchema"]["required"],
            json!(["content", "idempotency_key"])
        );
        assert_eq!(explicit_tool["inputSchema"]["additionalProperties"], false);
        let source_schema = &explicit_tool["inputSchema"]["$defs"]["SemanticExplicitSourceRequest"];
        assert_eq!(
            source_schema["required"],
            json!(["path", "file_id", "revision"])
        );
        assert_eq!(source_schema["additionalProperties"], false);
        let explicit = call_tool_json(
            &router,
            &token,
            899,
            "remember",
            json!({"content":"An explicit v1 assertion.","idempotency_key":"semantic-explicit-v1"}),
        )
        .await;
        assert_tool_ok(&explicit, "remember");
        assert_eq!(
            explicit["result"]["structuredContent"]["data"]["explicit"]["memory"]["ownership"],
            "explicit"
        );
        assert_eq!(
            explicit["result"]["structuredContent"]["data"]["explicit"]["memory"]["embedding_binding_present"],
            false
        );
        let sourced_explicit = call_tool_json(
            &router,
            &token,
            8991,
            "remember",
            json!({
                "content":"An explicit v1 sourced assertion.",
                "idempotency_key":"semantic-explicit-sourced-v1",
                "sources":[{"path":"notes/semantic.md","file_id":source_file_id,"revision":source_file_revision.parse::<u64>().unwrap()}]
            }),
        )
        .await;
        assert_tool_ok(&sourced_explicit, "remember");
        let listed = call_tool_json(
            &router,
            &token,
            900,
            "list_memory_cards",
            json!({"limit":20}),
        )
        .await;
        assert_tool_ok(&listed, "list_memory_cards");
        assert_eq!(
            listed["result"]["structuredContent"]["data"]["cards"][0]["card_id"],
            card.id.to_string()
        );
        let got = call_tool_json(
            &router,
            &token,
            901,
            "get_memory_card",
            json!({"card_id":card.id.to_string()}),
        )
        .await;
        assert_tool_ok(&got, "get_memory_card");
        let evidence = call_tool_json(
            &router,
            &token,
            902,
            "get_memory_evidence",
            json!({"evidence_ref_id":evidence_id,"source_id":source_id,"source_revision_id":source_revision_id}),
        )
        .await;
        assert_tool_ok(&evidence, "get_memory_evidence");
        assert!(
            evidence["result"]["structuredContent"]["data"]["evidence"]["body_spans"][0]["end_byte"]
                .as_u64()
                .unwrap() > 0
        );
        let pack = call_tool_json(
            &router,
            &token,
            903,
            "build_memory_pack",
            json!({"task":"semantic MCP"}),
        )
        .await;
        assert_tool_ok(&pack, "build_memory_pack");
        let status = call_tool_json(
            &router,
            &token,
            904,
            "get_processing_status",
            json!({"limit":20}),
        )
        .await;
        assert_tool_ok(&status, "get_processing_status");
        assert!(
            status["result"]["structuredContent"]["data"]
                .get("organization_jobs")
                .is_some()
        );
        let unknown = call_tool_json(
            &router,
            &token,
            905,
            "build_memory_pack",
            json!({"task":"x","unknown":true}),
        )
        .await;
        assert!(unknown["result"]["isError"].as_bool().unwrap_or(false));
        let unknown_explicit = call_tool_json(
            &router,
            &token,
            907,
            "remember",
            json!({"content":"x","actor":"caller-controlled","idempotency_key":"unknown-explicit"}),
        )
        .await;
        assert!(
            unknown_explicit["result"]["isError"]
                .as_bool()
                .unwrap_or(false)
        );
        let missing_explicit_key = call_tool_json(
            &router,
            &token,
            908,
            "remember",
            json!({"content":"missing key"}),
        )
        .await;
        assert!(
            missing_explicit_key["result"]["isError"]
                .as_bool()
                .unwrap_or(false)
        );
        let empty_explicit_key = call_tool_json(
            &router,
            &token,
            909,
            "remember",
            json!({"content":"empty key","idempotency_key":"  "}),
        )
        .await;
        assert!(
            empty_explicit_key["result"]["isError"]
                .as_bool()
                .unwrap_or(false)
        );
        let cross = call_tool_json(
            &router,
            &token,
            906,
            "get_memory_card",
            json!({"card_id":mcp_vault_domain::MemoryCardId::new().to_string()}),
        )
        .await;
        assert!(cross["result"]["isError"].as_bool().unwrap_or(false));
    }

    #[tokio::test]
    async fn semantic_v1_http_scope_filtered_tools_match_call_permissions() {
        let read_tools = [
            "build_memory_pack",
            "get_memory_card",
            "list_memory_cards",
            "get_memory_evidence",
            "get_processing_status",
        ];
        let mutation_tools = ["correct_memory", "forget_raw_memory"];
        let cases = [
            (
                ScopeSet::from_iter(Scope::ALL),
                true,
                true,
                read_tools.as_slice(),
                "full",
            ),
            (
                ScopeSet::from_iter([Scope::MemoryRead]),
                false,
                false,
                &[][..],
                "memory-only",
            ),
            (
                ScopeSet::from_iter([Scope::MemoryRead, Scope::VaultRead]),
                true,
                false,
                read_tools.as_slice(),
                "read-only",
            ),
            (
                ScopeSet::from_iter([Scope::MemoryWrite]),
                false,
                false,
                &[][..],
                "write-only",
            ),
            (
                ScopeSet::from_iter([Scope::MemoryManage]),
                false,
                true,
                &[][..],
                "manage-only",
            ),
        ];
        for (scopes, can_read, can_manage, expected_reads, label) in cases {
            let can_write = scopes.contains(Scope::MemoryWrite);
            let (
                router,
                token,
                _root,
                card,
                source_id,
                _source_revision,
                _evidence,
                _source_file_id,
                _source_file_revision,
                _maintenance,
            ) = configured_semantic_router_with_scopes(scopes).await;
            let listed = router
                .clone()
                .oneshot(list_tools_request(&token))
                .await
                .unwrap();
            let listed: serde_json::Value =
                serde_json::from_slice(&listed.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            let names = listed["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            for name in expected_reads {
                assert!(names.contains(name), "{label} missing {name}: {names:?}");
            }
            for name in read_tools {
                assert_eq!(names.contains(&name), can_read, "{label}: {name}");
            }
            for name in mutation_tools {
                assert_eq!(names.contains(&name), can_manage, "{label}: {name}");
            }
            assert_eq!(names.contains(&"remember"), can_write, "{label}: remember");

            let card_call = call_tool_json(
                &router,
                &token,
                920,
                "get_memory_card",
                json!({"card_id":card.id.to_string()}),
            )
            .await;
            assert_eq!(
                card_call["result"]["structuredContent"]["error"]["code"],
                if can_read {
                    Value::Null
                } else {
                    json!("permission_denied")
                },
                "{label}: {card_call}"
            );
            if can_read {
                assert_tool_ok(&card_call, "get_memory_card");
            }

            let correction = call_tool_json(
                &router,
                &token,
                921,
                "correct_memory",
                json!({
                    "target_ref":format!("card:{}", card.id),
                    "mutation":"correction",
                    "payload":{"replace":"The semantic MCP path is sourced after review.","remove":"The semantic MCP path is sourced."},
                    "expected_parent_revision":card.revision_number,
                    "expected_rules_revision":0,
                    "idempotency_key":format!("scope-{label}")
                }),
            )
            .await;
            assert_eq!(
                correction["result"]["structuredContent"]["error"]["code"],
                if can_manage {
                    Value::Null
                } else {
                    json!("permission_denied")
                },
                "{label}: {correction}"
            );
            if can_manage {
                assert_tool_ok(&correction, "correct_memory");
            }
            let explicit = call_tool_json(
                &router,
                &token,
                922,
                "remember",
                json!({
                    "content":"scope filtered explicit memory",
                    "idempotency_key":format!("scope-explicit-{label}")
                }),
            )
            .await;
            assert_eq!(
                explicit["result"]["structuredContent"]["error"]["code"],
                if can_write {
                    Value::Null
                } else {
                    json!("permission_denied")
                },
                "{label}: {explicit}"
            );
            if can_write {
                assert_tool_ok(&explicit, "remember");
                if !can_read {
                    let source_denied = call_tool_json(
                        &router,
                        &token,
                        923,
                        "remember",
                        json!({
                            "content":"must not probe source",
                            "idempotency_key":format!("scope-source-{label}"),
                            "sources":[{"path":"notes/semantic.md","file_id":source_id,"revision":1}]
                        }),
                    )
                    .await;
                    assert_eq!(
                        source_denied["result"]["structuredContent"]["error"]["code"],
                        "permission_denied",
                        "{label}: {source_denied}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn semantic_v1_http_mutations_are_revision_safe_idempotent_and_source_preserving() {
        let (
            router,
            token,
            _root,
            card,
            _source_id,
            _revision,
            _evidence,
            _source_file_id,
            _source_file_revision,
            _maintenance,
        ) = configured_semantic_router_with_scopes(ScopeSet::from_iter(Scope::ALL)).await;
        let correction_args = json!({
            "target_ref":format!("card:{}", card.id),
            "mutation":"correction",
            "payload":{"replace":"The semantic MCP path is sourced after review.","remove":"The semantic MCP path is sourced."},
            "expected_parent_revision":card.revision_number,
            "expected_rules_revision":0,
            "idempotency_key":"http-correction-once"
        });
        let first = call_tool_json(
            &router,
            &token,
            930,
            "correct_memory",
            correction_args.clone(),
        )
        .await;
        assert_tool_ok(&first, "correct_memory");
        let replay = call_tool_json(&router, &token, 931, "correct_memory", correction_args).await;
        assert_tool_ok(&replay, "correct_memory");
        assert_eq!(
            first["result"]["structuredContent"]["data"]["id"],
            replay["result"]["structuredContent"]["data"]["id"]
        );

        let stale_parent = call_tool_json(
            &router,
            &token,
            932,
            "correct_memory",
            json!({
                "target_ref":format!("card:{}", card.id),
                "mutation":"correction",
                "payload":{"replace":"The semantic MCP path is sourced after another review.","remove":"The semantic MCP path is sourced."},
                "expected_parent_revision":card.revision_number + 1,
                "expected_rules_revision":1,
                "idempotency_key":"http-correction-stale-parent"
            }),
        )
        .await;
        assert_eq!(
            stale_parent["result"]["structuredContent"]["error"]["code"],
            "memory_conflict"
        );
        let stale_rules = call_tool_json(
            &router,
            &token,
            933,
            "correct_memory",
            json!({
                "target_ref":format!("card:{}", card.id),
                "mutation":"correction",
                "payload":{"replace":"The semantic MCP path is sourced after a stale rules read.","remove":"The semantic MCP path is sourced."},
                "expected_parent_revision":card.revision_number,
                "expected_rules_revision":0,
                "idempotency_key":"http-correction-stale-rules"
            }),
        )
        .await;
        assert_eq!(
            stale_rules["result"]["structuredContent"]["error"]["code"],
            "memory_conflict"
        );

        let forgotten = call_tool_json(
            &router,
            &token,
            934,
            "forget_memory",
            json!({
                "target_ref":format!("card:{}", card.id),
                "mutation":"forget_current",
                "payload":{"reason":"http forget"},
                "expected_parent_revision":card.revision_number,
                "expected_rules_revision":1,
                "idempotency_key":"http-forget-once"
            }),
        )
        .await;
        assert_tool_ok(&forgotten, "forget_memory");
        let source = call_tool_json(
            &router,
            &token,
            935,
            "read_note",
            json!({"path":"notes/semantic.md"}),
        )
        .await;
        assert_tool_ok(&source, "read_note");
        assert!(
            source["result"]["structuredContent"]["data"]["content"]
                .as_str()
                .unwrap()
                .contains("semantic MCP path is sourced")
        );
        let hidden = call_tool_json(
            &router,
            &token,
            936,
            "get_memory_card",
            json!({"card_id":card.id.to_string()}),
        )
        .await;
        assert_eq!(
            hidden["result"]["structuredContent"]["error"]["code"],
            "not_found"
        );
    }

    #[tokio::test]
    async fn semantic_v1_http_read_only_maintenance_rejects_mutations_without_state_change() {
        let (
            router,
            token,
            _root,
            card,
            _source_id,
            _revision,
            _evidence,
            _source_file_id,
            _source_file_revision,
            maintenance,
        ) = configured_semantic_router_with_scopes(ScopeSet::from_iter(Scope::ALL)).await;
        maintenance.set(MaintenanceMode::ReadOnly);
        for (id, tool, mutation, key) in [
            (940, "correct_memory", "correction", "maintenance-correct"),
            (941, "forget_memory", "forget_current", "maintenance-forget"),
        ] {
            let body = call_tool_json(
                &router,
                &token,
                id,
                tool,
                json!({
                    "target_ref":format!("card:{}", card.id),
                    "mutation":mutation,
                    "payload":{"reason":"must not apply"},
                    "expected_parent_revision":card.revision_number,
                    "expected_rules_revision":0,
                    "idempotency_key":key
                }),
            )
            .await;
            assert_eq!(
                body["result"]["structuredContent"]["error"]["code"],
                "maintenance"
            );
        }
        let explicit = call_tool_json(
            &router,
            &token,
            944,
            "remember",
            json!({"content":"must not save","idempotency_key":"maintenance-explicit"}),
        )
        .await;
        assert_eq!(
            explicit["result"]["structuredContent"]["error"]["code"],
            "maintenance"
        );
        let read = call_tool_json(
            &router,
            &token,
            942,
            "read_note",
            json!({"path":"notes/semantic.md"}),
        )
        .await;
        assert_tool_ok(&read, "read_note");
        let listed = call_tool_json(
            &router,
            &token,
            943,
            "list_memory_cards",
            json!({"limit":20}),
        )
        .await;
        assert_tool_ok(&listed, "list_memory_cards");
        assert_eq!(
            listed["result"]["structuredContent"]["data"]["cards"][0]["card_id"],
            card.id.to_string()
        );
    }

    #[tokio::test]
    async fn semantic_v1_http_schema_limits_and_no_answer_are_fail_closed() {
        let (
            router,
            token,
            _root,
            _card,
            _source_id,
            _revision,
            _evidence,
            _source_file_id,
            _source_file_revision,
            _maintenance,
        ) = configured_semantic_router_with_scopes(ScopeSet::from_iter(Scope::ALL)).await;
        let tools = router
            .clone()
            .oneshot(list_tools_request(&token))
            .await
            .unwrap();
        let tools: serde_json::Value =
            serde_json::from_slice(&tools.into_body().collect().await.unwrap().to_bytes()).unwrap();
        for name in [
            "list_memory_cards",
            "get_processing_status",
            "get_memory_evidence",
        ] {
            let tool = tools["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap();
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        }
        for name in ["list_memory_cards", "get_processing_status"] {
            let limit = &tools["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap()["inputSchema"]["properties"]["limit"];
            assert_eq!(limit["minimum"], 1, "{name} minimum");
            assert_eq!(limit["maximum"], 200, "{name} maximum");
        }
        let card_tool = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "get_memory_card")
            .unwrap();
        assert_eq!(
            card_tool["inputSchema"]["$defs"]["SemanticCardKind"]["enum"],
            json!(["card", "composed_card"])
        );
        let overview_tool = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "get_raw_memory_overview")
            .unwrap();
        assert_eq!(
            overview_tool["inputSchema"]["properties"]["limit"]["minimum"],
            1
        );
        assert_eq!(
            overview_tool["inputSchema"]["properties"]["limit"]["maximum"],
            100
        );
        assert_eq!(
            overview_tool["inputSchema"]["properties"]["max_tokens"]["minimum"],
            256
        );
        assert_eq!(
            overview_tool["inputSchema"]["properties"]["max_tokens"]["maximum"],
            32000
        );

        for (id, name, arguments) in [
            (950, "get_processing_status", json!({"limit":0})),
            (951, "get_processing_status", json!({"limit":201})),
            (952, "list_memory_cards", json!({"limit":201})),
            (956, "get_raw_memory_overview", json!({"limit":101})),
            (957, "get_raw_memory_overview", json!({"max_tokens":255})),
        ] {
            let body = call_tool_json(&router, &token, id, name, arguments).await;
            assert_eq!(
                body["result"]["structuredContent"]["error"]["code"], "invalid_argument",
                "{name}: {body}"
            );
        }
        let unknown = call_tool_json(
            &router,
            &token,
            953,
            "list_memory_cards",
            json!({"include_details":true}),
        )
        .await;
        assert!(unknown["result"]["isError"].as_bool().unwrap_or(false));
        assert!(
            unknown["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown field")
        );
        let invalid_kind = call_tool_json(
            &router,
            &token,
            955,
            "get_memory_card",
            json!({"card_id":_card.id.to_string(),"card_kind":"bogus"}),
        )
        .await;
        assert_eq!(
            invalid_kind["result"]["structuredContent"]["error"]["code"],
            "invalid_argument"
        );
        let no_answer = call_tool_json(
            &router,
            &token,
            954,
            "build_memory_pack",
            json!({"task":"violet submarine"}),
        )
        .await;
        assert_tool_ok(&no_answer, "build_memory_pack");
        assert!(
            !no_answer["result"]["structuredContent"]["data"]["evidence_gaps"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn semantic_v1_http_foreign_vault_card_and_evidence_ids_are_not_visible() {
        let (alpha, bravo) = configured_semantic_dual_vault_routers().await;
        assert_ne!(alpha.card.id, bravo.card.id);
        let foreign_card_call = call_tool_json_at(
            &alpha.router,
            &alpha.slug,
            &alpha.token,
            960,
            "get_memory_card",
            json!({"card_id":bravo.card.id.to_string()}),
        )
        .await;
        assert_eq!(
            foreign_card_call["result"]["structuredContent"]["error"]["code"],
            "not_found"
        );
        let foreign_evidence_call = call_tool_json_at(
            &alpha.router,
            &alpha.slug,
            &alpha.token,
            961,
            "get_memory_evidence",
            json!({
                "evidence_ref_id":bravo.evidence_id,
                "source_id":bravo.source_id,
                "source_revision_id":bravo.source_revision_id
            }),
        )
        .await;
        assert_eq!(
            foreign_evidence_call["result"]["structuredContent"]["error"]["code"],
            "not_found"
        );
        let reverse_card_call = call_tool_json_at(
            &bravo.router,
            &bravo.slug,
            &bravo.token,
            962,
            "get_memory_card",
            json!({"card_id":alpha.card.id.to_string()}),
        )
        .await;
        assert_eq!(
            reverse_card_call["result"]["structuredContent"]["error"]["code"],
            "not_found"
        );
        let reverse_evidence_call = call_tool_json_at(
            &bravo.router,
            &bravo.slug,
            &bravo.token,
            963,
            "get_memory_evidence",
            json!({
                "evidence_ref_id":alpha.evidence_id,
                "source_id":alpha.source_id,
                "source_revision_id":alpha.source_revision_id
            }),
        )
        .await;
        assert_eq!(
            reverse_evidence_call["result"]["structuredContent"]["error"]["code"],
            "not_found"
        );
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
                "build_memory_pack",
                "get_memory_card",
                "list_memory_cards",
                "get_memory_evidence",
                "correct_memory",
                "forget_memory",
                "get_processing_status",
                "create_note",
                "edit_note",
                "move_note",
                "delete_note",
                "note_history",
                "restore_note_revision",
                "remember",
                "get_raw_memory",
                "list_raw_memories",
                "get_raw_memory_overview",
                "update_raw_memory",
                "forget_raw_memory",
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
            (16, "build_memory_pack", json!({"task": "OAuth"})),
            (17, "list_raw_memories", json!({})),
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

        let get_raw_memory = call_tool_json(
            &router,
            access_token,
            23,
            "get_raw_memory",
            json!({"memory_id": memory_id}),
        )
        .await;
        assert_tool_ok(&get_raw_memory, "get_raw_memory");

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
            remember["result"]["structuredContent"]["data"]["explicit"]["outcome"],
            "stored"
        );

        let update_raw_memory = call_tool_json(
            &router,
            access_token,
            25,
            "update_raw_memory",
            json!({
                "memory_id": memory_id,
                "expected_revision": 1,
                "patch": {"content": "OAuth memory operations remain available."}
            }),
        )
        .await;
        assert_tool_ok(&update_raw_memory, "update_raw_memory");
        assert_eq!(
            update_raw_memory["result"]["structuredContent"]["data"]["revision"],
            2
        );

        let forget_raw_memory = call_tool_json(
            &router,
            access_token,
            26,
            "forget_raw_memory",
            json!({"memory_id": memory_id, "expected_revision": 2, "idempotency_key":"oauth-forget"}),
        )
        .await;
        assert_tool_ok(&forget_raw_memory, "forget_raw_memory");
        assert_eq!(
            forget_raw_memory["result"]["structuredContent"]["data"]["deleted"],
            true
        );
        assert_eq!(
            forget_raw_memory["result"]["structuredContent"]["data"]["ownership"],
            "explicit"
        );
        assert_eq!(
            forget_raw_memory["result"]["structuredContent"]["data"]["source_extraction_paused"],
            false
        );
    }

    fn tool_request(
        token: &str,
        id: u64,
        name: &str,
        arguments: serde_json::Value,
    ) -> Request<Body> {
        tool_request_at(token, "work", id, name, arguments)
    }

    fn tool_request_at(
        token: &str,
        slug: &str,
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
            .uri(format!("/mcp/v1/vaults/{slug}"))
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

    async fn call_tool_json_at(
        router: &Router,
        slug: &str,
        token: &str,
        id: u64,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let response = router
            .clone()
            .oneshot(tool_request_at(token, slug, id, name, arguments))
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
        list_tools_request_at(token, "work")
    }

    fn list_tools_request_at(token: &str, slug: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(format!("/mcp/v1/vaults/{slug}"))
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
        let search = body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "search_notes")
            .unwrap();
        assert_eq!(search["title"], "Search Vault notes");
        assert!(
            search["description"]
                .as_str()
                .unwrap()
                .contains("`data.results`")
        );
        assert!(
            search["inputSchema"]["properties"]["query"]["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty())
        );
        assert_eq!(search["annotations"]["openWorldHint"], false);
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
        let (router, token, _root, state, core, context) = configured_indexed_router().await;
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

        let stale_response = router
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
        let stale_body = stale_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let stale_body: serde_json::Value = serde_json::from_slice(&stale_body).unwrap();
        assert_eq!(stale_body["result"]["isError"], false, "{stale_body}");
        assert_eq!(
            stale_body["result"]["structuredContent"]["data"]["results"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "a projection for the pre-move revision must not be treated as current"
        );

        IndexService::new(state)
            .rebuild_vault(&core, &context)
            .await
            .unwrap();

        let response = router
            .clone()
            .oneshot(tool_request(
                &token,
                12,
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
                13,
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
                    "include_details": true, "query": "WebDAV conflict handling",
                    "max_results": 5,
                    "max_related_notes": 5,
                    "max_tokens": 500
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
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
                    "include_details": true, "query": "WebDAV conflict handling",
                    "max_results": 5,
                    "max_related_notes": 5,
                    "max_tokens": 500
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
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
    async fn remember_and_forget_are_current_only_across_all_model_read_paths() {
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
            body["result"]["structuredContent"]["data"]["raw_ownership"],
            "explicit"
        );
        assert_eq!(
            body["result"]["structuredContent"]["data"]["representation"],
            "raw_explicit_memory"
        );
        assert_eq!(
            body["result"]["structuredContent"]["data"]["explicit"]["outcome"],
            "stored"
        );
        let memory_id =
            body["result"]["structuredContent"]["data"]["explicit"]["memory"]["memory_id"]
                .as_str()
                .unwrap()
                .to_owned();
        let memory_revision =
            body["result"]["structuredContent"]["data"]["explicit"]["memory"]["revision"]
                .as_u64()
                .unwrap();
        let canonical_path =
            body["result"]["structuredContent"]["data"]["explicit"]["memory"]["canonical_path"]
                .as_str()
                .unwrap()
                .to_owned();
        let canonical_revision = body["result"]["structuredContent"]["data"]["explicit"]["memory"]
            ["canonical_revision"]
            .as_u64()
            .unwrap();
        assert_eq!(
            body["result"]["structuredContent"]["data"]["explicit"]["memory"]["content"],
            "The memory subsystem keeps canonical Markdown."
        );

        let raw = router
            .clone()
            .oneshot(tool_request(
                &token,
                22,
                "get_raw_memory",
                serde_json::json!({"memory_id": memory_id}),
            ))
            .await
            .unwrap();
        let body = raw.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["isError"], false);
        assert_eq!(
            body["result"]["structuredContent"]["data"]["memory_id"],
            memory_id.as_str()
        );

        let resource = router
            .clone()
            .oneshot(resource_read_request(&token, "vault://raw-memory/context"))
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
            body["result"]["contents"][0]["text"]
                .as_str()
                .unwrap()
                .contains(&memory_id)
        );

        for (id, tool, arguments) in [
            (23, "read_note", serde_json::json!({"path": canonical_path})),
            (
                24,
                "read_note",
                serde_json::json!({
                    "path": canonical_path,
                    "revision": canonical_revision
                }),
            ),
            (
                25,
                "note_history",
                serde_json::json!({"path": canonical_path}),
            ),
        ] {
            let blocked = call_tool_json(&router, &token, id, tool, arguments).await;
            assert_eq!(blocked["result"]["isError"], true, "{tool}: {blocked}");
            assert_eq!(
                blocked["result"]["structuredContent"]["error"]["code"],
                "invalid_path"
            );
        }
        let managed_resource = router
            .clone()
            .oneshot(resource_read_request(
                &token,
                &format!("vault://note/{canonical_path}"),
            ))
            .await
            .unwrap();
        let managed_resource = managed_resource
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let managed_resource: serde_json::Value =
            serde_json::from_slice(&managed_resource).unwrap();
        assert!(managed_resource.get("error").is_some());

        let forgotten = call_tool_json(
            &router,
            &token,
            26,
            "forget_raw_memory",
            serde_json::json!({
                "memory_id": memory_id,
                "expected_revision": memory_revision,
                "idempotency_key": "forget-canonical-memory"
            }),
        )
        .await;
        assert_tool_ok(&forgotten, "forget_raw_memory");
        assert_eq!(
            forgotten["result"]["structuredContent"]["data"]["deleted"],
            true
        );

        for (id, tool, arguments) in [
            (
                27,
                "get_raw_memory",
                serde_json::json!({"memory_id": memory_id}),
            ),
            (28, "list_raw_memories", serde_json::json!({})),
        ] {
            let response = call_tool_json(&router, &token, id, tool, arguments).await;
            if tool == "get_raw_memory" {
                assert_eq!(response["result"]["isError"], true, "{response}");
            } else {
                assert_eq!(response["result"]["isError"], false, "{response}");
                assert!(
                    !response["result"]["structuredContent"]["data"]
                        .to_string()
                        .contains(&memory_id),
                    "{tool}: {response}"
                );
            }
        }

        for uri in [
            "vault://raw-memory/context".to_owned(),
            format!("vault://raw-memory/{memory_id}"),
        ] {
            let response = router
                .clone()
                .oneshot(resource_read_request(&token, &uri))
                .await
                .unwrap();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(!body.to_string().contains(&memory_id), "{uri}: {body}");
        }
    }

    #[tokio::test]
    async fn raw_memory_overview_is_vault_read_scoped_over_real_http() {
        let (full_router, full_token, memory_router, memory_token, _root) =
            configured_memory_router_with_memory_only_pair().await;
        let created = call_tool_json(
            &full_router,
            &full_token,
            700,
            "create_note",
            json!({"path":"notes/private.md","content":"# Private\n"}),
        )
        .await;
        assert_tool_ok(&created, "create_note");
        let saved = call_tool_json(
            &full_router,
            &full_token,
            701,
            "remember",
            json!({
                "content":"Private raw assertion",
                "idempotency_key":"raw-overview-http",
                "sources":[{
                    "path":"notes/private.md",
                    "file_id":created["result"]["structuredContent"]["data"]["file"]["file_id"],
                    "revision":created["result"]["structuredContent"]["data"]["file"]["revision"]
                }]
            }),
        )
        .await;
        assert_tool_ok(&saved, "remember");

        let full = call_tool_json(
            &full_router,
            &full_token,
            702,
            "get_raw_memory_overview",
            json!({"source_path":"notes/private.md","path_prefix":"notes"}),
        )
        .await;
        assert_tool_ok(&full, "get_raw_memory_overview");
        let full_data = &full["result"]["structuredContent"]["data"];
        assert_eq!(full_data["raw_ownership"], "explicit");
        assert_eq!(full_data["entries"].as_array().unwrap().len(), 1);
        assert_eq!(
            full_data["entries"][0]["sources"][0]["path"],
            "notes/private.md"
        );
        assert!(
            full_data["entries"][0]["resource_uri"]
                .as_str()
                .unwrap()
                .starts_with("vault://raw-memory/")
        );
        assert!(!full_data.to_string().contains("vault://memory/"));

        let filtered = call_tool_json(
            &memory_router,
            &memory_token,
            703,
            "get_raw_memory_overview",
            json!({
                "source_path":"notes/does-not-exist.md",
                "path_prefix":"notes/does-not-exist",
                "topic_ids":["does-not-exist"]
            }),
        )
        .await;
        let unfiltered = call_tool_json(
            &memory_router,
            &memory_token,
            704,
            "get_raw_memory_overview",
            json!({}),
        )
        .await;
        assert_tool_ok(&filtered, "get_raw_memory_overview");
        assert_tool_ok(&unfiltered, "get_raw_memory_overview");
        let filtered_data = &filtered["result"]["structuredContent"]["data"];
        let unfiltered_data = &unfiltered["result"]["structuredContent"]["data"];
        assert_eq!(filtered_data["scope_count"], unfiltered_data["scope_count"]);
        assert_eq!(
            filtered_data["entries"].as_array().unwrap().len(),
            unfiltered_data["entries"].as_array().unwrap().len()
        );
        assert!(!filtered_data.to_string().contains("notes/private.md"));
        assert!(!filtered_data.to_string().contains("vault://memory/"));
        assert!(!filtered_data.to_string().contains("generated_sections"));
        assert_eq!(filtered_data["entries"][0]["label"], "raw explicit memory");
    }

    #[tokio::test]
    async fn memory_cursor_survives_deletion_before_the_next_page() {
        let (router, token, _root) = configured_memory_router().await;
        for index in 0..5 {
            let saved = call_tool_json(
                &router,
                &token,
                800 + index,
                "remember",
                json!({"content":format!("Cursor fixture unique assertion {index}."),"idempotency_key":format!("cursor-fixture-{index}")}),
            )
            .await;
            assert_tool_ok(&saved, "remember");
        }
        let all = call_tool_json(
            &router,
            &token,
            810,
            "list_raw_memories",
            json!({"limit":100}),
        )
        .await;
        let all = &all["result"]["structuredContent"]["data"]["memories"];
        let first = call_tool_json(
            &router,
            &token,
            811,
            "list_raw_memories",
            json!({"limit":2}),
        )
        .await;
        let data = &first["result"]["structuredContent"]["data"];
        let deleted=call_tool_json(&router,&token,812,"forget_raw_memory",json!({"memory_id":data["memories"][0]["memory_id"],"expected_revision":data["memories"][0]["revision"],"idempotency_key":"cursor-forget"})).await;
        assert_tool_ok(&deleted, "forget_raw_memory");
        let next = call_tool_json(
            &router,
            &token,
            813,
            "list_raw_memories",
            json!({"limit":100,"cursor":data["next_cursor"]}),
        )
        .await;
        assert_tool_ok(&next, "list_raw_memories");
        let remaining = next["result"]["structuredContent"]["data"]["memories"]
            .as_array()
            .unwrap();
        assert_eq!(
            remaining
                .iter()
                .map(|m| m["memory_id"].clone())
                .collect::<Vec<_>>(),
            all.as_array()
                .unwrap()
                .iter()
                .skip(2)
                .map(|m| m["memory_id"].clone())
                .collect::<Vec<_>>()
        );
    }
}
