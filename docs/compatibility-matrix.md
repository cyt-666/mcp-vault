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
| Multi-Vault MCP links | Path-scoped MCP `2026-07-28` | Scoped Admin connection/PAT tests; cross-Vault PAT rejection; initialization availability; index/memory isolation | Connect two named client configurations to a release deployment | In-process and State boundaries verified; two live client configurations remain release-manual |
| ChatGPT plugin OAuth | OAuth 2.1 authorization code + PKCE `S256`; RFC 7591/7636/8414/8707/9207/9728 | Auth/MCP tests and `scripts/interop/http-smoke.sh` cover DCR, built-in login, `offline_access`, duplicate browser/edge form submission with a fresh single-use code, code exchange, resource, 180-day sliding refresh rotation, concurrent retry grace and delayed replay revocation, deterministic full-scope `tools/list`, every tool handler under a built-in OAuth principal, a real-HTTP OAuth `create_note`, Vault isolation, challenge metadata, and optional external RS256 JWT | Named ChatGPT surface plus DNS/TLS and one successful login/tool call after reconnecting for an offline grant | Complete built-in wire flow and OAuth tool surface verified automatically; live ChatGPT UI/account path remains manual until recorded |
| Hēsperus Sync Engine | WebDAV over HTTP(S), RFC 4918 semantics | `scripts/interop/http-smoke.sh`; `scripts/interop/webdav-litmus.sh`; DAV method/precondition tests | Run the exact plugin version against a release deployment | Protocol shape covered; plugin version remains release-manual |
| Remotely Save | WebDAV over HTTP(S) | Same DAV fixture and Litmus entry point | Run the exact plugin version against a release deployment | Protocol shape covered; plugin version remains release-manual |
| Multi-Vault WebDAV links | Distinct `/dav/v1/vaults/<slug>/` mounts | Cross-mount credential rejection; same-relative-path storage/history isolation; initialization availability | Configure two named plugin remotes against a release deployment | In-process boundary verified; two live plugin connections remain release-manual |
| Obsidian Desktop | Vault folder via the selected WebDAV plugin | DAV request fixtures and server tests | Sync, conflict, rename, attachment, reconnect, and large-file checklist | Not claimed until a named desktop/plugin version is recorded |
| Obsidian Mobile | Vault folder via the selected WebDAV plugin | DAV request fixtures and server tests | Background sync, offline edit, conflict, attachment, and reconnect checklist | Not claimed until a named mobile/plugin version is recorded |

## WebDAV request-shape fixture

The real HTTP smoke covers the minimum release path:

- authenticated `GET` and `PUT` of a Markdown note;
- 50 concurrent nested `PUT` requests followed by authenticated read-back of
  every object, matching an initial sync client's burst behavior;
- `If-Match` stale-write rejection;
- public listener/Admin route separation;
- readiness and MCP discovery on the same deployment;
- built-in OAuth metadata, DCR, login/consent, `offline_access`, authorization
  code + PKCE, access-token MCP use, 180-day sliding refresh rotation,
  duplicate-refresh grace, and delayed replay-family revocation.

The in-process WebDAV tests additionally cover PROPFIND, MKCOL, COPY/MOVE,
DELETE, LOCK/UNLOCK, ranges, path attacks, expiration/revocation, and
two-Vault isolation. The concurrent regression also verifies that successful
PUTs leave no incomplete durable operation journals. Litmus remains a separate
gate because its client drives the deployed HTTP adapter with the upstream
method/condition suite.

The multi-Vault release smoke must create a second managed Vault through Admin,
wait for `availability: ready`, generate its distinct WebDAV/MCP links, prove
that each credential fails on the other link, and use the same relative note
path with different bytes in both Vaults. Captured endpoints never contain
credentials.

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
