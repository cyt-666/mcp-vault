//! Application-level authentication and authorization services.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use http::{HeaderMap, Method};
use mcp_vault_domain::{
    Actor, ActorId, ActorType, AdminUserId, CredentialId, Permission, PermissionSet, Scope,
    ScopeSet, VaultContext,
};
use mcp_vault_state::{
    AdminSessionRecord, AdminUserRecord, AuthStateRepository, EncryptedSecretRecord,
    OAuthGrantRecord, OAuthIssuerRecord,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use crate::{
    error::AuthError,
    oauth::{JsonWebKeySet, OAuthPrincipal, parse_scopes, token_identity, validate_access_token},
    origin::OriginPolicy,
    password::PasswordPolicy,
    secret::{
        BearerToken, MasterKeyRing, SecretString, digest_bearer_token, generate_bearer_token,
        token_prefix,
    },
};

const PAT_LABEL: &str = "mcpv_pat_";
const SESSION_LABEL: &str = "mcpv_session_";
const CSRF_LABEL: &str = "mcpv_csrf_";
const PAT_DIGEST_PURPOSE: &str = "mcp-pat-digest";
const SESSION_DIGEST_PURPOSE: &str = "admin-session-digest";
const CSRF_DIGEST_PURPOSE: &str = "admin-csrf-digest";
const BOOTSTRAP_DIGEST_PURPOSE: &str = "bootstrap-token-digest";
const MAX_PASSWORD_WORKERS: usize = 4;

/// Session lifetime and cookie settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionPolicy {
    /// Idle lifetime after the last successful request.
    pub idle_timeout: Duration,
    /// Absolute lifetime from session creation.
    pub absolute_timeout: Duration,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30 * 60),
            absolute_timeout: Duration::from_secs(12 * 60 * 60),
        }
    }
}

/// Safe Admin session issuance result. Both secrets are shown only through
/// explicit accessors at the HTTP response boundary.
#[derive(Debug)]
pub struct AdminLogin {
    /// Admin identity.
    pub user_id: AdminUserId,
    /// Normalized username.
    pub username: String,
    /// Session row identifier for audit correlation.
    pub session_id: mcp_vault_domain::AdminSessionId,
    /// Opaque cookie value; never persisted in plaintext.
    pub session_token: BearerToken,
    /// Session-bound CSRF value; never persisted in plaintext.
    pub csrf_token: BearerToken,
    /// Absolute expiry timestamp in UTC Unix milliseconds.
    pub expires_at: i64,
}

/// An authenticated Admin principal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminPrincipal {
    /// Non-secret audit actor.
    pub actor: Actor,
    /// Stable identity ID.
    pub user_id: AdminUserId,
    /// Normalized username.
    pub username: String,
}

/// Validated first-run Admin material awaiting the atomic final insert.
/// Password-derived data remains private to the authentication boundary.
pub struct PreparedAdminSetup {
    username: String,
    password_hash: String,
}

impl std::fmt::Debug for PreparedAdminSetup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedAdminSetup")
            .field("username", &self.username)
            .field("password_hash", &"[REDACTED]")
            .finish()
    }
}

/// One-time WebDAV app-password issuance result.
#[derive(Debug)]
pub struct WebDavCredentialIssue {
    /// Stable credential ID.
    pub credential_id: CredentialId,
    /// Normalized Basic-auth username.
    pub username: String,
    /// Plaintext app password shown once.
    pub password: SecretString,
    /// Permissions bound to the Vault.
    pub permissions: PermissionSet,
    /// Optional UTC Unix millisecond expiry.
    pub expires_at: Option<i64>,
}

/// One-time MCP PAT issuance result.
#[derive(Debug)]
pub struct PatIssue {
    /// Stable credential ID.
    pub credential_id: CredentialId,
    /// Visible lookup prefix.
    pub token_prefix: String,
    /// Plaintext token shown once.
    pub token: BearerToken,
    /// Granted scopes.
    pub scopes: ScopeSet,
    /// Optional UTC Unix millisecond expiry.
    pub expires_at: Option<i64>,
}

/// Safe secret metadata for Admin responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretMetadata {
    /// Stable secret ID.
    pub id: mcp_vault_domain::SecretId,
    /// Secret purpose.
    pub purpose: String,
    /// Owner category.
    pub owner_type: String,
    /// Owner ID, if any.
    pub owner_id: Option<String>,
    /// Current key version.
    pub key_version: u32,
    /// Masked non-secret hint.
    pub hint: Option<String>,
}

/// OAuth issuer configuration accepted by the resource-server service.
#[derive(Clone, Eq, PartialEq)]
pub struct OAuthIssuerInput {
    /// Admin-visible name.
    pub name: String,
    /// Exact `iss` claim value.
    pub issuer_url: String,
    /// Optional discovery URL saved for a future refresh worker.
    pub discovery_url: Option<String>,
    /// Required JWT audience.
    pub audience: String,
    /// Required protected resource identifier.
    pub resource: String,
    /// Locally cached JWK set JSON.
    pub jwks_cache_json: String,
    /// Whether the issuer is enabled.
    pub enabled: bool,
}

impl std::fmt::Debug for OAuthIssuerInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthIssuerInput")
            .field("name", &self.name)
            .field("issuer_url", &self.issuer_url)
            .field("discovery_url", &self.discovery_url)
            .field("audience", &self.audience)
            .field("resource", &self.resource)
            .field("has_jwks_cache", &true)
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Application principal shared by later WebDAV/MCP adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthPrincipal {
    /// Audit actor identity.
    pub actor: Actor,
    /// Bound Vault, if this is a data-plane principal.
    pub vault_id: Option<mcp_vault_domain::VaultId>,
    /// Bound credential ID, if applicable.
    pub credential_id: Option<CredentialId>,
    /// Application permissions derived from the authentication grant.
    pub permissions: PermissionSet,
    /// Original protocol scopes, when present.
    pub scopes: ScopeSet,
}

/// Protocol-neutral authentication service.
#[derive(Clone)]
pub struct AuthService {
    repository: AuthStateRepository,
    keys: MasterKeyRing,
    password_policy: PasswordPolicy,
    session_policy: SessionPolicy,
    bootstrap_digest: Option<[u8; 32]>,
    managed_bootstrap_token_file: Option<PathBuf>,
    limiter: Arc<LoginRateLimiter>,
    password_workers: Arc<Semaphore>,
}

impl AuthService {
    /// Construct the service from the state boundary and loaded key ring.
    pub fn new(repository: AuthStateRepository, keys: MasterKeyRing) -> Self {
        Self {
            repository,
            keys,
            password_policy: PasswordPolicy::default(),
            session_policy: SessionPolicy::default(),
            bootstrap_digest: None,
            managed_bootstrap_token_file: None,
            limiter: Arc::new(LoginRateLimiter::default()),
            password_workers: Arc::new(Semaphore::new(MAX_PASSWORD_WORKERS)),
        }
    }

    /// Override password/session policies at the application boundary.
    pub fn with_policies(
        mut self,
        password_policy: PasswordPolicy,
        session_policy: SessionPolicy,
    ) -> Self {
        self.password_policy = password_policy;
        self.session_policy = session_policy;
        self
    }

    /// Bind a one-time bootstrap token without retaining its plaintext.
    pub fn with_bootstrap_token(mut self, token: &SecretString) -> Self {
        self.bootstrap_digest = Some(
            self.keys
                .keyed_digest(BOOTSTRAP_DIGEST_PURPOSE, token.as_bytes()),
        );
        self.managed_bootstrap_token_file = None;
        self
    }

    /// Bind an application-generated first-run token and remember only the
    /// managed file path needed for best-effort cleanup after setup commits.
    pub fn with_managed_bootstrap_token(mut self, token: &SecretString, path: PathBuf) -> Self {
        self.bootstrap_digest = Some(
            self.keys
                .keyed_digest(BOOTSTRAP_DIGEST_PURPOSE, token.as_bytes()),
        );
        self.managed_bootstrap_token_file = Some(path);
        self
    }

    /// Return the current master-key version for diagnostics.
    pub const fn current_key_version(&self) -> u32 {
        self.keys.current_version()
    }

    /// Return retained master-key version identifiers for backup compatibility
    /// checks. The key material itself never leaves the authentication boundary.
    pub fn key_version_ids(&self) -> Vec<u32> {
        self.keys.versions().collect()
    }

    async fn ensure_persistent_master_key(&self) -> Result<(), AuthError> {
        if !self.keys.is_persistent() {
            return Err(AuthError::MasterKeyUnavailable);
        }
        let version = self.keys.current_version();
        let digest = self.keys.installation_key_check();
        self.repository
            .insert_installation_key_check_if_absent(version, &digest)
            .await?;
        let stored = self
            .repository
            .get_installation_key_check(version)
            .await?
            .ok_or(AuthError::MasterKeyUnavailable)?;
        if !self.keys.matches_installation_key_check(&stored) {
            return Err(AuthError::MasterKeyUnavailable);
        }
        Ok(())
    }

    async fn hash_password(&self, password: &SecretString) -> Result<String, AuthError> {
        let permit = self
            .password_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AuthError::PasswordHash)?;
        let policy = self.password_policy;
        let password = password.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            policy.hash(&password)
        })
        .await
        .map_err(|_| AuthError::PasswordHash)?
    }

    async fn verify_password(
        &self,
        stored_hash: &str,
        password: &SecretString,
    ) -> Result<crate::PasswordVerification, AuthError> {
        let permit = self
            .password_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AuthError::PasswordHash)?;
        let policy = self.password_policy;
        let stored_hash = stored_hash.to_owned();
        let password = password.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            policy.verify(&stored_hash, &password)
        })
        .await
        .map_err(|_| AuthError::PasswordHash)?
    }

    async fn dummy_password_hash(&self) -> Result<String, AuthError> {
        let permit = self
            .password_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AuthError::PasswordHash)?;
        let policy = self.password_policy;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            dummy_password_hash(&policy)
        })
        .await
        .map_err(|_| AuthError::PasswordHash)?
    }

    /// Store a reversible secret bound to the supplied Vault context.
    pub async fn put_vault_secret(
        &self,
        context: &VaultContext,
        purpose: &str,
        owner_type: &str,
        secret: &SecretString,
    ) -> Result<SecretMetadata, AuthError> {
        self.repository.ensure_vault_context(context).await?;
        let owner_id = context.id().to_string();
        self.put_secret_record(purpose, owner_type, Some(&owner_id), secret)
            .await
    }

    /// Store an installation-scoped reversible secret. This explicit API is
    /// for global provider/configuration owners and cannot be mistaken for a
    /// Vault-bound data operation.
    pub async fn put_installation_secret(
        &self,
        purpose: &str,
        owner_type: &str,
        owner_id: Option<&str>,
        secret: &SecretString,
    ) -> Result<SecretMetadata, AuthError> {
        self.put_secret_record(purpose, owner_type, owner_id, secret)
            .await
    }

    async fn put_secret_record(
        &self,
        purpose: &str,
        owner_type: &str,
        owner_id: Option<&str>,
        secret: &SecretString,
    ) -> Result<SecretMetadata, AuthError> {
        self.ensure_persistent_master_key().await?;
        validate_public_label(purpose)?;
        validate_public_label(owner_type)?;
        let payload = self
            .keys
            .encrypt(purpose, owner_type, owner_id, secret.as_bytes())?;
        let record = self
            .repository
            .insert_secret(
                mcp_vault_domain::SecretId::new(),
                purpose,
                owner_type,
                owner_id,
                payload.key_version,
                &payload.nonce,
                &payload.ciphertext,
                Some(&secret.masked_hint()),
            )
            .await?;
        Ok(secret_metadata(record))
    }

    /// Decrypt a Vault-bound secret only when the owner context matches.
    pub async fn read_vault_secret(
        &self,
        context: &VaultContext,
        id: mcp_vault_domain::SecretId,
        purpose: &str,
        owner_type: &str,
    ) -> Result<SecretString, AuthError> {
        self.repository.ensure_vault_context(context).await?;
        let owner_id = context.id().to_string();
        self.read_secret_record(id, purpose, owner_type, Some(&owner_id))
            .await
    }

    /// Decrypt an installation-scoped secret for an authorized internal
    /// caller.
    pub async fn read_installation_secret(
        &self,
        id: mcp_vault_domain::SecretId,
        purpose: &str,
        owner_type: &str,
        owner_id: Option<&str>,
    ) -> Result<SecretString, AuthError> {
        self.read_secret_record(id, purpose, owner_type, owner_id)
            .await
    }

    async fn read_secret_record(
        &self,
        id: mcp_vault_domain::SecretId,
        purpose: &str,
        owner_type: &str,
        owner_id: Option<&str>,
    ) -> Result<SecretString, AuthError> {
        let record = self
            .repository
            .get_secret(id)
            .await?
            .ok_or(AuthError::SecretUnavailable)?;
        if record.purpose != purpose
            || record.owner_type != owner_type
            || record.owner_id.as_deref() != owner_id
        {
            return Err(AuthError::SecretUnavailable);
        }
        let plaintext = self.keys.decrypt(
            record.key_version,
            &record.nonce,
            &record.ciphertext,
            &record.purpose,
            &record.owner_type,
            record.owner_id.as_deref(),
        )?;
        let value =
            String::from_utf8(plaintext.to_vec()).map_err(|_| AuthError::SecretUnavailable)?;
        Ok(SecretString::new(value))
    }

    /// Re-encrypt every secret with a new current key while retaining old key
    /// versions in the caller-provided ring until verification completes.
    pub async fn rotate_secrets(&self, next_keys: &MasterKeyRing) -> Result<usize, AuthError> {
        let records = self.repository.list_secrets().await?;
        let mut rotated = 0;
        for record in records {
            if record.key_version == next_keys.current_version() {
                continue;
            }
            let plaintext = self.keys.decrypt(
                record.key_version,
                &record.nonce,
                &record.ciphertext,
                &record.purpose,
                &record.owner_type,
                record.owner_id.as_deref(),
            )?;
            let encrypted = next_keys.encrypt(
                &record.purpose,
                &record.owner_type,
                record.owner_id.as_deref(),
                &plaintext,
            )?;
            self.repository
                .update_secret_ciphertext(
                    record.id,
                    encrypted.key_version,
                    &encrypted.nonce,
                    &encrypted.ciphertext,
                    record.hint.as_deref(),
                )
                .await?;
            rotated += 1;
        }
        Ok(rotated)
    }

    /// Create the first Admin identity using the configured one-time token.
    pub async fn setup_admin(
        &self,
        bootstrap_token: &SecretString,
        username: &str,
        password: &SecretString,
    ) -> Result<AdminUserRecord, AuthError> {
        let prepared = self
            .prepare_admin_setup(bootstrap_token, username, password)
            .await?;
        self.commit_admin_setup(prepared).await
    }

    /// Validate bootstrap material and perform bounded password work before
    /// the Admin adapter initializes the default Vault.
    pub async fn prepare_admin_setup(
        &self,
        bootstrap_token: &SecretString,
        username: &str,
        password: &SecretString,
    ) -> Result<PreparedAdminSetup, AuthError> {
        let expected = self.bootstrap_digest.ok_or(AuthError::SetupUnavailable)?;
        let actual = self
            .keys
            .keyed_digest(BOOTSTRAP_DIGEST_PURPOSE, bootstrap_token.as_bytes());
        if !bool::from(actual.ct_eq(&expected)) {
            return Err(AuthError::SetupUnavailable);
        }
        if self.repository.has_admin_users().await? {
            return Err(AuthError::SetupUnavailable);
        }
        let username = normalize_username(username)?;
        let password_hash = self.hash_password(password).await?;
        Ok(PreparedAdminSetup {
            username,
            password_hash,
        })
    }

    /// Atomically commit one prepared first-run Admin identity.
    pub async fn commit_admin_setup(
        &self,
        prepared: PreparedAdminSetup,
    ) -> Result<AdminUserRecord, AuthError> {
        let user = self
            .repository
            .insert_first_admin_user(
                AdminUserId::new(),
                &prepared.username,
                &prepared.password_hash,
            )
            .await?
            .ok_or(AuthError::SetupUnavailable)?;
        if let Some(path) = self.managed_bootstrap_token_file.as_deref()
            && let Err(error) = tokio::fs::remove_file(path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %path.display(),
                %error,
                "first Admin was created but the managed bootstrap token file could not be removed"
            );
        }
        Ok(user)
    }

    /// Authenticate an Admin password and rotate all prior sessions.
    pub async fn login_admin(
        &self,
        username: &str,
        password: &SecretString,
        source_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<AdminLogin, AuthError> {
        let username = normalize_username(username)?;
        let limiter_key = format!("{}\0{}", source_ip.unwrap_or("unknown"), username);
        self.limiter.check(&limiter_key, now_millis()?)?;

        let user = self
            .repository
            .find_admin_user_by_username(&username)
            .await?;
        let hash = match user.as_ref() {
            Some(value) => value.password_hash.clone(),
            None => self.dummy_password_hash().await?,
        };
        let verification = self.verify_password(&hash, password).await?;
        let Some(user) = user else {
            self.limiter.failure(&limiter_key, now_millis()?);
            return Err(AuthError::InvalidCredential);
        };
        if user.disabled || !verification.valid {
            self.limiter.failure(&limiter_key, now_millis()?);
            return if user.disabled {
                Err(AuthError::Revoked)
            } else {
                Err(AuthError::InvalidCredential)
            };
        }
        self.limiter.success(&limiter_key);

        if verification.needs_rehash {
            let new_hash = self.hash_password(password).await?;
            self.repository
                .update_admin_password(user.id, &new_hash)
                .await?;
        }
        self.repository.revoke_admin_sessions(user.id).await?;

        let session_token = generate_bearer_token(SESSION_LABEL);
        let csrf_token = generate_bearer_token(CSRF_LABEL);
        let now = now_millis()?;
        let expires_at = now.saturating_add(duration_millis(self.session_policy.absolute_timeout)?);
        let session_id = mcp_vault_domain::AdminSessionId::new();
        let token_digest = digest_for_current(&self.keys, SESSION_DIGEST_PURPOSE, &session_token);
        let csrf_digest = digest_for_current(&self.keys, CSRF_DIGEST_PURPOSE, &csrf_token);
        let source_ip = safe_metadata(source_ip, 128)?;
        let user_agent_hash = user_agent.map(|value| Sha256::digest(value.as_bytes()).to_vec());
        self.repository
            .insert_admin_session(
                session_id,
                user.id,
                &token_digest,
                &csrf_digest,
                self.keys.current_version(),
                now,
                expires_at,
                source_ip.as_deref(),
                user_agent_hash.as_deref(),
            )
            .await?;

        Ok(AdminLogin {
            user_id: user.id,
            username: user.username,
            session_id,
            session_token,
            csrf_token,
            expires_at,
        })
    }

    /// Validate an Admin session and its CSRF/Origin policy for one request.
    pub async fn authenticate_admin_session(
        &self,
        session_token: &str,
        csrf_token: Option<&str>,
        headers: &HeaderMap,
        method: &Method,
        origin_policy: &OriginPolicy,
    ) -> Result<AdminPrincipal, AuthError> {
        origin_policy.validate_admin_request(headers, method)?;
        let session = self.find_session(session_token).await?;
        let now = now_millis()?;
        if session.revoked_at.is_some() || session.expires_at <= now {
            return Err(AuthError::SessionExpired);
        }
        if now.saturating_sub(session.last_seen_at)
            > duration_millis(self.session_policy.idle_timeout)?
        {
            self.repository.revoke_admin_session(session.id).await?;
            return Err(AuthError::SessionExpired);
        }
        if !is_safe_method(method) {
            let csrf_token = csrf_token.ok_or(AuthError::CsrfRejected)?;
            let csrf_digest = self.keys.keyed_digest_for(
                session.digest_key_version,
                CSRF_DIGEST_PURPOSE,
                csrf_token.as_bytes(),
            );
            if !bool::from(csrf_digest.ct_eq(session.csrf_secret_digest.as_slice())) {
                return Err(AuthError::CsrfRejected);
            }
        }
        let user = self
            .repository
            .get_admin_user(session.user_id)
            .await?
            .ok_or(AuthError::InvalidCredential)?;
        if user.disabled {
            return Err(AuthError::Revoked);
        }
        self.repository.touch_admin_session(session.id, now).await?;
        let actor_id = user.id.to_string();
        Ok(AdminPrincipal {
            actor: Actor::identified(ActorType::Admin, ActorId::new(&actor_id)?),
            user_id: user.id,
            username: user.username,
        })
    }

    /// Revoke a session token; missing tokens are intentionally idempotent.
    pub async fn logout_admin(&self, session_token: &str) -> Result<(), AuthError> {
        if let Some(session) = self.find_session_optional(session_token).await? {
            self.repository.revoke_admin_session(session.id).await?;
        }
        Ok(())
    }

    /// Change a password and revoke all sessions after a successful check.
    pub async fn change_admin_password(
        &self,
        user_id: AdminUserId,
        current_password: &SecretString,
        new_password: &SecretString,
    ) -> Result<(), AuthError> {
        let user = self
            .repository
            .get_admin_user(user_id)
            .await?
            .ok_or(AuthError::InvalidCredential)?;
        let verified = self
            .verify_password(&user.password_hash, current_password)
            .await?;
        if !verified.valid {
            return Err(AuthError::InvalidCredential);
        }
        let new_hash = self.hash_password(new_password).await?;
        self.repository
            .update_admin_password(user_id, &new_hash)
            .await?;
        self.repository.revoke_admin_sessions(user_id).await?;
        Ok(())
    }

    /// Re-verify the current Admin password for high-impact operations such
    /// as restore without creating another browser session or returning any
    /// password-derived material.
    pub async fn reauthenticate_admin(
        &self,
        user_id: AdminUserId,
        password: &SecretString,
    ) -> Result<(), AuthError> {
        let user = self
            .repository
            .get_admin_user(user_id)
            .await?
            .ok_or(AuthError::InvalidCredential)?;
        if user.disabled {
            return Err(AuthError::Revoked);
        }
        let verification = self.verify_password(&user.password_hash, password).await?;
        if verification.valid {
            Ok(())
        } else {
            Err(AuthError::InvalidCredential)
        }
    }

    /// Issue a Vault-bound WebDAV app password.
    pub async fn issue_webdav_credential(
        &self,
        context: &VaultContext,
        name: &str,
        username: &str,
        password: &SecretString,
        permissions: PermissionSet,
        expires_at: Option<i64>,
    ) -> Result<WebDavCredentialIssue, AuthError> {
        validate_public_label(name)?;
        let username = normalize_username(username)?;
        validate_webdav_permissions(&permissions)?;
        let password_hash = self.hash_password(password).await?;
        self.repository
            .insert_webdav_credential(
                context,
                CredentialId::new(),
                name,
                &username,
                &password_hash,
                &serde_json::to_string(&permissions).map_err(|_| AuthError::InvalidInput)?,
                expires_at,
            )
            .await?;
        Ok(WebDavCredentialIssue {
            credential_id: self
                .repository
                .find_webdav_credential(context, &username)
                .await?
                .ok_or(AuthError::InvalidInput)?
                .id,
            username,
            password: SecretString::new(password.expose_secret()),
            permissions,
            expires_at,
        })
    }

    /// Verify a Basic-auth app password inside exactly one Vault.
    pub async fn authenticate_webdav(
        &self,
        context: &VaultContext,
        username: &str,
        password: &SecretString,
        now: Option<i64>,
    ) -> Result<AuthPrincipal, AuthError> {
        let username = normalize_username(username)?;
        let record = self
            .repository
            .find_webdav_credential(context, &username)
            .await?;
        let hash = match record.as_ref() {
            Some(value) => value.password_hash.clone(),
            None => self.dummy_password_hash().await?,
        };
        let verification = self.verify_password(&hash, password).await?;
        let record = record.ok_or(AuthError::InvalidCredential)?;
        if !verification.valid || record.revoked_at.is_some() {
            return Err(AuthError::InvalidCredential);
        }
        let current_now = now.unwrap_or(now_millis()?);
        if record
            .expires_at
            .is_some_and(|expiry| expiry <= current_now)
        {
            return Err(AuthError::InvalidCredential);
        }
        let permissions: PermissionSet = serde_json::from_str(&record.permissions_json)
            .map_err(|_| AuthError::InvalidCredential)?;
        self.repository
            .touch_webdav_credential(context, record.id, current_now)
            .await?;
        let actor_id = record.id.to_string();
        Ok(AuthPrincipal {
            actor: Actor::identified(ActorType::WebDavCredential, ActorId::new(&actor_id)?),
            vault_id: Some(context.id()),
            credential_id: Some(record.id),
            permissions,
            scopes: ScopeSet::new(),
        })
    }

    /// Revoke a WebDAV credential only within its bound Vault.
    pub async fn revoke_webdav_credential(
        &self,
        context: &VaultContext,
        credential_id: CredentialId,
    ) -> Result<(), AuthError> {
        self.repository
            .revoke_webdav_credential(context, credential_id)
            .await?;
        Ok(())
    }

    /// Update non-secret WebDAV metadata without returning or replacing the
    /// existing password.
    pub async fn update_webdav_credential(
        &self,
        context: &VaultContext,
        credential_id: CredentialId,
        name: &str,
        permissions: PermissionSet,
        expires_at: Option<i64>,
    ) -> Result<(), AuthError> {
        validate_public_label(name)?;
        validate_webdav_permissions(&permissions)?;
        self.repository
            .update_webdav_credential(
                context,
                credential_id,
                name,
                &serde_json::to_string(&permissions).map_err(|_| AuthError::InvalidInput)?,
                expires_at,
            )
            .await?;
        Ok(())
    }

    /// Issue a Vault-bound high-entropy PAT and return its plaintext once.
    pub async fn issue_pat(
        &self,
        context: &VaultContext,
        name: &str,
        scopes: ScopeSet,
        expires_at: Option<i64>,
    ) -> Result<PatIssue, AuthError> {
        self.ensure_persistent_master_key().await?;
        validate_public_label(name)?;
        if scopes.iter().next().is_none() {
            return Err(AuthError::ScopeDenied);
        }
        let token = generate_bearer_token(PAT_LABEL);
        let prefix = token_prefix(&token, 16);
        let digest = digest_bearer_token(&self.keys, PAT_DIGEST_PURPOSE, &token);
        let scope_values = scopes.iter().map(ToString::to_string).collect::<Vec<_>>();
        let id = CredentialId::new();
        self.repository
            .insert_mcp_token(
                context,
                id,
                name,
                &prefix,
                &digest,
                self.keys.current_version(),
                &serde_json::to_string(&scope_values).map_err(|_| AuthError::InvalidInput)?,
                expires_at,
            )
            .await?;
        Ok(PatIssue {
            credential_id: id,
            token_prefix: prefix,
            token,
            scopes,
            expires_at,
        })
    }

    /// Verify a PAT against one endpoint Vault and optional required scopes.
    pub async fn authenticate_pat(
        &self,
        context: &VaultContext,
        token: &str,
        required_scopes: &[Scope],
        now: Option<i64>,
    ) -> Result<AuthPrincipal, AuthError> {
        if !token.starts_with(PAT_LABEL) {
            return Err(AuthError::InvalidCredential);
        }
        let prefix = token.chars().take(16).collect::<String>();
        let mut record = None;
        for version in self.keys.versions() {
            let digest = self
                .keys
                .keyed_digest_for(version, PAT_DIGEST_PURPOSE, token.as_bytes());
            if let Some(candidate) = self
                .repository
                .find_mcp_token(context, &prefix, &digest)
                .await?
                && candidate.digest_key_version == version
            {
                record = Some(candidate);
                break;
            }
        }
        let record = record.ok_or(AuthError::InvalidCredential)?;
        let current_now = now.unwrap_or(now_millis()?);
        if record.revoked_at.is_some()
            || record
                .expires_at
                .is_some_and(|expiry| expiry <= current_now)
        {
            return Err(AuthError::InvalidCredential);
        }
        let scopes = parse_scopes(&record.scopes_json).map_err(|_| AuthError::InvalidCredential)?;
        if required_scopes.iter().any(|scope| !scopes.contains(*scope)) {
            return Err(AuthError::ScopeDenied);
        }
        self.repository
            .touch_mcp_token(context, record.id, current_now)
            .await?;
        let actor_id = record.id.to_string();
        Ok(AuthPrincipal {
            actor: Actor::identified(ActorType::McpPat, ActorId::new(&actor_id)?),
            vault_id: Some(context.id()),
            credential_id: Some(record.id),
            permissions: scopes.permissions(),
            scopes,
        })
    }

    /// Rotate a PAT by revoking the old row and issuing a new one.
    pub async fn rotate_pat(
        &self,
        context: &VaultContext,
        credential_id: CredentialId,
        name: &str,
        scopes: ScopeSet,
        expires_at: Option<i64>,
    ) -> Result<PatIssue, AuthError> {
        self.repository
            .revoke_mcp_token(context, credential_id)
            .await?;
        self.issue_pat(context, name, scopes, expires_at).await
    }

    /// Revoke a PAT only within its bound Vault.
    pub async fn revoke_pat(
        &self,
        context: &VaultContext,
        credential_id: CredentialId,
    ) -> Result<(), AuthError> {
        self.repository
            .revoke_mcp_token(context, credential_id)
            .await?;
        Ok(())
    }

    /// Save a validated OAuth resource-server issuer configuration.
    pub async fn configure_oauth_issuer(
        &self,
        input: OAuthIssuerInput,
    ) -> Result<OAuthIssuerRecord, AuthError> {
        validate_public_label(&input.name)?;
        validate_url(&input.issuer_url)?;
        validate_url(&input.resource)?;
        if let Some(discovery_url) = &input.discovery_url {
            validate_url(discovery_url)?;
        }
        if input.audience.is_empty() || input.audience.chars().any(char::is_control) {
            return Err(AuthError::OAuthConfiguration);
        }
        let jwks = JsonWebKeySet::from_json(&input.jwks_cache_json)?;
        let public_jwks_json = jwks.to_public_json()?;
        if let Some(existing) = self.repository.find_oauth_issuer(&input.issuer_url).await? {
            self.repository
                .update_oauth_issuer(
                    existing.id,
                    &input.name,
                    &input.issuer_url,
                    input.discovery_url.as_deref(),
                    &input.audience,
                    Some(&input.resource),
                    Some(&public_jwks_json),
                    input.enabled,
                )
                .await
                .map_err(AuthError::from)
        } else {
            self.repository
                .insert_oauth_issuer(
                    mcp_vault_domain::OAuthIssuerId::new(),
                    &input.name,
                    &input.issuer_url,
                    input.discovery_url.as_deref(),
                    &input.audience,
                    Some(&input.resource),
                    Some(&public_jwks_json),
                    input.enabled,
                )
                .await
                .map_err(AuthError::from)
        }
    }

    /// Grant an OAuth subject a bounded scope set for one Vault.
    pub async fn grant_oauth_subject(
        &self,
        context: &VaultContext,
        issuer_id: mcp_vault_domain::OAuthIssuerId,
        subject: &str,
        scopes: ScopeSet,
    ) -> Result<OAuthGrantRecord, AuthError> {
        if subject.is_empty() || subject.len() > 512 || subject.chars().any(char::is_control) {
            return Err(AuthError::InvalidInput);
        }
        if scopes.iter().next().is_none() {
            return Err(AuthError::ScopeDenied);
        }
        let values = scopes.iter().map(ToString::to_string).collect::<Vec<_>>();
        self.repository
            .insert_oauth_grant(
                context,
                mcp_vault_domain::OAuthGrantId::new(),
                issuer_id,
                subject,
                &serde_json::to_string(&values).map_err(|_| AuthError::InvalidInput)?,
            )
            .await
            .map_err(AuthError::from)
    }

    /// List OAuth subject grants for exactly one Vault.
    pub async fn list_oauth_grants(
        &self,
        context: &VaultContext,
        include_revoked: bool,
        limit: u32,
    ) -> Result<Vec<OAuthGrantRecord>, AuthError> {
        self.repository
            .list_oauth_grants(context, include_revoked, limit)
            .await
            .map_err(AuthError::from)
    }

    /// Revoke one OAuth subject grant inside its bound Vault.
    pub async fn revoke_oauth_grant(
        &self,
        context: &VaultContext,
        grant_id: mcp_vault_domain::OAuthGrantId,
    ) -> Result<(), AuthError> {
        self.repository
            .revoke_oauth_grant(context, grant_id)
            .await
            .map_err(AuthError::from)
    }

    /// Validate an OAuth access token and derive its Vault-bound principal.
    pub async fn authenticate_oauth(
        &self,
        context: &VaultContext,
        token: &str,
        required_scopes: &[Scope],
        now_seconds: Option<i64>,
    ) -> Result<AuthPrincipal, AuthError> {
        let (issuer_url, subject) = token_identity(token)?;
        let issuer = self
            .repository
            .find_oauth_issuer(&issuer_url)
            .await?
            .ok_or(AuthError::OAuthTokenInvalid)?;
        let grant = self
            .repository
            .find_oauth_grant(context, issuer.id, &subject)
            .await?
            .ok_or(AuthError::OAuthTokenInvalid)?;
        let principal = validate_access_token(
            token,
            &issuer,
            &grant,
            context,
            required_scopes,
            now_seconds.unwrap_or(now_seconds_now()?),
        )?;
        Ok(auth_principal_from_oauth(context, principal))
    }

    /// Require an application permission at the adapter boundary.
    pub fn require_permission(
        principal: &AuthPrincipal,
        permission: Permission,
    ) -> Result<(), AuthError> {
        if principal.permissions.contains(permission) {
            Ok(())
        } else {
            Err(AuthError::ScopeDenied)
        }
    }

    async fn find_session(&self, token: &str) -> Result<AdminSessionRecord, AuthError> {
        self.find_session_optional(token)
            .await?
            .ok_or(AuthError::InvalidCredential)
    }

    async fn find_session_optional(
        &self,
        token: &str,
    ) -> Result<Option<AdminSessionRecord>, AuthError> {
        if !token.starts_with(SESSION_LABEL) {
            return Ok(None);
        }
        for version in self.keys.versions() {
            let digest =
                self.keys
                    .keyed_digest_for(version, SESSION_DIGEST_PURPOSE, token.as_bytes());
            if let Some(session) = self.repository.find_admin_session(&digest).await?
                && session.digest_key_version == version
            {
                return Ok(Some(session));
            }
        }
        Ok(None)
    }
}

/// Construct a secure Admin session cookie header value.
pub fn session_cookie_header(token: &BearerToken, max_age: Duration) -> String {
    format!(
        "mcp_vault_session={}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Strict",
        token.expose_secret(),
        max_age.as_secs()
    )
}

/// Extract the single opaque session value from a Cookie header.
pub fn parse_session_cookie(header: &str) -> Result<&str, AuthError> {
    let mut found = None;
    for part in header.split(';') {
        let mut pair = part.trim().splitn(2, '=');
        let name = pair.next().unwrap_or_default();
        let value = pair.next().unwrap_or_default();
        if name == "mcp_vault_session" {
            if found.is_some() || value.is_empty() || value.chars().any(char::is_control) {
                return Err(AuthError::InvalidCredential);
            }
            found = Some(value);
        }
    }
    found.ok_or(AuthError::InvalidCredential)
}

/// Construct a deletion cookie for logout.
pub fn clear_session_cookie_header() -> &'static str {
    "mcp_vault_session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict"
}

/// Basic authentication is acceptable only over TLS or a loopback transport.
pub fn require_secure_basic_auth(is_tls: bool, peer_is_loopback: bool) -> Result<(), AuthError> {
    if is_tls || peer_is_loopback {
        Ok(())
    } else {
        Err(AuthError::InvalidCredential)
    }
}

fn auth_principal_from_oauth(context: &VaultContext, principal: OAuthPrincipal) -> AuthPrincipal {
    AuthPrincipal {
        actor: principal.actor,
        vault_id: Some(context.id()),
        credential_id: None,
        permissions: principal.permissions,
        scopes: principal.scopes,
    }
}

fn secret_metadata(record: EncryptedSecretRecord) -> SecretMetadata {
    SecretMetadata {
        id: record.id,
        purpose: record.purpose,
        owner_type: record.owner_type,
        owner_id: record.owner_id,
        key_version: record.key_version,
        hint: record.hint,
    }
}

fn validate_webdav_permissions(permissions: &PermissionSet) -> Result<(), AuthError> {
    if permissions.iter().any(|permission| {
        !matches!(
            permission,
            Permission::DiscoverVault
                | Permission::ReadVault
                | Permission::WriteVault
                | Permission::DeleteVault
        )
    }) {
        return Err(AuthError::ScopeDenied);
    }
    Ok(())
}

fn normalize_username(value: &str) -> Result<String, AuthError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(AuthError::InvalidInput);
    }
    Ok(value)
}

fn validate_public_label(value: &str) -> Result<(), AuthError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn validate_url(value: &str) -> Result<(), AuthError> {
    let url = url::Url::parse(value).map_err(|_| AuthError::OAuthConfiguration)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AuthError::OAuthConfiguration);
    }
    Ok(())
}

fn safe_metadata(value: Option<&str>, max_bytes: usize) -> Result<Option<String>, AuthError> {
    value
        .map(|value| {
            if value.len() > max_bytes || value.chars().any(char::is_control) {
                Err(AuthError::InvalidInput)
            } else {
                Ok(value.to_owned())
            }
        })
        .transpose()
}

fn duration_millis(duration: Duration) -> Result<i64, AuthError> {
    i64::try_from(duration.as_millis()).map_err(|_| AuthError::InvalidInput)
}

fn now_millis() -> Result<i64, AuthError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::InvalidInput)?;
    i64::try_from(elapsed.as_millis()).map_err(|_| AuthError::InvalidInput)
}

fn now_seconds_now() -> Result<i64, AuthError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::InvalidInput)?;
    i64::try_from(elapsed.as_secs()).map_err(|_| AuthError::InvalidInput)
}

fn is_safe_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

fn digest_for_current(keys: &MasterKeyRing, purpose: &str, token: &BearerToken) -> [u8; 32] {
    keys.keyed_digest(purpose, token.expose_secret().as_bytes())
}

fn dummy_password_hash(policy: &PasswordPolicy) -> Result<String, AuthError> {
    static DUMMY: OnceLock<String> = OnceLock::new();
    Ok(DUMMY
        .get_or_init(|| {
            policy
                .hash(&SecretString::new("mcp-vault-dummy-password-value"))
                .expect("dummy password satisfies the configured default policy")
        })
        .clone())
}

#[derive(Default)]
struct LoginRateLimiter {
    entries: Mutex<HashMap<String, LoginAttempt>>,
}

struct LoginAttempt {
    failures: u32,
    blocked_until: i64,
    last_attempt: i64,
}

impl LoginRateLimiter {
    fn check(&self, key: &str, now: i64) -> Result<(), AuthError> {
        let mut entries = self.entries.lock().map_err(|_| AuthError::Cryptography)?;
        entries.retain(|_, attempt| now.saturating_sub(attempt.last_attempt) < 15 * 60 * 1000);
        if entries.len() > 4096 {
            entries.clear();
        }
        if entries
            .get(key)
            .is_some_and(|attempt| attempt.blocked_until > now)
        {
            return Err(AuthError::RateLimited);
        }
        Ok(())
    }

    fn failure(&self, key: &str, now: i64) {
        if let Ok(mut entries) = self.entries.lock() {
            let attempt = entries.entry(key.to_owned()).or_insert(LoginAttempt {
                failures: 0,
                blocked_until: 0,
                last_attempt: now,
            });
            attempt.failures = attempt.failures.saturating_add(1);
            let delay_seconds = if attempt.failures < 5 {
                0
            } else {
                2_i64.pow((attempt.failures - 5).min(4))
            };
            attempt.blocked_until = now.saturating_add(delay_seconds * 1000);
            attempt.last_attempt = now;
        }
    }

    fn success(&self, key: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use http::{HeaderMap, HeaderValue, Method};
    use mcp_vault_domain::{
        Permission, PermissionSet, Revision, Scope, ScopeSet, VaultContext, VaultId, VaultSlug,
    };
    use mcp_vault_state::{StateStore, VaultStatus};

    use super::{AuthService, SessionPolicy, session_cookie_header};
    use crate::{
        AuthError,
        origin::OriginPolicy,
        secret::{MasterKeyRing, SecretString},
    };

    async fn setup() -> (AuthService, VaultContext, StateStore) {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from("/srv/work"),
            Revision::new(1),
        )
        .unwrap();
        store
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        let keys = MasterKeyRing::from_bytes(1, &[9_u8; 32]).unwrap();
        let service = AuthService::new(store.auth(), keys)
            .with_policies(
                crate::password::PasswordPolicy::default(),
                SessionPolicy {
                    idle_timeout: Duration::from_secs(3600),
                    absolute_timeout: Duration::from_secs(3600),
                },
            )
            .with_bootstrap_token(&SecretString::new("bootstrap-token-value"));
        service
            .setup_admin(
                &SecretString::new("bootstrap-token-value"),
                "Admin",
                &SecretString::new("correct horse battery staple"),
            )
            .await
            .unwrap();
        (service, context, store)
    }

    #[tokio::test]
    async fn concurrent_first_admin_setup_has_exactly_one_winner() {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let service = AuthService::new(
            store.auth(),
            MasterKeyRing::from_bytes(1, &[3_u8; 32]).unwrap(),
        )
        .with_bootstrap_token(&SecretString::new("bootstrap-token-value"));
        let first = service.clone();
        let second = service.clone();
        let (first, second) = tokio::join!(
            async move {
                first
                    .setup_admin(
                        &SecretString::new("bootstrap-token-value"),
                        "first-admin",
                        &SecretString::new("correct horse battery staple"),
                    )
                    .await
            },
            async move {
                second
                    .setup_admin(
                        &SecretString::new("bootstrap-token-value"),
                        "second-admin",
                        &SecretString::new("correct horse battery staple"),
                    )
                    .await
            }
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let failure = if first.is_err() {
            first.err()
        } else {
            second.err()
        };
        assert!(matches!(failure, Some(AuthError::SetupUnavailable)));
    }

    #[tokio::test]
    async fn first_admin_consumes_only_an_application_managed_bootstrap_file() {
        let directory = tempfile::tempdir().unwrap();
        let managed_path = directory.path().join("managed-bootstrap-token");
        tokio::fs::write(&managed_path, b"bootstrap-token-value")
            .await
            .unwrap();
        let managed_store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let managed = AuthService::new(
            managed_store.auth(),
            MasterKeyRing::from_bytes(1, &[5_u8; 32]).unwrap(),
        )
        .with_managed_bootstrap_token(
            &SecretString::new("bootstrap-token-value"),
            managed_path.clone(),
        );
        managed
            .setup_admin(
                &SecretString::new("bootstrap-token-value"),
                "managed-owner",
                &SecretString::new("correct horse battery staple"),
            )
            .await
            .unwrap();
        assert!(!tokio::fs::try_exists(&managed_path).await.unwrap());

        let explicit_path = directory.path().join("operator-bootstrap-token");
        tokio::fs::write(&explicit_path, b"bootstrap-token-value")
            .await
            .unwrap();
        let explicit_store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let explicit = AuthService::new(
            explicit_store.auth(),
            MasterKeyRing::from_bytes(1, &[6_u8; 32]).unwrap(),
        )
        .with_bootstrap_token(&SecretString::new("bootstrap-token-value"));
        explicit
            .setup_admin(
                &SecretString::new("bootstrap-token-value"),
                "explicit-owner",
                &SecretString::new("correct horse battery staple"),
            )
            .await
            .unwrap();
        assert!(tokio::fs::try_exists(&explicit_path).await.unwrap());
    }

    #[tokio::test]
    async fn ephemeral_master_key_cannot_create_persistent_pat() {
        let store = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("ephemeral").unwrap(),
            "/srv/ephemeral".into(),
            Revision::ZERO,
        )
        .unwrap();
        store
            .vaults()
            .insert(&context, "Ephemeral", VaultStatus::Active)
            .await
            .unwrap();
        let service = AuthService::new(store.auth(), MasterKeyRing::ephemeral());
        let error = service
            .issue_pat(
                &context,
                "agent",
                [Scope::VaultRead].into_iter().collect(),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, AuthError::MasterKeyUnavailable));
    }

    #[tokio::test]
    async fn admin_session_requires_origin_and_csrf_and_rotates() {
        let (service, _context, _store) = setup().await;
        let login = service
            .login_admin(
                "admin",
                &SecretString::new("correct horse battery staple"),
                Some("127.0.0.1"),
                Some("test-agent"),
            )
            .await
            .unwrap();
        assert!(
            session_cookie_header(&login.session_token, Duration::from_secs(60))
                .contains("Secure; HttpOnly; SameSite=Strict")
        );
        let origin = OriginPolicy::new(["http://localhost:8081"]).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://localhost:8081"));
        assert!(
            service
                .authenticate_admin_session(
                    login.session_token.expose_secret(),
                    None,
                    &headers,
                    &Method::POST,
                    &origin,
                )
                .await
                .is_err()
        );
        let principal = service
            .authenticate_admin_session(
                login.session_token.expose_secret(),
                Some(login.csrf_token.expose_secret()),
                &headers,
                &Method::POST,
                &origin,
            )
            .await
            .unwrap();
        assert_eq!(principal.username, "admin");
    }

    #[tokio::test]
    async fn webdav_and_pat_credentials_cannot_cross_vaults() {
        let (service, first, _store) = setup().await;
        let second = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("other").unwrap(),
            "/srv/other".into(),
            Revision::new(1),
        )
        .unwrap();
        // The service's repository is intentionally bound to the first store;
        // an unregistered second context must not see its credentials.
        assert_ne!(first.id(), second.id());
        let mut permissions = PermissionSet::new();
        permissions.insert(Permission::ReadVault);
        let webdav = service
            .issue_webdav_credential(
                &first,
                "Laptop",
                "laptop",
                &SecretString::new("dav-password-123"),
                permissions,
                None,
            )
            .await
            .unwrap();
        assert!(
            service
                .authenticate_webdav(&second, &webdav.username, &webdav.password, None,)
                .await
                .is_err()
        );
        let scopes: ScopeSet = [Scope::VaultRead].into_iter().collect();
        let pat = service
            .issue_pat(&first, "Agent", scopes, None)
            .await
            .unwrap();
        assert!(
            service
                .authenticate_pat(&second, pat.token.expose_secret(), &[], None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn encrypted_secrets_rotate_and_remain_owner_bound() {
        let (service, context, store) = setup().await;
        let metadata = service
            .put_vault_secret(
                &context,
                "provider-api-key",
                "vault",
                &SecretString::new("provider-secret-value"),
            )
            .await
            .unwrap();
        let stored = store.auth().get_secret(metadata.id).await.unwrap().unwrap();
        assert!(!format!("{stored:?}").contains("provider-secret-value"));
        assert_eq!(
            service
                .read_vault_secret(&context, metadata.id, "provider-api-key", "vault",)
                .await
                .unwrap()
                .expose_secret(),
            "provider-secret-value"
        );
        let other_context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("other-secret").unwrap(),
            "/srv/other-secret".into(),
            Revision::new(1),
        )
        .unwrap();
        assert!(
            service
                .read_vault_secret(&other_context, metadata.id, "provider-api-key", "vault")
                .await
                .is_err()
        );

        let next_keys =
            MasterKeyRing::from_versions(2, vec![(1, vec![9_u8; 32]), (2, vec![8_u8; 32])])
                .unwrap();
        assert_eq!(service.rotate_secrets(&next_keys).await.unwrap(), 1);
        let rotated = AuthService::new(store.auth(), next_keys);
        assert_eq!(
            rotated
                .read_vault_secret(&context, metadata.id, "provider-api-key", "vault",)
                .await
                .unwrap()
                .expose_secret(),
            "provider-secret-value"
        );
    }

    #[tokio::test]
    async fn pat_digest_lookup_is_one_time_and_session_revocation_is_effective() {
        let (service, context, store) = setup().await;
        let scopes: ScopeSet = [Scope::VaultRead].into_iter().collect();
        let pat = service
            .issue_pat(&context, "Agent", scopes, None)
            .await
            .unwrap();
        let stored = store
            .auth()
            .find_mcp_token(
                &context,
                &pat.token_prefix,
                &service.keys.keyed_digest(
                    super::PAT_DIGEST_PURPOSE,
                    pat.token.expose_secret().as_bytes(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.digest_key_version, 1);
        assert_ne!(
            stored.token_digest.as_slice(),
            pat.token.expose_secret().as_bytes()
        );
        assert!(format!("{pat:?}").contains("[REDACTED]"));

        let login = service
            .login_admin(
                "admin",
                &SecretString::new("correct horse battery staple"),
                None,
                None,
            )
            .await
            .unwrap();
        service
            .logout_admin(login.session_token.expose_secret())
            .await
            .unwrap();
        let origin = OriginPolicy::new(["http://localhost:8081"]).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://localhost:8081"));
        assert!(
            service
                .authenticate_admin_session(
                    login.session_token.expose_secret(),
                    Some(login.csrf_token.expose_secret()),
                    &headers,
                    &Method::POST,
                    &origin,
                )
                .await
                .is_err()
        );
    }
}
