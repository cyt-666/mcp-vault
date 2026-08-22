//! Installation-key handling, authenticated secret encryption, and redaction
//! safe wrappers.

use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;
use zeroize::{Zeroize, Zeroizing};

use crate::error::AuthError;

const MASTER_KEY_BYTES: usize = 32;
const SECRET_NONCE_BYTES: usize = 24;
const SECRET_TEMP_NAME_BYTES: usize = 12;
const INSTALLATION_CHECK_PURPOSE: &str = "mcp-vault-installation-key-check-v1";
const INSTALLATION_CHECK_VALUE: &[u8] = b"installation-key-verifier";

type HmacSha256 = Hmac<Sha256>;

/// A string whose `Debug` output is always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    /// Construct a secret wrapper from owned text.
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Explicitly expose the secret to the narrow operation that needs it.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Return the UTF-8 bytes for hashing/encryption without copying into a
    /// loggable type.
    pub fn as_bytes(&self) -> &[u8] {
        self.expose_secret().as_bytes()
    }

    /// Return a non-secret masked hint suitable for Admin responses.
    pub fn masked_hint(&self) -> String {
        mask_hint(self.expose_secret())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Load an existing installation key or atomically create one at an
/// application-managed path. Callers must decide whether creating a new key is
/// safe for the current operational-state identity before invoking this
/// function.
pub async fn load_or_create_master_key(path: &Path) -> Result<MasterKeyRing, AuthError> {
    if !tokio::fs::try_exists(path)
        .await
        .map_err(|_| AuthError::MasterKeyUnavailable)?
    {
        let mut key = Zeroizing::new(vec![0_u8; MASTER_KEY_BYTES]);
        OsRng.fill_bytes(key.as_mut_slice());
        install_secret_file_if_absent(path, key.as_slice())
            .await
            .map_err(|_| AuthError::MasterKeyUnavailable)?;
    }
    MasterKeyRing::load_file(path).await
}

/// A high-entropy token with the same redaction behavior as a password.
pub struct BearerToken(SecretString);

impl BearerToken {
    /// Construct a token only at an issuance boundary.
    pub fn new(value: String) -> Self {
        Self(SecretString::new(value))
    }

    /// Explicitly expose the token for the one-time response or digest step.
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// A versioned installation master-key ring kept only in zeroizing memory.
#[derive(Clone)]
pub struct MasterKeyRing {
    keys: Arc<BTreeMap<u32, Zeroizing<Vec<u8>>>>,
    current_version: u32,
    persistent: bool,
}

impl fmt::Debug for MasterKeyRing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MasterKeyRing")
            .field("current_version", &self.current_version)
            .field("versions", &self.keys.keys().collect::<Vec<_>>())
            .field("persistent", &self.persistent)
            .finish()
    }
}

impl MasterKeyRing {
    /// Construct a process-local key for isolated tests or embedding contexts.
    /// The production composition root provisions a persistent managed key and
    /// never uses this fallback.
    pub fn ephemeral() -> Self {
        let mut bytes = [0_u8; MASTER_KEY_BYTES];
        OsRng.fill_bytes(&mut bytes);
        let mut ring = Self::from_bytes(1, &bytes).expect("ephemeral master key has a valid size");
        ring.persistent = false;
        ring
    }

    /// Construct a ring with one raw 256-bit key at the supplied version.
    pub fn from_bytes(version: u32, bytes: &[u8]) -> Result<Self, AuthError> {
        if version == 0 || bytes.len() != MASTER_KEY_BYTES {
            return Err(AuthError::MasterKeyUnavailable);
        }
        let mut keys = BTreeMap::new();
        keys.insert(version, Zeroizing::new(bytes.to_vec()));
        Ok(Self {
            keys: Arc::new(keys),
            current_version: version,
            persistent: true,
        })
    }

    /// Construct a ring from multiple raw key versions.
    pub fn from_versions(
        current_version: u32,
        versions: impl IntoIterator<Item = (u32, Vec<u8>)>,
    ) -> Result<Self, AuthError> {
        if current_version == 0 {
            return Err(AuthError::MasterKeyUnavailable);
        }
        let mut keys = BTreeMap::new();
        for (version, bytes) in versions {
            if version == 0 || bytes.len() != MASTER_KEY_BYTES {
                return Err(AuthError::MasterKeyUnavailable);
            }
            keys.insert(version, Zeroizing::new(bytes));
        }
        if !keys.contains_key(&current_version) {
            return Err(AuthError::MasterKeyUnavailable);
        }
        Ok(Self {
            keys: Arc::new(keys),
            current_version,
            persistent: true,
        })
    }

    /// Load a raw 32-byte or 64-hex-character key file asynchronously.
    pub async fn load_file(path: &Path) -> Result<Self, AuthError> {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|_| AuthError::MasterKeyUnavailable)?;
        if !metadata.is_file() {
            return Err(AuthError::MasterKeyUnavailable);
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|_| AuthError::MasterKeyUnavailable)?;
        Self::from_file_bytes(&bytes)
    }

    /// Parse the supported key-file encodings without accepting whitespace
    /// other than one conventional trailing newline.
    pub fn from_file_bytes(bytes: &[u8]) -> Result<Self, AuthError> {
        let mut normalized = Zeroizing::new(bytes.to_vec());
        if normalized.last() == Some(&b'\n') {
            normalized.pop();
            if normalized.last() == Some(&b'\r') {
                normalized.pop();
            }
        }

        if normalized.len() == MASTER_KEY_BYTES {
            return Self::from_bytes(1, &normalized);
        }
        if normalized.len() == MASTER_KEY_BYTES * 2 {
            let decoded = hex_decode(&normalized).ok_or(AuthError::MasterKeyUnavailable)?;
            return Self::from_bytes(1, &decoded);
        }
        Err(AuthError::MasterKeyUnavailable)
    }

    /// Return the version used for newly encrypted records.
    pub const fn current_version(&self) -> u32 {
        self.current_version
    }

    /// Return all retained key versions in deterministic order.
    pub fn versions(&self) -> impl Iterator<Item = u32> + '_ {
        self.keys.keys().copied()
    }

    /// Return whether this ring came from persistent managed/operator key
    /// material rather than an explicit process-local test fallback.
    pub const fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// Compute the non-secret one-way verifier stored in operational state.
    pub fn installation_key_check(&self) -> [u8; 32] {
        self.keyed_digest(INSTALLATION_CHECK_PURPOSE, INSTALLATION_CHECK_VALUE)
    }

    /// Compare a persisted verifier without a timing-dependent byte equality.
    pub fn matches_installation_key_check(&self, expected: &[u8]) -> bool {
        bool::from(self.installation_key_check().as_slice().ct_eq(expected))
    }

    /// Return a new ring with a higher current key version.
    pub fn with_rotated_key(&self, version: u32, bytes: &[u8]) -> Result<Self, AuthError> {
        if version <= self.current_version || bytes.len() != MASTER_KEY_BYTES {
            return Err(AuthError::MasterKeyUnavailable);
        }
        let mut keys = (*self.keys).clone();
        keys.insert(version, Zeroizing::new(bytes.to_vec()));
        Ok(Self {
            keys: Arc::new(keys),
            current_version: version,
            persistent: self.persistent,
        })
    }

    /// Compute an installation-keyed digest for a token/session lookup.
    pub fn keyed_digest(&self, purpose: &str, value: &[u8]) -> [u8; 32] {
        self.keyed_digest_for(self.current_version, purpose, value)
    }

    /// Compute a keyed digest using a retained historical key version.
    pub fn keyed_digest_for(&self, version: u32, purpose: &str, value: &[u8]) -> [u8; 32] {
        let key = self
            .keys
            .get(&version)
            .expect("MasterKeyRing key version is validated");
        let mut mac = <HmacSha256 as Mac>::new_from_slice(key.as_slice())
            .expect("HMAC accepts every 256-bit key");
        update_associated_data(&mut mac, &[purpose.as_bytes(), value]);
        let bytes = mac.finalize().into_bytes();
        bytes.into()
    }

    /// Encrypt a reversible secret with the current key version.
    pub fn encrypt(
        &self,
        purpose: &str,
        owner_type: &str,
        owner_id: Option<&str>,
        plaintext: &[u8],
    ) -> Result<EncryptedSecretPayload, AuthError> {
        let key = self
            .keys
            .get(&self.current_version)
            .ok_or(AuthError::SecretUnavailable)?;
        let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
            .map_err(|_| AuthError::Cryptography)?;
        let mut nonce = [0_u8; SECRET_NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let aad = associated_data(purpose, owner_type, owner_id);
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| AuthError::Cryptography)?;
        Ok(EncryptedSecretPayload {
            key_version: self.current_version,
            nonce,
            ciphertext,
        })
    }

    /// Decrypt/authenticate a persisted ciphertext using its recorded key
    /// version and exact owner-associated data.
    pub fn decrypt(
        &self,
        key_version: u32,
        nonce: &[u8],
        ciphertext: &[u8],
        purpose: &str,
        owner_type: &str,
        owner_id: Option<&str>,
    ) -> Result<Zeroizing<Vec<u8>>, AuthError> {
        if nonce.len() != SECRET_NONCE_BYTES {
            return Err(AuthError::SecretUnavailable);
        }
        let key = self
            .keys
            .get(&key_version)
            .ok_or(AuthError::SecretUnavailable)?;
        let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
            .map_err(|_| AuthError::Cryptography)?;
        let aad = associated_data(purpose, owner_type, owner_id);
        let plaintext = cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| AuthError::SecretUnavailable)?;
        Ok(Zeroizing::new(plaintext))
    }
}

/// Ciphertext returned before it is handed to the state repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedSecretPayload {
    /// Master-key version.
    pub key_version: u32,
    /// Random XChaCha20 nonce.
    pub nonce: [u8; SECRET_NONCE_BYTES],
    /// Authenticated ciphertext.
    pub ciphertext: Vec<u8>,
}

/// Generate a URL-safe high-entropy bearer token with a stable visible label.
pub fn generate_bearer_token(label: &str) -> BearerToken {
    let mut random = [0_u8; 32];
    OsRng.fill_bytes(&mut random);
    let encoded = URL_SAFE_NO_PAD.encode(random);
    random.zeroize();
    BearerToken::new(format!("{label}{encoded}"))
}

/// Return a stable lookup prefix without exposing the full token.
pub fn token_prefix(token: &BearerToken, prefix_bytes: usize) -> String {
    token.expose_secret().chars().take(prefix_bytes).collect()
}

/// Return an installation-keyed digest for a token.
pub fn digest_bearer_token(keys: &MasterKeyRing, purpose: &str, token: &BearerToken) -> [u8; 32] {
    keys.keyed_digest(purpose, token.expose_secret().as_bytes())
}

/// Mask a secret for an Admin response. This deliberately preserves only a
/// tiny prefix/suffix and never returns the original value for short secrets.
pub fn mask_hint(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 6 {
        return "••••".to_owned();
    }
    let prefix = chars.iter().take(3).collect::<String>();
    let suffix = chars.iter().rev().take(3).copied().collect::<String>();
    format!("{prefix}…{}", suffix.chars().rev().collect::<String>())
}

fn associated_data(purpose: &str, owner_type: &str, owner_id: Option<&str>) -> Vec<u8> {
    let owner_id = owner_id.unwrap_or_default();
    let mut data = Vec::with_capacity(purpose.len() + owner_type.len() + owner_id.len() + 12);
    for part in [
        purpose.as_bytes(),
        owner_type.as_bytes(),
        owner_id.as_bytes(),
    ] {
        data.extend_from_slice(&(part.len() as u32).to_be_bytes());
        data.extend_from_slice(part);
    }
    data
}

fn update_associated_data(mac: &mut HmacSha256, parts: &[&[u8]]) {
    for part in parts {
        mac.update(&(part.len() as u32).to_be_bytes());
        mac.update(part);
    }
}

fn hex_decode(value: &[u8]) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut output = Vec::with_capacity(value.len() / 2);
    for pair in value.chunks_exact(2) {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        output.push((high << 4) | low);
    }
    Some(output)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

async fn install_secret_file_if_absent(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;

    let temporary_path = create_temporary_secret_file(parent, contents).await?;
    let link_result = tokio::fs::hard_link(&temporary_path, path).await;
    let cleanup_result = tokio::fs::remove_file(&temporary_path).await;

    match link_result {
        Ok(()) => {
            cleanup_result?;
            sync_directory(parent).await?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            cleanup_result?;
            Ok(())
        }
        Err(error) => {
            let _ = cleanup_result;
            Err(error)
        }
    }
}

async fn create_temporary_secret_file(parent: &Path, contents: &[u8]) -> io::Result<PathBuf> {
    for _ in 0..16 {
        let mut random = [0_u8; SECRET_TEMP_NAME_BYTES];
        OsRng.fill_bytes(&mut random);
        let suffix = URL_SAFE_NO_PAD.encode(random);
        random.zeroize();
        let path = parent.join(format!(".mcp-vault-secret-{suffix}.tmp"));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                if let Err(error) = async {
                    file.write_all(contents).await?;
                    file.sync_all().await
                }
                .await
                {
                    drop(file);
                    let _ = tokio::fs::remove_file(&path).await;
                    return Err(error);
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a temporary secret file",
    ))
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> io::Result<()> {
    tokio::fs::File::open(path).await?.sync_all().await
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BearerToken, MasterKeyRing, SecretString, generate_bearer_token, load_or_create_master_key,
        mask_hint,
    };

    #[test]
    fn debug_output_redacts_secret_material() {
        let secret = SecretString::new("super-secret-value");
        let token = BearerToken::new("mcpv_pat_sensitive".to_owned());

        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(format!("{token:?}"), "[REDACTED]");
        assert!(!format!("{secret:?}").contains("super-secret"));
    }

    #[test]
    fn master_key_supports_raw_and_hex_files() {
        let raw = [7_u8; 32];
        let first = MasterKeyRing::from_file_bytes(&raw).unwrap();
        assert_eq!(first.current_version(), 1);

        let hex = raw
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect::<String>();
        assert!(MasterKeyRing::from_file_bytes(hex.as_bytes()).is_ok());
        assert!(MasterKeyRing::from_file_bytes(b"too-short").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn master_key_loader_does_not_enforce_permission_bits() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("master-key");
        tokio::fs::write(&path, [7_u8; 32]).await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .await
            .unwrap();
        assert!(MasterKeyRing::load_file(&path).await.is_ok());
    }

    #[tokio::test]
    async fn managed_secret_files_are_created_once_and_reused_concurrently() {
        let directory = tempfile::tempdir().unwrap();
        let key_path = directory.path().join("nested/master-key");
        let (first, second) = tokio::join!(
            load_or_create_master_key(&key_path),
            load_or_create_master_key(&key_path)
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(
            first.installation_key_check(),
            second.installation_key_check()
        );
        assert!(first.is_persistent());
    }

    #[test]
    fn encryption_binds_owner_metadata_and_key_version() {
        let first = MasterKeyRing::from_bytes(1, &[1_u8; 32]).unwrap();
        let payload = first
            .encrypt("provider", "vault", Some("vault-a"), b"api-key")
            .unwrap();
        let plaintext = first
            .decrypt(
                payload.key_version,
                &payload.nonce,
                &payload.ciphertext,
                "provider",
                "vault",
                Some("vault-a"),
            )
            .unwrap();
        assert_eq!(plaintext.as_slice(), b"api-key");
        assert!(
            first
                .decrypt(
                    payload.key_version,
                    &payload.nonce,
                    &payload.ciphertext,
                    "provider",
                    "vault",
                    Some("vault-b"),
                )
                .is_err()
        );

        let second = first.with_rotated_key(2, &[2_u8; 32]).unwrap();
        assert!(
            second
                .decrypt(
                    payload.key_version,
                    &payload.nonce,
                    &payload.ciphertext,
                    "provider",
                    "vault",
                    Some("vault-a"),
                )
                .is_ok()
        );
        assert_eq!(second.current_version(), 2);
    }

    #[test]
    fn bearer_tokens_are_high_entropy_and_hints_are_bounded() {
        let first = generate_bearer_token("mcpv_pat_");
        let second = generate_bearer_token("mcpv_pat_");
        assert_ne!(first.expose_secret(), second.expose_secret());
        assert!(first.expose_secret().starts_with("mcpv_pat_"));
        assert_eq!(mask_hint("abcdefghi"), "abc…ghi");
        assert_eq!(mask_hint("short"), "••••");
    }
}
