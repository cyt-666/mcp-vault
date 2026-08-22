# Protocol and client compatibility matrix

This matrix records evidence, not assumptions. An in-process adapter test is
not evidence that a desktop/mobile client can synchronize. A row is “verified”
only when the named client/version and the public HTTP deployment are both
identified in the release artifact.

## Current matrix

| Client or suite | Transport/version | Automated evidence | Manual evidence | Current status |
|---|---|---|---|---|
| Official MCP conformance | Streamable HTTP, MCP `2026-07-28` | `scripts/conformance/mcp.sh`; fixed conformance commit; stateless, list, headers, DNS, caching scenarios | Full requirements suite review for every release | Core gate passes with the narrow baseline in `tests/conformance/expected-failures.yml` |
| MCP Vault project client path | MCP `2026-07-28` | `scripts/interop/http-smoke.sh`; MCP crate tests; Vault-bound PAT and Origin checks | None required for the wire smoke | Verified automatically |
| Hēsperus Sync Engine | WebDAV over HTTP(S), RFC 4918 semantics | `scripts/interop/http-smoke.sh`; `scripts/interop/webdav-litmus.sh`; DAV method/precondition tests | Run the exact plugin version against a release deployment | Protocol shape covered; plugin version remains release-manual |
| Remotely Save | WebDAV over HTTP(S) | Same DAV fixture and Litmus entry point | Run the exact plugin version against a release deployment | Protocol shape covered; plugin version remains release-manual |
| Obsidian Desktop | Vault folder via the selected WebDAV plugin | DAV request fixtures and server tests | Sync, conflict, rename, attachment, reconnect, and large-file checklist | Not claimed until a named desktop/plugin version is recorded |
| Obsidian Mobile | Vault folder via the selected WebDAV plugin | DAV request fixtures and server tests | Background sync, offline edit, conflict, attachment, and reconnect checklist | Not claimed until a named mobile/plugin version is recorded |

## WebDAV request-shape fixture

The real HTTP smoke covers the minimum release path:

- authenticated `GET` and `PUT` of a Markdown note;
- 50 concurrent nested `PUT` requests followed by authenticated read-back of
  every object, matching an initial sync client's burst behavior;
- `If-Match` stale-write rejection;
- public listener/Admin route separation;
- readiness and MCP discovery on the same deployment.

The in-process WebDAV tests additionally cover PROPFIND, MKCOL, COPY/MOVE,
DELETE, LOCK/UNLOCK, ranges, path attacks, expiration/revocation, and
two-Vault isolation. The concurrent regression also verifies that successful
PUTs leave no incomplete durable operation journals. Litmus remains a separate
gate because its client drives the deployed HTTP adapter with the upstream
method/condition suite.

## Manual release record

For each release, attach a sanitized record containing:

```text
service image digest:
WebDAV endpoint (without credentials):
Obsidian Desktop version:
Obsidian Mobile version:
Sync Engine version/configuration:
Remotely Save version/configuration:
platforms:
date/time window:
result and evidence artifact:
```

Never attach passwords, PATs, cookies, Authorization headers, note bodies, or
memory contents. If a client is unavailable, record `unverified` rather than
inferring compatibility from the protocol fixture.
