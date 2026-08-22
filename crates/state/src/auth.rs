//! SQL repositories for authentication, authorization, and encrypted-secret
//! metadata.
//!
//! This module deliberately contains no password, token, encryption, or JWT
//! algorithms. It stores only the typed records produced by `mcp-vault-auth`
//! and enforces the Vault predicates at the repository boundary.

use std::fmt;

use mcp_vault_domain::{
    AdminSessionId, AdminUserId, CredentialId, OAuthGrantId, OAuthIssuerId, SecretId, VaultContext,
    VaultId,
};
use std::sync::Arc;

use sqlx::{FromRow, SqlitePool};
use tokio::sync::Semaphore;

use crate::{StateError, now_millis};

/// Persisted authenticated-encryption record. The plaintext is never present
/// in this type or in SQLite.
#[derive(Clone, Eq, PartialEq)]
pub struct EncryptedSecretRecord {
    /// Stable secret record ID.
    pub id: SecretId,
    /// Stable purpose used as AEAD associated data.
    pub purpose: String,
    /// Non-secret owner category.
    pub owner_type: String,
    /// Optional non-secret owner ID.
    pub owner_id: Option<String>,
    /// Master-key version used for this ciphertext.
    pub key_version: u32,
    /// XChaCha20-Poly1305 nonce.
    pub nonce: Vec<u8>,
    /// Authenticated ciphertext including the AEAD tag.
    pub ciphertext: Vec<u8>,
    /// Optional masked hint safe for an Admin response.
    pub hint: Option<String>,
    /// Creation timestamp in UTC Unix milliseconds.
    pub created_at: i64,
    /// Last ciphertext update timestamp.
    pub updated_at: i64,
}

impl fmt::Debug for EncryptedSecretRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedSecretRecord")
            .field("id", &self.id)
            .field("purpose", &self.purpose)
            .field("owner_type", &self.owner_type)
            .field("owner_id", &self.owner_id)
            .field("key_version", &self.key_version)
            .field("has_ciphertext", &(!self.ciphertext.is_empty()))
            .field("hint", &self.hint)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Persisted Admin identity without exposing a session or password secret.
#[derive(Clone, Eq, PartialEq)]
pub struct AdminUserRecord {
    /// Stable Admin user ID.
    pub id: AdminUserId,
    /// Normalized login name.
    pub username: String,
    /// Argon2id PHC string.
    pub password_hash: String,
    /// Disabled identities cannot create or use sessions.
    pub disabled: bool,
    /// Last password-change timestamp.
    pub password_changed_at: i64,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last identity update timestamp.
    pub updated_at: i64,
}

impl fmt::Debug for AdminUserRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminUserRecord")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("password_hash", &"[REDACTED]")
            .field("disabled", &self.disabled)
            .field("password_changed_at", &self.password_changed_at)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Persisted opaque Admin session metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct AdminSessionRecord {
    /// Stable session ID.
    pub id: AdminSessionId,
    /// Owning Admin identity.
    pub user_id: AdminUserId,
    /// Keyed digest of the opaque cookie token.
    pub token_digest: Vec<u8>,
    /// Keyed digest of the session-bound CSRF secret.
    pub csrf_secret_digest: Vec<u8>,
    /// Master-key version used for both session and CSRF digests.
    pub digest_key_version: u32,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last validated use timestamp.
    pub last_seen_at: i64,
    /// Absolute expiry timestamp.
    pub expires_at: i64,
    /// Safe peer address captured by the adapter.
    pub source_ip: Option<String>,
    /// SHA-256 digest of the user-agent, never the raw header.
    pub user_agent_hash: Option<Vec<u8>>,
    /// Explicit revocation timestamp.
    pub revoked_at: Option<i64>,
}

impl fmt::Debug for AdminSessionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminSessionRecord")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("token_digest", &"[REDACTED]")
            .field("csrf_secret_digest", &"[REDACTED]")
            .field("digest_key_version", &self.digest_key_version)
            .field("created_at", &self.created_at)
            .field("last_seen_at", &self.last_seen_at)
            .field("expires_at", &self.expires_at)
            .field("source_ip", &self.source_ip)
            .field("has_user_agent_hash", &self.user_agent_hash.is_some())
            .field("revoked_at", &self.revoked_at)
            .finish()
    }
}

/// Vault-bound WebDAV app credential.
#[derive(Clone, Eq, PartialEq)]
pub struct WebDavCredentialRecord {
    /// Stable credential ID.
    pub id: CredentialId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Admin-visible device/client name.
    pub name: String,
    /// Basic-auth username.
    pub username: String,
    /// Argon2id PHC password hash.
    pub password_hash: String,
    /// Serialized validated `PermissionSet`.
    pub permissions_json: String,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last successful verification timestamp.
    pub last_used_at: Option<i64>,
    /// Optional expiry timestamp.
    pub expires_at: Option<i64>,
    /// Explicit revocation timestamp.
    pub revoked_at: Option<i64>,
}

impl fmt::Debug for WebDavCredentialRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebDavCredentialRecord")
            .field("id", &self.id)
            .field("vault_id", &self.vault_id)
            .field("name", &self.name)
            .field("username", &self.username)
            .field("password_hash", &"[REDACTED]")
            .field("permissions_json", &self.permissions_json)
            .field("created_at", &self.created_at)
            .field("last_used_at", &self.last_used_at)
            .field("expires_at", &self.expires_at)
            .field("revoked_at", &self.revoked_at)
            .finish()
    }
}

/// Vault-bound MCP PAT metadata. The plaintext token is never stored.
#[derive(Clone, Eq, PartialEq)]
pub struct McpTokenRecord {
    /// Stable credential ID.
    pub id: CredentialId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Admin-visible token name.
    pub name: String,
    /// Visible lookup prefix.
    pub token_prefix: String,
    /// Installation-keyed token digest.
    pub token_digest: Vec<u8>,
    /// Master-key version used for the keyed token digest.
    pub digest_key_version: u32,
    /// Serialized validated `ScopeSet`.
    pub scopes_json: String,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last successful verification timestamp.
    pub last_used_at: Option<i64>,
    /// Optional expiry timestamp.
    pub expires_at: Option<i64>,
    /// Explicit revocation timestamp.
    pub revoked_at: Option<i64>,
}

impl fmt::Debug for McpTokenRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpTokenRecord")
            .field("id", &self.id)
            .field("vault_id", &self.vault_id)
            .field("name", &self.name)
            .field("token_prefix", &self.token_prefix)
            .field("token_digest", &"[REDACTED]")
            .field("digest_key_version", &self.digest_key_version)
            .field("scopes_json", &self.scopes_json)
            .field("created_at", &self.created_at)
            .field("last_used_at", &self.last_used_at)
            .field("expires_at", &self.expires_at)
            .field("revoked_at", &self.revoked_at)
            .finish()
    }
}

/// Configured OAuth resource-server issuer.
#[derive(Clone, Eq, PartialEq)]
pub struct OAuthIssuerRecord {
    /// Stable issuer ID.
    pub id: OAuthIssuerId,
    /// Admin-visible name.
    pub name: String,
    /// Exact `iss` claim value.
    pub issuer_url: String,
    /// Optional discovery URL for a later refresh worker.
    pub discovery_url: Option<String>,
    /// Required `aud` value.
    pub audience: String,
    /// Required protected-resource value.
    pub resource: Option<String>,
    /// Cached JSON JWK set used by local validation.
    pub jwks_cache_json: Option<String>,
    /// Timestamp of the cached key set.
    pub jwks_cached_at: Option<i64>,
    /// Disabled issuers cannot authorize requests.
    pub enabled: bool,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last configuration update timestamp.
    pub updated_at: i64,
}

impl fmt::Debug for OAuthIssuerRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthIssuerRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("issuer_url", &self.issuer_url)
            .field("discovery_url", &self.discovery_url)
            .field("audience", &self.audience)
            .field("resource", &self.resource)
            .field("has_jwks_cache", &self.jwks_cache_json.is_some())
            .field("jwks_cached_at", &self.jwks_cached_at)
            .field("enabled", &self.enabled)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// OAuth subject grant bound to exactly one Vault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthGrantRecord {
    /// Stable grant ID.
    pub id: OAuthGrantId,
    /// Issuer that authenticated the subject.
    pub issuer_id: OAuthIssuerId,
    /// Exact token subject.
    pub subject: String,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Serialized allowed scope set.
    pub scopes_json: String,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last update timestamp.
    pub updated_at: i64,
    /// Explicit revocation timestamp.
    pub revoked_at: Option<i64>,
}

#[derive(Debug, FromRow)]
struct EncryptedSecretRow {
    id: String,
    purpose: String,
    owner_type: String,
    owner_id: Option<String>,
    key_version: i64,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    hint: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct AdminUserRow {
    id: String,
    username: String,
    password_hash: String,
    disabled: i64,
    password_changed_at: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct AdminSessionRow {
    id: String,
    user_id: String,
    token_digest: Vec<u8>,
    csrf_secret_digest: Vec<u8>,
    digest_key_version: i64,
    created_at: i64,
    last_seen_at: i64,
    expires_at: i64,
    source_ip: Option<String>,
    user_agent_hash: Option<Vec<u8>>,
    revoked_at: Option<i64>,
}

#[derive(Debug, FromRow)]
struct WebDavCredentialRow {
    id: String,
    vault_id: String,
    name: String,
    username: String,
    password_hash: String,
    permissions_json: String,
    created_at: i64,
    last_used_at: Option<i64>,
    expires_at: Option<i64>,
    revoked_at: Option<i64>,
}

#[derive(Debug, FromRow)]
struct McpTokenRow {
    id: String,
    vault_id: String,
    name: String,
    token_prefix: String,
    token_digest: Vec<u8>,
    digest_key_version: i64,
    scopes_json: String,
    created_at: i64,
    last_used_at: Option<i64>,
    expires_at: Option<i64>,
    revoked_at: Option<i64>,
}

#[derive(Debug, FromRow)]
struct OAuthIssuerRow {
    id: String,
    name: String,
    issuer_url: String,
    discovery_url: Option<String>,
    audience: String,
    resource: Option<String>,
    jwks_cache_json: Option<String>,
    jwks_cached_at: Option<i64>,
    enabled: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct OAuthGrantRow {
    id: String,
    issuer_id: String,
    subject: String,
    vault_id: String,
    scopes_json: String,
    created_at: i64,
    updated_at: i64,
    revoked_at: Option<i64>,
}

/// SQL-owned authentication repository.
#[derive(Clone)]
pub struct AuthStateRepository {
    pool: SqlitePool,
    write_gate: Arc<Semaphore>,
}

impl AuthStateRepository {
    pub(crate) fn new(pool: SqlitePool, write_gate: Arc<Semaphore>) -> Self {
        Self { pool, write_gate }
    }

    /// Count encrypted rows used by bootstrap readiness validation.
    pub async fn count_encrypted_secrets(&self) -> Result<u64, StateError> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM encrypted_secrets")
                .fetch_one(&self.pool)
                .await? as u64,
        )
    }

    /// Count durable rows whose authentication depends on the installation
    /// master key. Password hashes are intentionally excluded.
    pub async fn count_master_key_dependencies(&self) -> Result<u64, StateError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT
                 (SELECT COUNT(*) FROM encrypted_secrets) +
                 (SELECT COUNT(*) FROM mcp_tokens)",
        )
        .fetch_one(&self.pool)
        .await? as u64)
    }

    /// Read the one-way verification digest for a retained key version.
    pub async fn get_installation_key_check(
        &self,
        key_version: u32,
    ) -> Result<Option<Vec<u8>>, StateError> {
        Ok(sqlx::query_scalar(
            "SELECT verification_digest FROM installation_key_checks
             WHERE key_version = ?",
        )
        .bind(i64::from(key_version))
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Return how many key-version verifiers are persisted.
    pub async fn count_installation_key_checks(&self) -> Result<u64, StateError> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM installation_key_checks")
                .fetch_one(&self.pool)
                .await? as u64,
        )
    }

    /// Persist a one-way key verifier without replacing an existing value.
    pub async fn insert_installation_key_check_if_absent(
        &self,
        key_version: u32,
        verification_digest: &[u8],
    ) -> Result<(), StateError> {
        if key_version == 0 || verification_digest.len() != 32 {
            return Err(StateError::InvalidInput(
                "installation key check is invalid",
            ));
        }
        let timestamp = now_millis()?;
        sqlx::query(
            "INSERT OR IGNORE INTO installation_key_checks
             (key_version, verification_digest, created_at, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(i64::from(key_version))
        .bind(verification_digest)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert an encrypted secret metadata row.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_secret(
        &self,
        id: SecretId,
        purpose: &str,
        owner_type: &str,
        owner_id: Option<&str>,
        key_version: u32,
        nonce: &[u8],
        ciphertext: &[u8],
        hint: Option<&str>,
    ) -> Result<EncryptedSecretRecord, StateError> {
        let timestamp = now_millis()?;
        sqlx::query(
            "INSERT INTO encrypted_secrets
             (id, purpose, owner_type, owner_id, key_version, nonce, ciphertext,
              hint, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(purpose)
        .bind(owner_type)
        .bind(owner_id)
        .bind(i64::from(key_version))
        .bind(nonce)
        .bind(ciphertext)
        .bind(hint)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;

        self.get_secret(id)
            .await?
            .ok_or(StateError::InvalidInput("inserted secret was not found"))
    }

    /// Fetch one encrypted secret by typed ID.
    pub async fn get_secret(
        &self,
        id: SecretId,
    ) -> Result<Option<EncryptedSecretRecord>, StateError> {
        let row = sqlx::query_as::<_, EncryptedSecretRow>(
            "SELECT id, purpose, owner_type, owner_id, key_version, nonce,
                    ciphertext, hint, created_at, updated_at
             FROM encrypted_secrets
             WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_secret).transpose()
    }

    /// List encrypted records in stable ID order for key rotation.
    pub async fn list_secrets(&self) -> Result<Vec<EncryptedSecretRecord>, StateError> {
        let rows = sqlx::query_as::<_, EncryptedSecretRow>(
            "SELECT id, purpose, owner_type, owner_id, key_version, nonce,
                    ciphertext, hint, created_at, updated_at
             FROM encrypted_secrets
             ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_secret).collect()
    }

    /// Replace ciphertext and key metadata without changing the secret ID.
    pub async fn update_secret_ciphertext(
        &self,
        id: SecretId,
        key_version: u32,
        nonce: &[u8],
        ciphertext: &[u8],
        hint: Option<&str>,
    ) -> Result<EncryptedSecretRecord, StateError> {
        let result = sqlx::query(
            "UPDATE encrypted_secrets
             SET key_version = ?, nonce = ?, ciphertext = ?, hint = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(i64::from(key_version))
        .bind(nonce)
        .bind(ciphertext)
        .bind(hint)
        .bind(now_millis()?)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("secret does not exist"));
        }
        self.get_secret(id)
            .await?
            .ok_or(StateError::InvalidInput("updated secret was not found"))
    }

    /// Return whether any Admin identity exists.
    pub async fn has_admin_users(&self) -> Result<bool, StateError> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM admin_users)")
                .fetch_one(&self.pool)
                .await?
                != 0,
        )
    }

    /// Create the first or a later Admin identity.
    pub async fn insert_admin_user(
        &self,
        id: AdminUserId,
        username: &str,
        password_hash: &str,
    ) -> Result<AdminUserRecord, StateError> {
        let timestamp = now_millis()?;
        sqlx::query(
            "INSERT INTO admin_users
             (id, username, password_hash, password_changed_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(username)
        .bind(password_hash)
        .bind(timestamp)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;

        self.get_admin_user(id)
            .await?
            .ok_or(StateError::InvalidInput(
                "inserted Admin user was not found",
            ))
    }

    /// Atomically create the sole first-run Admin identity. The conditional
    /// insert closes the check/insert race across concurrent setup requests.
    pub async fn insert_first_admin_user(
        &self,
        id: AdminUserId,
        username: &str,
        password_hash: &str,
    ) -> Result<Option<AdminUserRecord>, StateError> {
        let timestamp = now_millis()?;
        let result = sqlx::query(
            "INSERT INTO admin_users
             (id, username, password_hash, password_changed_at, created_at, updated_at)
             SELECT ?, ?, ?, ?, ?, ?
             WHERE NOT EXISTS (SELECT 1 FROM admin_users)",
        )
        .bind(id.to_string())
        .bind(username)
        .bind(password_hash)
        .bind(timestamp)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_admin_user(id).await
    }

    /// Find an Admin identity by exact username.
    pub async fn find_admin_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AdminUserRecord>, StateError> {
        let row = sqlx::query_as::<_, AdminUserRow>(
            "SELECT id, username, password_hash, disabled, password_changed_at,
                    created_at, updated_at
             FROM admin_users
             WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_user).transpose()
    }

    /// Find an Admin identity by typed ID.
    pub async fn get_admin_user(
        &self,
        id: AdminUserId,
    ) -> Result<Option<AdminUserRecord>, StateError> {
        let row = sqlx::query_as::<_, AdminUserRow>(
            "SELECT id, username, password_hash, disabled, password_changed_at,
                    created_at, updated_at
             FROM admin_users
             WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_user).transpose()
    }

    /// Replace a password hash and timestamp.
    pub async fn update_admin_password(
        &self,
        id: AdminUserId,
        password_hash: &str,
    ) -> Result<(), StateError> {
        let timestamp = now_millis()?;
        let result = sqlx::query(
            "UPDATE admin_users
             SET password_hash = ?, password_changed_at = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(password_hash)
        .bind(timestamp)
        .bind(timestamp)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("Admin user does not exist"));
        }
        Ok(())
    }

    /// Insert an opaque Admin session.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_admin_session(
        &self,
        id: AdminSessionId,
        user_id: AdminUserId,
        token_digest: &[u8],
        csrf_secret_digest: &[u8],
        digest_key_version: u32,
        created_at: i64,
        expires_at: i64,
        source_ip: Option<&str>,
        user_agent_hash: Option<&[u8]>,
    ) -> Result<AdminSessionRecord, StateError> {
        sqlx::query(
            "INSERT INTO admin_sessions
            (id, user_id, token_digest, csrf_secret_digest, digest_key_version,
              created_at, last_seen_at, expires_at, source_ip, user_agent_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(user_id.to_string())
        .bind(token_digest)
        .bind(csrf_secret_digest)
        .bind(i64::from(digest_key_version))
        .bind(created_at)
        .bind(created_at)
        .bind(expires_at)
        .bind(source_ip)
        .bind(user_agent_hash)
        .execute(&self.pool)
        .await?;

        self.find_admin_session_by_id(id)
            .await?
            .ok_or(StateError::InvalidInput("inserted session was not found"))
    }

    /// Find a session by its keyed cookie digest.
    pub async fn find_admin_session(
        &self,
        token_digest: &[u8],
    ) -> Result<Option<AdminSessionRecord>, StateError> {
        let row = sqlx::query_as::<_, AdminSessionRow>(
            "SELECT id, user_id, token_digest, csrf_secret_digest, digest_key_version,
                    created_at, last_seen_at, expires_at, source_ip, user_agent_hash, revoked_at
             FROM admin_sessions
             WHERE token_digest = ?",
        )
        .bind(token_digest)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_session).transpose()
    }

    async fn find_admin_session_by_id(
        &self,
        id: AdminSessionId,
    ) -> Result<Option<AdminSessionRecord>, StateError> {
        let row = sqlx::query_as::<_, AdminSessionRow>(
            "SELECT id, user_id, token_digest, csrf_secret_digest, digest_key_version,
                    created_at, last_seen_at, expires_at, source_ip, user_agent_hash, revoked_at
             FROM admin_sessions
             WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_session).transpose()
    }

    /// Touch a valid session without changing its expiry policy.
    pub async fn touch_admin_session(
        &self,
        id: AdminSessionId,
        last_seen_at: i64,
    ) -> Result<(), StateError> {
        sqlx::query("UPDATE admin_sessions SET last_seen_at = ? WHERE id = ?")
            .bind(last_seen_at)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Revoke one session.
    pub async fn revoke_admin_session(&self, id: AdminSessionId) -> Result<(), StateError> {
        sqlx::query(
            "UPDATE admin_sessions
             SET revoked_at = COALESCE(revoked_at, ?)
             WHERE id = ?",
        )
        .bind(now_millis()?)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Revoke every session for an Admin identity, e.g. after password change.
    pub async fn revoke_admin_sessions(&self, user_id: AdminUserId) -> Result<(), StateError> {
        sqlx::query(
            "UPDATE admin_sessions
             SET revoked_at = COALESCE(revoked_at, ?)
             WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(now_millis()?)
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert a Vault-bound WebDAV credential.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_webdav_credential(
        &self,
        context: &VaultContext,
        id: CredentialId,
        name: &str,
        username: &str,
        password_hash: &str,
        permissions_json: &str,
        expires_at: Option<i64>,
    ) -> Result<WebDavCredentialRecord, StateError> {
        self.ensure_vault_context(context).await?;
        let timestamp = now_millis()?;
        sqlx::query(
            "INSERT INTO webdav_credentials
             (id, vault_id, name, username, password_hash, permissions_json,
              created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(context.id().to_string())
        .bind(name)
        .bind(username)
        .bind(password_hash)
        .bind(permissions_json)
        .bind(timestamp)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;

        self.find_webdav_credential(context, username)
            .await?
            .ok_or(StateError::InvalidInput(
                "inserted WebDAV credential was not found",
            ))
    }

    /// Find a WebDAV credential only inside the supplied Vault.
    pub async fn find_webdav_credential(
        &self,
        context: &VaultContext,
        username: &str,
    ) -> Result<Option<WebDavCredentialRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, WebDavCredentialRow>(
            "SELECT id, vault_id, name, username, password_hash, permissions_json,
                    created_at, last_used_at, expires_at, revoked_at
             FROM webdav_credentials
             WHERE vault_id = ? AND username = ?",
        )
        .bind(context.id().to_string())
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_webdav).transpose()
    }

    /// List WebDAV credential metadata for one Vault without password hashes.
    pub async fn list_webdav_credentials(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<WebDavCredentialRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        if limit == 0 || limit > 1000 {
            return Err(StateError::InvalidInput(
                "WebDAV credential page is invalid",
            ));
        }
        let rows = sqlx::query_as::<_, WebDavCredentialRow>(
            "SELECT id, vault_id, name, username, password_hash, permissions_json,
                    created_at, last_used_at, expires_at, revoked_at
             FROM webdav_credentials
             WHERE vault_id = ?
             ORDER BY created_at DESC, id ASC
             LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_webdav).collect()
    }

    /// Record a successful WebDAV credential use for one Vault.
    pub async fn touch_webdav_credential(
        &self,
        context: &VaultContext,
        id: CredentialId,
        last_used_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let _write_permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        sqlx::query(
            "UPDATE webdav_credentials SET last_used_at = ?
             WHERE id = ? AND vault_id = ?",
        )
        .bind(last_used_at)
        .bind(id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update non-secret WebDAV credential metadata inside one Vault.
    pub async fn update_webdav_credential(
        &self,
        context: &VaultContext,
        id: CredentialId,
        name: &str,
        permissions_json: &str,
        expires_at: Option<i64>,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let result = sqlx::query(
            "UPDATE webdav_credentials
             SET name = ?, permissions_json = ?, expires_at = ?
             WHERE id = ? AND vault_id = ?",
        )
        .bind(name)
        .bind(permissions_json)
        .bind(expires_at)
        .bind(id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("WebDAV credential does not exist"));
        }
        Ok(())
    }

    /// Revoke one WebDAV credential inside its Vault.
    pub async fn revoke_webdav_credential(
        &self,
        context: &VaultContext,
        id: CredentialId,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE webdav_credentials
             SET revoked_at = COALESCE(revoked_at, ?)
             WHERE id = ? AND vault_id = ?",
        )
        .bind(now_millis()?)
        .bind(id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert a Vault-bound MCP PAT metadata row.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_mcp_token(
        &self,
        context: &VaultContext,
        id: CredentialId,
        name: &str,
        token_prefix: &str,
        token_digest: &[u8],
        digest_key_version: u32,
        scopes_json: &str,
        expires_at: Option<i64>,
    ) -> Result<McpTokenRecord, StateError> {
        self.ensure_vault_context(context).await?;
        let timestamp = now_millis()?;
        sqlx::query(
            "INSERT INTO mcp_tokens
            (id, vault_id, name, token_prefix, token_digest, digest_key_version,
              scopes_json, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(context.id().to_string())
        .bind(name)
        .bind(token_prefix)
        .bind(token_digest)
        .bind(i64::from(digest_key_version))
        .bind(scopes_json)
        .bind(timestamp)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;

        self.find_mcp_token(context, token_prefix, token_digest)
            .await?
            .ok_or(StateError::InvalidInput("inserted MCP token was not found"))
    }

    /// Find a PAT by both visible prefix and keyed digest inside one Vault.
    pub async fn find_mcp_token(
        &self,
        context: &VaultContext,
        token_prefix: &str,
        token_digest: &[u8],
    ) -> Result<Option<McpTokenRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, McpTokenRow>(
            "SELECT id, vault_id, name, token_prefix, token_digest, digest_key_version,
                    scopes_json, created_at, last_used_at, expires_at, revoked_at
             FROM mcp_tokens
             WHERE vault_id = ? AND token_prefix = ? AND token_digest = ?",
        )
        .bind(context.id().to_string())
        .bind(token_prefix)
        .bind(token_digest)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_mcp).transpose()
    }

    /// List Vault-bound MCP token metadata without token digests.
    pub async fn list_mcp_tokens(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<McpTokenRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        if limit == 0 || limit > 1000 {
            return Err(StateError::InvalidInput("MCP token page is invalid"));
        }
        let rows = sqlx::query_as::<_, McpTokenRow>(
            "SELECT id, vault_id, name, token_prefix, token_digest, digest_key_version,
                    scopes_json, created_at, last_used_at, expires_at, revoked_at
             FROM mcp_tokens
             WHERE vault_id = ?
             ORDER BY created_at DESC, id ASC
             LIMIT ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_mcp).collect()
    }

    /// Record a successful PAT use for one Vault.
    pub async fn touch_mcp_token(
        &self,
        context: &VaultContext,
        id: CredentialId,
        last_used_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE mcp_tokens SET last_used_at = ?
             WHERE id = ? AND vault_id = ?",
        )
        .bind(last_used_at)
        .bind(id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Revoke one PAT inside its Vault.
    pub async fn revoke_mcp_token(
        &self,
        context: &VaultContext,
        id: CredentialId,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE mcp_tokens
             SET revoked_at = COALESCE(revoked_at, ?)
             WHERE id = ? AND vault_id = ?",
        )
        .bind(now_millis()?)
        .bind(id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert one OAuth resource-server issuer configuration.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_oauth_issuer(
        &self,
        id: OAuthIssuerId,
        name: &str,
        issuer_url: &str,
        discovery_url: Option<&str>,
        audience: &str,
        resource: Option<&str>,
        jwks_cache_json: Option<&str>,
        enabled: bool,
    ) -> Result<OAuthIssuerRecord, StateError> {
        let timestamp = now_millis()?;
        sqlx::query(
            "INSERT INTO oauth_issuers
             (id, name, issuer_url, discovery_url, audience, resource,
              jwks_cache_json, jwks_cached_at, enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(issuer_url)
        .bind(discovery_url)
        .bind(audience)
        .bind(resource)
        .bind(jwks_cache_json)
        .bind(jwks_cache_json.map(|_| timestamp))
        .bind(if enabled { 1_i64 } else { 0 })
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        self.get_oauth_issuer(id)
            .await?
            .ok_or(StateError::InvalidInput(
                "inserted OAuth issuer was not found",
            ))
    }

    /// Replace one issuer configuration while preserving its stable ID.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_oauth_issuer(
        &self,
        id: OAuthIssuerId,
        name: &str,
        issuer_url: &str,
        discovery_url: Option<&str>,
        audience: &str,
        resource: Option<&str>,
        jwks_cache_json: Option<&str>,
        enabled: bool,
    ) -> Result<OAuthIssuerRecord, StateError> {
        let timestamp = now_millis()?;
        let result = sqlx::query(
            "UPDATE oauth_issuers
             SET name = ?, issuer_url = ?, discovery_url = ?, audience = ?, resource = ?,
                 jwks_cache_json = ?, jwks_cached_at = ?, enabled = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(name)
        .bind(issuer_url)
        .bind(discovery_url)
        .bind(audience)
        .bind(resource)
        .bind(jwks_cache_json)
        .bind(jwks_cache_json.map(|_| timestamp))
        .bind(if enabled { 1_i64 } else { 0 })
        .bind(timestamp)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("OAuth issuer does not exist"));
        }
        self.get_oauth_issuer(id)
            .await?
            .ok_or(StateError::InvalidInput(
                "updated OAuth issuer was not found",
            ))
    }

    /// Fetch an issuer by its stable ID.
    pub async fn get_oauth_issuer(
        &self,
        id: OAuthIssuerId,
    ) -> Result<Option<OAuthIssuerRecord>, StateError> {
        let row = sqlx::query_as::<_, OAuthIssuerRow>(
            "SELECT id, name, issuer_url, discovery_url, audience, resource,
                    jwks_cache_json, jwks_cached_at, enabled, created_at, updated_at
             FROM oauth_issuers
             WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_issuer).transpose()
    }

    /// Fetch an enabled/disabled issuer by its exact issuer URL.
    pub async fn find_oauth_issuer(
        &self,
        issuer_url: &str,
    ) -> Result<Option<OAuthIssuerRecord>, StateError> {
        let row = sqlx::query_as::<_, OAuthIssuerRow>(
            "SELECT id, name, issuer_url, discovery_url, audience, resource,
                    jwks_cache_json, jwks_cached_at, enabled, created_at, updated_at
             FROM oauth_issuers
             WHERE issuer_url = ?",
        )
        .bind(issuer_url)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_issuer).transpose()
    }

    /// List configured OAuth resource-server issuers without cached key bodies.
    pub async fn list_oauth_issuers(
        &self,
        limit: u32,
    ) -> Result<Vec<OAuthIssuerRecord>, StateError> {
        if limit == 0 || limit > 1000 {
            return Err(StateError::InvalidInput("OAuth issuer page is invalid"));
        }
        let rows = sqlx::query_as::<_, OAuthIssuerRow>(
            "SELECT id, name, issuer_url, discovery_url, audience, resource,
                    jwks_cache_json, jwks_cached_at, enabled, created_at, updated_at
             FROM oauth_issuers
             ORDER BY name ASC, id ASC
             LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_issuer).collect()
    }

    /// Insert a Vault-bound OAuth subject grant.
    pub async fn insert_oauth_grant(
        &self,
        context: &VaultContext,
        id: OAuthGrantId,
        issuer_id: OAuthIssuerId,
        subject: &str,
        scopes_json: &str,
    ) -> Result<OAuthGrantRecord, StateError> {
        self.ensure_vault_context(context).await?;
        let timestamp = now_millis()?;
        sqlx::query(
            "INSERT INTO oauth_subject_grants
             (id, issuer_id, subject, vault_id, scopes_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(issuer_id, subject, vault_id) DO UPDATE SET
                 scopes_json = excluded.scopes_json,
                 revoked_at = NULL,
                 updated_at = excluded.updated_at",
        )
        .bind(id.to_string())
        .bind(issuer_id.to_string())
        .bind(subject)
        .bind(context.id().to_string())
        .bind(scopes_json)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        self.find_oauth_grant(context, issuer_id, subject)
            .await?
            .ok_or(StateError::InvalidInput(
                "inserted OAuth grant was not found",
            ))
    }

    /// Find a subject grant only for the requested Vault and issuer.
    pub async fn find_oauth_grant(
        &self,
        context: &VaultContext,
        issuer_id: OAuthIssuerId,
        subject: &str,
    ) -> Result<Option<OAuthGrantRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, OAuthGrantRow>(
            "SELECT id, issuer_id, subject, vault_id, scopes_json,
                    created_at, updated_at, revoked_at
             FROM oauth_subject_grants
             WHERE issuer_id = ? AND subject = ? AND vault_id = ?",
        )
        .bind(issuer_id.to_string())
        .bind(subject)
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_grant).transpose()
    }

    /// List bounded OAuth grants for exactly one Vault without crossing the
    /// isolation predicate.
    pub async fn list_oauth_grants(
        &self,
        context: &VaultContext,
        include_revoked: bool,
        limit: u32,
    ) -> Result<Vec<OAuthGrantRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        if limit == 0 || limit > 1000 {
            return Err(StateError::InvalidInput("OAuth grant page is invalid"));
        }
        let rows = if include_revoked {
            sqlx::query_as::<_, OAuthGrantRow>(
                "SELECT id, issuer_id, subject, vault_id, scopes_json,
                        created_at, updated_at, revoked_at
                 FROM oauth_subject_grants
                 WHERE vault_id = ?
                 ORDER BY created_at DESC, id ASC
                 LIMIT ?",
            )
            .bind(context.id().to_string())
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, OAuthGrantRow>(
                "SELECT id, issuer_id, subject, vault_id, scopes_json,
                        created_at, updated_at, revoked_at
                 FROM oauth_subject_grants
                 WHERE vault_id = ? AND revoked_at IS NULL
                 ORDER BY created_at DESC, id ASC
                 LIMIT ?",
            )
            .bind(context.id().to_string())
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter().map(row_to_grant).collect()
    }

    /// Revoke one subject grant within its Vault.
    pub async fn revoke_oauth_grant(
        &self,
        context: &VaultContext,
        id: OAuthGrantId,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let result = sqlx::query(
            "UPDATE oauth_subject_grants
             SET revoked_at = COALESCE(revoked_at, ?)
             WHERE id = ? AND vault_id = ?",
        )
        .bind(now_millis()?)
        .bind(id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("OAuth grant does not exist"));
        }
        Ok(())
    }

    /// Confirm that a context matches the registered Vault ID, slug, and root.
    pub async fn ensure_vault_context(&self, context: &VaultContext) -> Result<(), StateError> {
        let content_root = context
            .content_root()
            .to_str()
            .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM vaults
                 WHERE id = ? AND slug = ? AND content_root = ?
             )",
        )
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .bind(content_root)
        .fetch_one(&self.pool)
        .await?;
        if exists == 0 {
            return Err(StateError::InvalidInput("Vault context is not registered"));
        }
        Ok(())
    }
}

fn row_to_secret(row: EncryptedSecretRow) -> Result<EncryptedSecretRecord, StateError> {
    Ok(EncryptedSecretRecord {
        id: SecretId::parse(&row.id)?,
        purpose: row.purpose,
        owner_type: row.owner_type,
        owner_id: row.owner_id,
        key_version: u32::try_from(row.key_version)
            .map_err(|_| StateError::InvalidInput("secret key version is invalid"))?,
        nonce: row.nonce,
        ciphertext: row.ciphertext,
        hint: row.hint,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_admin_user(row: AdminUserRow) -> Result<AdminUserRecord, StateError> {
    Ok(AdminUserRecord {
        id: AdminUserId::parse(&row.id)?,
        username: row.username,
        password_hash: row.password_hash,
        disabled: row.disabled != 0,
        password_changed_at: row.password_changed_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_session(row: AdminSessionRow) -> Result<AdminSessionRecord, StateError> {
    Ok(AdminSessionRecord {
        id: AdminSessionId::parse(&row.id)?,
        user_id: AdminUserId::parse(&row.user_id)?,
        token_digest: row.token_digest,
        digest_key_version: u32::try_from(row.digest_key_version)
            .map_err(|_| StateError::InvalidInput("session key version is invalid"))?,
        csrf_secret_digest: row.csrf_secret_digest,
        created_at: row.created_at,
        last_seen_at: row.last_seen_at,
        expires_at: row.expires_at,
        source_ip: row.source_ip,
        user_agent_hash: row.user_agent_hash,
        revoked_at: row.revoked_at,
    })
}

fn row_to_webdav(row: WebDavCredentialRow) -> Result<WebDavCredentialRecord, StateError> {
    Ok(WebDavCredentialRecord {
        id: CredentialId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        name: row.name,
        username: row.username,
        password_hash: row.password_hash,
        permissions_json: row.permissions_json,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
        expires_at: row.expires_at,
        revoked_at: row.revoked_at,
    })
}

fn row_to_mcp(row: McpTokenRow) -> Result<McpTokenRecord, StateError> {
    Ok(McpTokenRecord {
        id: CredentialId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        name: row.name,
        token_prefix: row.token_prefix,
        token_digest: row.token_digest,
        digest_key_version: u32::try_from(row.digest_key_version)
            .map_err(|_| StateError::InvalidInput("token key version is invalid"))?,
        scopes_json: row.scopes_json,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
        expires_at: row.expires_at,
        revoked_at: row.revoked_at,
    })
}

fn row_to_issuer(row: OAuthIssuerRow) -> Result<OAuthIssuerRecord, StateError> {
    Ok(OAuthIssuerRecord {
        id: OAuthIssuerId::parse(&row.id)?,
        name: row.name,
        issuer_url: row.issuer_url,
        discovery_url: row.discovery_url,
        audience: row.audience,
        resource: row.resource,
        jwks_cache_json: row.jwks_cache_json,
        jwks_cached_at: row.jwks_cached_at,
        enabled: row.enabled != 0,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_grant(row: OAuthGrantRow) -> Result<OAuthGrantRecord, StateError> {
    Ok(OAuthGrantRecord {
        id: OAuthGrantId::parse(&row.id)?,
        issuer_id: OAuthIssuerId::parse(&row.issuer_id)?,
        subject: row.subject,
        vault_id: VaultId::parse(&row.vault_id)?,
        scopes_json: row.scopes_json,
        created_at: row.created_at,
        updated_at: row.updated_at,
        revoked_at: row.revoked_at,
    })
}
