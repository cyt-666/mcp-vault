-- MCP Vault WP-05 authentication and secret metadata.
--
-- Existing credential ciphertext and OAuth issuer rows remain valid. These
-- nullable fields allow older rows to be migrated without inventing values;
-- auth services fail closed until a protected resource is configured.

ALTER TABLE encrypted_secrets ADD COLUMN hint TEXT;
ALTER TABLE oauth_issuers ADD COLUMN resource TEXT;
ALTER TABLE admin_sessions ADD COLUMN digest_key_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE mcp_tokens ADD COLUMN digest_key_version INTEGER NOT NULL DEFAULT 1;

CREATE INDEX encrypted_secrets_owner_idx
    ON encrypted_secrets(owner_type, owner_id);

CREATE INDEX mcp_tokens_vault_prefix_idx
    ON mcp_tokens(vault_id, token_prefix);

CREATE INDEX oauth_subject_grants_lookup_idx
    ON oauth_subject_grants(issuer_id, subject, vault_id);
