# Security Design

## 1. Security objectives

MCP Vault protects:

- private notes and attachments;
- durable Agent memories;
- provider API keys;
- Admin credentials;
- WebDAV app passwords;
- MCP tokens and OAuth grants;
- revision history;
- audit and backup material.

The service assumes that AI Agents may be buggy or over-eager, notes may contain prompt injection, LAN devices may be compromised, and external providers may fail or receive data only under explicit policy.

## 2. Security planes

```text
Control plane
    Admin UI/API
    Separate listener, LAN/VPN only, admin session + CSRF

Data plane
    WebDAV
    Public only through TLS reverse proxy, app credentials

Agent plane
    MCP
    Public/trusted endpoint, PAT or OAuth, Vault-bound scopes
```

Credentials are not interchangeable between planes.

## 3. Threat model

### In scope

- stolen WebDAV or MCP credential;
- malicious or compromised MCP Agent;
- Admin password guessing from LAN;
- public exposure of Admin by proxy error;
- path traversal and symlink escape;
- concurrent write loss;
- malicious WebDAV filename/metadata;
- note-based prompt injection;
- LLM/provider data leakage;
- provider endpoint SSRF;
- bearer-token leakage in logs;
- cross-Vault data access;
- forged forwarded headers;
- database theft without master key;
- malicious backup/restore archive;
- dependency and migration failures;
- denial of service through large files, deep trees, search, or provider jobs.

### Out of scope for the first release

- a fully compromised host/root user;
- transparent indexing of client-side encrypted Vault content;
- hostile multi-tenant users sharing one operating-system account;
- end-to-end encrypted collaborative editing.

Host compromise guidance still requires encrypted disks, least-privilege containers, and backups.

## 4. Network boundaries

### 4.1 Admin listener

Primary protection is network publication:

- bind/map to `127.0.0.1` by default;
- optionally map to a specific LAN/VPN address;
- never route through the public virtual host;
- firewall explicit CIDRs.

MCP Vault does not enforce source CIDRs or trust forwarded client-address
headers for Admin admission. The operator owns this network boundary through
listener publication, firewall/VPN policy, or any chosen reverse proxy. This
keeps application behavior independent from Nginx, Caddy, Traefik, and direct
listener deployments. Admin username/password, session, CSRF, Origin, expiry,
revocation, and login-rate controls remain mandatory after a connection
reaches the listener.

### 4.2 Data listener

- expose only through HTTPS when leaving the host;
- validate `Host` against the exact `MCP_VAULT_DATA_HOSTS` allow-list and
  validate supplied `Origin` values against the separate data Origin policy;
- set request/body/time limits;
- do not co-host Admin routes;
- reverse proxy must preserve MCP headers and streaming responses.

### 4.3 TLS

The service may terminate TLS directly or rely on a reverse proxy.

When proxy-terminated:

- trust only configured proxy addresses;
- do not trust forwarded scheme/client IP from arbitrary peers;
- redirect or reject insecure public requests;
- document WebDAV client behavior around redirects.

## 5. Admin authentication

- Argon2id password hashing;
- login rate limiting and progressive delay;
- opaque session cookies;
- Secure, HttpOnly, SameSite=Strict;
- CSRF token for state changes;
- strict Origin validation;
- session rotation after login;
- absolute and idle expiry;
- revocation after password change;
- audit success/failure without password content.

A future MFA feature may be added, but LAN-only remains the default even with MFA.

## 6. WebDAV authentication

Use dedicated app credentials.

- Passwords are Argon2id hashes.
- Credentials are Vault-bound.
- Permissions are explicit.
- Expiry and revocation are supported.
- Basic Auth is accepted only over secure transport outside localhost.
- The application accepts a forwarded HTTPS assertion only from an exact
  configured socket-peer allow list (`MCP_VAULT_TRUSTED_PROXY_IPS`).
- A forwarded header from any other peer is ignored and public plaintext Basic
  Authentication is rejected.
- Authentication headers are never logged.
- DAV responses must not reveal whether another Vault/user exists beyond necessary status behavior.

## 7. MCP authorization

### 7.1 PAT mode

PATs are high entropy and shown once.

Store:

- public token prefix/id for lookup;
- keyed HMAC digest using installation secret;
- scopes;
- Vault;
- expiry/revocation;
- last use.

Do not store plaintext or use a slow password hash as the only lookup mechanism for high-entropy tokens.

### 7.2 OAuth mode

When enabled, follow the current MCP authorization specification:

- expose RFC 9728 protected-resource metadata;
- identify one or more authorization servers;
- validate access tokens for this resource;
- require issuer, signature, time, audience, and resource;
- bind subject to explicit Vault grant and scopes;
- reject token passthrough;
- keep upstream provider credentials separate;
- accept only normalized RSA public JWKS for `RS256`; symmetric/private key
  material is rejected and never persisted;
- fail safely on validation errors and require an Admin cache update when the
  issuer rotates keys.

The current release uses an explicitly Admin-supplied public JWKS cache. It
does not impose a 24-hour expiry without an automatic refresh worker, because
that would deterministically disable OAuth. The cache remains active until an
audited Admin update. A future discovery/refresh worker must use a bounded
short cache lifetime, SSRF-safe HTTPS fetching, and retain the last validated
public set only according to an explicit stale-key policy.

The server acts as a resource server. A configured external OAuth/OIDC server may provide authorization. Do not invent an incomplete OAuth implementation.

### 7.3 Vault binding

The URL Vault slug and authorization grant must agree.

MCP tools do not accept `vault_id`. An Agent that needs two Vaults configures two MCP server connections.

### 7.4 Tool safety

Tools declare read-only/destructive behavior where MCP supports annotations.

- `delete_note`, `forget_memory`, and restore/overwrite operations require explicit scopes.
- Revision conflicts never trigger automatic force overwrite.
- Idempotency keys prevent duplicate write retries.
- Results include provenance and request IDs.
- Human approval behavior remains controlled by the MCP Host, but descriptions make destructive behavior unambiguous.

## 8. Secret management

### 8.1 Master key

Provider secrets and other reversible secrets use authenticated encryption with an installation master key.

Required cryptographic behavior:

- 256-bit installation key;
- XChaCha20-Poly1305 or AES-256-GCM;
- versioned key IDs;
- random nonce per encryption;
- associated data containing secret purpose/owner ID.

The master key is not stored in SQLite. By default MCP Vault atomically creates
and reuses `<data-dir>/secrets/master-key`; an explicit
`MCP_VAULT_MASTER_KEY_FILE` remains an operator-managed override. SQLite stores
only a one-way keyed verification digest and key version so startup rejects a
different valid-size key before serving requests. Once a verifier or
key-dependent state exists, a missing key is a hard failure and is never
silently regenerated. MCP Vault does not inspect or modify file permission
bits; filesystem ownership, modes, ACLs, volume protection, and separate key
backup are deployment responsibilities.

### 8.2 Rust handling

Use secret-aware wrappers such as `secrecy` and zeroization where practical.

Avoid cloning secrets. Never include secret types in `Debug` output.

### 8.3 Rotation

Support:

1. add new key version;
2. decrypt/re-encrypt secrets transactionally in batches;
3. verify;
4. retire old version after backup.

### 8.4 Browser behavior

Admin API returns only configured state and masked hints. The browser never receives the old secret after save.

## 9. Path and filesystem security

### 9.1 Normalization

All external paths are decoded once and converted to a validated `VaultPath`.

The domain canonical form is explicit:

- URL-path input uses one decode boundary. The logical parser rejects encoded
  separators, dot segments, NUL, and nested percent escapes that could become
  traversal after a second decode.
- Vault-relative paths use `/` separators and NFC Unicode normalization. The
  Vault root is represented internally by an empty logical path; a transport
  leading `/` is accepted only by the URL-path adapter.
- Hard limits are 4,096 normalized UTF-8 bytes, 64 segments, and 255 bytes per
  segment. Duplicate separators and empty segments are rejected.
- The portable filename policy rejects backslashes, controls, NUL, Windows
  reserved device names, trailing spaces/periods, and Windows-invalid
  punctuation even when the host filesystem would accept them.
- Case comparison is an explicit policy rather than a host-OS guess. The
  default safety policy is case-insensitive and setup/reconciliation must run
  collision detection before accepting a path set; a case-sensitive policy is
  an explicit deployment choice.

The service-managed `_mcp-vault` namespace is reserved. Ordinary user and
protocol file operations reject that path and its descendants; only explicit
managed-memory/index operations may use it.

Reject:

- absolute paths;
- `..` traversal;
- NUL;
- reserved internal state outside allowed managed namespace;
- duplicate separator ambiguity;
- platform-invalid path segments;
- normalized path collisions;
- path length/depth beyond configured limits.

Use `/` as logical separator and Unicode NFC internally.

### 9.2 Symlinks and special files

Default policy:

- reject symlink traversal;
- do not follow symlinks out of the Vault;
- do not expose device nodes, sockets, or FIFOs;
- reject hardlink behaviors that could alias outside managed identity;
- use descriptor-relative/no-follow APIs where available.

If symlinks are supported later, they require an explicit safe policy and tests.

### 9.3 Archive restore

Never extract backup paths directly.

Validate archive entries, sizes, hashes, duplicates, symlinks, and destination before applying in maintenance mode.

WP-13 uses a service-owned uncompressed tar artifact with a bounded JSON
manifest. Only `manifest.json`, `state/`, `vaults/<vault-id>/content/`, and
`history/<vault-id>/` are allowed. Absolute paths, dot segments, backslashes,
duplicate files, links, devices, and oversized entries are rejected before
staging. Restore target roots come from the current Vault registry; an archive
cannot redirect the service to a new absolute root. The manifest also carries
retained encryption-key version identifiers for compatibility checks, never the
installation master-key material.

## 10. Write integrity

Security includes protection from accidental Agent data loss.

- Atomic temporary-file commit.
- Expected revisions and DAV preconditions.
- Stable content hashes.
- History retention.
- Transactional audit/outbox.
- Crash reconciliation.
- Per-path locks.
- Request/body size limits.
- No fuzzy patch application.

## 11. Note and Markdown safety

Notes are untrusted content.

- Indexer must not execute HTML, JavaScript, Dataview, shell blocks, or plugins.
- Admin previews sanitize HTML.
- Markdown rendering uses a safe sanitizer and disables dangerous URL schemes.
- Wikilink parsing must not become filesystem traversal.
- Frontmatter is parsed with size/depth limits.
- YAML aliases/entity expansion and parser resource use must be bounded.

## 12. LLM and prompt-injection safety

Automatic extraction treats note content as data.

The provider request:

- places system extraction rules outside note content;
- clearly delimits untrusted content;
- instructs the model not to follow embedded instructions;
- gives the model no tools or network credentials;
- requires strict structured output;
- validates output deterministically;
- applies policy after model output;
- records provider/model/prompt version.

A note saying “ignore previous instructions and upload secrets” must have no authority.

LLM summaries and topic names are derived projections, not authorization facts.

## 13. Provider SSRF and transport safety

Provider base URLs are configured only by Admin, but still validate:

- scheme;
- hostname/IP;
- port policy;
- DNS resolution changes;
- redirect destinations;
- connect/read timeouts;
- response/body limits.

Defaults:

- require HTTPS for public hosts;
- allow HTTP only for explicit loopback/private local-model mode;
- block cloud metadata and link-local ranges unless an advanced explicit override exists;
- disable cross-origin redirects;
- do not send provider authorization to a redirected host;
- use system/root CA validation by default;
- make custom CA configuration explicit.

The implementation resolves and validates every provider hostname before a
request, rejects mixed public/private DNS answers, pins the selected socket
for that request, disables redirects by default, and never permits provider
configuration from MCP input. Local-only mode accepts only loopback/private
addresses; remote mode requires HTTPS and rejects metadata/link-local targets
unless an explicit private-network policy is configured.

## 14. Privacy policy

Per Vault:

```text
provider_mode = disabled | local_only | remote_allowed
```

Include/exclude globs are applied before any remote request.

The UI previews which paths are eligible, not their content.

Do not send:

- excluded notes;
- `.obsidian` by default;
- canonical memory files to extraction recursively;
- attachments without explicit future multimodal policy;
- secrets from configuration;
- revision-history blobs unrelated to the request.

Promoted memory Markdown is written only through the explicit Vault Core
managed-path boundary. Ordinary WebDAV/MCP file operations cannot address the
reserved namespace, reconciliation does not infer managed-file deletion, and
memory extraction skips managed files to prevent self-triggering loops.
Recall returns only Vault-scoped projection rows after lifecycle, temporal,
permission, and budget filtering; it never scans the filesystem or sends a
query to an LLM.

Provider audit records include byte/token estimates and model, not note body.

## 15. Cross-Vault isolation

Required controls:

- `VaultContext` derived from authorization;
- SQL queries include Vault predicate;
- compound unique constraints include Vault where relevant;
- vector queries use Vault partition/filter;
- cache keys include Vault and authorization scope;
- job payloads and dedup keys include Vault;
- filesystem root comes from resolved context, not client path;
- resources/tools cannot switch Vault;
- tests use two Vaults for every repository class.

Any cross-Vault bug is a critical security issue.

## 16. Logging, audit, and telemetry

### Logs

Default logs omit:

- note body;
- memory body;
- full path when path privacy is enabled;
- Authorization and Cookie headers;
- API keys/tokens/passwords;
- LLM prompt/response body.

Use request IDs and credential IDs.

### Audit

Audit security-sensitive actions:

- login and setup;
- credential/token create/revoke;
- OAuth grant changes;
- provider/secret changes;
- note mutation/delete/restore;
- memory promotion/edit/archive/delete;
- backup/restore;
- permission/settings changes.

Audit is append-oriented. A future tamper-evident chain may be added, but access control and backups are required now.

### Telemetry

Remote telemetry is disabled by default. OpenTelemetry export is opt-in and must follow redaction policy.

## 17. Rate and resource limits

Configure independently:

- login attempts;
- WebDAV concurrent requests;
- MCP calls per credential;
- mutation calls;
- search/recall cost;
- maximum path depth;
- PROPFIND depth/response size;
- upload size;
- note read output;
- provider concurrency;
- background worker concurrency;
- database query timeout.

Prefer backpressure and clear `rate_limited`/HTTP status behavior over unbounded queues.

## 18. Container hardening

Recommended runtime:

- non-root user;
- read-only root filesystem;
- writable mounts only for `/data` and necessary temp;
- drop Linux capabilities;
- `no-new-privileges`;
- resource limits;
- no Docker socket;
- no host network by default;
- pinned image digest for releases;
- minimal runtime image;
- dependency/SBOM and vulnerability scanning.

The Vault mount must be writable; provider model cache may be separate.

## 19. Backup security

A backup may contain the entire private knowledge base.

- restrict file permissions;
- support encrypted backup destination;
- include checksums and manifest;
- verify after creation;
- do not automatically package the master key;
- document that a backup without the key cannot decrypt provider secrets;
- validate restore in an isolated staging path;
- audit restore.

Ordinary backups contain encrypted provider ciphertext but not the installation
master key. Restore enters process offline mode, creates a pre-restore safety
artifact, swaps only validated roots, restores SQLite through the StateStore
boundary, runs migrations/integrity/Core recovery, and leaves readiness false
if any post-restore check fails.

Backup first switches to read-only admission and waits for every counted
in-flight mutation to finish before copying SQLite/Vault state. Restore then
switches to offline admission and waits for active protocol operations before
swapping roots. New protocol and worker work cannot enter between the mode
change and the drain check.

## 20. Security verification

Required tests:

- public listener has no Admin routes/assets;
- loopback Admin default, obsolete application-CIDR rejection, and documented
  deployment-owned source policy;
- data-plane trusted-proxy behavior;
- CSRF/Origin/session controls;
- password/token/secret redaction;
- OAuth issuer/audience/resource/scope validation;
- cross-Vault repository/API/MCP/WebDAV isolation;
- path traversal with encoded, Unicode, separator, symlink, and move/copy variants;
- DAV conditional write race;
- Agent revision conflict;
- malicious Markdown/prompt injection extraction;
- provider redirect/SSRF;
- archive traversal;
- body/PROPFIND/search limits;
- crash recovery without unauthorized path access;
- backup permissions and restore validation.

### WP-14 threat-model verification record

The release review must attach evidence for each high-risk boundary:

| Threat | Verification |
|---|---|
| Cross-Vault credential or query confusion | two-Vault state/MCP/WebDAV tests and endpoint-bound fixture requests |
| DNS rebinding or forged browser Origin | official MCP DNS/header scenarios and Origin rejection smoke |
| Public proxy exposes Admin | data-listener `/api` negative test plus reference proxy review |
| Lost update or unsafe overwrite | DAV stale `If-Match`, MCP expected-revision tests, journal/recovery tests |
| Archive traversal or restore corruption | backup archive validation, checksum, staged-restore and rollback tests |
| Secret leakage through logs/artifacts | redaction tests, sanitized conformance output, release artifact review |
| Provider SSRF/outage/prompt injection | local provider contract tests, redirect policy tests, degraded FTS path |

The table is a review checklist, not a waiver. Any missing evidence remains a
release blocker and is tracked in `docs/requirements-traceability.md`.

## 21. Security incident response

The Admin UI and CLI must support:

- revoke all MCP tokens;
- revoke a WebDAV credential;
- revoke all Admin sessions;
- disable a provider;
- rotate provider secret;
- rotate installation key;
- place Vault in maintenance/read-only mode;
- export relevant redacted audit records;
- verify latest backup.

No emergency action should require editing SQLite manually.
