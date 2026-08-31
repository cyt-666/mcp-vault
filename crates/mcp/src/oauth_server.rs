//! Public HTTP adapter for MCP Vault's built-in OAuth authorization server.

use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{
        ConnectInfo, DefaultBodyLimit, Form, Query, RawQuery, State,
        rejection::{FormRejection, JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use mcp_vault_auth::{
    AuthError, LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE, LocalOAuthAuthorizationInput,
    LocalOAuthAuthorizationPrompt, LocalOAuthClientRegistration, LocalOAuthCodeExchange,
    LocalOAuthRefreshExchange, LocalOAuthTokenIssue, SecretString,
};
use mcp_vault_domain::{Scope, ScopeSet, VaultContext};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use super::McpService;

const OAUTH_BODY_LIMIT: usize = 32 * 1024;
pub(super) const AUTHORIZATION_PATH: &str = "/oauth/v2/authorize";
pub(super) const VERSIONED_V1_AUTHORIZATION_PATH: &str = "/oauth/v1/authorize";
pub(super) const LEGACY_AUTHORIZATION_PATH: &str = "/oauth/authorize";
pub(super) const TOKEN_PATH: &str = "/oauth/token";

pub(super) fn routes() -> Router<McpService> {
    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route("/oauth/register", post(register_client))
        .route(
            AUTHORIZATION_PATH,
            get(begin_authorization).post(complete_authorization),
        )
        .route(
            VERSIONED_V1_AUTHORIZATION_PATH,
            get(redirect_to_current_authorization).post(complete_authorization),
        )
        .route(
            LEGACY_AUTHORIZATION_PATH,
            get(redirect_to_current_authorization).post(complete_authorization),
        )
        .route(TOKEN_PATH, post(token))
        .layer(DefaultBodyLimit::max(OAUTH_BODY_LIMIT))
}

async fn authorization_server_metadata(State(service): State<McpService>) -> Response {
    let Some(issuer) = issuer_origin(&service) else {
        return oauth_json_error(
            StatusCode::NOT_FOUND,
            "server_error",
            "The built-in authorization server is not configured.",
        );
    };
    match has_enabled_local_oauth(&service).await {
        Ok(true) => {}
        Ok(false) => {
            return oauth_json_error(
                StatusCode::NOT_FOUND,
                "server_error",
                "The built-in authorization server is not configured.",
            );
        }
        Err(()) => {
            return oauth_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "The authorization service is unavailable.",
            );
        }
    }
    let issuer = issuer.trim_end_matches('/');
    let mut scopes_supported = Scope::ALL.map(|scope| scope.to_string()).to_vec();
    scopes_supported.push(LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE.to_owned());
    oauth_json(
        StatusCode::OK,
        json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}{AUTHORIZATION_PATH}"),
            "token_endpoint": format!("{issuer}/oauth/token"),
            "registration_endpoint": format!("{issuer}/oauth/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": scopes_supported,
            "authorization_response_iss_parameter_supported": true,
            "service_documentation": format!("{issuer}/.well-known/oauth-authorization-server"),
        }),
    )
}

#[derive(Debug, Deserialize)]
struct RegistrationRequest {
    #[serde(default)]
    client_name: Option<String>,
    redirect_uris: Vec<String>,
    #[serde(default)]
    grant_types: Option<Vec<String>>,
    #[serde(default)]
    response_types: Option<Vec<String>>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

async fn register_client(
    State(service): State<McpService>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    input: Result<Json<RegistrationRequest>, JsonRejection>,
) -> Response {
    if issuer_origin(&service).is_none() {
        return oauth_json_error(
            StatusCode::NOT_FOUND,
            "server_error",
            "The built-in authorization server is not configured.",
        );
    }
    match has_enabled_local_oauth(&service).await {
        Ok(true) => {}
        Ok(false) => {
            return oauth_json_error(
                StatusCode::NOT_FOUND,
                "server_error",
                "The built-in authorization server is not configured.",
            );
        }
        Err(()) => {
            return oauth_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "The authorization service is unavailable.",
            );
        }
    }
    let Json(input) = match input {
        Ok(input) => input,
        Err(_) => {
            return oauth_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_client_metadata",
                "The client registration document is invalid.",
            );
        }
    };
    match service
        .auth_state
        .auth
        .register_local_oauth_client(
            LocalOAuthClientRegistration {
                client_name: input.client_name,
                redirect_uris: input.redirect_uris,
                grant_types: input.grant_types,
                response_types: input.response_types,
                token_endpoint_auth_method: input.token_endpoint_auth_method,
            },
            Some(&peer.ip().to_string()),
        )
        .await
    {
        Ok(client) => oauth_json(
            StatusCode::CREATED,
            json!({
                "client_id": client.client_id.to_string(),
                "client_id_issued_at": client.client_id_issued_at,
                "client_name": client.client_name,
                "redirect_uris": client.redirect_uris,
                "grant_types": client.grant_types,
                "response_types": client.response_types,
                "token_endpoint_auth_method": client.token_endpoint_auth_method,
            }),
        ),
        Err(AuthError::State(_)) => oauth_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "The client registration service is unavailable.",
        ),
        Err(_) => oauth_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "The public client metadata is unsupported or invalid.",
        ),
    }
}

#[derive(Clone, Debug, Deserialize)]
struct AuthorizationQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    state: Option<String>,
    code_challenge: String,
    code_challenge_method: String,
    resource: String,
}

async fn redirect_to_current_authorization(RawQuery(query): RawQuery) -> Response {
    let target = query
        .filter(|query| !query.is_empty())
        .map(|query| format!("{AUTHORIZATION_PATH}?{query}"))
        .unwrap_or_else(|| AUTHORIZATION_PATH.to_owned());
    let location = match HeaderValue::from_str(&target) {
        Ok(location) => location,
        Err(_) => {
            return oauth_html_error(
                StatusCode::BAD_REQUEST,
                "授权请求缺少必要参数或格式不正确。",
            );
        }
    };
    let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    apply_no_store(response.headers_mut());
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

async fn begin_authorization(
    State(service): State<McpService>,
    query: Result<Query<AuthorizationQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => {
            return oauth_html_error(
                StatusCode::BAD_REQUEST,
                "授权请求缺少必要参数或格式不正确。",
            );
        }
    };
    let issuer = match issuer_origin(&service) {
        Some(issuer) => issuer.trim_end_matches('/').to_owned(),
        None => {
            return oauth_html_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "授权服务尚未配置公网地址。",
            );
        }
    };
    let (context, expected_resource) = match resolve_local_resource(&service, &query.resource).await
    {
        Ok(value) => value,
        Err(ResourceError::NotFound) => {
            return oauth_html_error(StatusCode::BAD_REQUEST, "请求的 MCP 资源不可用。");
        }
        Err(ResourceError::Unavailable) => {
            return oauth_html_error(StatusCode::INTERNAL_SERVER_ERROR, "授权服务暂时不可用。");
        }
    };
    match service
        .auth_state
        .auth
        .begin_local_oauth_authorization(
            &context,
            &expected_resource,
            LocalOAuthAuthorizationInput {
                response_type: query.response_type,
                client_id: query.client_id,
                redirect_uri: query.redirect_uri,
                scope: query.scope,
                state: query.state,
                code_challenge: query.code_challenge,
                code_challenge_method: query.code_challenge_method,
                resource: query.resource,
            },
        )
        .await
    {
        Ok(prompt) => authorization_page(prompt, None, StatusCode::OK, &issuer),
        Err(AuthError::State(_)) => {
            oauth_html_error(StatusCode::INTERNAL_SERVER_ERROR, "授权服务暂时不可用。")
        }
        Err(_) => oauth_html_error(
            StatusCode::BAD_REQUEST,
            "客户端、回调地址、权限、resource 或 PKCE 参数无效。",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizationForm {
    request_handle: String,
    resource: String,
    username: String,
    password: String,
}

async fn complete_authorization(
    State(service): State<McpService>,
    form: Result<Form<AuthorizationForm>, FormRejection>,
) -> Response {
    let Form(form) = match form {
        Ok(form) => form,
        Err(_) => {
            return oauth_html_error(StatusCode::BAD_REQUEST, "登录表单格式不正确。");
        }
    };
    let issuer = match issuer_origin(&service) {
        Some(issuer) => issuer.trim_end_matches('/').to_owned(),
        None => {
            return oauth_html_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "授权服务尚未配置公网地址。",
            );
        }
    };
    let (context, _) = match resolve_local_resource(&service, &form.resource).await {
        Ok(value) => value,
        Err(ResourceError::NotFound) => {
            return oauth_html_error(StatusCode::BAD_REQUEST, "请求的 MCP 资源不可用。");
        }
        Err(ResourceError::Unavailable) => {
            return oauth_html_error(StatusCode::INTERNAL_SERVER_ERROR, "授权服务暂时不可用。");
        }
    };
    match service
        .auth_state
        .auth
        .complete_local_oauth_authorization(
            &context,
            &form.request_handle,
            &form.username,
            &SecretString::new(form.password),
            None,
            &issuer,
        )
        .await
    {
        Ok(result) => authorization_redirect(result),
        Err(error @ (AuthError::InvalidCredential | AuthError::RateLimited)) => {
            let prompt = service
                .auth_state
                .auth
                .local_oauth_authorization_prompt(&context, &form.request_handle)
                .await;
            match prompt {
                Ok(prompt) => authorization_page(
                    prompt,
                    Some(if matches!(error, AuthError::RateLimited) {
                        "尝试次数过多，请稍后再试。"
                    } else {
                        "用户名或密码不正确。"
                    }),
                    if matches!(error, AuthError::RateLimited) {
                        StatusCode::TOO_MANY_REQUESTS
                    } else {
                        StatusCode::UNAUTHORIZED
                    },
                    &issuer,
                ),
                Err(_) => oauth_html_error(StatusCode::BAD_REQUEST, "授权请求已过期，请重新连接。"),
            }
        }
        Err(AuthError::Expired) => {
            oauth_html_error(StatusCode::BAD_REQUEST, "授权请求已过期，请重新连接。")
        }
        Err(AuthError::State(_)) => {
            oauth_html_error(StatusCode::INTERNAL_SERVER_ERROR, "授权服务暂时不可用。")
        }
        Err(_) => oauth_html_error(StatusCode::BAD_REQUEST, "授权请求无效。"),
    }
}

#[derive(Debug, Deserialize)]
struct TokenForm {
    grant_type: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    client_id: String,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    code_verifier: Option<String>,
    resource: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    client_assertion: Option<String>,
}

async fn token(
    State(service): State<McpService>,
    headers: HeaderMap,
    form: Result<Form<TokenForm>, FormRejection>,
) -> Response {
    if headers.contains_key(header::AUTHORIZATION) {
        return oauth_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_client",
            "This authorization server accepts public clients with token auth method none.",
        );
    }
    let Form(form) = match form {
        Ok(form) => form,
        Err(_) => {
            return oauth_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The token request is malformed.",
            );
        }
    };
    if form.client_secret.is_some() || form.client_assertion.is_some() {
        return oauth_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_client",
            "Client secrets and assertions are not accepted for this public client.",
        );
    }
    let (context, expected_resource) = match resolve_local_resource(&service, &form.resource).await
    {
        Ok(value) => value,
        Err(ResourceError::NotFound) => {
            return oauth_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_target",
                "The requested resource is invalid.",
            );
        }
        Err(ResourceError::Unavailable) => {
            return oauth_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "The token service is unavailable.",
            );
        }
    };
    let result = match form.grant_type.as_str() {
        "authorization_code" => {
            let (Some(code), Some(redirect_uri), Some(code_verifier)) = (
                form.code.as_deref(),
                form.redirect_uri.as_deref(),
                form.code_verifier.as_deref(),
            ) else {
                return oauth_json_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "The authorization code exchange is incomplete.",
                );
            };
            service
                .auth_state
                .auth
                .exchange_local_oauth_code(
                    &context,
                    LocalOAuthCodeExchange {
                        code,
                        client_id: &form.client_id,
                        redirect_uri,
                        code_verifier,
                        resource: &expected_resource,
                    },
                )
                .await
        }
        "refresh_token" => {
            let Some(refresh_token) = form.refresh_token.as_deref() else {
                return oauth_json_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "The refresh token is missing.",
                );
            };
            service
                .auth_state
                .auth
                .refresh_local_oauth_token(
                    &context,
                    LocalOAuthRefreshExchange {
                        refresh_token,
                        client_id: &form.client_id,
                        resource: &expected_resource,
                        scope: form.scope.as_deref(),
                    },
                )
                .await
        }
        _ => {
            return oauth_json_error(
                StatusCode::BAD_REQUEST,
                "unsupported_grant_type",
                "Only authorization_code and refresh_token are supported.",
            );
        }
    };
    match result {
        Ok(issue) => token_response(issue, &expected_resource),
        Err(AuthError::ScopeDenied) => oauth_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_scope",
            "The requested scope is not granted.",
        ),
        Err(AuthError::State(_) | AuthError::Cryptography | AuthError::MasterKeyUnavailable) => {
            oauth_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "The token service is unavailable.",
            )
        }
        Err(AuthError::Domain(_) | AuthError::InvalidInput | AuthError::OAuthConfiguration) => {
            oauth_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The token request is invalid.",
            )
        }
        Err(_) => oauth_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The authorization grant is invalid, expired, or already used.",
        ),
    }
}

async fn has_enabled_local_oauth(service: &McpService) -> Result<bool, ()> {
    let vaults = service
        .auth_state
        .state
        .vaults()
        .list()
        .await
        .map_err(|_| ())?;
    for vault in vaults {
        if vault.status != mcp_vault_state::VaultStatus::Active {
            continue;
        }
        let context = vault.context().map_err(|_| ())?;
        if service
            .auth_state
            .auth
            .local_oauth_enabled(&context)
            .await
            .map_err(|_| ())?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

enum ResourceError {
    NotFound,
    Unavailable,
}

async fn resolve_local_resource(
    service: &McpService,
    resource: &str,
) -> Result<(VaultContext, String), ResourceError> {
    let origin = issuer_origin(service)
        .ok_or(ResourceError::NotFound)?
        .trim_end_matches('/');
    let vaults = service
        .auth_state
        .state
        .vaults()
        .list()
        .await
        .map_err(|_| ResourceError::Unavailable)?;
    for vault in vaults {
        if vault.status != mcp_vault_state::VaultStatus::Active {
            continue;
        }
        let expected = format!("{origin}/mcp/v1/vaults/{}", vault.slug);
        if resource != expected {
            continue;
        }
        let context = vault.context().map_err(|_| ResourceError::Unavailable)?;
        if service
            .auth_state
            .auth
            .local_oauth_enabled(&context)
            .await
            .map_err(|_| ResourceError::Unavailable)?
        {
            return Ok((context, expected));
        }
    }
    Err(ResourceError::NotFound)
}

fn authorization_page(
    prompt: LocalOAuthAuthorizationPrompt,
    error: Option<&str>,
    status: StatusCode,
    issuer: &str,
) -> Response {
    let mut scope_items = prompt
        .scopes
        .iter()
        .map(|scope| format!("<li><code>{}</code></li>", escape_html(&scope.to_string())))
        .collect::<String>();
    if prompt.offline_access {
        scope_items.push_str("<li><code>offline_access</code>（保持长期连接）</li>");
    }
    let error = error
        .map(|message| {
            format!(
                "<p class=\"error\" role=\"alert\">{}</p>",
                escape_html(message)
            )
        })
        .unwrap_or_default();
    let action = format!("{}{AUTHORIZATION_PATH}", issuer.trim_end_matches('/'));
    let html = format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Cache-Control\" content=\"no-store, no-cache, must-revalidate\"><meta http-equiv=\"Pragma\" content=\"no-cache\"><meta http-equiv=\"Expires\" content=\"0\"><title>MCP Vault 授权</title><style>body{{font-family:system-ui,sans-serif;background:#f5f3ee;color:#20201e;margin:0}}main{{max-width:36rem;margin:8vh auto;background:white;padding:2rem;border-radius:16px;box-shadow:0 8px 32px #0002}}label{{display:block;margin:1rem 0 .35rem}}input{{box-sizing:border-box;width:100%;padding:.75rem;border:1px solid #aaa;border-radius:8px}}button{{margin-top:1.25rem;width:100%;padding:.8rem;border:0;border-radius:8px;background:#315b4c;color:white;font-weight:700}}code{{font-size:.9em}}.muted{{color:#666;overflow-wrap:anywhere}}.error{{color:#a11;background:#fee;padding:.75rem;border-radius:8px}}</style></head><body><main><h1>授权 ChatGPT 访问 MCP Vault</h1><p><strong>{}</strong> 请求访问：</p><p class=\"muted\">{}</p><p>授权范围：</p><ul>{}</ul>{}<form id=\"oauth-login\" method=\"post\" action=\"{}\" accept-charset=\"UTF-8\" autocomplete=\"on\"><input type=\"hidden\" name=\"request_handle\" value=\"{}\"><input type=\"hidden\" name=\"resource\" value=\"{}\"><label for=\"username\">Vault OAuth 用户名</label><input id=\"username\" name=\"username\" type=\"text\" autocomplete=\"username\" autocapitalize=\"none\" spellcheck=\"false\" required maxlength=\"128\"><label for=\"password\">Vault OAuth 密码</label><input id=\"password\" name=\"password\" type=\"password\" autocomplete=\"current-password\" required><button type=\"submit\" form=\"oauth-login\">登录并授权</button></form><p class=\"muted\">这里使用独立的 Vault OAuth 凭据，不是 Admin 密码。</p></main></body></html>",
        escape_html(&prompt.client_name),
        escape_html(&prompt.resource),
        scope_items,
        error,
        escape_html(&action),
        escape_html(prompt.request_handle.expose_secret()),
        escape_html(&prompt.resource),
    );
    html_response(status, html, true)
}

fn authorization_redirect(result: mcp_vault_auth::LocalOAuthAuthorizationResult) -> Response {
    let mut redirect = match Url::parse(&result.redirect_uri) {
        Ok(redirect) => redirect,
        Err(_) => {
            return oauth_html_error(StatusCode::INTERNAL_SERVER_ERROR, "回调地址无效。");
        }
    };
    {
        let mut query = redirect.query_pairs_mut();
        query.append_pair("code", result.code.expose_secret());
        if let Some(state) = result.state.as_deref() {
            query.append_pair("state", state);
        }
        query.append_pair("iss", &result.issuer);
    }
    let location = match HeaderValue::from_str(redirect.as_str()) {
        Ok(location) => location,
        Err(_) => {
            return oauth_html_error(StatusCode::INTERNAL_SERVER_ERROR, "回调地址无效。");
        }
    };
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    apply_no_store(response.headers_mut());
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn token_response(issue: LocalOAuthTokenIssue, resource: &str) -> Response {
    oauth_json(
        StatusCode::OK,
        json!({
            "access_token": issue.access_token.expose_secret(),
            "token_type": "Bearer",
            "expires_in": issue.expires_in,
            "refresh_token": issue.refresh_token.expose_secret(),
            "scope": scope_string(&issue.scopes, issue.offline_access),
            "resource": resource,
        }),
    )
}

fn scope_string(scopes: &ScopeSet, offline_access: bool) -> String {
    let mut values = scopes.iter().map(ToString::to_string).collect::<Vec<_>>();
    if offline_access {
        values.push(LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE.to_owned());
    }
    values.join(" ")
}

fn oauth_json(status: StatusCode, value: serde_json::Value) -> Response {
    let mut response = (status, Json(value)).into_response();
    apply_no_store(response.headers_mut());
    response
}

fn oauth_json_error(
    status: StatusCode,
    error: &'static str,
    description: &'static str,
) -> Response {
    oauth_json(
        status,
        json!({"error": error, "error_description": description}),
    )
}

fn oauth_html_error(status: StatusCode, message: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>OAuth 请求失败</title></head><body><main><h1>无法完成授权</h1><p>{}</p></main></body></html>",
        escape_html(message)
    );
    html_response(status, html, false)
}

fn html_response(status: StatusCode, html: String, authorization_form: bool) -> Response {
    let mut response = (status, Html(html)).into_response();
    apply_no_store(response.headers_mut());
    let content_security_policy = if authorization_form {
        // Chromium can reject native OAuth form submission even when
        // `form-action` names the exact canonical origin. The form action is
        // fixed server-side and the POST remains bound to an opaque request
        // handle, exact resource/client/redirect/PKCE transaction, and Host
        // validation. `form-action` has no `default-src` fallback, so omitting
        // only this navigation directive preserves the rest of the deny-by-
        // default policy while avoiding the browser regression.
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'",
        )
    } else {
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'",
        )
    };
    response
        .headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, content_security_policy);
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn apply_no_store(headers: &mut HeaderMap) {
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache, no-store, max-age=0, must-revalidate"),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(header::EXPIRES, HeaderValue::from_static("0"));
    headers.insert("cdn-cache-control", HeaderValue::from_static("no-store"));
    headers.insert("surrogate-control", HeaderValue::from_static("no-store"));
    headers.insert(header::VARY, HeaderValue::from_static("*"));
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Return the canonical built-in issuer only when transport is HTTPS or an
/// explicit loopback development origin.
pub(super) fn issuer_origin(service: &McpService) -> Option<&str> {
    let origin = service.auth_state.public_origin.as_deref()?;
    let url = Url::parse(origin).ok()?;
    if url.scheme() == "https" {
        return Some(origin);
    }
    if url.scheme() != "http" {
        return None;
    }
    match url.host() {
        Some(url::Host::Domain(host)) if host.eq_ignore_ascii_case("localhost") => Some(origin),
        Some(url::Host::Ipv4(address)) if address.is_loopback() => Some(origin),
        Some(url::Host::Ipv6(address)) if address.is_loopback() => Some(origin),
        _ => None,
    }
}
