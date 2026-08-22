//! Argon2id password policy and PHC verification.

use argon2::password_hash::SaltString;
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use rand::rngs::OsRng;

use crate::{error::AuthError, secret::SecretString};

const MIN_PASSWORD_BYTES: usize = 12;
const MEMORY_COST_KIB: u32 = 19_456;
const TIME_COST: u32 = 2;
const PARALLELISM: u32 = 1;
const OUTPUT_LENGTH: usize = 32;

/// Password policy used by Admin and WebDAV app-password creation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PasswordPolicy {
    /// Minimum UTF-8 byte length accepted by the service.
    pub minimum_bytes: usize,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            minimum_bytes: MIN_PASSWORD_BYTES,
        }
    }
}

impl PasswordPolicy {
    /// Validate a password without logging or returning the password.
    pub fn validate(&self, password: &SecretString) -> Result<(), AuthError> {
        let value = password.expose_secret();
        if value.len() < self.minimum_bytes || value.chars().any(char::is_control) {
            return Err(AuthError::PasswordPolicy);
        }
        if matches!(
            value.to_ascii_lowercase().as_str(),
            "password" | "password123" | "changeme" | "admin" | "admin123" | "letmein"
        ) {
            return Err(AuthError::PasswordPolicy);
        }
        Ok(())
    }

    /// Hash a password as an Argon2id PHC string.
    pub fn hash(&self, password: &SecretString) -> Result<String, AuthError> {
        self.validate(password)?;
        let salt = SaltString::generate(&mut OsRng);
        current_argon2()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| AuthError::PasswordHash)
    }

    /// Verify a stored PHC string and report whether it should be rehashed.
    pub fn verify(
        &self,
        stored_hash: &str,
        password: &SecretString,
    ) -> Result<PasswordVerification, AuthError> {
        let parsed = PasswordHash::new(stored_hash).map_err(|_| AuthError::PasswordHash)?;
        let valid = current_argon2()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        Ok(PasswordVerification {
            valid,
            needs_rehash: valid && needs_rehash(&parsed),
        })
    }
}

/// Result of a password verification operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PasswordVerification {
    /// Whether the supplied password matched.
    pub valid: bool,
    /// Whether a successful login should replace the stored PHC string.
    pub needs_rehash: bool,
}

fn current_argon2() -> Argon2<'static> {
    let params = Params::new(MEMORY_COST_KIB, TIME_COST, PARALLELISM, Some(OUTPUT_LENGTH))
        .expect("configured Argon2id parameters are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn needs_rehash(parsed: &PasswordHash<'_>) -> bool {
    let Ok(argon2id) = argon2::password_hash::Ident::new("argon2id") else {
        return true;
    };
    if parsed.algorithm != argon2id {
        return true;
    }
    let expected = format!("m={MEMORY_COST_KIB},t={TIME_COST},p={PARALLELISM}");
    parsed.params.to_string() != expected
}

#[cfg(test)]
mod tests {
    use argon2::password_hash::SaltString;
    use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
    use rand::rngs::OsRng;

    use super::PasswordPolicy;
    use crate::secret::SecretString;

    #[test]
    fn hashes_argon2id_and_detects_rehash_parameters() {
        let policy = PasswordPolicy::default();
        let password = SecretString::new("correct horse battery staple");
        let hash = policy.hash(&password).unwrap();
        assert!(hash.starts_with("$argon2id$") || hash.starts_with("$argon2id"));
        let result = policy.verify(&hash, &password).unwrap();
        assert!(result.valid);
        assert!(!result.needs_rehash);
        assert!(
            !policy
                .verify(&hash, &SecretString::new("wrong password"))
                .unwrap()
                .valid
        );
    }

    #[test]
    fn rejects_weak_or_placeholder_passwords_without_composition_rules() {
        let policy = PasswordPolicy::default();
        assert!(policy.validate(&SecretString::new("short")).is_err());
        assert!(policy.validate(&SecretString::new("password123")).is_err());
        assert!(
            policy
                .validate(&SecretString::new("中文密码与额外长度"))
                .is_ok()
        );
    }

    #[test]
    fn detects_a_legacy_argon2id_parameter_set_for_transparent_rehash() {
        let legacy_params = Params::new(8_192, 1, 1, Some(32)).unwrap();
        let legacy = Argon2::new(Algorithm::Argon2id, Version::V0x13, legacy_params)
            .hash_password(
                b"correct horse battery staple",
                &SaltString::generate(&mut OsRng),
            )
            .unwrap()
            .to_string();
        let result = PasswordPolicy::default()
            .verify(
                &legacy,
                &super::SecretString::new("correct horse battery staple"),
            )
            .unwrap();
        assert!(result.valid);
        assert!(result.needs_rehash);
    }
}
