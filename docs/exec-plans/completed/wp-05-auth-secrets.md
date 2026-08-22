# WP-05 Authentication, Authorization, and Secret Storage

> Security supersession (2026-08-22): WP-14 removed the prerelease HS256/
> symmetric-JWK path. Current code accepts normalized RSA public JWKS for
> RS256 only, and migration 0009 clears legacy cached key JSON. References
> below to HS256 describe the historical WP-05 implementation, not current
> behavior.

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Implement the independent Admin, WebDAV, and MCP authentication boundaries
needed by the later protocol adapters. The service will load an installation
master key without persisting it in SQLite, encrypt reversible secrets with
versioned authenticated encryption, hash passwords with Argon2id, issue and
validate opaque Admin sessions with CSRF protection, issue Vault-bound WebDAV
credentials, and issue/verify Vault-bound MCP PATs without storing plaintext
tokens.

The slice will also validate configured OAuth resource-server JWTs against
issuer, signature, time, audience, resource, subject grant, and scope rules;
provide deterministic scope-to-permission mapping; reject cross-Vault
credential use; validate per-listener Origin policy; and expose redaction-safe
secret/token types for future handlers and logs.

## Governing requirements

- `AGENTS.md`: separate Admin/WebDAV/MCP authentication domains, Vault
  isolation, Argon2id passwords, keyed token digests, encrypted provider
  secrets, and redaction-safe types.
- `docs/implementation-plan.md` section 8 (WP-05): master-key loading and
  versioning, encrypted secret repository, passwords, sessions/CSRF, WebDAV
  credentials, PATs, OAuth resource-server validation, scope mapping, Origin,
  and security tests.
- `docs/product-requirements.md` sections 3.7-3.8, 4.5, and 6: separate
  security planes, Vault-bound credentials, OAuth 2.1 resource-server mode,
  and secret-redacting operation.
- `docs/architecture.md` sections 1, 4.5, 5.1-5.3, 12, 13, and 16:
  listener separation, repository ownership, protocol-neutral auth services,
  Vault binding, and redacted observability.
- `docs/interfaces.md` sections 1-2, 3.2, 9, 10.2, and 10.5: credential
  separation, endpoint binding, PAT/OAuth contracts, Admin session/CSRF
  behavior, and masked secret responses.
- `docs/security.md` sections 2, 5-8, 15-16, and 20: password/session
  controls, WebDAV app passwords, PAT keyed digests, JWT validation, master
  key handling, Vault isolation, logging, and required security tests.
- `docs/admin-and-configuration.md` sections 3-5, 9-10, and 20: setup
  bootstrap, typed bootstrap settings, credential/token management, OAuth
  resource configuration, and acceptance checks.
- `docs/data-model.md` sections 4-7 and ADR-0008: existing operational auth
  tables, forward-only schema evolution, and one-Vault-per-MCP-credential.

## Current repository state

WP-00 through WP-04 are present in the working tree. `crates/auth` contains
only a protocol-neutral placeholder. `migrations/0001_operational_state.sql`
already defines `encrypted_secrets`, `admin_users`, `admin_sessions`,
`webdav_credentials`, `mcp_tokens`, `oauth_issuers`, and
`oauth_subject_grants`; `0002` adds operation idempotency. `mcp-vault-state`
owns the pool, migrations, Vault/settings, and file repositories but has no
typed auth/secret repository. `server::AppConfig` already accepts master-key
and bootstrap-token file paths but startup does not load or validate them.
Domain scopes and permissions already provide deterministic application
capabilities and all prior Core operations require `VaultContext`.

## Scope

### Included

- Add migration `0003_auth_security.sql` for OAuth protected-resource
  configuration and persisted non-secret secret hints.
- Add typed state records and repositories for encrypted secrets, Admin users
  and sessions, WebDAV credentials, MCP PATs, OAuth issuers, and subject grants.
- Add versioned `MasterKeyRing`, authenticated XChaCha20-Poly1305 secret
  encryption/decryption, per-secret associated data, rotation support, and
  redaction-aware secret/token wrappers.
- Add Argon2id password policy, transparent rehash detection, Admin setup/
  login/logout/password-change/session-expiry/CSRF behavior, and bounded login
  failure tracking.
- Add Vault-bound WebDAV app credential issue/verify/revoke behavior and
  high-entropy Vault-bound PAT issue/verify/rotate/revoke behavior using keyed
  HMAC-SHA-256 lookup digests.
- Add configured OAuth issuer/JWK validation for HS256 and RS256 JWTs with
  issuer, signature, expiry, not-before, audience, protected-resource,
  subject-grant, and scope checks. No token passthrough or arbitrary
  `vault_id` selection is allowed.
- Add strict Admin state-changing Origin/Referer + CSRF validation and
  optional data-plane Origin validation helpers without putting policy in
  protocol handlers.
- Load the configured master key before readiness and fail startup when
  encrypted secrets exist but no usable key is available.
- Add two-Vault and redaction/security tests and update schema/security docs,
  checksums, and migration-version assertions.

### Not included

- Admin HTTP routes/UI, WebDAV `dav-server` adapter, MCP RMCP middleware, or
  OAuth authorization-server implementation; those later packages consume
  this auth service and map its principals/errors to their protocols.
- Network JWKS discovery/refresh worker; the repository stores a validated
  cached JWK set and its discovery URL, while provider/network fetching can be
  added behind the WP-06/08 worker/transport boundaries.
- MFA, password reset email, distributed rate limiting, or cross-Vault/federated
  authorization.

## Invariants and risks

- Every WebDAV/PAT/OAuth authorization lookup includes the requested
  `VaultContext`/Vault ID. A credential/grant from another Vault is
  indistinguishable from invalid credentials at the protocol boundary.
- Admin credentials and sessions are global control-plane records; they never
  authorize WebDAV or MCP access. WebDAV passwords are never reused as PATs,
  and PATs never become upstream provider credentials.
- Passwords are slow Argon2id hashes. High-entropy tokens use a visible
  prefix plus installation-keyed HMAC digest; plaintext is returned only once
  in an explicit issuance result and never stored or logged.
- Reversible secrets use XChaCha20-Poly1305 with a random nonce and
  associated data containing purpose and owner identity. Key versions are
  explicit; decryption rejects unknown versions or swapped metadata.
- OAuth validation is fail-closed: only configured algorithms/keys are used;
  `none`, unsigned tokens, mismatched issuer/audience/resource, invalid time,
  missing subject grant, revoked grant, and disallowed scopes are rejected.
- Session cookies are opaque, Secure, HttpOnly, SameSite=Strict, and only
  token digests are persisted. State-changing Admin requests require a
  session-bound CSRF token and strict Origin/Referer validation.
- Auth errors and `Debug` implementations never contain password, token,
  cookie, authorization-header, key, or secret material.
- Existing `0001` and `0002` migrations remain immutable; `0003` is forward
  only. Missing master key blocks only when encrypted secret rows require it,
  while provider availability does not block core readiness.

## Proposed design

```text
protocol adapter (later)
  → auth service / principal validator
  → state auth repositories (SQL only here)
  → domain VaultContext, ScopeSet, PermissionSet

bootstrap config
  → MasterKeyRing
  → SecretRepository (AEAD)
```

`mcp-vault-auth` owns password/token/JWT/CSRF/origin algorithms and
application orchestration. `mcp-vault-state` owns all SQL and returns typed
records. The server composition root loads the key and only uses a count/
health repository check; it does not parse credentials itself.

### State records and migration

`AuthStateRepository` will provide typed CRUD/lookup methods for the existing
auth tables. Vault-owned methods take `&VaultContext`; global Admin and
installation-secret methods are explicitly separate. OAuth issuer records
store a protected-resource value in addition to the existing audience and
encrypted-secret rows store a masked hint that contains no recoverable
plaintext.

### Master key and secrets

`MasterKeyRing` holds one or more 32-byte key versions in zeroizing memory.
New ciphertext uses the current version. XChaCha20-Poly1305 associated data is
`purpose || owner_type || owner_id`; a record's persisted key version and
metadata must match at decrypt time. `SecretService::rotate_all` decrypts and
rewrites records in bounded batches, preserving IDs and hints.

### Principal flows

- Admin setup is create-only while no Admin user exists. Login verifies the
  Argon2id hash, rotates/revokes prior sessions, creates an opaque session and
  independent CSRF token, and records only safe source/user-agent metadata.
  Session validation checks disabled/revoked/idle/absolute expiry and touches
  last use. Password change rehashes and revokes all sessions.
- WebDAV issue/verify is bound to one Vault and an explicit `PermissionSet`.
  Expired/revoked credentials fail without revealing whether the username
  exists in another Vault.
- PAT issuance creates 32 random bytes, returns a one-time `mcpv_pat_...`
  secret, stores only prefix/HMAC digest/scopes/expiry, and validates against
  the endpoint Vault before returning a principal.
- OAuth JWT validation parses untrusted header/payload safely, selects a
  configured `kid`, verifies HS256/RS256, checks claims and time with bounded
  clock skew, then intersects token scopes with the configured subject grant
  for the requested Vault.

### Listener security helpers

`OriginPolicy` parses exact scheme/host/port origins. Admin mutation checks
`Origin` or a same-origin `Referer` plus the session CSRF token; the data-plane
helper validates an Origin when present. These are reusable by Axum/WebDAV/
RMCP adapters and do not decide application permissions themselves.

## Work breakdown

1. Add `0003_auth_security.sql`, typed auth state records/repositories, and
   migration/version/isolation tests.
2. Add key loading/versioning, AEAD secret repository, redaction wrappers,
   Argon2id password policy, and secure random token utilities.
3. Add Admin session/CSRF/origin service, WebDAV credential service, PAT
   service, and bounded login rate limiter.
4. Add OAuth issuer/grant repository operations and JWT/JWK resource-server
   validation with scope-to-permission mapping.
5. Integrate bootstrap key validation before server readiness and update
   security/schema/deployment docs without exposing routes prematurely.
6. Run focused/security/workspace/frontend/docs/container checks, record
   evidence, and move this plan to `docs/exec-plans/completed/`.

## Progress

- [x] 2026-08-20 — Read root instructions, governing product/architecture/
  interface/security/operations documents, WP-05, PLANS, and ADR-0008.
- [x] 2026-08-20 — Inspected existing auth placeholder, authentication tables,
  domain scopes, state pool, server bootstrap config, and protocol boundaries.
- [x] 2026-08-20 — Added migration 0003 and typed auth state repositories with
  Vault predicates, key-version metadata, OAuth resource, and safe Debug
  implementations.
- [x] 2026-08-20 — Implemented key loading/versioning, AEAD secret storage and
  rotation, redaction wrappers, Argon2id policy/rehash, and token utilities.
- [x] 2026-08-20 — Implemented Admin setup/login/session expiry/revocation,
  WebDAV app credentials, PAT issue/verify/rotate/revoke, Origin/CSRF, and a
  bounded login limiter.
- [x] 2026-08-20 — Implemented HS256/RS256 OAuth JWT validation, JWK cache
  validation, resource/audience/time checks, subject grants, and scope mapping.
- [x] 2026-08-20 — Integrated bootstrap key/token validation before readiness,
  typed listener Origin configuration, migration/docs/check updates, and
  server missing-key coverage.
- [x] 2026-08-20 — Ran final workspace/frontend/docs/container checks,
  startup smoke, and container build; completed the plan.
- [ ] 2026-08-20 — Run final checks and complete the plan.

## Decisions

- Use Argon2id PHC strings with explicit current parameters so the encoded
  string carries its salt and algorithm parameters and login can detect
  rehash needs without a custom password schema.
- Use HMAC-SHA-256 keyed by the installation master key for PAT and session
  lookup digests. This avoids slow password hashing for high-entropy tokens and
  prevents a database-only attacker from using stored digests as token tests.
- Support HS256 and RS256 only for the first resource-server validator. Reject
  all other/unsigned algorithms explicitly; a future algorithm requires a
  separate compatibility and threat-model decision.
- Store a protected-resource value and masked secret hint through a forward
  migration rather than overloading `audience` or recomputing browser hints
  from plaintext on every response.
- Keep OAuth JWK discovery/network refresh outside this work package. A
  configured and cached JWK set is validated locally; later workers may refresh
  it with SSRF/redirect policy and bounded cache lifetimes.

## Surprises and discoveries

- The WP-02 schema already reserved the complete credential/session/OAuth table
  shape, so this package can add typed repositories without rewriting prior
  migrations.
- `mcp-vault-auth` had no dependencies beyond the domain crate and no public
  behavior, so all security algorithms need to be introduced behind explicit
  redaction-safe types before protocol middleware is added.

## Validation

```bash
cargo fmt --all --check
cargo test -p mcp-vault-auth --all-features
cargo test -p mcp-vault-state --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
make build
docker build --tag mcp-vault:wp05 .
```

Security evidence must include Argon2id rehash, session expiry/revocation,
CSRF/Origin, PAT one-time issuance/digest lookup, secret encryption/rotation/
redaction, OAuth claim/signature/grant checks, two-Vault credential isolation,
and startup failure when an encrypted secret has no master key.

## Rollback and recovery

`0003` is forward-only and must be backed up with the SQLite state before
deployment. A binary rollback to pre-WP-05 requires restoring a pre-`0003`
database or using a compatible binary; no migration is edited or reversed.

If the master key cannot be loaded, startup fails before readiness when
encrypted rows exist. Existing canonical Vault content and opaque credentials
remain untouched. Secret rotation writes authenticated ciphertext and can be
retried by key version; a failed row remains decryptable with its prior key.

## Outcomes

- Added migration `0003_auth_security.sql` with masked secret hints, OAuth
  protected-resource metadata, and digest key versions for sessions/PATs.
- Added typed, Vault-context-checked state repositories for secrets, Admin
  users/sessions, WebDAV credentials, MCP PATs, OAuth issuers, and subject
  grants. Sensitive state records have redacted `Debug` output.
- Implemented `mcp-vault-auth` with versioned XChaCha20-Poly1305 key rings,
  Argon2id hashing/rehash detection, opaque Secure/HttpOnly/SameSite session
  cookies, session-bound CSRF, bounded login rate limiting, WebDAV app
  passwords, keyed PAT digests, rotation/revocation, and redaction types.
- Implemented exact Origin/Referer policy and OAuth resource-server
  validation for HS256/RS256 cached JWKs, issuer/signature/time/audience/
  resource checks, Vault-bound subject grants, and scope-to-permission
  intersection. Unsigned/unsupported/stale/mismatched tokens fail closed.
- Startup now validates configured master-key/bootstrap material and blocks
  readiness when encrypted state exists without a usable master key. Listener
  configuration carries independent Admin/data Origin allow-lists.
- Focused auth/state/server tests and `make check` passed, including 17 auth
  tests, three auth-repository isolation tests, and startup missing-key
  coverage. Loopback smoke returned `{"status":"ok"}` and
  `{"status":"ready"}`.
- `make build` passed. `docker build --tag mcp-vault:wp05 .` passed with image
  digest `sha256:0e243c6ced0daf94a52f83ee633d6fc25c37fe2c9a34bea8e2ebd4f75d007554`.
- Network JWKS discovery/refresh remains intentionally outside WP-05; the
  validator consumes a persisted, age-checked cache and fails safely until a
  later worker/transport package refreshes it.

This plan is complete and is moved to `docs/exec-plans/completed/`.
