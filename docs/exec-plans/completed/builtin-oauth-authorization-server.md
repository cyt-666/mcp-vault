# Built-in OAuth 2.1 authorization server for ChatGPT

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-29
- Last updated: 2026-08-29

## Purpose and user-visible result

MCP Vault 0.1.2 will be usable as a ChatGPT MCP plugin without depending on an
external identity provider. An owner configures one independent OAuth login for
a Vault in the LAN-only Admin console, adds the public MCP URL to ChatGPT, signs
in on MCP Vault's own authorization page, and receives Vault-bound access.

The data listener will provide protected-resource metadata, authorization-
server metadata, Dynamic Client Registration (DCR), authorization code with
PKCE `S256`, opaque access tokens, rotating refresh tokens, and MCP bearer-token
validation. The existing external RS256 issuer mode remains an optional
advanced compatibility mode rather than the default setup path.

## Governing requirements

- `AGENTS.md`: Vault isolation, separate planes, protocol adapters without SQL
  or business logic, stateless MCP, redacted secrets, and complete tests.
- `docs/product-requirements.md` sections 1, 3.7, 3.8, and 6: single-owner
  self-hosting, complete MCP authorization, Chinese Admin configuration, and
  release-quality operation.
- `docs/architecture.md` sections 1, 3.2, 4.5, 5.2, 6, and 16: public
  authorization metadata on the data plane, Auth/State ownership, process-
  local application services, and Vault-bound principals.
- `docs/interfaces.md` section 9: MCP bearer discovery, exact resource
  indicators, scopes, and Vault binding.
- `docs/security.md` sections 2, 4, 5, 7, 8, 11, and 15: credential-plane
  separation, TLS, password hashing, token digests, rate limits, redaction, and
  audit requirements.
- ADR-0008: every MCP credential and grant binds to one Vault.
- ADR-0018: MCP Vault owns a complete built-in OAuth authorization server while
  retaining external resource-server validation as an optional mode.
- RFC 8414, RFC 7591, RFC 7636, RFC 8707, RFC 9207, RFC 9728, and current MCP
  authorization/OpenAI plugin authentication requirements.

## Current repository state

The uncommitted 0.1.1 work already adds RFC 9728 protected-resource metadata,
same-origin bearer challenges, external RS256 JWT validation compatible with an
RFC 8707 `aud`, Admin connection information, and reverse-proxy routes. It does
not expose an authorization endpoint, token endpoint, DCR, login/consent UI, or
locally issued token. Therefore a ChatGPT connection still requires an external
OAuth/OIDC service and does not satisfy the intended self-hosted workflow.

`mcp-vault-auth` already owns Argon2id password handling, installation-keyed
bearer digests, scope parsing, rate limiting, and principal construction.
`mcp-vault-state` owns auth SQL and migrations. `mcp-vault-mcp` owns public data-
plane adaptation, and `mcp-vault-admin-api` owns the LAN-only management API.
Those boundaries will be extended rather than bypassed.

## Scope and non-scope

### Included

- One Admin-managed, Vault-bound local OAuth user per Vault, with a distinct
  username/password and explicit maximum scopes.
- RFC 8414 authorization-server metadata at the public origin.
- RFC 7591 DCR for public clients using `token_endpoint_auth_method=none`.
- Authorization code flow with exact registered redirects, RFC 8707
  `resource`, PKCE `S256`, short-lived request handles, single-use codes, and RFC 9207
  `iss` in authorization responses.
- Opaque access tokens and rotating refresh tokens stored only as versioned
  installation-keyed digests, with expiry, revocation, and Vault/resource/
  client/user/scope binding.
- MCP authentication dispatch for PAT, locally issued OAuth tokens, and the
  existing optional external JWT format.
- Chinese Admin controls, public no-store login/consent HTML, deployment/docs,
  migration, focused tests, real-HTTP smoke coverage, and a rebuilt 0.1.2 image.

### Not included

- Social login, federation, MFA, multiple interactive people per Vault, device
  authorization, client credentials, implicit flow, password grant, or token
  introspection.
- Client secrets: ChatGPT is treated as a public client and PKCE is mandatory.
- Reusing Admin credentials on the public data listener.
- Turning the local authorization state into MCP protocol-session state.
- Removing the existing external issuer/grant mode.

## Invariants and risks

- Every request, code, access token, refresh token, and local user is resolved
  to one `VaultContext`; the URL Vault, OAuth `resource`, and persisted Vault ID
  must agree.
- Redirect URIs are exact registered strings. The server never accepts prefix,
  wildcard, request-Host-derived, or fragment-bearing redirects.
- PKCE accepts only `S256`; codes are short-lived, single use, and consumed in
  the same transaction that issues tokens.
- Passwords, authorization codes, access tokens, refresh tokens, and request
  handles are never logged or stored in plaintext.
- The public endpoint cannot authenticate an Admin session or Admin password.
- Requested scopes may only narrow the local user's configured maximum scopes.
- DCR is bounded and public-client only. Unsupported authentication/grant modes
  fail closed without issuing credentials.
- Public authorization pages use no external assets, set `Cache-Control:
  no-store`, frame denial, a restrictive CSP, and do not place credentials or
  codes in logs.
- Rotating or disabling the local OAuth user revokes all of its active codes
  and tokens so old credentials cannot survive a password/security change.

## Proposed design

### Component and request flow

```text
ChatGPT -> RFC 9728 resource metadata -> built-in issuer URL
        -> RFC 8414 metadata -> DCR public client
        -> GET /oauth/v2/authorize (resource + PKCE + exact redirect)
        -> POST /oauth/v2/authorize (Vault OAuth user login + consent)
        -> redirect code + state + iss
        -> POST /oauth/token (code + verifier + resource)
        -> opaque access/refresh tokens
        -> MCP bearer middleware -> AuthService -> Vault principal
```

Axum handlers parse HTTP/query/form DTOs and translate OAuth errors.
`AuthService` validates clients, resources, passwords, scopes, PKCE and token
lifecycle. `StateStore` repository methods own SQL and atomic consume/rotate
transactions. MCP handlers receive only an authenticated principal.

### Persistence and transaction boundaries

Migration `0012_builtin_oauth_authorization_server.sql` adds:

- `oauth_local_users`: Vault, normalized username, Argon2id hash, allowed
  scopes, enabled/revision timestamps;
- `oauth_clients`: public client metadata and exact redirect URI JSON;
- `oauth_authorization_requests`: keyed request-handle digest plus validated
  client/resource/redirect/scope/PKCE/state and expiry;
- `oauth_authorization_codes`: keyed code digest and the frozen authorization
  grant, expiry and consumption state;
- `oauth_access_tokens`: keyed digest, safe prefix, frozen grant, expiry,
  revocation and last-use state;
- `oauth_refresh_tokens`: keyed digest, rotation family, frozen grant, expiry,
  revocation and rotation state.

Successful authorization atomically consumes the request and creates one code.
Code exchange atomically consumes the code and creates access/refresh rows.
Refresh atomically revokes the presented token and creates its replacement plus
a new access token. User replacement/disable atomically revokes all associated
authorization state.

### Public interfaces

- `GET /.well-known/oauth-authorization-server`
- `POST /oauth/register`
- `GET /oauth/v2/authorize`
- `POST /oauth/v2/authorize`
- compatibility aliases: `/oauth/v1/authorize`, `/oauth/authorize`
- `POST /oauth/token`
- existing RFC 9728 paths advertise the built-in issuer first when configured
- `GET/PUT/DELETE /api/v1/mcp/oauth/local` on the control listener

Authorization-server metadata advertises only `code`,
`authorization_code`/`refresh_token`, PKCE `S256`, public-client token auth
`none`, DCR, and the scopes actually supported by MCP Vault.

### Failure, recovery, and cleanup

Invalid client/redirect/resource requests are not redirected. Safe
authorization errors use an already validated exact redirect URI and preserve
only `state` plus RFC 9207 `iss`. Token/DCR errors are JSON and no-store.
Expired or consumed state remains unusable after restart. Bounded opportunistic
cleanup deletes expired requests/codes and expired/revoked tokens after a
retention window; an interruption requires no replay because issuance and
consumption are transactional.

## Work breakdown

1. Add ADR-0018, migration 0012, domain IDs/records and State repository
   methods including atomic consume/rotation and cross-Vault tests.
2. Add AuthService local-user management, DCR validation, authorization
   request/login/consent, PKCE/code exchange, refresh rotation and opaque-token
   authentication with secret-safe tests.
3. Add data-plane metadata/DCR/authorization/token adapters and integrate
   built-in tokens into MCP middleware without changing RMCP business logic.
4. Add Admin API/UI local OAuth setup/rotation/disable controls and retain
   external issuer controls under an advanced disclosure.
5. Update interfaces, security, schema, deployment, compatibility, release,
   smoke fixtures and checksum documentation.
6. Run focused and workspace Rust checks, frontend checks, docs/checksum checks,
   public HTTP OAuth E2E, official MCP conformance, and rebuild/inspect the
   linux/amd64 `mcp-vault:0.1.2` image.

## Progress

- [x] 2026-08-29 — Reproduced the architectural gap: the current 0.1.1 changes
  expose only a resource server and require an external authorization service.
- [x] 2026-08-29 — Rechecked governing requirements, current auth/state/MCP
  call paths, and official ChatGPT OAuth requirements.
- [x] 2026-08-29 — Chose a distinct Vault OAuth user and complete public-client
  authorization-code flow; Admin credentials remain control-plane only.
- [x] 2026-08-29 — Added migration 0012, typed records, atomic code/token
  consumption and refresh rotation/replay-family revocation, DCR/login limits,
  PKCE/resource/scope validation, and focused cross-Vault tests.
- [x] 2026-08-29 — Added RFC 8414/DCR/authorize/token routes, built-in bearer
  dispatch, exact Host/Origin/public-origin checks, public security headers,
  and preserved optional external JWT compatibility.
- [x] 2026-08-29 — Added Chinese Admin setup/rotation/disable controls, exact
  connection metadata, Nginx routes, ADR/specification/operator updates, and
  migration/checksum evidence.
- [x] 2026-08-29 — Passed focused/full Rust, Clippy, frontend, documentation,
  checksum, real-HTTP OAuth smoke, and official MCP conformance checks; built
  and inspected the 0.1.2 linux/amd64 image.

## Decisions

- The built-in authorization server is the default first-party path. External
  issuers remain optional for operators that already run an IdP.
- Use DCR because ChatGPT supports it and a self-hosted instance cannot know a
  user's callback URL in advance. Registered redirect strings remain exact.
- Use an independent Vault OAuth password rather than Admin credentials because
  control-plane credentials must never cross into a public data-plane form.
- Issue opaque tokens so immediate revocation, rotation, Vault binding, and key
  versioning remain locally enforceable without publishing signing keys.
- Keep one local OAuth user per Vault for the single-owner first release; the
  schema uses stable IDs and Vault predicates so more users can be added later.

## Surprises and discoveries

- The previous completed plan treated an external IdP as an acceptable product
  boundary. That was standards-compatible but did not satisfy the intended
  standalone deployment or the user's requested ChatGPT plugin experience.
- Existing Auth primitives already cover the two hard secret classes: bounded
  Argon2id verification for human passwords and versioned installation-keyed
  digests for high-entropy bearer values.
- Browser form POSTs include the public origin even when the general data
  Origin allow-list is empty. The OAuth guard therefore admits the exact
  configured public origin while retaining the configured optional allow-list
  and exact Host checks.
- The frontend dependency wrapper needed one lockfile-pinned reinstall because
  the sandbox could not access a cached tarball; the approved install reused
  all 288 locked packages and changed no dependency versions.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
bash scripts/interop/http-smoke.sh
bash scripts/conformance/mcp.sh
docker build --platform linux/amd64 -t mcp-vault:0.1.2 -t mcp-vault:latest .
```

Focused OAuth tests must prove DCR, exact redirect matching, `resource`
propagation, PKCE mismatch, request expiry, authenticated duplicate-form retry,
single-use codes, login failure limiting, code
exchange, refresh rotation/replay rejection, scope narrowing, local-user
rotation revocation, MCP access, external-JWT compatibility, and cross-Vault
rejection. Real-HTTP smoke must exercise the complete built-in flow rather than
only in-process handlers.

Validation completed on 2026-08-29:

- `cargo fmt --all --check`, workspace Clippy with `-D warnings`, and
  `cargo test --workspace --all-features` passed.
- Frontend lint passed; Vitest passed 26 tests; TypeScript/Vite build passed.
- Documentation checks, `git diff --check`, and every `SHA256SUMS` entry passed.
- Real-HTTP smoke passed DCR, built-in login, code + PKCE, OAuth-token MCP use,
  refresh rotation, replay-family revocation, public metadata, Origin rejection,
  WebDAV concurrency/preconditions, and Admin-plane separation.
- Official MCP conformance at pinned commit
  `74edef34d674f563537be8c6587cebaa58e830ca` passed with only the existing
  reviewed baseline (unsupported `prompts/list` caching and diagnostic probes).
- Docker built `mcp-vault:0.1.2` and `mcp-vault:latest` as the same non-root
  `linux/amd64` image
  `sha256:bdcf5e3570c028777394b512141c46d68927465f64583b74006741df5f464b23`;
  the container's supported `--check-config` command passed. The CLI does not
  implement `--version`; build logs and MCP conformance reported
  `serverInfo.version = 0.1.2`.
- A live user-account ChatGPT UI login was not claimed because this checkout
  has no deployed public DNS/TLS endpoint or ChatGPT account session. That is
  the remaining environment-specific manual compatibility record, not an
  external IdP dependency.

## Rollback and recovery

The migration is additive. Rolling application code back leaves the new tables
unused; no canonical Vault content changes. Disabling/deleting a local OAuth
user revokes its authorization requests, codes, access tokens, and refresh
tokens. External issuer and PAT rows are unchanged. An interrupted OAuth
exchange cannot partially consume a code or refresh token because each
consume-and-issue step is a single SQLite transaction.

## Outcomes

MCP Vault 0.1.2 now owns a complete standalone OAuth path for ChatGPT. A Vault
owner creates one independent OAuth login in Admin, supplies only the MCP URL
to ChatGPT, and uses MCP Vault's discovery, DCR, login/consent, PKCE code
exchange, opaque access token, and rotating refresh token endpoints. Every
grant is exact-resource and Vault scoped; user rotation/disable and refresh
replay revoke outstanding authorization state. PATs and optional external
RS256 issuers remain compatible. The release image is built locally and the
automated public wire path is verified; only a named live ChatGPT UI/account
record remains outside this repository-controlled validation.

## Corrective follow-up: browser authorization retry

On 2026-08-29 a live ChatGPT/browser connection repeatedly replaced a
successful authorization attempt with the generic “authorization request
expired” page. The original automated flow submitted the login form only once
and therefore missed a real browser/edge behavior: a second POST for the same
still-valid handle was rejected after the first POST recorded `consumed_at`.

The corrected contract permits a correctly authenticated retry while the
validated request handle remains unexpired. State records the first completion
time and creates a distinct, short-lived, single-use authorization code for
each successful retry. Password rotation or disable deletes every outstanding
request before revoking codes/tokens, so retry tolerance cannot cross a Vault
OAuth credential change. OAuth responses include browser, CDN, and surrogate
no-store controls. Fresh metadata advertises the current versioned path; former
paths remain aliases for compatibility.

Regression evidence now submits the exact same form twice at both the Auth and
public MCP HTTP boundaries, asserts two different codes and exchanges the
second successfully. The real-HTTP smoke follows metadata to the advertised
endpoint, repeats the form with `Origin: null`, then completes token exchange
and the full MCP tool path.

The corrective artifact is versioned `0.1.7` after locally built images reached
`0.1.6`; it does not overwrite an existing tag. Metadata and the form use the
new `/oauth/v2/authorize` path; the reference proxies already admit the whole
`/oauth/` prefix.

## Corrective follow-up: controlled browser evidence boundary

On 2026-08-30 a fresh ChatGPT authorization request was exercised in Chrome.
The generated form was natively valid and resolved to
`POST /oauth/v2/authorize`. A separate copy of the same request filled with
synthetic credentials submitted successfully through the public edge and
returned the expected invalid-credential page at that POST endpoint. This
proves the browser form, public route, and invalid-login response path; it does
not prove a successful live credential, callback, or token exchange.

The Chrome control surface protects user-entered password fields. A tab holding
the user's real credential did not dispatch a form request through either
manual or controlled activation while it remained under that surface, so that
observation is not valid evidence of a server-side rejection. Password-manager
behavior observed in that controlled tab is likewise not a valid root-cause
signal. Final live acceptance must use an ordinary, unclaimed browser tab after
deployment and must verify the callback, token POST, and authenticated MCP
request separately.

## Corrective follow-up: real-process local reproduction

The production binary was then launched through `make run` against a disposable
data directory and configured through the real Admin API. The first DCR request
to `127.0.0.1:18080` returned 403 before OAuth code because the generated local
URL included the listener port while the default Host allow-list contained only
bare loopback names. `default_data_hosts` now includes bare and port-qualified
IPv4, IPv6, and `localhost` authorities. With no explicit
`MCP_VAULT_DATA_HOSTS`, the same DCR request returns 201.

Against that real process, a complete HTTP flow returned authorization 302,
token 200, `tools/list` 200 with all 17 tools, and `create_note` 200 with a
successful structured result. This separates the browser symptom from the
OAuth server and write-tool path.

The supplied origin access log records ChatGPT discovery and DCR at the origin
but no later authorization-page GET or POST, while Chrome repeatedly opens the
obsolete `/oauth/authorize` endpoint. Serving a transaction page directly from
that legacy GET leaves it vulnerable to a stale client/CDN cache entry. Legacy
GETs now preserve the complete query in a 307 redirect to
`/oauth/v2/authorize`, legacy POSTs remain compatible, and every OAuth response
adds `Vary: *`. The form uses standard password-manager autocomplete semantics;
vendor-specific ignore attributes are not part of the correction.

The intermediate image with digest beginning `878a70d3` was superseded before
deployment because it contained the rejected password-manager workaround. It
must not be used as the final 0.1.7 artifact.

The final standards-compatible `mcp-vault:0.1.7` image is `linux/amd64` with
digest
`sha256:41f67aad034ac9f3390725cdabf6730c6a6f7bbb3acafb1373b7489a87ada964`.
It contains the loopback Host-default fix, standard password-manager form
semantics, legacy authorization GET redirects, `Vary: *`, and the previously
validated OAuth retry behavior.

## Corrective follow-up: token Origin interoperability

On 2026-08-31 the no-error browser refresh was reproduced past the login form.
Against the same valid, unconsumed authorization code, 0.1.7 returned 403
`request rejected` when `/oauth/token` carried either
`Origin: https://chatgpt.com` or `Origin: null`, while the identical request
without `Origin` returned 200. The middleware exempted browser authorization
POSTs but still applied the MCP data-plane Origin allow-list to the public OAuth
token endpoint. A rejected token exchange can cause the OpenAI host to restart
authorization, presenting the same login page without a Vault login error.

The token endpoint now remains Host-validated but Origin-independent. It has no
cookie/session authority; code exchange still requires the exact client,
redirect URI, resource, single-use code and PKCE verifier, while refresh still
requires the exact client/resource and rotating token. Focused HTTP coverage
sends the authorization-code exchange with the ChatGPT Origin and refresh with
`Origin: null`. That token correction was first built as `0.1.10`, but the
image remained affected by the Chromium native-form incompatibility described
below. Neither the skipped 0.1.8 tag, the pre-CSP 0.1.9 image, nor that 0.1.10
image is a release candidate for the final fix.

Historical-state testing used the retained 0.1.0 through 0.1.7 runtime images,
not only current source fixtures. A single database upgraded sequentially from
0.1.0 to 0.1.7 completed authorization and token exchange at every built-in
OAuth release. Direct upgrades from 0.1.2 through 0.1.6 preserved pending
requests, clients, local users, unconsumed codes, access tokens, refresh tokens,
and authenticated consumed-request retry. The shared migration-12 checksum and
OAuth schema were identical across those images; ordinary historical rows were
not the reproduced failure.

The superseded pre-CSP `mcp-vault:0.1.9` image is `linux/amd64` with digest
`sha256:f6b0d9eac2d22f28b35bdceface107237d4bdb8108524fe8b1c9dd6a87f0218d`.
Three independent authorization codes exchanged through that image with
`Origin: https://chatgpt.com`, `Origin: null`, and no Origin respectively; all
three returned 200. The image runs as `mcpvault`, its entrypoint remains
`/usr/local/bin/mcp-vault`, and `--check-config` passed.

## Corrective follow-up: Chromium native-form compatibility

On 2026-08-31 the first live browser console showed a native POST rejected by
`form-action 'self'`. Adding the canonical public Origin did not resolve it: a
second live screenshot showed Chrome rejecting the exact target while quoting
`form-action 'self' https://mcp-vault.cyt.cool`. That falsified the assumption
that an opaque document origin plus a missing explicit source was the complete
cause.

A controlled test in the same Chrome submitted an absolute-action form under
the exact-source policy and the local server received all form fields. This is
consistent with Chromium issue 393431137, where the console message can be
misleading, so console output alone is not proof of origin arrival. The known
working MoviePilotMCP implementation and its live authorization response were
then inspected: its native login form uses an absolute action and does not send
a `form-action` CSP directive.

MCP Vault now follows that browser-compatibility boundary. Interactive
authorization HTML keeps the absolute action derived from the validated
canonical public Origin but omits only `form-action`; the remaining policy is
`default-src 'none'; style-src 'unsafe-inline'; base-uri 'none';
frame-ancestors 'none'`. Error-only pages retain `form-action 'none'`. The POST
still requires the opaque short-lived handle bound to the exact client,
redirect, state, resource, scopes, and PKCE challenge, in addition to Host
validation and credential verification. Unit and real-HTTP coverage assert
both CSP variants. The previous
`sha256:7bd3bdfdcd381224cc23ca4d2bb26b39d474dc24f7c8c200f6a1c45bec5cbad5`
0.1.10 image predates this compatibility correction and is not a release
candidate. The corrected source is versioned `0.1.11`. The local
`mcp-vault:0.1.11` image is `linux/amd64` with digest
`sha256:936309e66872e931a337bc6a316a7b5207c309c7d283236a30b0089b63699728`
and size 102,242,757 bytes. Docker reports runtime user `mcpvault`, UID 999,
and stop signal `SIGTERM`; `--check-config` passes, and the runtime binary
contains the interactive authorization policy without a `form-action`
directive.
