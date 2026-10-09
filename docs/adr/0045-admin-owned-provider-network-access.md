# ADR-0045: Administrator-owned Provider network access

- Status: Accepted
- Date: 2026-10-09
- Scope: Provider endpoint transport and its Vault-scoped call switch
- Supersedes: the Provider DNS/IP and local/remote admission rules in
  [ADR-0010](0010-provider-adapters-and-vector-fallback.md) and their shared-transport
  guarantee in [ADR-0012](0012-compose-provider-presets-behind-the-shared-transport.md).
  Their adapter, credential, response-validation and resource-bound decisions remain.

## Context

Provider endpoints are installation-level operational configuration. Online
creation and updates require an authenticated Admin session, CSRF and Origin
checks. Host-side initialization tools and authenticated backup restoration are
also trusted configuration paths. MCP tools, model output and ordinary Vault
content cannot supply or change a Provider destination.

Application DNS pre-resolution prevents requests in managed environments where
HTTPS proxies resolve external destinations. The owner explicitly approved
delegating Provider target-network access to the administrator and deployment,
and removing the LocalOnly feature instead of introducing a proxy-policy system.

## Decision

ProviderMode contains only `disabled` and `enabled`, with `disabled` the default.
Enabled requests use the administrator-configured HTTP or HTTPS base URL.
ProviderTransport does not resolve DNS in advance, classify target IPs, or pin
target sockets. reqwest uses its normal direct/environment-proxy behavior,
including standard proxy exclusions and fallback semantics. Merely configuring
a proxy does not imply a fail-closed proxy route.

Keep URL scheme/host/port validation; reject URL credentials, query and fragment;
append only adapter-owned relative API paths. Keep TLS certificate verification
for HTTPS, reject every redirect, protect credential/Host headers, and retain
encrypted Provider secrets, resource ceilings, concurrency, timeouts and request
budgets. Explicitly selecting HTTP means no TLS protection, including for
credentials; the administrator is responsible for that choice. No application
target-IP, metadata-address or DNS-rebinding guarantee remains for Providers.

Local FastEmbed follows the same enabled/disabled switch. Vault permissions,
source include/exclude rules, note eligibility, encrypted storage, and non-Provider
URL security are outside this change and remain enforced.

## Compatibility and upgrade

Forward migration 0044 rewrites only `provider.mode` settings:

- `local_only` becomes `disabled`, so upgrade never silently authorizes external
  calls. The administrator may subsequently use the ordinary enable action.
- `remote_allowed` becomes `enabled`.
- Already canonical values and unrelated settings remain unchanged.

Changed setting rows increment their optimistic revision. The migration runs
once. Readers also accept these legacy values with the same mapping for old
serialized configuration and diagnostic artifacts; no LocalOnly runtime branch
or UI option survives. The historical `allow_private_networks` JSON field remains
readable but has no effect and no editable UI control.

Provider credentials remain associated with the selected Provider record, not
cryptographically bound to an origin. An administrator changing its base URL
without replacing its secret retains that secret, as before. No request follows
a redirect with those credentials.

Old sealed M6 fingerprints/configuration are not rewritten or reinterpreted as
authorization to resume. Any new evaluation preparation and paid run require the
applicable existing authorization process; this decision itself authorizes none.

## Validation

Use synthetic tests for one-time migration and Vault isolation, disabled zero-I/O
behavior, endpoint syntax, Admin permissions, redirect/credential containment,
untrusted TLS rejection and proxy CONNECT without local destination resolution.
No real paid Provider is needed for these checks.
