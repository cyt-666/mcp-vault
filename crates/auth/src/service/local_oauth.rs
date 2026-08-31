//! Complete built-in OAuth 2.1 authorization-server application flow.

use std::{net::IpAddr, str::FromStr, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mcp_vault_domain::{
    Actor, ActorId, ActorType, OAuthAccessTokenId, OAuthAuthorizationCodeId,
    OAuthAuthorizationRequestId, OAuthClientId, OAuthLocalUserId, OAuthRefreshTokenId,
    OAuthTokenFamilyId, Scope, ScopeSet, VaultContext,
};
use mcp_vault_state::{
    NewOAuthAccessToken, NewOAuthAuthorizationCode, NewOAuthRefreshToken,
    OAuthAuthorizationCodeRecord, OAuthAuthorizationRequestRecord, OAuthClientRecord,
    OAuthLocalUserRecord, OAuthRefreshTokenRecord, StateError,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use super::{AuthPrincipal, AuthService, normalize_username, now_millis};
use crate::{
    AuthError, BearerToken, SecretString, generate_bearer_token, parse_scopes, token_prefix,
};

const REQUEST_LABEL: &str = "mcpv_oauth_req_";
const CODE_LABEL: &str = "mcpv_oauth_code_";
const ACCESS_LABEL: &str = "mcpv_oauth_";
const REFRESH_LABEL: &str = "mcpv_refresh_";
const REQUEST_DIGEST_PURPOSE: &str = "local-oauth-request-digest";
const CODE_DIGEST_PURPOSE: &str = "local-oauth-code-digest";
const ACCESS_DIGEST_PURPOSE: &str = "local-oauth-access-digest";
const REFRESH_DIGEST_PURPOSE: &str = "local-oauth-refresh-digest";
const AUTHORIZATION_REQUEST_LIFETIME: Duration = Duration::from_secs(10 * 60);
const AUTHORIZATION_CODE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const ACCESS_TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);
const REFRESH_TOKEN_LIFETIME: Duration = Duration::from_secs(180 * 24 * 60 * 60);
const REFRESH_TOKEN_REUSE_GRACE: Duration = Duration::from_secs(60);
const OAUTH_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_ACTIVE_CLIENTS: u32 = 1024;
const TOKEN_LOOKUP_PREFIX: usize = 20;

/// OAuth protocol scope used by long-lived clients. It deliberately does not
/// map to a Vault permission or appear in protected-resource metadata.
pub const LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE: &str = "offline_access";

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalOAuthGrantedScopes {
    application: ScopeSet,
    offline_access: bool,
}

/// Redaction-safe local OAuth user metadata shown in Admin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalOAuthUser {
    pub id: OAuthLocalUserId,
    pub vault_id: mcp_vault_domain::VaultId,
    pub username: String,
    pub scopes: ScopeSet,
    pub enabled: bool,
    pub password_changed_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// RFC 7591 public-client registration input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalOAuthClientRegistration {
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
    pub token_endpoint_auth_method: Option<String>,
}

/// Public client metadata returned after DCR. No secret exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalOAuthClientRegistrationResult {
    pub client_id: OAuthClientId,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub client_id_issued_at: i64,
}

/// Validated authorization endpoint input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalOAuthAuthorizationInput {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: String,
}

/// Safe prompt material rendered by the public login/consent adapter.
pub struct LocalOAuthAuthorizationPrompt {
    pub request_handle: BearerToken,
    pub client_name: String,
    pub resource: String,
    pub scopes: ScopeSet,
    pub offline_access: bool,
    pub expires_at: i64,
}

impl std::fmt::Debug for LocalOAuthAuthorizationPrompt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalOAuthAuthorizationPrompt")
            .field("request_handle", &"[REDACTED]")
            .field("client_name", &self.client_name)
            .field("resource", &self.resource)
            .field("scopes", &self.scopes)
            .field("offline_access", &self.offline_access)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Successful login/consent result used to construct the exact redirect.
pub struct LocalOAuthAuthorizationResult {
    pub redirect_uri: String,
    pub code: BearerToken,
    pub state: Option<String>,
    pub issuer: String,
}

/// Authorization-code token exchange input.
pub struct LocalOAuthCodeExchange<'a> {
    pub code: &'a str,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub code_verifier: &'a str,
    pub resource: &'a str,
}

/// Refresh-token exchange input.
pub struct LocalOAuthRefreshExchange<'a> {
    pub refresh_token: &'a str,
    pub client_id: &'a str,
    pub resource: &'a str,
    pub scope: Option<&'a str>,
}

/// One-time token endpoint result. Plaintext tokens never implement `Debug`.
pub struct LocalOAuthTokenIssue {
    pub access_token: BearerToken,
    pub refresh_token: BearerToken,
    pub expires_in: u64,
    pub scopes: ScopeSet,
    pub offline_access: bool,
}

impl AuthService {
    /// Create or rotate the independent Vault OAuth login. Replacing it
    /// revokes all existing local OAuth grants and tokens for this Vault.
    pub async fn configure_local_oauth_user(
        &self,
        context: &VaultContext,
        username: &str,
        password: &SecretString,
        scopes: ScopeSet,
    ) -> Result<LocalOAuthUser, AuthError> {
        self.ensure_persistent_master_key().await?;
        let username = normalize_username(username)?;
        if scopes.iter().next().is_none() {
            return Err(AuthError::ScopeDenied);
        }
        let password_hash = self.hash_password(password).await?;
        let scopes_json = scopes_json(&scopes)?;
        let record = self
            .repository
            .upsert_local_oauth_user(
                context,
                OAuthLocalUserId::new(),
                &username,
                &password_hash,
                &scopes_json,
            )
            .await?;
        local_user_view(record)
    }

    /// Return redaction-safe local OAuth configuration for one Vault.
    pub async fn local_oauth_user(
        &self,
        context: &VaultContext,
    ) -> Result<Option<LocalOAuthUser>, AuthError> {
        self.repository
            .get_local_oauth_user(context)
            .await?
            .map(local_user_view)
            .transpose()
    }

    /// Return whether built-in authorization is ready for this Vault.
    pub async fn local_oauth_enabled(&self, context: &VaultContext) -> Result<bool, AuthError> {
        Ok(self
            .repository
            .get_local_oauth_user(context)
            .await?
            .is_some_and(|user| user.enabled))
    }

    /// Disable the local OAuth login and revoke all outstanding local tokens.
    pub async fn disable_local_oauth_user(
        &self,
        context: &VaultContext,
    ) -> Result<bool, AuthError> {
        self.repository
            .disable_local_oauth_user(context)
            .await
            .map_err(AuthError::from)
    }

    /// Register a public client through the bounded RFC 7591 subset.
    pub async fn register_local_oauth_client(
        &self,
        input: LocalOAuthClientRegistration,
        source_ip: Option<&str>,
    ) -> Result<LocalOAuthClientRegistrationResult, AuthError> {
        self.ensure_persistent_master_key().await?;
        let limiter_key = format!("oauth-dcr\0{}", source_ip.unwrap_or("unknown"));
        let now = now_millis()?;
        self.limiter.check(&limiter_key, now)?;
        // DCR is public and deliberately counted even when successful.
        self.limiter.failure(&limiter_key, now);
        let client_name = input
            .client_name
            .unwrap_or_else(|| "ChatGPT MCP client".to_owned());
        validate_client_name(&client_name)?;
        let redirect_uris = validate_redirect_uris(input.redirect_uris)?;
        let grant_types = validate_exact_values(
            input.grant_types,
            &["authorization_code", "refresh_token"],
            "authorization_code",
        )?;
        if !["authorization_code", "refresh_token"]
            .iter()
            .all(|required| grant_types.iter().any(|value| value == required))
        {
            return Err(AuthError::OAuthConfiguration);
        }
        let response_types = validate_exact_values(input.response_types, &["code"], "code")?;
        let token_endpoint_auth_method = input
            .token_endpoint_auth_method
            .unwrap_or_else(|| "none".to_owned());
        if token_endpoint_auth_method != "none" {
            return Err(AuthError::OAuthConfiguration);
        }
        let record = self
            .repository
            .insert_oauth_client(
                OAuthClientId::new(),
                &client_name,
                &serde_json::to_string(&redirect_uris).map_err(|_| AuthError::InvalidInput)?,
                &serde_json::to_string(&grant_types).map_err(|_| AuthError::InvalidInput)?,
                &serde_json::to_string(&response_types).map_err(|_| AuthError::InvalidInput)?,
                MAX_ACTIVE_CLIENTS,
            )
            .await?;
        client_registration_view(record)
    }

    /// Validate an authorization request and create a short-lived opaque
    /// browser handle before asking for the Vault OAuth password.
    pub async fn begin_local_oauth_authorization(
        &self,
        context: &VaultContext,
        expected_resource: &str,
        input: LocalOAuthAuthorizationInput,
    ) -> Result<LocalOAuthAuthorizationPrompt, AuthError> {
        self.ensure_persistent_master_key().await?;
        if input.response_type != "code"
            || input.code_challenge_method != "S256"
            || input.resource != expected_resource
            || !valid_pkce_challenge(&input.code_challenge)
        {
            return Err(AuthError::OAuthConfiguration);
        }
        validate_oauth_state(input.state.as_deref())?;
        let client_id = OAuthClientId::parse(&input.client_id)?;
        let client = self
            .repository
            .get_oauth_client(client_id)
            .await?
            .filter(|client| client.revoked_at.is_none())
            .ok_or(AuthError::InvalidCredential)?;
        ensure_client_capabilities(&client)?;
        let redirect_uris: Vec<String> = serde_json::from_str(&client.redirect_uris_json)
            .map_err(|_| AuthError::OAuthConfiguration)?;
        if !redirect_uris.iter().any(|uri| uri == &input.redirect_uri) {
            return Err(AuthError::OAuthConfiguration);
        }
        let user = self
            .repository
            .get_local_oauth_user(context)
            .await?
            .filter(|user| user.enabled)
            .ok_or(AuthError::OAuthConfiguration)?;
        let allowed = parse_scopes(&user.scopes_json)?;
        let granted_scopes = requested_authorization_scopes(input.scope.as_deref(), &allowed)?;
        let granted_scopes_json = local_oauth_scopes_json(&granted_scopes)?;
        let handle = generate_bearer_token(REQUEST_LABEL);
        let digest = self
            .keys
            .keyed_digest(REQUEST_DIGEST_PURPOSE, handle.expose_secret().as_bytes());
        let now = now_millis()?;
        let expires_at = now.saturating_add(duration_millis(AUTHORIZATION_REQUEST_LIFETIME)?);
        self.repository
            .insert_oauth_authorization_request(
                context,
                OAuthAuthorizationRequestId::new(),
                &digest,
                self.keys.current_version(),
                client_id,
                expected_resource,
                &input.redirect_uri,
                &granted_scopes_json,
                input.state.as_deref(),
                &input.code_challenge,
                now,
                expires_at,
            )
            .await?;
        let remove_before = now.saturating_sub(duration_millis(OAUTH_RETENTION)?);
        self.repository.cleanup_local_oauth(remove_before).await?;
        Ok(LocalOAuthAuthorizationPrompt {
            request_handle: handle,
            client_name: client.client_name,
            resource: expected_resource.to_owned(),
            scopes: granted_scopes.application,
            offline_access: granted_scopes.offline_access,
            expires_at,
        })
    }

    /// Verify the distinct Vault OAuth login and atomically create one code.
    #[allow(clippy::too_many_arguments)]
    pub async fn complete_local_oauth_authorization(
        &self,
        context: &VaultContext,
        request_handle: &str,
        username: &str,
        password: &SecretString,
        source_ip: Option<&str>,
        issuer: &str,
    ) -> Result<LocalOAuthAuthorizationResult, AuthError> {
        let request = self
            .find_authorization_request(context, request_handle)
            .await?
            .ok_or(AuthError::InvalidCredential)?;
        let now = now_millis()?;
        if request.expires_at <= now {
            return Err(AuthError::Expired);
        }
        let normalized = normalize_username(username)?;
        let limiter_key = format!(
            "oauth\0{}\0{}\0{}",
            source_ip.unwrap_or("unknown"),
            context.id(),
            normalized
        );
        self.limiter.check(&limiter_key, now)?;
        let user = self.repository.get_local_oauth_user(context).await?;
        let hash = match user.as_ref() {
            Some(user) if user.enabled && user.username == normalized => user.password_hash.clone(),
            _ => self.dummy_password_hash().await?,
        };
        let verified = self.verify_password(&hash, password).await?;
        let Some(user) = user.filter(|user| user.enabled && user.username == normalized) else {
            self.limiter.failure(&limiter_key, now_millis()?);
            return Err(AuthError::InvalidCredential);
        };
        if !verified.valid {
            self.limiter.failure(&limiter_key, now_millis()?);
            return Err(AuthError::InvalidCredential);
        }
        self.limiter.success(&limiter_key);
        let code = generate_bearer_token(CODE_LABEL);
        let digest = self
            .keys
            .keyed_digest(CODE_DIGEST_PURPOSE, code.expose_secret().as_bytes());
        let expires_at = now.saturating_add(duration_millis(AUTHORIZATION_CODE_LIFETIME)?);
        match self
            .repository
            .complete_oauth_authorization_request(
                context,
                request.id,
                user.id,
                NewOAuthAuthorizationCode {
                    id: OAuthAuthorizationCodeId::new(),
                    code_digest: &digest,
                    digest_key_version: self.keys.current_version(),
                    created_at: now,
                    expires_at,
                },
                now,
            )
            .await
        {
            Ok(()) => {}
            Err(StateError::InvalidInput(_)) => return Err(AuthError::InvalidCredential),
            Err(error) => return Err(error.into()),
        }
        Ok(LocalOAuthAuthorizationResult {
            redirect_uri: request.redirect_uri,
            code,
            state: request.state,
            issuer: issuer.to_owned(),
        })
    }

    /// Reload one still-valid prompt after a failed password attempt without
    /// creating another authorization transaction.
    pub async fn local_oauth_authorization_prompt(
        &self,
        context: &VaultContext,
        request_handle: &str,
    ) -> Result<LocalOAuthAuthorizationPrompt, AuthError> {
        let request = self
            .find_authorization_request(context, request_handle)
            .await?
            .ok_or(AuthError::InvalidCredential)?;
        if request.expires_at <= now_millis()? {
            return Err(AuthError::Expired);
        }
        let client = self
            .repository
            .get_oauth_client(request.client_id)
            .await?
            .filter(|client| client.revoked_at.is_none())
            .ok_or(AuthError::InvalidCredential)?;
        let granted_scopes = parse_local_oauth_scopes(&request.scopes_json)?;
        Ok(LocalOAuthAuthorizationPrompt {
            request_handle: BearerToken::new(request_handle.to_owned()),
            client_name: client.client_name,
            resource: request.resource,
            scopes: granted_scopes.application,
            offline_access: granted_scopes.offline_access,
            expires_at: request.expires_at,
        })
    }

    /// Exchange one code with its exact client, redirect, resource and PKCE
    /// verifier. The code consume and token inserts are one SQL transaction.
    pub async fn exchange_local_oauth_code(
        &self,
        context: &VaultContext,
        input: LocalOAuthCodeExchange<'_>,
    ) -> Result<LocalOAuthTokenIssue, AuthError> {
        self.ensure_persistent_master_key().await?;
        if !valid_pkce_verifier(input.code_verifier) {
            return Err(AuthError::OAuthTokenInvalid);
        }
        let client_id = OAuthClientId::parse(input.client_id)?;
        let code = self
            .find_authorization_code(context, input.code)
            .await?
            .ok_or(AuthError::OAuthTokenInvalid)?;
        let now = now_millis()?;
        if code.consumed_at.is_some()
            || code.expires_at <= now
            || code.client_id != client_id
            || code.redirect_uri != input.redirect_uri
            || code.resource != input.resource
            || !pkce_matches(input.code_verifier, &code.code_challenge)
        {
            return Err(AuthError::OAuthTokenInvalid);
        }
        let client = self
            .repository
            .get_oauth_client(client_id)
            .await?
            .filter(|client| client.revoked_at.is_none())
            .ok_or(AuthError::OAuthTokenInvalid)?;
        ensure_client_capabilities(&client).map_err(|_| AuthError::OAuthTokenInvalid)?;
        let user = self
            .repository
            .get_local_oauth_user(context)
            .await?
            .filter(|user| user.enabled && user.id == code.user_id)
            .ok_or(AuthError::OAuthTokenInvalid)?;
        let granted_scopes = parse_local_oauth_scopes(&code.scopes_json)?;
        ensure_scopes_allowed(
            &granted_scopes.application,
            &parse_scopes(&user.scopes_json)?,
        )?;
        let issue = self.new_token_issue(&granted_scopes, now)?;
        let access_digest = self.keys.keyed_digest(
            ACCESS_DIGEST_PURPOSE,
            issue.access_token.expose_secret().as_bytes(),
        );
        let refresh_digest = self.keys.keyed_digest(
            REFRESH_DIGEST_PURPOSE,
            issue.refresh_token.expose_secret().as_bytes(),
        );
        let family_id = OAuthTokenFamilyId::new();
        match self
            .repository
            .consume_oauth_code_and_insert_tokens(
                context,
                code.id,
                NewOAuthAccessToken {
                    id: OAuthAccessTokenId::new(),
                    family_id,
                    token_prefix: &token_prefix(&issue.access_token, TOKEN_LOOKUP_PREFIX),
                    token_digest: &access_digest,
                    digest_key_version: self.keys.current_version(),
                    created_at: now,
                    expires_at: now.saturating_add(duration_millis(ACCESS_TOKEN_LIFETIME)?),
                    scopes_json: None,
                },
                NewOAuthRefreshToken {
                    id: OAuthRefreshTokenId::new(),
                    family_id,
                    token_prefix: &token_prefix(&issue.refresh_token, TOKEN_LOOKUP_PREFIX),
                    token_digest: &refresh_digest,
                    digest_key_version: self.keys.current_version(),
                    created_at: now,
                    expires_at: now.saturating_add(duration_millis(REFRESH_TOKEN_LIFETIME)?),
                    scopes_json: None,
                },
                now,
            )
            .await
        {
            Ok(()) => {}
            Err(StateError::InvalidInput(_)) => return Err(AuthError::OAuthTokenInvalid),
            Err(error) => return Err(error.into()),
        }
        Ok(issue)
    }

    /// Rotate a refresh token. A duplicate retry inside the bounded grace
    /// window is rejected without destroying the concurrently issued pair;
    /// delayed replay revokes the complete token family.
    pub async fn refresh_local_oauth_token(
        &self,
        context: &VaultContext,
        input: LocalOAuthRefreshExchange<'_>,
    ) -> Result<LocalOAuthTokenIssue, AuthError> {
        self.refresh_local_oauth_token_at(context, input, now_millis()?)
            .await
    }

    async fn refresh_local_oauth_token_at(
        &self,
        context: &VaultContext,
        input: LocalOAuthRefreshExchange<'_>,
        now: i64,
    ) -> Result<LocalOAuthTokenIssue, AuthError> {
        self.ensure_persistent_master_key().await?;
        let client_id = OAuthClientId::parse(input.client_id)?;
        let record = self
            .find_refresh_token(context, input.refresh_token)
            .await?
            .ok_or(AuthError::OAuthTokenInvalid)?;
        if record.revoked_at.is_some()
            || record.client_id != client_id
            || record.resource != input.resource
        {
            return Err(AuthError::OAuthTokenInvalid);
        }
        if let Some(rotated_at) = record.rotated_at {
            self.revoke_refresh_family_if_replay_is_late(
                context,
                record.family_id,
                rotated_at,
                now,
            )
            .await?;
            return Err(AuthError::OAuthTokenInvalid);
        }
        if record.expires_at <= now {
            return Err(AuthError::OAuthTokenInvalid);
        }
        let client = self
            .repository
            .get_oauth_client(client_id)
            .await?
            .filter(|client| client.revoked_at.is_none())
            .ok_or(AuthError::OAuthTokenInvalid)?;
        ensure_client_capabilities(&client).map_err(|_| AuthError::OAuthTokenInvalid)?;
        let user = self
            .repository
            .get_local_oauth_user(context)
            .await?
            .filter(|user| user.enabled && user.id == record.user_id)
            .ok_or(AuthError::OAuthTokenInvalid)?;
        let original_scopes = parse_local_oauth_scopes(&record.scopes_json)?;
        let granted_scopes = requested_refresh_scopes(input.scope, &original_scopes)?;
        ensure_scopes_allowed(
            &granted_scopes.application,
            &parse_scopes(&user.scopes_json)?,
        )?;
        let narrowed_scopes_json = local_oauth_scopes_json(&granted_scopes)?;
        let issue = self.new_token_issue(&granted_scopes, now)?;
        let access_digest = self.keys.keyed_digest(
            ACCESS_DIGEST_PURPOSE,
            issue.access_token.expose_secret().as_bytes(),
        );
        let refresh_digest = self.keys.keyed_digest(
            REFRESH_DIGEST_PURPOSE,
            issue.refresh_token.expose_secret().as_bytes(),
        );
        match self
            .repository
            .rotate_oauth_refresh_token(
                context,
                record.id,
                NewOAuthAccessToken {
                    id: OAuthAccessTokenId::new(),
                    family_id: record.family_id,
                    token_prefix: &token_prefix(&issue.access_token, TOKEN_LOOKUP_PREFIX),
                    token_digest: &access_digest,
                    digest_key_version: self.keys.current_version(),
                    created_at: now,
                    expires_at: now.saturating_add(duration_millis(ACCESS_TOKEN_LIFETIME)?),
                    scopes_json: Some(&narrowed_scopes_json),
                },
                NewOAuthRefreshToken {
                    id: OAuthRefreshTokenId::new(),
                    family_id: record.family_id,
                    token_prefix: &token_prefix(&issue.refresh_token, TOKEN_LOOKUP_PREFIX),
                    token_digest: &refresh_digest,
                    digest_key_version: self.keys.current_version(),
                    created_at: now,
                    expires_at: now.saturating_add(duration_millis(REFRESH_TOKEN_LIFETIME)?),
                    scopes_json: Some(&narrowed_scopes_json),
                },
                now,
            )
            .await
        {
            Ok(()) => {}
            Err(StateError::InvalidInput(_)) => {
                if let Some(latest) = self
                    .find_refresh_token(context, input.refresh_token)
                    .await?
                    .filter(|latest| latest.family_id == record.family_id)
                    && let Some(rotated_at) = latest.rotated_at
                {
                    self.revoke_refresh_family_if_replay_is_late(
                        context,
                        latest.family_id,
                        rotated_at,
                        now,
                    )
                    .await?;
                }
                return Err(AuthError::OAuthTokenInvalid);
            }
            Err(error) => return Err(error.into()),
        }
        Ok(issue)
    }

    async fn revoke_refresh_family_if_replay_is_late(
        &self,
        context: &VaultContext,
        family_id: OAuthTokenFamilyId,
        rotated_at: i64,
        now: i64,
    ) -> Result<(), AuthError> {
        if refresh_replay_is_outside_grace(rotated_at, now)? {
            self.repository
                .revoke_oauth_token_family(context, family_id, now)
                .await?;
        }
        Ok(())
    }

    /// Authenticate one locally issued opaque token at an exact Vault MCP
    /// resource endpoint.
    pub async fn authenticate_local_oauth(
        &self,
        context: &VaultContext,
        token: &str,
        expected_resource: &str,
        required_scopes: &[Scope],
        now: Option<i64>,
    ) -> Result<AuthPrincipal, AuthError> {
        let record = self
            .find_access_token(context, token)
            .await?
            .ok_or(AuthError::InvalidCredential)?;
        let current = now.unwrap_or(now_millis()?);
        if record.revoked_at.is_some()
            || record.expires_at <= current
            || record.vault_id != context.id()
            || record.resource != expected_resource
        {
            return Err(AuthError::InvalidCredential);
        }
        let user = self
            .repository
            .get_local_oauth_user(context)
            .await?
            .filter(|user| user.enabled && user.id == record.user_id)
            .ok_or(AuthError::InvalidCredential)?;
        let granted_scopes = parse_local_oauth_scopes(&record.scopes_json)
            .map_err(|_| AuthError::InvalidCredential)?;
        let scopes = granted_scopes.application;
        ensure_scopes_allowed(
            &scopes,
            &parse_scopes(&user.scopes_json).map_err(|_| AuthError::InvalidCredential)?,
        )?;
        if required_scopes.iter().any(|scope| !scopes.contains(*scope)) {
            return Err(AuthError::ScopeDenied);
        }
        self.repository
            .touch_oauth_access_token(context, record.id, current)
            .await?;
        Ok(AuthPrincipal {
            actor: Actor::identified(
                ActorType::McpOAuthSubject,
                ActorId::new(&user.id.to_string())?,
            ),
            vault_id: Some(context.id()),
            credential_id: None,
            permissions: scopes.permissions(),
            scopes,
        })
    }

    async fn find_authorization_request(
        &self,
        context: &VaultContext,
        handle: &str,
    ) -> Result<Option<OAuthAuthorizationRequestRecord>, AuthError> {
        if !handle.starts_with(REQUEST_LABEL) {
            return Ok(None);
        }
        for version in self.keys.versions() {
            let digest =
                self.keys
                    .keyed_digest_for(version, REQUEST_DIGEST_PURPOSE, handle.as_bytes());
            if let Some(record) = self
                .repository
                .find_oauth_authorization_request(context, &digest, version)
                .await?
            {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    async fn find_authorization_code(
        &self,
        context: &VaultContext,
        code: &str,
    ) -> Result<Option<OAuthAuthorizationCodeRecord>, AuthError> {
        if !code.starts_with(CODE_LABEL) {
            return Ok(None);
        }
        for version in self.keys.versions() {
            let digest = self
                .keys
                .keyed_digest_for(version, CODE_DIGEST_PURPOSE, code.as_bytes());
            if let Some(record) = self
                .repository
                .find_oauth_authorization_code(context, &digest, version)
                .await?
            {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    async fn find_access_token(
        &self,
        context: &VaultContext,
        token: &str,
    ) -> Result<Option<mcp_vault_state::OAuthAccessTokenRecord>, AuthError> {
        if !token.starts_with(ACCESS_LABEL) {
            return Ok(None);
        }
        let prefix = token.chars().take(TOKEN_LOOKUP_PREFIX).collect::<String>();
        for version in self.keys.versions() {
            let digest =
                self.keys
                    .keyed_digest_for(version, ACCESS_DIGEST_PURPOSE, token.as_bytes());
            if let Some(record) = self
                .repository
                .find_oauth_access_token(context, &prefix, &digest, version)
                .await?
            {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    async fn find_refresh_token(
        &self,
        context: &VaultContext,
        token: &str,
    ) -> Result<Option<OAuthRefreshTokenRecord>, AuthError> {
        if !token.starts_with(REFRESH_LABEL) {
            return Ok(None);
        }
        let prefix = token.chars().take(TOKEN_LOOKUP_PREFIX).collect::<String>();
        for version in self.keys.versions() {
            let digest =
                self.keys
                    .keyed_digest_for(version, REFRESH_DIGEST_PURPOSE, token.as_bytes());
            if let Some(record) = self
                .repository
                .find_oauth_refresh_token(context, &prefix, &digest, version)
                .await?
            {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    fn new_token_issue(
        &self,
        granted_scopes: &LocalOAuthGrantedScopes,
        _now: i64,
    ) -> Result<LocalOAuthTokenIssue, AuthError> {
        Ok(LocalOAuthTokenIssue {
            access_token: generate_bearer_token(ACCESS_LABEL),
            refresh_token: generate_bearer_token(REFRESH_LABEL),
            expires_in: ACCESS_TOKEN_LIFETIME.as_secs(),
            scopes: granted_scopes.application.clone(),
            offline_access: granted_scopes.offline_access,
        })
    }
}

fn local_user_view(record: OAuthLocalUserRecord) -> Result<LocalOAuthUser, AuthError> {
    Ok(LocalOAuthUser {
        id: record.id,
        vault_id: record.vault_id,
        username: record.username,
        scopes: parse_scopes(&record.scopes_json)?,
        enabled: record.enabled,
        password_changed_at: record.password_changed_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

fn client_registration_view(
    record: OAuthClientRecord,
) -> Result<LocalOAuthClientRegistrationResult, AuthError> {
    Ok(LocalOAuthClientRegistrationResult {
        client_id: record.id,
        client_name: record.client_name,
        redirect_uris: serde_json::from_str(&record.redirect_uris_json)
            .map_err(|_| AuthError::OAuthConfiguration)?,
        grant_types: serde_json::from_str(&record.grant_types_json)
            .map_err(|_| AuthError::OAuthConfiguration)?,
        response_types: serde_json::from_str(&record.response_types_json)
            .map_err(|_| AuthError::OAuthConfiguration)?,
        token_endpoint_auth_method: record.token_endpoint_auth_method,
        client_id_issued_at: record.created_at / 1000,
    })
}

fn ensure_client_capabilities(client: &OAuthClientRecord) -> Result<(), AuthError> {
    let grants: Vec<String> = serde_json::from_str(&client.grant_types_json)
        .map_err(|_| AuthError::OAuthConfiguration)?;
    let responses: Vec<String> = serde_json::from_str(&client.response_types_json)
        .map_err(|_| AuthError::OAuthConfiguration)?;
    if client.token_endpoint_auth_method != "none"
        || !["authorization_code", "refresh_token"]
            .iter()
            .all(|required| grants.iter().any(|value| value == required))
        || responses != ["code"]
    {
        return Err(AuthError::OAuthConfiguration);
    }
    Ok(())
}

fn validate_client_name(value: &str) -> Result<(), AuthError> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn validate_redirect_uris(values: Vec<String>) -> Result<Vec<String>, AuthError> {
    if values.is_empty() || values.len() > 16 {
        return Err(AuthError::OAuthConfiguration);
    }
    let mut unique = Vec::with_capacity(values.len());
    for value in values {
        if value.len() > 2048 || unique.contains(&value) {
            return Err(AuthError::OAuthConfiguration);
        }
        let url = Url::parse(&value).map_err(|_| AuthError::OAuthConfiguration)?;
        if url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.host_str().is_none()
            || !redirect_scheme_allowed(&url)
        {
            return Err(AuthError::OAuthConfiguration);
        }
        unique.push(value);
    }
    Ok(unique)
}

fn redirect_scheme_allowed(url: &Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .ok()
            .is_some_and(|address| address.is_loopback()),
        None => false,
    }
}

fn validate_exact_values(
    values: Option<Vec<String>>,
    defaults: &[&str],
    required: &str,
) -> Result<Vec<String>, AuthError> {
    let values =
        values.unwrap_or_else(|| defaults.iter().map(|value| (*value).to_owned()).collect());
    if values.is_empty()
        || values.len() > defaults.len()
        || !values.iter().any(|value| value == required)
        || values
            .iter()
            .any(|value| !defaults.iter().any(|allowed| value == allowed))
    {
        return Err(AuthError::OAuthConfiguration);
    }
    let mut unique = Vec::with_capacity(values.len());
    for value in values {
        if unique.contains(&value) {
            return Err(AuthError::OAuthConfiguration);
        }
        unique.push(value);
    }
    Ok(unique)
}

fn validate_oauth_state(state: Option<&str>) -> Result<(), AuthError> {
    if state.is_some_and(|value| value.len() > 2048 || value.chars().any(char::is_control)) {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn requested_authorization_scopes(
    value: Option<&str>,
    allowed: &ScopeSet,
) -> Result<LocalOAuthGrantedScopes, AuthError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(LocalOAuthGrantedScopes {
            application: allowed.clone(),
            offline_access: false,
        });
    };
    let requested = parse_requested_local_oauth_scopes(value)?;
    ensure_scopes_allowed(&requested.application, allowed)?;
    Ok(requested)
}

fn requested_refresh_scopes(
    value: Option<&str>,
    original: &LocalOAuthGrantedScopes,
) -> Result<LocalOAuthGrantedScopes, AuthError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(original.clone());
    };
    let requested = parse_requested_local_oauth_scopes(value)?;
    if requested.offline_access && !original.offline_access {
        return Err(AuthError::ScopeDenied);
    }
    ensure_scopes_allowed(&requested.application, &original.application)?;
    Ok(LocalOAuthGrantedScopes {
        application: requested.application,
        offline_access: original.offline_access,
    })
}

fn parse_requested_local_oauth_scopes(value: &str) -> Result<LocalOAuthGrantedScopes, AuthError> {
    if value.len() > 1024 {
        return Err(AuthError::ScopeDenied);
    }
    let mut requested = ScopeSet::new();
    let mut offline_access = false;
    for item in value.split_ascii_whitespace() {
        if item == LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE {
            offline_access = true;
        } else {
            requested.insert(Scope::from_str(item).map_err(|_| AuthError::ScopeDenied)?);
        }
    }
    if requested.iter().next().is_none() {
        return Err(AuthError::ScopeDenied);
    }
    Ok(LocalOAuthGrantedScopes {
        application: requested,
        offline_access,
    })
}

fn parse_local_oauth_scopes(value: &str) -> Result<LocalOAuthGrantedScopes, AuthError> {
    let values: Vec<String> =
        serde_json::from_str(value).map_err(|_| AuthError::OAuthConfiguration)?;
    let mut application = ScopeSet::new();
    let mut offline_access = false;
    for value in values {
        if value == LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE {
            offline_access = true;
        } else {
            application.insert(Scope::from_str(&value).map_err(|_| AuthError::OAuthConfiguration)?);
        }
    }
    if application.iter().next().is_none() {
        return Err(AuthError::OAuthConfiguration);
    }
    Ok(LocalOAuthGrantedScopes {
        application,
        offline_access,
    })
}

fn ensure_scopes_allowed(scopes: &ScopeSet, allowed: &ScopeSet) -> Result<(), AuthError> {
    if scopes.iter().any(|scope| !allowed.contains(*scope)) {
        Err(AuthError::ScopeDenied)
    } else {
        Ok(())
    }
}

fn scopes_json(scopes: &ScopeSet) -> Result<String, AuthError> {
    serde_json::to_string(&scopes.iter().map(ToString::to_string).collect::<Vec<_>>())
        .map_err(|_| AuthError::InvalidInput)
}

fn local_oauth_scopes_json(scopes: &LocalOAuthGrantedScopes) -> Result<String, AuthError> {
    let mut values = scopes
        .application
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if scopes.offline_access {
        values.push(LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE.to_owned());
    }
    serde_json::to_string(&values).map_err(|_| AuthError::InvalidInput)
}

fn refresh_replay_is_outside_grace(rotated_at: i64, now: i64) -> Result<bool, AuthError> {
    Ok(now.saturating_sub(rotated_at) > duration_millis(REFRESH_TOKEN_REUSE_GRACE)?)
}

fn valid_pkce_challenge(value: &str) -> bool {
    value.len() == 43 && value.bytes().all(is_unreserved)
}

fn valid_pkce_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len()) && value.bytes().all(is_unreserved)
}

fn is_unreserved(value: u8) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, b'-' | b'.' | b'_' | b'~')
}

fn pkce_matches(verifier: &str, expected: &str) -> bool {
    let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    bool::from(actual.as_bytes().ct_eq(expected.as_bytes()))
}

fn duration_millis(duration: Duration) -> Result<i64, AuthError> {
    i64::try_from(duration.as_millis()).map_err(|_| AuthError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use mcp_vault_domain::{Revision, Scope, ScopeSet, VaultContext, VaultId, VaultSlug};
    use mcp_vault_state::{StateStore, VaultStatus};

    use super::{
        LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE, LocalOAuthAuthorizationInput,
        LocalOAuthClientRegistration, LocalOAuthClientRegistrationResult, LocalOAuthCodeExchange,
        LocalOAuthRefreshExchange, LocalOAuthTokenIssue, REFRESH_TOKEN_LIFETIME,
        REFRESH_TOKEN_REUSE_GRACE, duration_millis, parse_requested_local_oauth_scopes,
        pkce_matches,
    };
    use crate::{AuthError, AuthService, MasterKeyRing, SecretString};

    async fn setup() -> (AuthService, VaultContext, VaultContext) {
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from("/srv/work"),
            Revision::new(1),
        )
        .unwrap();
        let other = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("other").unwrap(),
            PathBuf::from("/srv/other"),
            Revision::new(1),
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        state
            .vaults()
            .insert(&other, "Other", VaultStatus::Active)
            .await
            .unwrap();
        let service = AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[42_u8; 32]).unwrap(),
        );
        service
            .configure_local_oauth_user(
                &context,
                "chatgpt",
                &SecretString::new("correct horse battery staple"),
                [Scope::VaultDiscover, Scope::VaultRead, Scope::MemoryRead]
                    .into_iter()
                    .collect(),
            )
            .await
            .unwrap();
        (service, context, other)
    }

    fn challenge(verifier: &str) -> String {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }

    async fn issue_grant(
        service: &AuthService,
        context: &VaultContext,
        scope: &str,
        source_ip: &str,
    ) -> (
        LocalOAuthClientRegistrationResult,
        String,
        LocalOAuthTokenIssue,
    ) {
        let client = service
            .register_local_oauth_client(
                LocalOAuthClientRegistration {
                    client_name: Some("ChatGPT".to_owned()),
                    redirect_uris: vec![
                        "https://chatgpt.com/connector_platform_oauth_redirect".to_owned(),
                    ],
                    grant_types: None,
                    response_types: None,
                    token_endpoint_auth_method: None,
                },
                Some(source_ip),
            )
            .await
            .unwrap();
        let resource = "https://vault.example.test/mcp/v1/vaults/work".to_owned();
        let verifier = "g".repeat(64);
        let prompt = service
            .begin_local_oauth_authorization(
                context,
                &resource,
                LocalOAuthAuthorizationInput {
                    response_type: "code".to_owned(),
                    client_id: client.client_id.to_string(),
                    redirect_uri: client.redirect_uris[0].clone(),
                    scope: Some(scope.to_owned()),
                    state: None,
                    code_challenge: challenge(&verifier),
                    code_challenge_method: "S256".to_owned(),
                    resource: resource.clone(),
                },
            )
            .await
            .unwrap();
        let authorization = service
            .complete_local_oauth_authorization(
                context,
                prompt.request_handle.expose_secret(),
                "chatgpt",
                &SecretString::new("correct horse battery staple"),
                Some(source_ip),
                "https://vault.example.test",
            )
            .await
            .unwrap();
        let issue = service
            .exchange_local_oauth_code(
                context,
                LocalOAuthCodeExchange {
                    code: authorization.code.expose_secret(),
                    client_id: &client.client_id.to_string(),
                    redirect_uri: &client.redirect_uris[0],
                    code_verifier: &verifier,
                    resource: &resource,
                },
            )
            .await
            .unwrap();
        (client, resource, issue)
    }

    #[tokio::test]
    async fn local_oauth_code_pkce_refresh_and_replay_are_vault_bound() {
        let (service, context, other) = setup().await;
        let client = service
            .register_local_oauth_client(
                LocalOAuthClientRegistration {
                    client_name: Some("ChatGPT".to_owned()),
                    redirect_uris: vec![
                        "https://chatgpt.com/connector_platform_oauth_redirect".to_owned(),
                    ],
                    grant_types: None,
                    response_types: None,
                    token_endpoint_auth_method: None,
                },
                Some("127.0.0.1"),
            )
            .await
            .unwrap();
        let resource = "https://vault.example.test/mcp/v1/vaults/work";
        let verifier = "v".repeat(64);
        let prompt = service
            .begin_local_oauth_authorization(
                &context,
                resource,
                LocalOAuthAuthorizationInput {
                    response_type: "code".to_owned(),
                    client_id: client.client_id.to_string(),
                    redirect_uri: client.redirect_uris[0].clone(),
                    scope: Some("vault:discover vault:read memory:read offline_access".to_owned()),
                    state: Some("opaque-state".to_owned()),
                    code_challenge: challenge(&verifier),
                    code_challenge_method: "S256".to_owned(),
                    resource: resource.to_owned(),
                },
            )
            .await
            .unwrap();
        assert!(prompt.offline_access);
        assert_eq!(prompt.scopes.iter().count(), 3);
        assert!(
            prompt
                .request_handle
                .expose_secret()
                .starts_with("mcpv_oauth_req_")
        );
        let authorization = service
            .complete_local_oauth_authorization(
                &context,
                prompt.request_handle.expose_secret(),
                "CHATGPT",
                &SecretString::new("correct horse battery staple"),
                Some("127.0.0.1"),
                "https://vault.example.test",
            )
            .await
            .unwrap();
        assert_eq!(authorization.state.as_deref(), Some("opaque-state"));
        let first_code = authorization.code.expose_secret().to_owned();
        let retried_authorization = service
            .complete_local_oauth_authorization(
                &context,
                prompt.request_handle.expose_secret(),
                "CHATGPT",
                &SecretString::new("correct horse battery staple"),
                Some("127.0.0.1"),
                "https://vault.example.test",
            )
            .await
            .unwrap();
        let code = retried_authorization.code.expose_secret().to_owned();
        assert_ne!(code, first_code);
        assert_eq!(retried_authorization.state.as_deref(), Some("opaque-state"));
        let issue = service
            .exchange_local_oauth_code(
                &context,
                LocalOAuthCodeExchange {
                    code: &code,
                    client_id: &client.client_id.to_string(),
                    redirect_uri: &client.redirect_uris[0],
                    code_verifier: &verifier,
                    resource,
                },
            )
            .await
            .unwrap();
        assert!(
            issue
                .access_token
                .expose_secret()
                .starts_with("mcpv_oauth_")
        );
        assert!(!issue.access_token.expose_secret().contains('.'));
        assert!(issue.offline_access);
        service
            .authenticate_local_oauth(
                &context,
                issue.access_token.expose_secret(),
                resource,
                &[Scope::VaultRead],
                None,
            )
            .await
            .unwrap();
        assert!(
            service
                .authenticate_local_oauth(
                    &other,
                    issue.access_token.expose_secret(),
                    "https://vault.example.test/mcp/v1/vaults/other",
                    &[],
                    None,
                )
                .await
                .is_err()
        );
        assert!(
            service
                .exchange_local_oauth_code(
                    &context,
                    LocalOAuthCodeExchange {
                        code: &code,
                        client_id: &client.client_id.to_string(),
                        redirect_uri: &client.redirect_uris[0],
                        code_verifier: &verifier,
                        resource,
                    },
                )
                .await
                .is_err()
        );

        let old_refresh = issue.refresh_token.expose_secret().to_owned();
        let initial_refresh = service
            .find_refresh_token(&context, &old_refresh)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            initial_refresh.expires_at - initial_refresh.created_at,
            duration_millis(REFRESH_TOKEN_LIFETIME).unwrap()
        );
        let rotation_time = initial_refresh.created_at.saturating_add(1);
        let rotated = service
            .refresh_local_oauth_token_at(
                &context,
                LocalOAuthRefreshExchange {
                    refresh_token: &old_refresh,
                    client_id: &client.client_id.to_string(),
                    resource,
                    scope: Some("vault:discover vault:read"),
                },
                rotation_time,
            )
            .await
            .unwrap();
        assert!(!rotated.scopes.contains(Scope::MemoryRead));
        assert!(rotated.offline_access);
        let rotated_refresh = service
            .find_refresh_token(&context, rotated.refresh_token.expose_secret())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rotated_refresh.expires_at,
            rotation_time + duration_millis(REFRESH_TOKEN_LIFETIME).unwrap()
        );
        let grace_boundary = rotation_time + duration_millis(REFRESH_TOKEN_REUSE_GRACE).unwrap();
        assert!(
            service
                .refresh_local_oauth_token_at(
                    &context,
                    LocalOAuthRefreshExchange {
                        refresh_token: &old_refresh,
                        client_id: &client.client_id.to_string(),
                        resource,
                        scope: None,
                    },
                    grace_boundary,
                )
                .await
                .is_err()
        );
        service
            .authenticate_local_oauth(
                &context,
                rotated.access_token.expose_secret(),
                resource,
                &[],
                Some(grace_boundary),
            )
            .await
            .unwrap();
        assert!(
            service
                .refresh_local_oauth_token_at(
                    &context,
                    LocalOAuthRefreshExchange {
                        refresh_token: &old_refresh,
                        client_id: &client.client_id.to_string(),
                        resource,
                        scope: None,
                    },
                    grace_boundary.saturating_add(1),
                )
                .await
                .is_err()
        );
        assert!(
            service
                .authenticate_local_oauth(
                    &context,
                    rotated.access_token.expose_secret(),
                    resource,
                    &[],
                    Some(grace_boundary.saturating_add(1)),
                )
                .await
                .is_err(),
            "refresh replay outside the grace window must revoke the token family"
        );
    }

    #[tokio::test]
    async fn concurrent_refresh_rejects_one_request_without_revoking_the_winner() {
        let (service, context, _other) = setup().await;
        let (client, resource, issue) =
            issue_grant(&service, &context, "vault:read offline_access", "127.0.0.3").await;
        let refresh_token = issue.refresh_token.expose_secret().to_owned();
        let client_id = client.client_id.to_string();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_barrier = Arc::clone(&barrier);
        let second_barrier = Arc::clone(&barrier);
        let first = async {
            first_barrier.wait().await;
            service
                .refresh_local_oauth_token(
                    &context,
                    LocalOAuthRefreshExchange {
                        refresh_token: &refresh_token,
                        client_id: &client_id,
                        resource: &resource,
                        scope: None,
                    },
                )
                .await
        };
        let second = async {
            second_barrier.wait().await;
            service
                .refresh_local_oauth_token(
                    &context,
                    LocalOAuthRefreshExchange {
                        refresh_token: &refresh_token,
                        client_id: &client_id,
                        resource: &resource,
                        scope: None,
                    },
                )
                .await
        };
        let (_, first, second) = tokio::join!(barrier.wait(), first, second);
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert!(
            [first.as_ref(), second.as_ref()]
                .into_iter()
                .filter_map(|result| result.err())
                .all(|error| matches!(error, AuthError::OAuthTokenInvalid))
        );
        let winner = first.or(second).unwrap();
        assert!(winner.offline_access);
        service
            .authenticate_local_oauth(
                &context,
                winner.access_token.expose_secret(),
                &resource,
                &[Scope::VaultRead],
                None,
            )
            .await
            .unwrap();
        service
            .find_refresh_token(&context, winner.refresh_token.expose_secret())
            .await
            .unwrap()
            .expect("the successful rotated refresh token must remain usable");
    }

    #[tokio::test]
    async fn legacy_grant_refreshes_but_cannot_add_offline_access() {
        let (service, context, _other) = setup().await;
        let (client, resource, issue) =
            issue_grant(&service, &context, "vault:read", "127.0.0.4").await;
        assert!(!issue.offline_access);
        let refresh_token = issue.refresh_token.expose_secret().to_owned();
        let client_id = client.client_id.to_string();
        let expanded = service
            .refresh_local_oauth_token(
                &context,
                LocalOAuthRefreshExchange {
                    refresh_token: &refresh_token,
                    client_id: &client_id,
                    resource: &resource,
                    scope: Some("vault:read offline_access"),
                },
            )
            .await;
        assert!(matches!(expanded, Err(AuthError::ScopeDenied)));

        let rotated = service
            .refresh_local_oauth_token(
                &context,
                LocalOAuthRefreshExchange {
                    refresh_token: &refresh_token,
                    client_id: &client_id,
                    resource: &resource,
                    scope: None,
                },
            )
            .await
            .unwrap();
        assert!(!rotated.offline_access);
        assert_eq!(rotated.scopes, ScopeSet::from_iter([Scope::VaultRead]));
    }

    #[test]
    fn local_oauth_scope_parser_rejects_unknown_or_permissionless_requests() {
        assert!(matches!(
            parse_requested_local_oauth_scopes("vault:read unknown:scope"),
            Err(AuthError::ScopeDenied)
        ));
        assert!(matches!(
            parse_requested_local_oauth_scopes(LOCAL_OAUTH_OFFLINE_ACCESS_SCOPE),
            Err(AuthError::ScopeDenied)
        ));
    }

    #[tokio::test]
    async fn local_oauth_rejects_redirect_pkce_and_user_rotation_reuses_no_token() {
        let (service, context, _other) = setup().await;
        let client = service
            .register_local_oauth_client(
                LocalOAuthClientRegistration {
                    client_name: Some("ChatGPT".to_owned()),
                    redirect_uris: vec![
                        "https://chatgpt.com/connector_platform_oauth_redirect".to_owned(),
                    ],
                    grant_types: Some(vec![
                        "authorization_code".to_owned(),
                        "refresh_token".to_owned(),
                    ]),
                    response_types: Some(vec!["code".to_owned()]),
                    token_endpoint_auth_method: Some("none".to_owned()),
                },
                Some("127.0.0.2"),
            )
            .await
            .unwrap();
        let resource = "https://vault.example.test/mcp/v1/vaults/work";
        let verifier = "x".repeat(64);
        let invalid = service
            .begin_local_oauth_authorization(
                &context,
                resource,
                LocalOAuthAuthorizationInput {
                    response_type: "code".to_owned(),
                    client_id: client.client_id.to_string(),
                    redirect_uri: "https://evil.example.test/callback".to_owned(),
                    scope: None,
                    state: None,
                    code_challenge: challenge(&verifier),
                    code_challenge_method: "S256".to_owned(),
                    resource: resource.to_owned(),
                },
            )
            .await;
        assert!(matches!(invalid, Err(AuthError::OAuthConfiguration)));
        assert!(pkce_matches(&verifier, &challenge(&verifier)));
        assert!(!pkce_matches(&"y".repeat(64), &challenge(&verifier)));

        let pending_before_rotation = service
            .begin_local_oauth_authorization(
                &context,
                resource,
                LocalOAuthAuthorizationInput {
                    response_type: "code".to_owned(),
                    client_id: client.client_id.to_string(),
                    redirect_uri: client.redirect_uris[0].clone(),
                    scope: Some("vault:read".to_owned()),
                    state: Some("must-not-survive-rotation".to_owned()),
                    code_challenge: challenge(&verifier),
                    code_challenge_method: "S256".to_owned(),
                    resource: resource.to_owned(),
                },
            )
            .await
            .unwrap();

        let user = service.local_oauth_user(&context).await.unwrap().unwrap();
        assert_eq!(user.username, "chatgpt");
        service
            .configure_local_oauth_user(
                &context,
                "chatgpt-new",
                &SecretString::new("another correct horse battery staple"),
                ScopeSet::from_iter([Scope::VaultRead]),
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .local_oauth_user(&context)
                .await
                .unwrap()
                .unwrap()
                .username,
            "chatgpt-new"
        );
        assert!(
            service
                .complete_local_oauth_authorization(
                    &context,
                    pending_before_rotation.request_handle.expose_secret(),
                    "chatgpt-new",
                    &SecretString::new("another correct horse battery staple"),
                    Some("127.0.0.2"),
                    "https://vault.example.test",
                )
                .await
                .is_err(),
            "password rotation must delete every prior browser authorization request"
        );
    }
}
