//! Independent Admin, WebDAV, and MCP authentication boundaries.
//!
//! This crate owns credential algorithms and principal derivation. It never
//! executes SQL directly; all durable metadata goes through
//! `mcp-vault-state` repositories, and protocol adapters remain responsible
//! for translating these results into HTTP/DAV/RMCP responses.

mod error;
mod oauth;
mod origin;
mod password;
mod secret;
mod service;

pub use error::AuthError;
pub use oauth::{
    JsonWebKey, JsonWebKeySet, OAuthPrincipal, parse_scopes, token_identity, validate_access_token,
};
pub use origin::{AdminCookieSecurity, OriginPolicy};
pub use password::{PasswordPolicy, PasswordVerification};
pub use secret::{
    BearerToken, EncryptedSecretPayload, MasterKeyRing, SecretString, digest_bearer_token,
    generate_bearer_token, load_or_create_master_key, mask_hint, token_prefix,
};
pub use service::{
    AdminLogin, AdminPrincipal, AuthPrincipal, AuthService, OAuthIssuerInput, PatIssue,
    PreparedAdminSetup, SecretMetadata, SessionPolicy, WebDavCredentialIssue,
    clear_csrf_cookie_header, clear_session_cookie_header, csrf_cookie_header,
    parse_session_cookie, require_secure_basic_auth, session_cookie_header,
};
