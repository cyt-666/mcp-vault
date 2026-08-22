-- Review-remediation authentication/runtime hardening.
--
-- The digest is a one-way installation-key verification value. It lets
-- startup distinguish the configured key from a different valid 32-byte key
-- before accepting requests; no key material is stored in SQLite.

CREATE TABLE installation_key_checks (
    key_version INTEGER PRIMARY KEY CHECK (key_version > 0),
    verification_digest BLOB NOT NULL CHECK (length(verification_digest) = 32),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Earlier prerelease builds accepted symmetric HS256 JWK material in this
-- ordinary JSON column. The migration cannot prove which historical JSON is
-- public-only, so fail closed and require Admin to re-save normalized RSA
-- public keys through the hardened API.
UPDATE oauth_issuers
SET jwks_cache_json = NULL,
    jwks_cached_at = NULL,
    enabled = 0
WHERE jwks_cache_json IS NOT NULL;
