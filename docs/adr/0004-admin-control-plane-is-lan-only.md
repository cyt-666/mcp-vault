# ADR-0004: Admin control plane is LAN-only

- Status: Accepted
- Date: 2026-08-19
- Clarified: 2026-08-22

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

Admin still requires password authentication, secure session cookies, CSRF protection, Origin validation, and rate limiting because LAN devices are not inherently trusted.

## Consequences

Positive:

- smaller public attack surface;
- simpler personal-service security model;
- network rules can be independently audited.

Costs:

- remote administration requires VPN or tunnel;
- reference Docker/proxy configuration must avoid accidental exposure;
- source-IP allow lists and trusted-proxy behavior remain deployment concerns.

## Rejected alternatives

- Public `/admin` route on the same listener as MCP/WebDAV.
- No Admin authentication because “LAN is trusted.”
- Build a SaaS-style public account/recovery system for the first release.
