-- Built-in OAuth 2.1 authorization server state.
--
-- Human passwords use Argon2id PHC strings. Every high-entropy request handle,
-- authorization code, access token, and refresh token is persisted only as an
-- installation-keyed digest. OAuth grants remain bound to one Vault and exact
-- protected resource.

CREATE TABLE oauth_local_users (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL UNIQUE,
    username TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    scopes_json TEXT NOT NULL CHECK (json_valid(scopes_json)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    password_changed_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (vault_id, username),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE TABLE oauth_clients (
    id TEXT PRIMARY KEY,
    client_name TEXT NOT NULL,
    redirect_uris_json TEXT NOT NULL CHECK (json_valid(redirect_uris_json)),
    grant_types_json TEXT NOT NULL CHECK (json_valid(grant_types_json)),
    response_types_json TEXT NOT NULL CHECK (json_valid(response_types_json)),
    token_endpoint_auth_method TEXT NOT NULL
        CHECK (token_endpoint_auth_method = 'none'),
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
);

CREATE TABLE oauth_authorization_requests (
    id TEXT PRIMARY KEY,
    request_digest BLOB NOT NULL UNIQUE CHECK (length(request_digest) = 32),
    digest_key_version INTEGER NOT NULL CHECK (digest_key_version > 0),
    client_id TEXT NOT NULL,
    vault_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    scopes_json TEXT NOT NULL CHECK (json_valid(scopes_json)),
    state TEXT,
    code_challenge TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    consumed_at INTEGER,
    FOREIGN KEY (client_id) REFERENCES oauth_clients(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE TABLE oauth_authorization_codes (
    id TEXT PRIMARY KEY,
    code_digest BLOB NOT NULL UNIQUE CHECK (length(code_digest) = 32),
    digest_key_version INTEGER NOT NULL CHECK (digest_key_version > 0),
    client_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    vault_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    scopes_json TEXT NOT NULL CHECK (json_valid(scopes_json)),
    code_challenge TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    consumed_at INTEGER,
    FOREIGN KEY (client_id) REFERENCES oauth_clients(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES oauth_local_users(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE TABLE oauth_access_tokens (
    id TEXT PRIMARY KEY,
    family_id TEXT NOT NULL,
    token_prefix TEXT NOT NULL,
    token_digest BLOB NOT NULL UNIQUE CHECK (length(token_digest) = 32),
    digest_key_version INTEGER NOT NULL CHECK (digest_key_version > 0),
    client_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    vault_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    scopes_json TEXT NOT NULL CHECK (json_valid(scopes_json)),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER,
    FOREIGN KEY (client_id) REFERENCES oauth_clients(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES oauth_local_users(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE TABLE oauth_refresh_tokens (
    id TEXT PRIMARY KEY,
    family_id TEXT NOT NULL,
    token_prefix TEXT NOT NULL,
    token_digest BLOB NOT NULL UNIQUE CHECK (length(token_digest) = 32),
    digest_key_version INTEGER NOT NULL CHECK (digest_key_version > 0),
    client_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    vault_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    scopes_json TEXT NOT NULL CHECK (json_valid(scopes_json)),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    rotated_at INTEGER,
    revoked_at INTEGER,
    FOREIGN KEY (client_id) REFERENCES oauth_clients(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES oauth_local_users(id) ON DELETE CASCADE,
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);

CREATE INDEX oauth_clients_active_idx
    ON oauth_clients(revoked_at, created_at, id);

CREATE INDEX oauth_authorization_requests_expiry_idx
    ON oauth_authorization_requests(expires_at, consumed_at);

CREATE INDEX oauth_authorization_codes_expiry_idx
    ON oauth_authorization_codes(expires_at, consumed_at);

CREATE INDEX oauth_access_tokens_lookup_idx
    ON oauth_access_tokens(vault_id, token_prefix, token_digest);

CREATE INDEX oauth_access_tokens_user_idx
    ON oauth_access_tokens(user_id, revoked_at, expires_at);

CREATE INDEX oauth_access_tokens_family_idx
    ON oauth_access_tokens(family_id, revoked_at, expires_at);

CREATE INDEX oauth_refresh_tokens_lookup_idx
    ON oauth_refresh_tokens(token_prefix, token_digest);

CREATE INDEX oauth_refresh_tokens_family_idx
    ON oauth_refresh_tokens(family_id, revoked_at, expires_at);
