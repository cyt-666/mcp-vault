# ADR-0004: Admin control plane is LAN-only

- Status: Accepted
- Date: 2026-08-19
- Clarified: 2026-08-28

## Context

The Admin Console controls Vault paths, credentials, MCP grants, provider API keys, memory policy, backups, and restore. Public exposure creates an unnecessary high-value attack surface for a personal self-hosted service.

## Decision

Admin UI/API runs on a separate listener and is published only to localhost, a specific LAN address, or a configured VPN address.

The data-plane reverse proxy must not route the Admin listener.

MCP Vault defaults the Admin listener to loopback but does not enforce client
CIDRs in the application. The operator owns publication and source-network
admission through host/container bindings, firewall or VPN policy, or any
chosen reverse proxy. Nginx is one optional deployment example, not an
application dependency.

Before an Admin exists, initialization is a first-claim operation: the browser
submits only the desired username and password, and the state repository
atomically permits one first account. MCP Vault does not require a manually
retrieved bootstrap token. Therefore every client that can reach an
uninitialized Admin listener is trusted to attempt ownership. Keep the default
loopback bind, or an equivalently narrow deployment policy, until setup is
complete.

Admin still requires password authentication, transport-aware session cookies,
CSRF protection, Origin validation, and rate limiting because LAN devices are
not inherently trusted.

### 2026-08-28 amendment: explicit private-LAN HTTP access

HTTPS remains preferred and its Admin session/CSRF cookies always carry the
`Secure` attribute. An operator may explicitly list a loopback or literal
private/link-local IP HTTP Origin in `MCP_VAULT_ADMIN_ORIGINS`. For a mutation
whose exact Origin or same-origin Referer matches that HTTP entry, MCP Vault
omits only the cookie `Secure` attribute so direct LAN browsers can maintain a
session. The session cookie remains HttpOnly; both cookies remain host-only,
SameSite=Strict, session-bound, expiring, and protected by exact Origin/Referer
plus CSRF validation.

Cleartext DNS names and public IP origins are rejected during configuration.
This opt-in does not add a public listener, interpret forwarded client IPs, or
weaken the HTTPS cookie path. It accepts that credentials and session traffic
are visible to a compromised LAN and must therefore be used only on an
operator-controlled network; HTTPS or VPN remains required when that risk is
unacceptable.

## Consequences

Positive:

- smaller public attack surface;
- simpler personal-service security model;
- network rules can be independently audited.

Costs:

- remote administration requires VPN or tunnel;
- reference Docker/proxy configuration must avoid accidental exposure;
- source-IP allow lists and trusted-proxy behavior remain deployment concerns.
- exposing Admin before initialization delegates first-owner selection to the
  admitted network because there is no separate bootstrap secret.
- explicit LAN HTTP administration trades transport confidentiality for
  appliance compatibility and is visually warned in the Admin login page.

## Rejected alternatives

- Public `/admin` route on the same listener as MCP/WebDAV.
- No Admin authentication because “LAN is trusted.”
- A manually generated/copied bootstrap token; it adds deployment friction and
  is redundant when setup is intentionally constrained by Admin-listener
  publication plus an atomic first claim.
- Build a SaaS-style public account/recovery system for the first release.
