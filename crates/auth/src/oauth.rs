//! Local OAuth resource-server JWT validation.
//!
//! Network discovery/JWKS refresh is intentionally outside this module. The
//! issuer record supplies a validated cached JWK set; this module verifies an
//! access token without contacting an upstream server or passing the token on.

use std::{fmt, str::FromStr};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mcp_vault_domain::{Actor, ActorId, ActorType, PermissionSet, Scope, ScopeSet, VaultContext};
use mcp_vault_state::{OAuthGrantRecord, OAuthIssuerRecord};
use rsa::{RsaPublicKey, pkcs1v15::VerifyingKey, signature::Verifier};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::AuthError;

const CLOCK_SKEW_SECONDS: i64 = 30;

/// A JSON Web Key set loaded from the issuer's persisted cache.
#[derive(Clone, Deserialize, Serialize)]
pub struct JsonWebKeySet {
    /// Keys used for access-token signature validation.
    pub keys: Vec<JsonWebKey>,
}

impl fmt::Debug for JsonWebKeySet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonWebKeySet")
            .field("key_count", &self.keys.len())
            .finish()
    }
}

impl JsonWebKeySet {
    /// Parse and structurally validate a cached JWK set.
    pub fn from_json(value: &str) -> Result<Self, AuthError> {
        let set: Self = serde_json::from_str(value).map_err(|_| AuthError::OAuthConfiguration)?;
        if set.keys.is_empty() || set.keys.len() > 32 {
            return Err(AuthError::OAuthConfiguration);
        }
        for key in &set.keys {
            key.validate_shape()?;
        }
        Ok(set)
    }

    /// Serialize only the validated public-key fields accepted by this
    /// resource server. Unknown input fields are intentionally discarded.
    pub fn to_public_json(&self) -> Result<String, AuthError> {
        serde_json::to_string(self).map_err(|_| AuthError::OAuthConfiguration)
    }

    fn verify(
        &self,
        algorithm: &str,
        kid: Option<&str>,
        signing_input: &[u8],
        signature: &[u8],
    ) -> Result<(), AuthError> {
        let candidates = self
            .keys
            .iter()
            .filter(|key| kid.is_none_or(|wanted| key.kid.as_deref() == Some(wanted)))
            .filter(|key| key.alg.as_deref().is_none_or(|value| value == algorithm))
            .collect::<Vec<_>>();
        let key = if candidates.len() == 1 {
            candidates[0]
        } else {
            return Err(AuthError::OAuthTokenInvalid);
        };

        match algorithm {
            "RS256" if key.kty == "RSA" => {
                let modulus =
                    decode_base64(key.n.as_deref().ok_or(AuthError::OAuthConfiguration)?)?;
                let exponent =
                    decode_base64(key.e.as_deref().ok_or(AuthError::OAuthConfiguration)?)?;
                let modulus = rsa::BigUint::from_bytes_be(&modulus);
                let exponent = rsa::BigUint::from_bytes_be(&exponent);
                let public_key = RsaPublicKey::new(modulus, exponent)
                    .map_err(|_| AuthError::OAuthConfiguration)?;
                let verifying_key = VerifyingKey::<Sha256>::new(public_key);
                let signature = rsa::pkcs1v15::Signature::try_from(signature)
                    .map_err(|_| AuthError::OAuthTokenInvalid)?;
                verifying_key
                    .verify(signing_input, &signature)
                    .map_err(|_| AuthError::OAuthTokenInvalid)
            }
            _ => Err(AuthError::OAuthTokenInvalid),
        }
    }
}

/// A JWK with a redacted `k` field in `Debug` output.
#[derive(Clone, Deserialize, Serialize)]
pub struct JsonWebKey {
    /// Key type (`RSA`).
    pub kty: String,
    /// Key ID selected by the JWT header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    /// Declared algorithm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alg: Option<String>,
    /// Intended use, when present.
    #[serde(rename = "use")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_: Option<String>,
    /// Symmetric key material is parsed only so configuration can reject it.
    #[serde(skip_serializing)]
    pub k: Option<String>,
    /// RSA modulus for RS256.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
    /// RSA exponent for RS256.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<String>,
}

impl fmt::Debug for JsonWebKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonWebKey")
            .field("kty", &self.kty)
            .field("kid", &self.kid)
            .field("alg", &self.alg)
            .field("use_", &self.use_)
            .field("has_symmetric_key", &self.k.is_some())
            .field("has_rsa_modulus", &self.n.is_some())
            .field("has_rsa_exponent", &self.e.is_some())
            .finish()
    }
}

impl JsonWebKey {
    fn validate_shape(&self) -> Result<(), AuthError> {
        if self.kty == "RSA" && self.k.is_none() {
            if self.n.is_none()
                || self.e.is_none()
                || self.alg.as_deref().is_some_and(|alg| alg != "RS256")
            {
                return Err(AuthError::OAuthConfiguration);
            }
            let modulus =
                decode_jwk_base64(self.n.as_deref().ok_or(AuthError::OAuthConfiguration)?)?;
            if modulus.len() < 256 {
                return Err(AuthError::OAuthConfiguration);
            }
            let exponent =
                decode_jwk_base64(self.e.as_deref().ok_or(AuthError::OAuthConfiguration)?)?;
            RsaPublicKey::new(
                rsa::BigUint::from_bytes_be(&modulus),
                rsa::BigUint::from_bytes_be(&exponent),
            )
            .map_err(|_| AuthError::OAuthConfiguration)?;
        } else {
            return Err(AuthError::OAuthConfiguration);
        }
        if self.use_.as_deref().is_some_and(|value| value != "sig") {
            return Err(AuthError::OAuthConfiguration);
        }
        Ok(())
    }
}

/// The authenticated application principal produced by OAuth validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthPrincipal {
    /// Non-secret audit actor.
    pub actor: Actor,
    /// Exact issuer that authenticated the subject.
    pub issuer_id: mcp_vault_domain::OAuthIssuerId,
    /// Exact OAuth subject.
    pub subject: String,
    /// Token/grant scope intersection.
    pub scopes: ScopeSet,
    /// Application permissions derived from the intersection.
    pub permissions: PermissionSet,
}

#[derive(Debug, Deserialize)]
struct JwtHeader {
    alg: String,
    kid: Option<String>,
    #[allow(dead_code)]
    typ: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwtClaims {
    iss: String,
    sub: String,
    aud: ClaimValue,
    exp: i64,
    nbf: Option<i64>,
    resource: Option<ClaimValue>,
    scope: Option<String>,
    scp: Option<ClaimValue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ClaimValue {
    String(String),
    Strings(Vec<String>),
}

impl ClaimValue {
    fn values(&self) -> impl Iterator<Item = &str> {
        match self {
            Self::String(value) => std::slice::from_ref(value).iter().map(String::as_str),
            Self::Strings(values) => values.iter().map(String::as_str),
        }
    }
}

/// Validate a configured OAuth access token for one endpoint Vault.
pub fn validate_access_token(
    token: &str,
    issuer: &OAuthIssuerRecord,
    grant: &OAuthGrantRecord,
    context: &VaultContext,
    required_scopes: &[Scope],
    now_seconds: i64,
) -> Result<OAuthPrincipal, AuthError> {
    if !issuer.enabled
        || issuer.resource.as_deref().is_none_or(str::is_empty)
        || grant.revoked_at.is_some()
        || grant.vault_id != context.id()
    {
        return Err(AuthError::OAuthTokenInvalid);
    }
    let jwks_json = issuer
        .jwks_cache_json
        .as_deref()
        .ok_or(AuthError::OAuthConfiguration)?;
    let jwks = JsonWebKeySet::from_json(jwks_json)?;
    let (header, claims, signing_input, signature) = parse_jwt(token)?;
    if header.alg != "RS256" {
        return Err(AuthError::OAuthTokenInvalid);
    }
    jwks.verify(
        &header.alg,
        header.kid.as_deref(),
        signing_input.as_bytes(),
        &signature,
    )?;

    if claims.iss != issuer.issuer_url
        || claims.sub.is_empty()
        || claims.exp < now_seconds - CLOCK_SKEW_SECONDS
        || claims
            .nbf
            .is_some_and(|nbf| nbf > now_seconds + CLOCK_SKEW_SECONDS)
        || !claims.aud.values().any(|aud| aud == issuer.audience)
    {
        return Err(AuthError::OAuthTokenInvalid);
    }
    let expected_resource = issuer
        .resource
        .as_deref()
        .ok_or(AuthError::OAuthConfiguration)?;
    let resource_matches_audience = claims.aud.values().any(|value| value == expected_resource);
    let resource_matches_claim = claims
        .resource
        .as_ref()
        .is_some_and(|resource| resource.values().any(|value| value == expected_resource));
    if !resource_matches_audience && !resource_matches_claim {
        return Err(AuthError::OAuthTokenInvalid);
    }

    let token_scopes = parse_claim_scopes(&claims)?;
    let grant_scopes = parse_scopes(&grant.scopes_json)?;
    let effective = token_scopes
        .iter()
        .copied()
        .filter(|scope| grant_scopes.contains(*scope))
        .collect::<ScopeSet>();
    if required_scopes
        .iter()
        .any(|scope| !effective.contains(*scope))
    {
        return Err(AuthError::ScopeDenied);
    }

    let actor_value = format!("{}:{}", issuer.id, claims.sub);
    let actor_id = ActorId::new(&actor_value)?;
    Ok(OAuthPrincipal {
        actor: Actor::identified(ActorType::McpOAuthSubject, actor_id),
        issuer_id: issuer.id,
        subject: claims.sub,
        permissions: effective.permissions(),
        scopes: effective,
    })
}

/// Extract the unverified issuer/subject pair used only to select the cached
/// issuer and Vault grant. The complete signature and claim validation still
/// runs in `validate_access_token` before a principal is returned.
pub fn token_identity(token: &str) -> Result<(String, String), AuthError> {
    let (_, claims, _, _) = parse_jwt(token)?;
    if claims.iss.is_empty() || claims.sub.is_empty() {
        return Err(AuthError::OAuthTokenInvalid);
    }
    Ok((claims.iss, claims.sub))
}

fn parse_jwt(token: &str) -> Result<(JwtHeader, JwtClaims, String, Vec<u8>), AuthError> {
    let mut segments = token.split('.');
    let header_segment = segments.next().ok_or(AuthError::OAuthTokenInvalid)?;
    let payload_segment = segments.next().ok_or(AuthError::OAuthTokenInvalid)?;
    let signature_segment = segments.next().ok_or(AuthError::OAuthTokenInvalid)?;
    if segments.next().is_some()
        || header_segment.is_empty()
        || payload_segment.is_empty()
        || signature_segment.is_empty()
    {
        return Err(AuthError::OAuthTokenInvalid);
    }
    let header: JwtHeader = serde_json::from_slice(&decode_base64(header_segment)?)
        .map_err(|_| AuthError::OAuthTokenInvalid)?;
    let claims: JwtClaims = serde_json::from_slice(&decode_base64(payload_segment)?)
        .map_err(|_| AuthError::OAuthTokenInvalid)?;
    Ok((
        header,
        claims,
        format!("{header_segment}.{payload_segment}"),
        decode_base64(signature_segment)?,
    ))
}

fn parse_claim_scopes(claims: &JwtClaims) -> Result<ScopeSet, AuthError> {
    let mut scopes = ScopeSet::new();
    if let Some(scope) = &claims.scope {
        for value in scope.split_ascii_whitespace() {
            scopes.insert(Scope::from_str(value).map_err(|_| AuthError::OAuthTokenInvalid)?);
        }
    }
    if let Some(scp) = &claims.scp {
        for value in scp.values() {
            scopes.insert(Scope::from_str(value).map_err(|_| AuthError::OAuthTokenInvalid)?);
        }
    }
    Ok(scopes)
}

/// Parse the persisted grant scope JSON with a fail-closed scope vocabulary.
pub fn parse_scopes(value: &str) -> Result<ScopeSet, AuthError> {
    let values: Vec<String> =
        serde_json::from_str(value).map_err(|_| AuthError::OAuthConfiguration)?;
    let mut scopes = ScopeSet::new();
    for value in values {
        scopes.insert(Scope::from_str(&value).map_err(|_| AuthError::OAuthConfiguration)?);
    }
    Ok(scopes)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, AuthError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AuthError::OAuthTokenInvalid)
}

fn decode_jwk_base64(value: &str) -> Result<Vec<u8>, AuthError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AuthError::OAuthConfiguration)
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use mcp_vault_domain::{Revision, Scope, VaultContext, VaultId, VaultSlug};
    use mcp_vault_state::{OAuthGrantRecord, OAuthIssuerRecord};
    use rand::rngs::OsRng;
    use rsa::{
        RsaPrivateKey, RsaPublicKey,
        pkcs1v15::SigningKey,
        signature::{SignatureEncoding, Signer},
        traits::PublicKeyParts,
    };
    use sha2::Sha256;

    use super::validate_access_token;

    fn context() -> VaultContext {
        VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            "/srv/work".into(),
            Revision::new(1),
        )
        .unwrap()
    }

    #[test]
    fn rejects_symmetric_jwk_material() {
        let secret = b"01234567890123456789012345678901";
        let jwk = URL_SAFE_NO_PAD.encode(secret);
        assert!(
            super::JsonWebKeySet::from_json(&format!(
                r#"{{"keys":[{{"kty":"oct","kid":"test","alg":"HS256","use":"sig","k":"{jwk}"}}]}}"#
            ))
            .is_err()
        );
    }

    #[test]
    fn rejects_wrong_resource_or_vault_grant() {
        let context = context();
        let other = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("other").unwrap(),
            "/srv/other".into(),
            Revision::new(1),
        )
        .unwrap();
        let grant = OAuthGrantRecord {
            id: mcp_vault_domain::OAuthGrantId::new(),
            issuer_id: mcp_vault_domain::OAuthIssuerId::new(),
            subject: "agent".to_owned(),
            vault_id: other.id(),
            scopes_json: r#"["vault:read"]"#.to_owned(),
            created_at: 1,
            updated_at: 1,
            revoked_at: None,
        };
        let issuer = OAuthIssuerRecord {
            id: grant.issuer_id,
            name: "test".to_owned(),
            issuer_url: "https://issuer.example.test".to_owned(),
            discovery_url: None,
            audience: "mcp-vault".to_owned(),
            resource: Some("https://vault.example.test/mcp".to_owned()),
            jwks_cache_json: None,
            jwks_cached_at: None,
            enabled: true,
            created_at: 1,
            updated_at: 1,
        };
        assert!(validate_access_token("not-a-token", &issuer, &grant, &context, &[], 1).is_err());
    }

    #[test]
    fn validates_rs256_jwk_signatures() {
        let context = context();
        let private = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let public = RsaPublicKey::from(&private);
        let modulus = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let exponent = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
        let issuer = OAuthIssuerRecord {
            id: mcp_vault_domain::OAuthIssuerId::new(),
            name: "test".to_owned(),
            issuer_url: "https://issuer.example.test".to_owned(),
            discovery_url: None,
            audience: "mcp-vault".to_owned(),
            resource: Some("https://vault.example.test/mcp".to_owned()),
            jwks_cache_json: Some(format!(
                r#"{{"keys":[{{"kty":"RSA","kid":"rsa-test","alg":"RS256","use":"sig","n":"{modulus}","e":"{exponent}"}}]}}"#
            )),
            jwks_cached_at: Some(1),
            enabled: true,
            created_at: 1,
            updated_at: 1,
        };
        let grant = OAuthGrantRecord {
            id: mcp_vault_domain::OAuthGrantId::new(),
            issuer_id: issuer.id,
            subject: "agent".to_owned(),
            vault_id: context.id(),
            scopes_json: r#"["vault:read"]"#.to_owned(),
            created_at: 1,
            updated_at: 1,
            revoked_at: None,
        };
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","kid":"rsa-test"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"iss":"https://issuer.example.test","sub":"agent","aud":"mcp-vault","resource":"https://vault.example.test/mcp","exp":4102444800,"scope":"vault:read vault:write"}"#,
        );
        let signing_input = format!("{header}.{payload}");
        let signature = SigningKey::<Sha256>::new(private).sign(signing_input.as_bytes());
        let token = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        );
        let principal = validate_access_token(
            &token,
            &issuer,
            &grant,
            &context,
            &[Scope::VaultRead],
            1_700_000_000,
        )
        .unwrap();
        assert!(
            principal
                .permissions
                .contains(mcp_vault_domain::Permission::ReadVault)
        );
        assert!(
            !principal
                .permissions
                .contains(mcp_vault_domain::Permission::WriteVault)
        );
    }

    #[test]
    fn accepts_resource_indicator_in_audience_without_custom_resource_claim() {
        let context = context();
        let private = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let public = RsaPublicKey::from(&private);
        let modulus = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let exponent = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
        let resource = "https://vault.example.test/mcp/v1/vaults/work";
        let issuer = OAuthIssuerRecord {
            id: mcp_vault_domain::OAuthIssuerId::new(),
            name: "test".to_owned(),
            issuer_url: "https://issuer.example.test".to_owned(),
            discovery_url: None,
            audience: resource.to_owned(),
            resource: Some(resource.to_owned()),
            jwks_cache_json: Some(format!(
                r#"{{"keys":[{{"kty":"RSA","kid":"rsa-test","alg":"RS256","use":"sig","n":"{modulus}","e":"{exponent}"}}]}}"#
            )),
            jwks_cached_at: Some(1),
            enabled: true,
            created_at: 1,
            updated_at: 1,
        };
        let grant = OAuthGrantRecord {
            id: mcp_vault_domain::OAuthGrantId::new(),
            issuer_id: issuer.id,
            subject: "agent".to_owned(),
            vault_id: context.id(),
            scopes_json: r#"["vault:read"]"#.to_owned(),
            created_at: 1,
            updated_at: 1,
            revoked_at: None,
        };
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","kid":"rsa-test"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"iss":"https://issuer.example.test","sub":"agent","aud":"{resource}","exp":4102444800,"scope":"vault:read"}}"#
        ));
        let signing_input = format!("{header}.{payload}");
        let signature = SigningKey::<Sha256>::new(private).sign(signing_input.as_bytes());
        let token = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        );

        validate_access_token(
            &token,
            &issuer,
            &grant,
            &context,
            &[Scope::VaultRead],
            1_700_000_000,
        )
        .unwrap();
    }
}
