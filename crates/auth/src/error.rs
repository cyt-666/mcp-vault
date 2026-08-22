//! Stable authentication errors that never carry credential material.

use thiserror::Error;

use mcp_vault_domain::DomainError;

/// Errors exposed by the protocol-neutral authentication boundary.
#[derive(Debug, Error)]
pub enum AuthError {
    /// A typed domain value was invalid.
    #[error("invalid authentication value")]
    Domain(#[from] DomainError),
    /// The SQL repository rejected or could not read operational state.
    #[error("authentication state is unavailable")]
    State(#[from] mcp_vault_state::StateError),
    /// A required master-key file could not be created, read, or parsed.
    #[error("installation key is unavailable")]
    MasterKeyUnavailable,
    /// An encrypted record cannot be authenticated or its key version is not
    /// available.
    #[error("encrypted secret is unavailable")]
    SecretUnavailable,
    /// A password does not satisfy the configured policy.
    #[error("password does not satisfy policy")]
    PasswordPolicy,
    /// Argon2id could not hash or parse a stored PHC string.
    #[error("password hash is invalid")]
    PasswordHash,
    /// Credentials, sessions, grants, and tokens all fail closed with this
    /// non-distinguishing public category.
    #[error("authentication failed")]
    InvalidCredential,
    /// A valid-looking credential has expired.
    #[error("credential expired")]
    Expired,
    /// A credential/session/grant was explicitly revoked or disabled.
    #[error("credential revoked")]
    Revoked,
    /// A one-time setup operation is no longer available.
    #[error("setup is unavailable")]
    SetupUnavailable,
    /// The request was rejected by the Admin Origin/Referer policy.
    #[error("request origin is not allowed")]
    OriginRejected,
    /// The Admin state-changing request lacks a valid session-bound CSRF
    /// token.
    #[error("CSRF validation failed")]
    CsrfRejected,
    /// A session is no longer inside its idle/absolute lifetime.
    #[error("session expired")]
    SessionExpired,
    /// The in-process login limiter rejected an attempt.
    #[error("authentication temporarily rate limited")]
    RateLimited,
    /// OAuth issuer/JWK configuration is invalid or incomplete.
    #[error("OAuth configuration is invalid")]
    OAuthConfiguration,
    /// OAuth token validation failed one of its required checks.
    #[error("OAuth token is invalid")]
    OAuthTokenInvalid,
    /// A requested scope is not granted by the authenticated principal.
    #[error("scope is not granted")]
    ScopeDenied,
    /// An internal cryptographic operation failed without exposing details.
    #[error("cryptographic operation failed")]
    Cryptography,
    /// A safe public input exceeded an implementation bound.
    #[error("authentication input is invalid")]
    InvalidInput,
}
