//! Durable state for the built-in OAuth authorization server.

use mcp_vault_domain::{
    OAuthAccessTokenId, OAuthAuthorizationCodeId, OAuthAuthorizationRequestId, OAuthClientId,
    OAuthLocalUserId, OAuthRefreshTokenId, OAuthTokenFamilyId, VaultContext, VaultId,
};
use sqlx::FromRow;

use super::AuthStateRepository;
use crate::{StateError, now_millis};

/// One Vault-bound interactive OAuth identity. Password hashes never leave the
/// authentication application boundary.
#[derive(Clone, Eq, PartialEq)]
pub struct OAuthLocalUserRecord {
    pub id: OAuthLocalUserId,
    pub vault_id: VaultId,
    pub username: String,
    pub password_hash: String,
    pub scopes_json: String,
    pub enabled: bool,
    pub password_changed_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl std::fmt::Debug for OAuthLocalUserRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthLocalUserRecord")
            .field("id", &self.id)
            .field("vault_id", &self.vault_id)
            .field("username", &self.username)
            .field("password_hash", &"[REDACTED]")
            .field("scopes_json", &self.scopes_json)
            .field("enabled", &self.enabled)
            .field("password_changed_at", &self.password_changed_at)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// RFC 7591 public-client registration. No client secret is issued or stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthClientRecord {
    pub id: OAuthClientId,
    pub client_name: String,
    pub redirect_uris_json: String,
    pub grant_types_json: String,
    pub response_types_json: String,
    pub token_endpoint_auth_method: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Short-lived validated authorization request. The browser receives only the
/// random handle; SQLite receives only its keyed digest.
#[derive(Clone, Eq, PartialEq)]
pub struct OAuthAuthorizationRequestRecord {
    pub id: OAuthAuthorizationRequestId,
    pub request_digest: Vec<u8>,
    pub digest_key_version: u32,
    pub client_id: OAuthClientId,
    pub vault_id: VaultId,
    pub resource: String,
    pub redirect_uri: String,
    pub scopes_json: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub consumed_at: Option<i64>,
}

/// Single-use authorization code metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct OAuthAuthorizationCodeRecord {
    pub id: OAuthAuthorizationCodeId,
    pub code_digest: Vec<u8>,
    pub digest_key_version: u32,
    pub client_id: OAuthClientId,
    pub user_id: OAuthLocalUserId,
    pub vault_id: VaultId,
    pub resource: String,
    pub redirect_uri: String,
    pub scopes_json: String,
    pub code_challenge: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub consumed_at: Option<i64>,
}

/// Locally issued opaque access-token metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct OAuthAccessTokenRecord {
    pub id: OAuthAccessTokenId,
    pub family_id: OAuthTokenFamilyId,
    pub token_prefix: String,
    pub token_digest: Vec<u8>,
    pub digest_key_version: u32,
    pub client_id: OAuthClientId,
    pub user_id: OAuthLocalUserId,
    pub vault_id: VaultId,
    pub resource: String,
    pub scopes_json: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Locally issued rotating refresh-token metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct OAuthRefreshTokenRecord {
    pub id: OAuthRefreshTokenId,
    pub family_id: OAuthTokenFamilyId,
    pub token_prefix: String,
    pub token_digest: Vec<u8>,
    pub digest_key_version: u32,
    pub client_id: OAuthClientId,
    pub user_id: OAuthLocalUserId,
    pub vault_id: VaultId,
    pub resource: String,
    pub scopes_json: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub rotated_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Secret-safe insertion material for a new authorization code.
pub struct NewOAuthAuthorizationCode<'a> {
    pub id: OAuthAuthorizationCodeId,
    pub code_digest: &'a [u8],
    pub digest_key_version: u32,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Secret-safe insertion material for a new access token.
pub struct NewOAuthAccessToken<'a> {
    pub id: OAuthAccessTokenId,
    pub family_id: OAuthTokenFamilyId,
    pub token_prefix: &'a str,
    pub token_digest: &'a [u8],
    pub digest_key_version: u32,
    pub created_at: i64,
    pub expires_at: i64,
    /// Optional narrowed scopes for refresh rotation; `None` inherits the
    /// frozen grant from the source code/token.
    pub scopes_json: Option<&'a str>,
}

/// Secret-safe insertion material for a new refresh token.
pub struct NewOAuthRefreshToken<'a> {
    pub id: OAuthRefreshTokenId,
    pub family_id: OAuthTokenFamilyId,
    pub token_prefix: &'a str,
    pub token_digest: &'a [u8],
    pub digest_key_version: u32,
    pub created_at: i64,
    pub expires_at: i64,
    /// Optional narrowed scopes for refresh rotation.
    pub scopes_json: Option<&'a str>,
}

#[derive(FromRow)]
struct OAuthLocalUserRow {
    id: String,
    vault_id: String,
    username: String,
    password_hash: String,
    scopes_json: String,
    enabled: i64,
    password_changed_at: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(FromRow)]
struct OAuthClientRow {
    id: String,
    client_name: String,
    redirect_uris_json: String,
    grant_types_json: String,
    response_types_json: String,
    token_endpoint_auth_method: String,
    created_at: i64,
    last_used_at: Option<i64>,
    revoked_at: Option<i64>,
}

#[derive(FromRow)]
struct OAuthAuthorizationRequestRow {
    id: String,
    request_digest: Vec<u8>,
    digest_key_version: i64,
    client_id: String,
    vault_id: String,
    resource: String,
    redirect_uri: String,
    scopes_json: String,
    state: Option<String>,
    code_challenge: String,
    created_at: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
}

#[derive(FromRow)]
struct OAuthAuthorizationCodeRow {
    id: String,
    code_digest: Vec<u8>,
    digest_key_version: i64,
    client_id: String,
    user_id: String,
    vault_id: String,
    resource: String,
    redirect_uri: String,
    scopes_json: String,
    code_challenge: String,
    created_at: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
}

#[derive(FromRow)]
struct OAuthAccessTokenRow {
    id: String,
    family_id: String,
    token_prefix: String,
    token_digest: Vec<u8>,
    digest_key_version: i64,
    client_id: String,
    user_id: String,
    vault_id: String,
    resource: String,
    scopes_json: String,
    created_at: i64,
    expires_at: i64,
    last_used_at: Option<i64>,
    revoked_at: Option<i64>,
}

#[derive(FromRow)]
struct OAuthRefreshTokenRow {
    id: String,
    family_id: String,
    token_prefix: String,
    token_digest: Vec<u8>,
    digest_key_version: i64,
    client_id: String,
    user_id: String,
    vault_id: String,
    resource: String,
    scopes_json: String,
    created_at: i64,
    expires_at: i64,
    rotated_at: Option<i64>,
    revoked_at: Option<i64>,
}

impl AuthStateRepository {
    /// Create or replace the one local OAuth user for a Vault and revoke all
    /// previously issued authorization state in the same transaction.
    pub async fn upsert_local_oauth_user(
        &self,
        context: &VaultContext,
        id: OAuthLocalUserId,
        username: &str,
        password_hash: &str,
        scopes_json: &str,
    ) -> Result<OAuthLocalUserRecord, StateError> {
        self.ensure_vault_context(context).await?;
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let timestamp = now_millis()?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO oauth_local_users
             (id, vault_id, username, password_hash, scopes_json, enabled,
              password_changed_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?)
             ON CONFLICT(vault_id) DO UPDATE SET
                 username = excluded.username,
                 password_hash = excluded.password_hash,
                 scopes_json = excluded.scopes_json,
                 enabled = 1,
                 password_changed_at = excluded.password_changed_at,
                 updated_at = excluded.updated_at",
        )
        .bind(id.to_string())
        .bind(context.id().to_string())
        .bind(username)
        .bind(password_hash)
        .bind(scopes_json)
        .bind(timestamp)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await?;
        let user_id: String =
            sqlx::query_scalar("SELECT id FROM oauth_local_users WHERE vault_id = ?")
                .bind(context.id().to_string())
                .fetch_one(&mut *transaction)
                .await?;
        revoke_user_state(&mut transaction, &user_id, context.id(), timestamp).await?;
        transaction.commit().await?;
        self.get_local_oauth_user(context)
            .await?
            .ok_or(StateError::InvalidInput("local OAuth user was not found"))
    }

    /// Return the local OAuth user for exactly one Vault.
    pub async fn get_local_oauth_user(
        &self,
        context: &VaultContext,
    ) -> Result<Option<OAuthLocalUserRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, OAuthLocalUserRow>(
            "SELECT id, vault_id, username, password_hash, scopes_json, enabled,
                    password_changed_at, created_at, updated_at
             FROM oauth_local_users WHERE vault_id = ?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_local_user).transpose()
    }

    /// Disable a Vault's local OAuth user and revoke all outstanding state.
    pub async fn disable_local_oauth_user(
        &self,
        context: &VaultContext,
    ) -> Result<bool, StateError> {
        self.ensure_vault_context(context).await?;
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let timestamp = now_millis()?;
        let mut transaction = self.pool.begin().await?;
        let user_id =
            sqlx::query_scalar::<_, String>("SELECT id FROM oauth_local_users WHERE vault_id = ?")
                .bind(context.id().to_string())
                .fetch_optional(&mut *transaction)
                .await?;
        let Some(user_id) = user_id else {
            transaction.rollback().await?;
            return Ok(false);
        };
        sqlx::query(
            "UPDATE oauth_local_users SET enabled = 0, updated_at = ?
             WHERE id = ? AND vault_id = ?",
        )
        .bind(timestamp)
        .bind(&user_id)
        .bind(context.id().to_string())
        .execute(&mut *transaction)
        .await?;
        revoke_user_state(&mut transaction, &user_id, context.id(), timestamp).await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Register one bounded public OAuth client without issuing a secret.
    pub async fn insert_oauth_client(
        &self,
        id: OAuthClientId,
        client_name: &str,
        redirect_uris_json: &str,
        grant_types_json: &str,
        response_types_json: &str,
        max_active_clients: u32,
    ) -> Result<OAuthClientRecord, StateError> {
        if max_active_clients == 0 || max_active_clients > 10_000 {
            return Err(StateError::InvalidInput("OAuth client limit is invalid"));
        }
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let timestamp = now_millis()?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM oauth_clients
             WHERE revoked_at IS NOT NULL
                OR (last_used_at IS NULL AND created_at < ?)",
        )
        .bind(timestamp.saturating_sub(24 * 60 * 60 * 1000))
        .execute(&mut *transaction)
        .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM oauth_clients WHERE revoked_at IS NULL")
                .fetch_one(&mut *transaction)
                .await?;
        if count >= i64::from(max_active_clients) {
            transaction.rollback().await?;
            return Err(StateError::InvalidInput("OAuth client limit was reached"));
        }
        sqlx::query(
            "INSERT INTO oauth_clients
             (id, client_name, redirect_uris_json, grant_types_json,
              response_types_json, token_endpoint_auth_method, created_at)
             VALUES (?, ?, ?, ?, ?, 'none', ?)",
        )
        .bind(id.to_string())
        .bind(client_name)
        .bind(redirect_uris_json)
        .bind(grant_types_json)
        .bind(response_types_json)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.get_oauth_client(id)
            .await?
            .ok_or(StateError::InvalidInput("OAuth client was not found"))
    }

    /// Fetch one DCR client by exact public client ID.
    pub async fn get_oauth_client(
        &self,
        id: OAuthClientId,
    ) -> Result<Option<OAuthClientRecord>, StateError> {
        let row = sqlx::query_as::<_, OAuthClientRow>(
            "SELECT id, client_name, redirect_uris_json, grant_types_json,
                    response_types_json, token_endpoint_auth_method,
                    created_at, last_used_at, revoked_at
             FROM oauth_clients WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_client).transpose()
    }

    /// Persist one validated, short-lived authorization request handle.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_oauth_authorization_request(
        &self,
        context: &VaultContext,
        id: OAuthAuthorizationRequestId,
        request_digest: &[u8],
        digest_key_version: u32,
        client_id: OAuthClientId,
        resource: &str,
        redirect_uri: &str,
        scopes_json: &str,
        state: Option<&str>,
        code_challenge: &str,
        created_at: i64,
        expires_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "INSERT INTO oauth_authorization_requests
             (id, request_digest, digest_key_version, client_id, vault_id,
              resource, redirect_uri, scopes_json, state, code_challenge,
              created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(request_digest)
        .bind(i64::from(digest_key_version))
        .bind(client_id.to_string())
        .bind(context.id().to_string())
        .bind(resource)
        .bind(redirect_uri)
        .bind(scopes_json)
        .bind(state)
        .bind(code_challenge)
        .bind(created_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Find an authorization request only inside the expected Vault.
    pub async fn find_oauth_authorization_request(
        &self,
        context: &VaultContext,
        request_digest: &[u8],
        digest_key_version: u32,
    ) -> Result<Option<OAuthAuthorizationRequestRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, OAuthAuthorizationRequestRow>(
            "SELECT id, request_digest, digest_key_version, client_id, vault_id,
                    resource, redirect_uri, scopes_json, state, code_challenge,
                    created_at, expires_at, consumed_at
             FROM oauth_authorization_requests
             WHERE vault_id = ? AND request_digest = ? AND digest_key_version = ?",
        )
        .bind(context.id().to_string())
        .bind(request_digest)
        .bind(i64::from(digest_key_version))
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_authorization_request).transpose()
    }

    /// Atomically record completion and create a fresh single-use code.
    ///
    /// A still-valid interactive request may be submitted more than once.
    /// Browsers and reverse proxies can replay a form POST after the first
    /// response has already committed; each authenticated retry therefore
    /// receives its own code while `consumed_at` retains the first completion
    /// time. Authorization codes themselves remain strictly single-use.
    pub async fn complete_oauth_authorization_request(
        &self,
        context: &VaultContext,
        request_id: OAuthAuthorizationRequestId,
        user_id: OAuthLocalUserId,
        code: NewOAuthAuthorizationCode<'_>,
        consumed_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let mut transaction = self.pool.begin().await?;
        let user_enabled: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM oauth_local_users
                 WHERE id = ? AND vault_id = ? AND enabled = 1
             )",
        )
        .bind(user_id.to_string())
        .bind(context.id().to_string())
        .fetch_one(&mut *transaction)
        .await?;
        if user_enabled == 0 {
            transaction.rollback().await?;
            return Err(StateError::InvalidInput("local OAuth user is disabled"));
        }
        let inserted = sqlx::query(
            "INSERT INTO oauth_authorization_codes
             (id, code_digest, digest_key_version, client_id, user_id, vault_id,
              resource, redirect_uri, scopes_json, code_challenge,
              created_at, expires_at)
             SELECT ?, ?, ?, client_id, ?, vault_id, resource, redirect_uri,
                    scopes_json, code_challenge, ?, ?
             FROM oauth_authorization_requests
             WHERE id = ? AND vault_id = ? AND expires_at > ?",
        )
        .bind(code.id.to_string())
        .bind(code.code_digest)
        .bind(i64::from(code.digest_key_version))
        .bind(user_id.to_string())
        .bind(code.created_at)
        .bind(code.expires_at)
        .bind(request_id.to_string())
        .bind(context.id().to_string())
        .bind(consumed_at)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StateError::InvalidInput(
                "OAuth authorization code was not created",
            ));
        }
        sqlx::query(
            "UPDATE oauth_authorization_requests
             SET consumed_at = COALESCE(consumed_at, ?)
             WHERE id = ? AND vault_id = ?",
        )
        .bind(consumed_at)
        .bind(request_id.to_string())
        .bind(context.id().to_string())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE oauth_clients SET last_used_at = ?
             WHERE id = (SELECT client_id FROM oauth_authorization_requests WHERE id = ?)",
        )
        .bind(consumed_at)
        .bind(request_id.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Find a code digest only inside the resource's Vault.
    pub async fn find_oauth_authorization_code(
        &self,
        context: &VaultContext,
        code_digest: &[u8],
        digest_key_version: u32,
    ) -> Result<Option<OAuthAuthorizationCodeRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, OAuthAuthorizationCodeRow>(
            "SELECT id, code_digest, digest_key_version, client_id, user_id,
                    vault_id, resource, redirect_uri, scopes_json,
                    code_challenge, created_at, expires_at, consumed_at
             FROM oauth_authorization_codes
             WHERE vault_id = ? AND code_digest = ? AND digest_key_version = ?",
        )
        .bind(context.id().to_string())
        .bind(code_digest)
        .bind(i64::from(digest_key_version))
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_authorization_code).transpose()
    }

    /// Atomically consume a code and mint one access/refresh token pair using
    /// the frozen client, Vault, resource and scopes from the code row.
    pub async fn consume_oauth_code_and_insert_tokens(
        &self,
        context: &VaultContext,
        code_id: OAuthAuthorizationCodeId,
        access: NewOAuthAccessToken<'_>,
        refresh: NewOAuthRefreshToken<'_>,
        consumed_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        if access.family_id != refresh.family_id {
            return Err(StateError::InvalidInput("OAuth token family is invalid"));
        }
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let mut transaction = self.pool.begin().await?;
        let consumed = sqlx::query(
            "UPDATE oauth_authorization_codes SET consumed_at = ?
             WHERE id = ? AND vault_id = ? AND consumed_at IS NULL AND expires_at > ?",
        )
        .bind(consumed_at)
        .bind(code_id.to_string())
        .bind(context.id().to_string())
        .bind(consumed_at)
        .execute(&mut *transaction)
        .await?;
        if consumed.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StateError::InvalidInput(
                "OAuth authorization code is unavailable",
            ));
        }
        insert_access_from_code(&mut transaction, context, code_id, &access).await?;
        insert_refresh_from_code(&mut transaction, context, code_id, &refresh).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Find a locally issued access token by prefix and keyed digest in one
    /// Vault. Revoked/expired rows are returned so Auth can fail uniformly.
    pub async fn find_oauth_access_token(
        &self,
        context: &VaultContext,
        token_prefix: &str,
        token_digest: &[u8],
        digest_key_version: u32,
    ) -> Result<Option<OAuthAccessTokenRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, OAuthAccessTokenRow>(
            "SELECT id, family_id, token_prefix, token_digest,
                    digest_key_version, client_id, user_id, vault_id, resource,
                    scopes_json, created_at, expires_at, last_used_at, revoked_at
             FROM oauth_access_tokens
             WHERE vault_id = ? AND token_prefix = ? AND token_digest = ?
               AND digest_key_version = ?",
        )
        .bind(context.id().to_string())
        .bind(token_prefix)
        .bind(token_digest)
        .bind(i64::from(digest_key_version))
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_access_token).transpose()
    }

    pub async fn touch_oauth_access_token(
        &self,
        context: &VaultContext,
        id: OAuthAccessTokenId,
        used_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        sqlx::query(
            "UPDATE oauth_access_tokens SET last_used_at = ?
             WHERE id = ? AND vault_id = ?",
        )
        .bind(used_at)
        .bind(id.to_string())
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Find a refresh token inside the exact resource Vault.
    pub async fn find_oauth_refresh_token(
        &self,
        context: &VaultContext,
        token_prefix: &str,
        token_digest: &[u8],
        digest_key_version: u32,
    ) -> Result<Option<OAuthRefreshTokenRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, OAuthRefreshTokenRow>(
            "SELECT id, family_id, token_prefix, token_digest,
                    digest_key_version, client_id, user_id, vault_id, resource,
                    scopes_json, created_at, expires_at, rotated_at, revoked_at
             FROM oauth_refresh_tokens
             WHERE vault_id = ? AND token_prefix = ? AND token_digest = ?
               AND digest_key_version = ?",
        )
        .bind(context.id().to_string())
        .bind(token_prefix)
        .bind(token_digest)
        .bind(i64::from(digest_key_version))
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_refresh_token).transpose()
    }

    /// Rotate one refresh token and mint the next pair in one transaction.
    pub async fn rotate_oauth_refresh_token(
        &self,
        context: &VaultContext,
        old_id: OAuthRefreshTokenId,
        access: NewOAuthAccessToken<'_>,
        refresh: NewOAuthRefreshToken<'_>,
        rotated_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        if access.family_id != refresh.family_id {
            return Err(StateError::InvalidInput("OAuth token family is invalid"));
        }
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let mut transaction = self.pool.begin().await?;
        let rotated = sqlx::query(
            "UPDATE oauth_refresh_tokens SET rotated_at = ?
             WHERE id = ? AND vault_id = ? AND rotated_at IS NULL
               AND revoked_at IS NULL AND expires_at > ?",
        )
        .bind(rotated_at)
        .bind(old_id.to_string())
        .bind(context.id().to_string())
        .bind(rotated_at)
        .execute(&mut *transaction)
        .await?;
        if rotated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StateError::InvalidInput(
                "OAuth refresh token is unavailable",
            ));
        }
        insert_access_from_refresh(&mut transaction, context, old_id, &access).await?;
        insert_refresh_from_refresh(&mut transaction, context, old_id, &refresh).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Revoke both access and refresh rows in a replayed token family.
    pub async fn revoke_oauth_token_family(
        &self,
        context: &VaultContext,
        family_id: OAuthTokenFamilyId,
        revoked_at: i64,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, ?)
             WHERE vault_id = ? AND family_id = ?",
        )
        .bind(revoked_at)
        .bind(context.id().to_string())
        .bind(family_id.to_string())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, ?)
             WHERE vault_id = ? AND family_id = ?",
        )
        .bind(revoked_at)
        .bind(context.id().to_string())
        .bind(family_id.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Bounded opportunistic cleanup for already unusable OAuth rows.
    pub async fn cleanup_local_oauth(&self, remove_before: i64) -> Result<(), StateError> {
        let _permit = self
            .write_gate
            .acquire()
            .await
            .map_err(|_| StateError::Connection("state write gate is closed".to_owned()))?;
        let mut transaction = self.pool.begin().await?;
        for statement in [
            "DELETE FROM oauth_authorization_requests
             WHERE expires_at < ? OR (consumed_at IS NOT NULL AND consumed_at < ?)",
            "DELETE FROM oauth_authorization_codes
             WHERE expires_at < ? OR (consumed_at IS NOT NULL AND consumed_at < ?)",
            "DELETE FROM oauth_access_tokens
             WHERE expires_at < ? OR (revoked_at IS NOT NULL AND revoked_at < ?)",
            "DELETE FROM oauth_refresh_tokens
             WHERE expires_at < ? OR (revoked_at IS NOT NULL AND revoked_at < ?)",
        ] {
            sqlx::query(statement)
                .bind(remove_before)
                .bind(remove_before)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

async fn revoke_user_state(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: &str,
    vault_id: VaultId,
    timestamp: i64,
) -> Result<(), StateError> {
    // Authorization requests have no user foreign key because they are
    // created before login. Delete this short-lived browser state when the
    // Vault OAuth credential changes so a previously authenticated retry can
    // never cross a password rotation.
    sqlx::query("DELETE FROM oauth_authorization_requests WHERE vault_id = ?")
        .bind(vault_id.to_string())
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "UPDATE oauth_authorization_codes SET consumed_at = COALESCE(consumed_at, ?)
         WHERE vault_id = ? AND user_id = ?",
    )
    .bind(timestamp)
    .bind(vault_id.to_string())
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, ?)
         WHERE vault_id = ? AND user_id = ?",
    )
    .bind(timestamp)
    .bind(vault_id.to_string())
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, ?)
         WHERE vault_id = ? AND user_id = ?",
    )
    .bind(timestamp)
    .bind(vault_id.to_string())
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_access_from_code(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    context: &VaultContext,
    code_id: OAuthAuthorizationCodeId,
    token: &NewOAuthAccessToken<'_>,
) -> Result<(), StateError> {
    let result = sqlx::query(
        "INSERT INTO oauth_access_tokens
         (id, family_id, token_prefix, token_digest, digest_key_version,
          client_id, user_id, vault_id, resource, scopes_json,
          created_at, expires_at)
         SELECT ?, ?, ?, ?, ?, client_id, user_id, vault_id, resource,
                COALESCE(?, scopes_json), ?, ?
         FROM oauth_authorization_codes WHERE id = ? AND vault_id = ?",
    )
    .bind(token.id.to_string())
    .bind(token.family_id.to_string())
    .bind(token.token_prefix)
    .bind(token.token_digest)
    .bind(i64::from(token.digest_key_version))
    .bind(token.scopes_json)
    .bind(token.created_at)
    .bind(token.expires_at)
    .bind(code_id.to_string())
    .bind(context.id().to_string())
    .execute(&mut **transaction)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StateError::InvalidInput(
            "OAuth access token was not created",
        ))
    }
}

async fn insert_refresh_from_code(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    context: &VaultContext,
    code_id: OAuthAuthorizationCodeId,
    token: &NewOAuthRefreshToken<'_>,
) -> Result<(), StateError> {
    let result = sqlx::query(
        "INSERT INTO oauth_refresh_tokens
         (id, family_id, token_prefix, token_digest, digest_key_version,
          client_id, user_id, vault_id, resource, scopes_json,
          created_at, expires_at)
         SELECT ?, ?, ?, ?, ?, client_id, user_id, vault_id, resource,
                COALESCE(?, scopes_json), ?, ?
         FROM oauth_authorization_codes WHERE id = ? AND vault_id = ?",
    )
    .bind(token.id.to_string())
    .bind(token.family_id.to_string())
    .bind(token.token_prefix)
    .bind(token.token_digest)
    .bind(i64::from(token.digest_key_version))
    .bind(token.scopes_json)
    .bind(token.created_at)
    .bind(token.expires_at)
    .bind(code_id.to_string())
    .bind(context.id().to_string())
    .execute(&mut **transaction)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StateError::InvalidInput(
            "OAuth refresh token was not created",
        ))
    }
}

async fn insert_access_from_refresh(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    context: &VaultContext,
    old_id: OAuthRefreshTokenId,
    token: &NewOAuthAccessToken<'_>,
) -> Result<(), StateError> {
    let result = sqlx::query(
        "INSERT INTO oauth_access_tokens
         (id, family_id, token_prefix, token_digest, digest_key_version,
          client_id, user_id, vault_id, resource, scopes_json,
          created_at, expires_at)
         SELECT ?, family_id, ?, ?, ?, client_id, user_id, vault_id, resource,
                COALESCE(?, scopes_json), ?, ?
         FROM oauth_refresh_tokens WHERE id = ? AND vault_id = ? AND family_id = ?",
    )
    .bind(token.id.to_string())
    .bind(token.token_prefix)
    .bind(token.token_digest)
    .bind(i64::from(token.digest_key_version))
    .bind(token.scopes_json)
    .bind(token.created_at)
    .bind(token.expires_at)
    .bind(old_id.to_string())
    .bind(context.id().to_string())
    .bind(token.family_id.to_string())
    .execute(&mut **transaction)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StateError::InvalidInput(
            "OAuth access token was not rotated",
        ))
    }
}

async fn insert_refresh_from_refresh(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    context: &VaultContext,
    old_id: OAuthRefreshTokenId,
    token: &NewOAuthRefreshToken<'_>,
) -> Result<(), StateError> {
    let result = sqlx::query(
        "INSERT INTO oauth_refresh_tokens
         (id, family_id, token_prefix, token_digest, digest_key_version,
          client_id, user_id, vault_id, resource, scopes_json,
          created_at, expires_at)
         SELECT ?, family_id, ?, ?, ?, client_id, user_id, vault_id, resource,
                COALESCE(?, scopes_json), ?, expires_at
         FROM oauth_refresh_tokens WHERE id = ? AND vault_id = ? AND family_id = ?",
    )
    .bind(token.id.to_string())
    .bind(token.token_prefix)
    .bind(token.token_digest)
    .bind(i64::from(token.digest_key_version))
    .bind(token.scopes_json)
    .bind(token.created_at)
    .bind(old_id.to_string())
    .bind(context.id().to_string())
    .bind(token.family_id.to_string())
    .execute(&mut **transaction)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StateError::InvalidInput(
            "OAuth refresh token was not rotated",
        ))
    }
}

fn parse_key_version(value: i64) -> Result<u32, StateError> {
    u32::try_from(value).map_err(|_| StateError::InvalidInput("OAuth key version is invalid"))
}

fn row_to_local_user(row: OAuthLocalUserRow) -> Result<OAuthLocalUserRecord, StateError> {
    Ok(OAuthLocalUserRecord {
        id: OAuthLocalUserId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        username: row.username,
        password_hash: row.password_hash,
        scopes_json: row.scopes_json,
        enabled: row.enabled != 0,
        password_changed_at: row.password_changed_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_client(row: OAuthClientRow) -> Result<OAuthClientRecord, StateError> {
    Ok(OAuthClientRecord {
        id: OAuthClientId::parse(&row.id)?,
        client_name: row.client_name,
        redirect_uris_json: row.redirect_uris_json,
        grant_types_json: row.grant_types_json,
        response_types_json: row.response_types_json,
        token_endpoint_auth_method: row.token_endpoint_auth_method,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
        revoked_at: row.revoked_at,
    })
}

fn row_to_authorization_request(
    row: OAuthAuthorizationRequestRow,
) -> Result<OAuthAuthorizationRequestRecord, StateError> {
    Ok(OAuthAuthorizationRequestRecord {
        id: OAuthAuthorizationRequestId::parse(&row.id)?,
        request_digest: row.request_digest,
        digest_key_version: parse_key_version(row.digest_key_version)?,
        client_id: OAuthClientId::parse(&row.client_id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        resource: row.resource,
        redirect_uri: row.redirect_uri,
        scopes_json: row.scopes_json,
        state: row.state,
        code_challenge: row.code_challenge,
        created_at: row.created_at,
        expires_at: row.expires_at,
        consumed_at: row.consumed_at,
    })
}

fn row_to_authorization_code(
    row: OAuthAuthorizationCodeRow,
) -> Result<OAuthAuthorizationCodeRecord, StateError> {
    Ok(OAuthAuthorizationCodeRecord {
        id: OAuthAuthorizationCodeId::parse(&row.id)?,
        code_digest: row.code_digest,
        digest_key_version: parse_key_version(row.digest_key_version)?,
        client_id: OAuthClientId::parse(&row.client_id)?,
        user_id: OAuthLocalUserId::parse(&row.user_id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        resource: row.resource,
        redirect_uri: row.redirect_uri,
        scopes_json: row.scopes_json,
        code_challenge: row.code_challenge,
        created_at: row.created_at,
        expires_at: row.expires_at,
        consumed_at: row.consumed_at,
    })
}

fn row_to_access_token(row: OAuthAccessTokenRow) -> Result<OAuthAccessTokenRecord, StateError> {
    Ok(OAuthAccessTokenRecord {
        id: OAuthAccessTokenId::parse(&row.id)?,
        family_id: OAuthTokenFamilyId::parse(&row.family_id)?,
        token_prefix: row.token_prefix,
        token_digest: row.token_digest,
        digest_key_version: parse_key_version(row.digest_key_version)?,
        client_id: OAuthClientId::parse(&row.client_id)?,
        user_id: OAuthLocalUserId::parse(&row.user_id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        resource: row.resource,
        scopes_json: row.scopes_json,
        created_at: row.created_at,
        expires_at: row.expires_at,
        last_used_at: row.last_used_at,
        revoked_at: row.revoked_at,
    })
}

fn row_to_refresh_token(row: OAuthRefreshTokenRow) -> Result<OAuthRefreshTokenRecord, StateError> {
    Ok(OAuthRefreshTokenRecord {
        id: OAuthRefreshTokenId::parse(&row.id)?,
        family_id: OAuthTokenFamilyId::parse(&row.family_id)?,
        token_prefix: row.token_prefix,
        token_digest: row.token_digest,
        digest_key_version: parse_key_version(row.digest_key_version)?,
        client_id: OAuthClientId::parse(&row.client_id)?,
        user_id: OAuthLocalUserId::parse(&row.user_id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        resource: row.resource,
        scopes_json: row.scopes_json,
        created_at: row.created_at,
        expires_at: row.expires_at,
        rotated_at: row.rotated_at,
        revoked_at: row.revoked_at,
    })
}
