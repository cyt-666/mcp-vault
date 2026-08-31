# ADR-0018: Provide a built-in OAuth 2.1 authorization server

- Status: Accepted
- Date: 2026-08-29

## Context

MCP Vault already supports Vault-bound PATs and can validate access tokens from
a configured external OAuth issuer. RFC 9728 discovery makes that resource-
server mode visible to ChatGPT, but an owner must still deploy and configure a
separate identity provider before adding the self-hosted Vault as a plugin.
That dependency conflicts with the product's single-owner, self-contained
deployment goal.

Reusing the Admin password on a public authorization page would collapse the
control and data security planes. Shipping only discovery or a token minting
shortcut would be an incomplete and unsafe OAuth implementation.

## Decision

MCP Vault provides a complete built-in OAuth authorization server on the data
listener and makes it the default ChatGPT integration path.

The first release supports public clients registered through bounded Dynamic
Client Registration, authorization code with mandatory PKCE `S256`, exact
redirect URIs, RFC 8707 resource indicators, RFC 9207 issuer identification,
opaque short-lived access tokens, and rotating refresh tokens. All grants and
tokens bind to one Vault and one exact MCP resource. Human login uses a
separate Vault OAuth credential configured on the LAN-only Admin listener;
Admin credentials are never accepted by the public authorization endpoint.

Passwords use Argon2id. Request handles, codes, access tokens, and refresh
tokens are stored only as installation-keyed, versioned digests. OAuth
consumption and rotation use State-owned SQLite transactions. Public protocol
handlers contain no SQL or authorization policy.

Authorization request handles are short-lived but tolerate a correctly
authenticated duplicate browser/proxy form submission while they remain valid.
Each successful submission creates a different short-lived, strictly
single-use authorization code. Rotating or disabling the Vault OAuth credential
deletes all outstanding request handles before revoking issued state. OAuth
HTML and redirects carry browser and intermediary no-store controls. Fresh
metadata advertises a versioned authorization path while retaining the former
path as an alias, preventing a stale edge/browser response from standing in for
a new transaction.

The existing external RS256 issuer/resource-server implementation remains an
optional advanced compatibility mode. It is no longer required for a normal
ChatGPT connection.

## Consequences

- A default MCP Vault installation can complete ChatGPT OAuth without an
  external service after the owner configures TLS/public origin and one local
  OAuth login.
- The service now owns public login/consent, client registration, token
  lifecycle, abuse limits, security headers, migrations, and compatibility
  tests; these become release-critical security surfaces.
- Credential planes remain separate and Vault isolation stays explicit.
- Public clients have no client secret. PKCE, exact redirects, short expiry,
  single-use codes, authenticated request retries, refresh rotation, and local
  revocation provide the applicable protections.
- Operators with an existing IdP may continue using external JWT validation,
  but that setup is optional and disclosed as advanced configuration.

## Alternatives considered

### Require an external identity provider

Standards-compatible and operationally mature, but rejected as the only path
because it makes a nominally self-hosted plugin depend on another deployed
service and was not the requested product behavior.

### Reuse Admin login credentials

Rejected because it exposes a control-plane credential to the public data
plane and couples Admin password/session policy to third-party OAuth clients.

### Issue long-lived bearer tokens from a login form

Rejected because it omits client binding, redirect validation, PKCE, resource
indicators, expiry, refresh rotation, and standard discovery.
