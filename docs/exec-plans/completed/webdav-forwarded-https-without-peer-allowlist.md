# WebDAV Forwarded HTTPS Without a Peer Allow-list

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-31
- Updated: 2026-08-31

## Purpose and user-visible result

Remove `MCP_VAULT_TRUSTED_PROXY_IPS` from MCP Vault. A non-loopback WebDAV
request is transport-eligible when the direct request carries
`X-Forwarded-Proto: https`, without comparing the socket peer to an application
allow-list. Loopback HTTP remains supported and all WebDAV credential, Vault,
permission, expiry, and revocation checks remain unchanged.

## Governing requirements

- `AGENTS.md`: authentication and network-exposure changes require deliberate
  documentation, tests, and redaction-safe operation.
- `docs/security.md` section 6: WebDAV uses dedicated Vault-bound Argon2id app
  credentials and accepts Basic Auth only through a transport treated as
  secure.
- `docs/admin-and-configuration.md`: removed environment variables must no
  longer appear in supported configuration.
- Completed plan `docs/exec-plans/completed/wp-07-webdav.md` records the prior
  exact-peer decision; ADR-0019 supersedes that transport-policy decision.

## Current repository state

`AppConfig` parses `MCP_VAULT_TRUSTED_PROXY_IPS` into a `BTreeSet<IpAddr>` and
passes it through `server::run` to `WebDavService`. `WebDavService::handle`
requires both an exact peer match and `X-Forwarded-Proto: https` before it
verifies an Argon2id app password.

The deployed reverse proxy receives the WebDAV Basic username and returns MCP
Vault's fixed 24-byte rejection body, while invalid Basic and missing Basic
requests have effectively the same warmed response time. This identifies the
transport gate before Argon2 as the likely rejection boundary. The operator
explicitly rejected a wildcard opt-out and requested complete removal of the
environment variable and peer restriction.

## Scope and non-scope

### Included

- Delete `trusted_proxy_ips` from `AppConfig`, configuration parsing, server
  composition, deployment examples, and public documentation.
- Delete the trusted-peer set from `WebDavService`.
- Accept `X-Forwarded-Proto: https` from any direct peer for the WebDAV Basic
  transport decision.
- Keep the loopback path and HTTPS forwarded-scheme requirement.
- Replace exact-peer tests with tests for forwarded HTTPS and missing/incorrect
  forwarded scheme.
- Update security/operations documentation, ADR index, and checksums.

### Not included

- No plaintext non-loopback Basic Auth without the HTTPS forwarded scheme.
- No trust of forwarded client IP, Host, Origin, or authorization identity.
- No OAuth, MCP bearer, Admin session, database, or credential-schema change.
- No change to Nginx header forwarding; the effective `/dav/` location must
  still send `X-Forwarded-Proto: https` and preserve `Authorization`.

## Invariants and risks

- Credentials remain Argon2id-hashed, Vault-bound, permission-scoped,
  expirable, and revocable.
- A client that can directly reach the plaintext data listener can forge
  `X-Forwarded-Proto: https` and submit Basic credentials over plaintext.
  Deployment firewall and port-publication policy therefore become the only
  boundary ensuring that port 8080 is reachable solely by the intended proxy.
- Authentication headers, passwords, and note data remain absent from logs.
- Existing configurations containing `MCP_VAULT_TRUSTED_PROXY_IPS` become
  ignored only if the variable remains in an external deployment environment;
  repository examples and documentation remove it. Startup does not reject the
  obsolete value because MCP Vault does not reject arbitrary unknown
  environment variables.

## Proposed design

Delete the typed configuration field and parser. Remove the trusted peer set
from `WebDavService`. The transport predicate becomes:

```text
socket peer is loopback
OR X-Forwarded-Proto equals https (case-insensitive)
```

This keeps the caller-visible WebDAV interface unchanged. No schema, state, or
protocol DTO changes are required.

## Work breakdown

1. Record ADR-0019 and this execution plan.
2. Remove the server configuration field/parser and update configuration tests.
3. Remove the WebDAV peer allow-list and update protocol tests.
4. Remove the environment variable from Compose examples and documentation;
   document the stronger network-isolation requirement.
5. Run formatting, Clippy, workspace tests, frontend checks, documentation
   checks, checksum verification, and `git diff --check`.

## Progress

- [x] 2026-08-31 — Traced the live 401 to the pre-Argon WebDAV transport gate
  and confirmed the current exact-peer implementation and historical decision.
- [x] 2026-08-31 — Operator explicitly rejected a wildcard opt-out and requested
  complete removal of `MCP_VAULT_TRUSTED_PROXY_IPS`.
- [x] 2026-08-31 — Removed the configuration field/parser, server/service
  plumbing, Compose values, and WebDAV peer comparison.
- [x] 2026-08-31 — Replaced the exact-peer test with forwarded HTTPS,
  missing-scheme, and explicit HTTP-scheme coverage.
- [x] 2026-08-31 — Updated ADR/security/configuration/deployment/release
  documentation and completed all required validation.

## Decisions

- Remove the environment variable rather than retaining a compatibility
  wildcard or interpreting `0.0.0.0` specially.
- Retain the `X-Forwarded-Proto: https` requirement so non-loopback requests
  without an asserted TLS terminator remain unauthorized.
- Move direct-listener protection fully to deployment networking; clearly
  document that port 8080 must not be exposed to untrusted clients.

## Surprises and discoveries

- Nginx inherits a server-level `proxy_set_header` set only when a nested
  location defines none of its own; effective proxy configuration still needs
  location-level verification after the application peer check is removed.

## Validation

```bash
cargo fmt --all --check
cargo test -p mcp-vault-webdav forwarded_https_allows_non_loopback_basic_auth
cargo test -p mcp-vault-server config::tests
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
make frontend-lint frontend-test frontend-build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
git diff --check
```

Observed results:

- `cargo fmt --all --check` passed.
- The focused WebDAV forwarded-HTTPS test passed; six server configuration
  tests passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  passed.
- `cargo test --workspace --all-features` passed all 266 tests, including all
  eight WebDAV tests and the existing OAuth and atomic-write recovery suites.
- `make frontend-lint frontend-test frontend-build` passed; Vitest ran 26
  tests and the production bundle completed.
- `bash scripts/check-docs.sh`, `shasum -a 256 -c SHA256SUMS`, and
  `git diff --check` passed.

## Rollback and recovery

There is no state or schema migration. Binary rollback restores the old peer
allow-list behavior and requires an exact `MCP_VAULT_TRUSTED_PROXY_IPS` value
before non-loopback WebDAV works. Vault data and credentials are unaffected.

## Outcomes

`MCP_VAULT_TRUSTED_PROXY_IPS` no longer exists in typed configuration,
composition, WebDAV policy, Compose examples, or current public documentation.
Any non-loopback peer may supply `X-Forwarded-Proto: https`, after which normal
Vault-bound WebDAV authentication runs. Missing or explicit HTTP schemes still
receive 401. Deployment networking now exclusively protects the plaintext data
listener from forged forwarded-scheme assertions. No image or version change
was made as part of this plan.
