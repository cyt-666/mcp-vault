//! Exact Origin/Referer policy shared by Admin and data-plane adapters.

use std::collections::BTreeSet;

use http::{HeaderMap, Method};
use url::Url;

use crate::error::AuthError;

/// Whether browser cookies for one validated Admin mutation must carry the
/// `Secure` attribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminCookieSecurity {
    /// The Admin page origin is HTTPS.
    Secure,
    /// The Admin page origin is explicitly configured cleartext HTTP.
    Insecure,
}

/// Exact scheme/host/port allow-list for one listener.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginPolicy {
    allowed_origins: BTreeSet<String>,
}

impl OriginPolicy {
    /// Build a policy from exact origin strings such as
    /// `https://vault.example.test`.
    pub fn new<I, S>(origins: I) -> Result<Self, AuthError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut allowed_origins = BTreeSet::new();
        for origin in origins {
            allowed_origins.insert(canonical_origin(origin.as_ref())?);
        }
        Ok(Self { allowed_origins })
    }

    /// Return the canonical configured origins for diagnostics without
    /// exposing request credentials.
    pub fn allowed_origins(&self) -> impl Iterator<Item = &str> {
        self.allowed_origins.iter().map(String::as_str)
    }

    /// Validate a data-plane Origin only when the client supplied one.
    pub fn validate_optional(&self, headers: &HeaderMap) -> Result<(), AuthError> {
        let Some(origin) = single_header(headers, "origin")? else {
            return Ok(());
        };
        self.validate_value(origin)
    }

    /// Validate an Admin request. Safe methods do not need CSRF/Origin
    /// validation; every state-changing method must provide an allowed Origin
    /// or same-origin Referer.
    pub fn validate_admin_request(
        &self,
        headers: &HeaderMap,
        method: &Method,
    ) -> Result<(), AuthError> {
        if is_safe_method(method) {
            return Ok(());
        }
        self.admin_cookie_security(headers).map(|_| ())
    }

    /// Derive cookie transport policy from an exact allowed Admin
    /// Origin/Referer. Callers use this only for state-changing requests.
    pub fn admin_cookie_security(
        &self,
        headers: &HeaderMap,
    ) -> Result<AdminCookieSecurity, AuthError> {
        let canonical = if let Some(origin) = single_header(headers, "origin")? {
            canonical_origin(origin)?
        } else {
            let referer = single_header(headers, "referer")?.ok_or(AuthError::OriginRejected)?;
            Url::parse(referer)
                .ok()
                .and_then(|url| canonical_origin_url(&url).ok())
                .ok_or(AuthError::OriginRejected)?
        };
        if !self.allowed_origins.contains(&canonical) {
            return Err(AuthError::OriginRejected);
        }
        Ok(if canonical.starts_with("https://") {
            AdminCookieSecurity::Secure
        } else {
            AdminCookieSecurity::Insecure
        })
    }

    fn validate_value(&self, value: &str) -> Result<(), AuthError> {
        let canonical = canonical_origin(value)?;
        if self.allowed_origins.contains(&canonical) {
            Ok(())
        } else {
            Err(AuthError::OriginRejected)
        }
    }
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, AuthError> {
    if headers.get_all(name).iter().count() > 1 {
        return Err(AuthError::OriginRejected);
    }
    headers
        .get(name)
        .map(|value| value.to_str().map_err(|_| AuthError::OriginRejected))
        .transpose()
}

fn canonical_origin(value: &str) -> Result<String, AuthError> {
    if value == "null" {
        return Err(AuthError::OriginRejected);
    }
    let url = Url::parse(value).map_err(|_| AuthError::OriginRejected)?;
    if (url.path() != "" && url.path() != "/") || url.query().is_some() || url.fragment().is_some()
    {
        return Err(AuthError::OriginRejected);
    }
    canonical_origin_url(&url)
}

fn canonical_origin_url(url: &Url) -> Result<String, AuthError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
    {
        return Err(AuthError::OriginRejected);
    }

    let scheme = url.scheme().to_ascii_lowercase();
    let host = url
        .host_str()
        .ok_or(AuthError::OriginRejected)?
        .to_ascii_lowercase();
    let port = url
        .port()
        .filter(|port| !((*port == 80 && scheme == "http") || (*port == 443 && scheme == "https")));
    Ok(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

fn is_safe_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue, Method};

    use super::{AdminCookieSecurity, OriginPolicy};

    #[test]
    fn admin_mutations_require_exact_origin_or_referer() {
        let policy = OriginPolicy::new(["https://vault.example.test/"]).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "origin",
            HeaderValue::from_static("https://vault.example.test"),
        );
        policy
            .validate_admin_request(&headers, &Method::POST)
            .unwrap();
        assert_eq!(
            policy.admin_cookie_security(&headers).unwrap(),
            AdminCookieSecurity::Secure
        );

        headers.insert(
            "origin",
            HeaderValue::from_static("https://evil.example.test"),
        );
        assert!(
            policy
                .validate_admin_request(&headers, &Method::POST)
                .is_err()
        );
        assert!(
            policy
                .validate_admin_request(&HeaderMap::new(), &Method::GET)
                .is_ok()
        );
    }

    #[test]
    fn referer_is_reduced_to_an_origin_and_null_is_rejected() {
        let policy = OriginPolicy::new(["http://localhost:8081"]).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "referer",
            HeaderValue::from_static("http://localhost:8081/login?next=/"),
        );
        policy
            .validate_admin_request(&headers, &Method::PATCH)
            .unwrap();
        assert_eq!(
            policy.admin_cookie_security(&headers).unwrap(),
            AdminCookieSecurity::Insecure
        );

        headers.insert("origin", HeaderValue::from_static("null"));
        assert!(
            policy
                .validate_admin_request(&headers, &Method::PATCH)
                .is_err()
        );
    }

    #[test]
    fn data_origin_is_optional_but_not_wildcarded() {
        let policy = OriginPolicy::new(["https://agent.example.test"]).unwrap();
        assert!(policy.validate_optional(&HeaderMap::new()).is_ok());
        let mut headers = HeaderMap::new();
        headers.insert(
            "origin",
            HeaderValue::from_static("https://agent.example.test:443"),
        );
        assert!(policy.validate_optional(&headers).is_ok());
        headers.insert(
            "origin",
            HeaderValue::from_static("https://evil.example.test"),
        );
        assert!(policy.validate_optional(&headers).is_err());
    }
}
