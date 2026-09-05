//! Shared normalization and hashing for current canonical memory content.

use unicode_normalization::UnicodeNormalization;

/// Normalize memory text for stable duplicate identity and hashes.
pub fn normalize_content(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Return a stable content hash with the project prefix.
pub fn hash_content(value: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}
