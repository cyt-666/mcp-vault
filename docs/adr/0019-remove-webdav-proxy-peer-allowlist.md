# ADR-0019: Remove the WebDAV proxy-peer allow-list

- Status: Accepted
- Date: 2026-08-31

## Context

MCP Vault currently accepts WebDAV Basic Auth on a non-loopback connection only
when the direct socket peer is listed in `MCP_VAULT_TRUSTED_PROXY_IPS` and the
request carries `X-Forwarded-Proto: https`. This protects against a client that
can reach the plaintext data listener and forges the forwarded scheme.

In appliance deployments, a host proxy, container bridge, or managed network
can obscure or change the socket-peer address. This makes an application-level
exact-IP allow-list operationally fragile. The operator explicitly requested
complete removal of both the restriction and its environment variable rather
than a wildcard compatibility mode.

## Decision

Remove `MCP_VAULT_TRUSTED_PROXY_IPS` from application configuration and remove
the socket-peer comparison from WebDAV. A non-loopback request is treated as
transport-secure when it carries `X-Forwarded-Proto: https`; loopback HTTP
remains accepted.

The reverse proxy must preserve the Basic `Authorization` header and set the
forwarded scheme in the effective `/dav/` location. Deployment networking must
make the plaintext data listener reachable only from the intended proxy.
Vault-bound username/password, permission, expiry, and revocation checks remain
unchanged.

## Consequences

- Reverse proxies no longer need a stable source IP and the environment
  variable is removed from supported configuration.
- A client with direct access to the plaintext data listener can forge
  `X-Forwarded-Proto: https`. Firewall, container-network, and port-publication
  isolation are now mandatory rather than defense in depth.
- MCP, OAuth, Admin, credential storage, and Vault isolation are unchanged.
- Existing deployments may leave the obsolete environment variable present;
  it has no effect and should be removed from deployment configuration.

## Alternatives considered

### Keep exact IPs and diagnose the actual peer

This retains defense in depth but was rejected by the operator because the
restriction itself is not desired.

### Support a `*` wildcard

This preserves the secure default and gives individual deployments an opt-out,
but was explicitly rejected in favor of deleting the environment variable.

### Treat `0.0.0.0` as a wildcard

Rejected because it overloads a real unspecified address and would not cover
IPv6.
