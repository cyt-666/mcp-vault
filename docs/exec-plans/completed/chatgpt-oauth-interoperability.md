# ChatGPT OAuth 2.1 interoperability for the MCP data plane

> Superseded for the default deployment path on 2026-08-29 by ADR-0018 and
> `docs/exec-plans/active/builtin-oauth-authorization-server.md`. The resource-
> server work remains valid as optional external-issuer compatibility, but it
> no longer defines the standalone ChatGPT setup.

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-28
- Updated: 2026-08-28

## Purpose and user-visible result

Make the existing external-issuer OAuth resource-server mode discoverable and
usable by ChatGPT plugin connections. After an administrator configures a
compatible OAuth/OIDC authorization server, ChatGPT can discover protected
resource metadata, start an authorization-code + PKCE flow at that issuer,
send the resulting access token to MCP Vault, and receive a Vault-bound MCP
principal. PAT clients continue to work unchanged.

The Admin MCP page shows the exact protected-resource metadata URL and uses the
advertised MCP endpoint as the safe default for both OAuth `resource` and
`audience`, reducing configuration mismatches.

## Governing requirements

- `AGENTS.md`: separate MCP authorization, Vault-bound credentials, no protocol
  business logic, safe secrets, official RMCP transport, and complete tests.
- `docs/product-requirements.md` sections 3.7-3.8 and 6: OAuth 2.1 resource
  server behavior, Admin configuration, Vault binding, and complete MCP auth.
- `docs/architecture.md` sections 1, 3.2, 4.5, 5.2, 6, 13, and 16: public
  authorization metadata on the data plane, AuthService ownership, stateless
  MCP, multi-Vault isolation, and redacted observability.
- `docs/interfaces.md` section 9: RFC 9728 metadata, `WWW-Authenticate`
  `resource_metadata`, authorization-server discovery, resource indicators,
  JWT checks, grants, and scopes.
- `docs/security.md` section 7 and ADR-0008: external authorization server,
  fail-closed JWT validation, no token passthrough, exact Vault/resource
  binding, and no incomplete local authorization-server implementation.
- `docs/standards-and-references.md` sections 2 and 7: MCP 2026-07-28, RFC
  9728, RFC 8707, RFC 8414, and OAuth 2.1 security behavior.
- Official OpenAI Plugins authentication guidance verified 2026-08-28:
  protected-resource metadata, authorization-server metadata, `resource`
  propagation, authorization code + PKCE S256, and per-request token checks.

## Current repository state

`mcp-vault-auth` already validates configured RS256 JWTs and intersects token
scopes with a Vault-scoped subject grant. `mcp-vault-state` persists external
issuer, resource, public JWKS cache, and subject grants. Admin can configure
those records.

The MCP middleware nevertheless rejects an unauthenticated request with only
`WWW-Authenticate: Bearer realm="mcp-vault"`. No RFC 9728 route exists on the
data listener, so ChatGPT cannot discover the external authorization server.
The validator also requires a non-standard separate `resource` JWT claim even
when the RFC 8707 resource indicator is correctly represented by `aud`, which
breaks common authorization-server tokens. The Admin form defaults `audience`
to `mcp-vault` instead of the actual MCP resource URL and does not display the
metadata URL.

## Scope

### Included

- Publish Vault-specific protected-resource metadata at the RFC path-insertion
  URL and a root alias when exactly one eligible Vault/resource is configured.
- Return a same-origin `resource_metadata` URL on MCP 401 challenges without
  reflecting Host/forwarded-header input.
- Select metadata only from enabled external issuers whose configured resource
  exactly matches the canonical public MCP endpoint; fail closed on missing or
  ambiguous configuration.
- Advertise deterministic supported MCP scopes and header bearer transport.
- Accept the configured resource indicator from either the JWT `aud` claim or
  an explicit `resource` claim while still enforcing configured audience,
  issuer, signature, time, subject grant, and scope checks.
- Wire the public data origin into the MCP adapter, update Admin connection
  information/UI defaults and guidance, and document the external IdP contract.
- Add focused Auth, MCP, server-router, Admin API, and frontend tests.

### Not included

- Implementing a new authorization server, user directory, consent UI, DCR,
  CIMD validation, token endpoint, or refresh-token store inside MCP Vault.
  The configured external IdP owns those responsibilities.
- Automatic network discovery or JWKS refresh; public RSA keys remain an
  explicitly audited Admin update under the current security policy.
- ChatGPT mTLS allow-listing, public deployment changes, or a live external
  IdP/ChatGPT connection performed without operator credentials and DNS/TLS.
- Tool-level incremental step-up authorization. This release authenticates the
  MCP endpoint before RMCP dispatch and advertises the resource-server scopes.

## Invariants and risks

- Metadata never exposes JWKS cache bodies, token contents, subjects, grants,
  note content, or secrets.
- The metadata `resource` is an exact configured HTTPS identifier and must
  match the advertised Vault endpoint; untrusted request headers never define
  it.
- A path for one Vault cannot advertise an issuer/resource configured for
  another Vault path. Root metadata is available only when selection is
  unambiguous.
- PAT behavior, RMCP statelessness, permission checks, and tool filtering do
  not change.
- A valid JWT must still pass signature, exact issuer, time, configured
  audience, resource indicator, Vault subject grant, and scope intersection.
- External authorization servers must publish OAuth/OIDC metadata, support
  authorization code + PKCE S256, support ChatGPT client identification via
  CIMD/DCR/pre-registration, and echo the MCP resource into the access token.

## Proposed design

### Components and dependency direction

```text
ChatGPT GET RFC 9728 metadata
  -> MCP data-plane metadata adapter
  -> AuthService safe enabled-resource projection
  -> State OAuth issuer repository

ChatGPT POST MCP without/with bearer
  -> MCP middleware emits discovery challenge or validates bearer
  -> AuthService JWT + Vault grant validation
  -> request-scoped MCP principal
```

`AuthService` exposes a redaction-safe grouped view of enabled OAuth resources
and authorization-server issuers. The MCP adapter resolves a real Vault, builds
its canonical resource from the configured public data origin, and translates
the safe view to RFC 9728 JSON. SQL and JWT details remain outside handlers.

### Data and transaction flow

Metadata reads are bounded read-only repository queries. Token validation stays
read-only except for existing safe last-use behavior. No canonical files,
indexes, jobs, or migrations are involved.

### Public interfaces and schema changes

- `GET /.well-known/oauth-protected-resource/mcp/v1/vaults/{vault_slug}`
- `GET /.well-known/oauth-protected-resource` when one eligible resource exists
- MCP 401 `WWW-Authenticate` gains a canonical absolute `resource_metadata`
  parameter when the public origin is configured, a same-origin relative local
  fallback, and a safe OAuth error for invalid tokens.
- `GET /api/v1/mcp/connection-info` gains
  `oauth_protected_resource_metadata_url`.

No database migration is planned.

### Failure, retry, and recovery

Missing, disabled, mismatched, or ambiguous OAuth configuration returns 404 for
metadata and never weakens PAT/token checks. Invalid/expired tokens return 401
with a discovery challenge. Configuration fixes take effect on the next request;
there is no durable in-flight state to recover.

## Work breakdown

1. Add a safe AuthService projection for enabled OAuth resources and update JWT
   resource-indicator validation with focused negative/positive tests.
2. Add RFC 9728 routes and OAuth-aware 401 challenges in the MCP adapter,
   compose them only on the public data listener, and add Vault/ambiguity tests.
3. Wire the canonical public data origin through server composition and expose
   the exact metadata URL through Admin connection info.
4. Update the Chinese Admin MCP page with exact endpoint defaults, external IdP
   requirements, and metadata copy affordance; add UI/API tests.
5. Update interface/security/deployment/standards documentation and checksums.
6. Run focused checks, workspace Rust checks, frontend checks, docs/checksum
   checks, and the available MCP conformance wrapper; record any external/live
   validation blocker precisely.

## Progress

- [x] 2026-08-28 — Read governing requirements, architecture, OAuth/MCP
  completed plans, current Auth/MCP/Admin call paths, official OpenAI Plugins
  authentication guidance, and the pinned RMCP OAuth discovery behavior.
- [x] 2026-08-28 — Confirmed the root cause: token validation exists, but RFC
  9728 metadata and `resource_metadata` challenges are absent; resource-claim
  validation also rejects the common RFC 8707 `aud` representation.
- [x] 2026-08-28 — Implemented AuthService public resource grouping and JWT
  resource-indicator compatibility; focused Auth tests pass.
- [x] 2026-08-28 — Implemented Vault-specific/root RFC 9728 routes, absolute
  configured-origin challenges, fail-closed selection, reference-proxy routing,
  and real-HTTP fixture coverage; focused MCP/server tests pass.
- [x] 2026-08-28 — Added Admin connection metadata, ChatGPT-safe defaults and
  Chinese IdP guidance, API/UI tests, operator/security/interface/compatibility
  documentation, and reverse-proxy instructions.
- [x] 2026-08-28 — Completed formatting, workspace Clippy/tests, frontend
  lint/test/build, docs/checksums, real-HTTP smoke, and pinned official MCP
  conformance validation; recorded the live ChatGPT/IdP boundary separately.

## Decisions

- Keep the accepted external-authorization-server architecture. This is the
  smallest complete standards boundary and follows both project security
  requirements and OpenAI's production recommendation to use an established
  IdP instead of inventing authentication.
- Use the RFC path-insertion metadata URL for a path-based MCP resource and a
  root alias only for the single unambiguous first-release resource.
- Treat `aud` as a valid resource-indicator carrier, matching current OpenAI
  guidance, while retaining an explicit `resource` claim as a compatible
  alternative.
- Do not derive canonical OAuth URLs from Host or forwarded headers.

## Surprises and discoveries

- The completed WP-05/WP-08 plans described OAuth resource-server support, but
  only the authenticated-token path was implemented; the documented public
  discovery contract was never mounted.
- The pinned RMCP 3.0.1 client probes both RFC path-insertion and origin-root
  protected-resource metadata paths and honors a same-origin challenge URL,
  which gives a local primary-source compatibility target for server tests.
- The reference Nginx virtual host previously proxied `/mcp/` but returned 404
  for the origin-root RFC 9728 route. OAuth required an explicit narrow public
  proxy location in addition to application routes.
- The clean starting revision already had seven stale `SHA256SUMS` entries for
  unchanged governing documents. Full checksum validation exposed the drift;
  the manifest was mechanically realigned to the committed document bytes in
  addition to refreshing files changed by this plan.

## Validation

```bash
cargo fmt --all --check
cargo test -p mcp-vault-auth --all-features
cargo test -p mcp-vault-mcp --all-features
cargo test -p mcp-vault-server --all-features
cargo test -p mcp-vault-admin-api --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
bash scripts/conformance/mcp.sh
```

Expected results: focused and workspace checks pass; metadata/challenge tests
prove exact resource/issuer/scopes and no secret fields; invalid audience,
resource, issuer, signature, time, grant, and scope cases remain fail-closed.
Any conformance or live ChatGPT test requiring external network, TLS, IdP, or
account state is recorded as blocked rather than counted as passing.

Validation completed on 2026-08-28:

- `cargo fmt --all --check` passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  passed; a final focused Auth/Server Clippy run also passed after fixture
  hardening.
- `cargo test --workspace --all-features` passed. Final focused Auth/MCP tests
  passed with 23 and 18 tests respectively.
- Frontend lint passed; Vitest passed 26 tests; TypeScript/Vite build passed.
- `bash scripts/check-docs.sh`, `git diff --check`, and all entries in
  `SHA256SUMS` passed.
- `bash scripts/interop/http-smoke.sh` passed the public OAuth metadata, MCP,
  Origin, WebDAV, and plane-separation path. One later repetition produced a
  transient existing WebDAV concurrency failure (`concurrent WebDAV PUT 4
  returned 500`); the immediate identical retry passed all 50 PUTs. The OAuth
  metadata assertion runs before that unrelated section and passed on every
  run.
- Official MCP conformance at pinned commit
  `74edef34d674f563537be8c6587cebaa58e830ca` passed all configured scenarios
  with only the repository's reviewed expected-failure baseline (four
  diagnostic-only `server-stateless` probes and unsupported `prompts/list`
  caching).
- A real ChatGPT + external IdP login was not claimed: this checkout has no
  operator DNS/TLS deployment, IdP tenant/client registration, callback
  allow-list, or ChatGPT account state. The documented release checklist owns
  that manual environment-specific gate.

## Rollback and recovery

There is no migration or canonical-data mutation. Reverting the Auth/MCP/Admin
code and documentation restores the previous PAT/resource-server validator.
Existing issuer and grant rows remain compatible. An interrupted deployment has
no OAuth transaction state to reconcile.

## Outcomes

MCP Vault now exposes the missing OAuth discovery contract required by
ChatGPT: Vault-specific RFC 9728 metadata, a single-resource root alias, and
absolute same-origin `resource_metadata` challenges when the public data
origin is configured. Metadata selection is exact and fail-closed, contains
only redaction-safe public fields, and requires an enabled issuer with a public
JWKS cache.

The existing RS256 resource server now accepts the standard RFC 8707 shape
where the MCP resource is carried by `aud`, while preserving separate issuer,
configured audience, time, subject-grant, Vault, and scope validation. PAT
behavior is unchanged.

Admin exposes/copies the metadata URL and pre-fills resource/audience with the
canonical MCP endpoint. Chinese guidance explains PKCE `S256`, CIMD/DCR or
pre-registration, exact callback allow-listing, public-JWKS-only handling, and
resource propagation. Reference Nginx and existing-proxy instructions now
publish only the required well-known path in addition to MCP/WebDAV/health.

Automated coverage includes Auth/MCP unit tests, Admin API/UI tests, real HTTP
metadata smoke, full workspace checks, and official MCP conformance. The only
remaining acceptance item is the intentionally environment-owned live
ChatGPT/external-IdP login and tool call.
