# LAN HTTP Admin sessions and MCP Vault branding

- Owner/Agent: Codex
- Created: 2026-08-28
- Last updated: 2026-08-28
- Status: Completed

## Purpose and user-visible result

MCP Vault Admin will remain HTTPS-first while allowing an operator to opt into
an exact loopback or private-LAN HTTP Origin such as
`http://192.168.1.20:8081`. Login and logout cookies will retain `Secure` on
HTTPS and omit only that attribute for a validated local HTTP Origin. Session
authentication, HttpOnly, SameSite=Strict, CSRF, exact Origin/Referer checks,
expiry, revocation, and LAN publication boundaries remain unchanged.

The Admin login shell, authenticated sidebar, and browser tab will use a real
MCP Vault logo instead of the `MV` text placeholder. The project will contain
the generated transparent raster asset consumed by Vite and embedded in the
server image.

## Governing requirements

- `docs/product-requirements.md` sections 3.7 and 3.8: Chinese Admin console,
  durable authenticated reloads, separate control plane, CSRF, and network
  restriction.
- `docs/architecture.md` section 5.3: Admin remains a protocol adapter around
  the shared session/CSRF boundary.
- `docs/security.md` sections 4.1 and 5: LAN/VPN publication, strict Origin,
  session, CSRF, expiry, and rate limiting.
- `docs/admin-and-configuration.md` sections 4.2 and 4.3: session and CSRF
  cookie contracts.
- `docs/adr/0004-admin-control-plane-is-lan-only.md`: the Admin listener remains
  a separate LAN-only plane; this plan will record the explicit private HTTP
  cookie amendment there.
- `AGENTS.md`: handlers keep business logic out of protocol layers and secrets
  never enter logs or frontend storage.

## Current repository state

- `crates/auth/src/service.rs` always emits `Secure` on session and CSRF set and
  clear cookies.
- `crates/auth/src/origin.rs` validates exact Admin Origin/Referer values but
  discards whether the accepted Origin was HTTP or HTTPS.
- `crates/admin-api/src/lib.rs` creates and clears cookies in login/logout
  handlers without transport-specific cookie policy.
- `crates/server/src/config.rs` accepts exact HTTP or HTTPS Admin origins but
  does not reject public cleartext Admin origins.
- `frontend/admin/src/App.tsx` renders `MV` text in `.brand-mark` and
  `.sidebar-logo`; `frontend/admin/index.html` has no favicon.
- The worktree also contains the requested uncommitted
  `deploy/existing-nginx/` single-service NAS deployment variant and its
  documentation link. Those changes must be preserved and updated with the
  local HTTP Origin example.

## Scope and non-scope

In scope:

- dynamic Admin cookie `Secure` behavior derived from an already validated
  state-changing request Origin/Referer;
- startup rejection of cleartext Admin origins outside loopback, private IPv4,
  IPv6 unique-local, and link-local address space;
- exact private HTTP Origin in the NAS Compose example;
- a generated transparent MCP Vault logo, frontend use, favicon, styling, and
  frontend tests;
- authentication/config/Admin integration tests and governing documentation.

Out of scope:

- serving TLS directly from MCP Vault;
- accepting public cleartext Admin traffic;
- weakening WebDAV TLS requirements;
- changing session storage, schema, expiry, or CSRF token design;
- public Admin proxying or automatic LAN CIDR enforcement in the application.

## Invariants and risks

- HTTPS login/logout cookies must remain `Secure`.
- Only an exact configured local HTTP Origin may receive non-Secure cookies.
- Public HTTP hostnames and public IPs must fail configuration validation.
- Cookie deletion must use the same security mode as cookie creation so HTTP
  logout actually clears local cookies.
- `HttpOnly` remains on the session cookie; `SameSite=Strict` remains on both;
  CSRF headers and exact Origin/Referer validation remain mandatory.
- LAN HTTP transmits the Admin password and session without transport
  encryption; documentation and Admin copy must state this risk and recommend
  HTTPS/VPN on untrusted LANs.
- The logo must contain no generated text, preserve transparency, remain
  legible at favicon/sidebar sizes, and be copied into the project rather than
  referenced from a generated-image cache.

## Proposed design

1. Add a typed cookie security mode in `mcp-vault-auth`. Extend
   `OriginPolicy` so a state-changing Admin request returns whether its exact
   accepted Origin/Referer is HTTPS or HTTP while preserving the existing
   validation API.
2. Parameterize Admin session/CSRF set and clear cookie builders by that typed
   mode. Login and logout derive the mode only after exact Origin validation.
3. Validate `MCP_VAULT_ADMIN_ORIGINS` during server config loading. HTTP values
   are permitted only for localhost or literal private/loopback/link-local IP
   addresses; HTTPS remains unrestricted by this local-host test.
4. Generate one no-text, transparent, vector-friendly square logo in the
   existing navy/mint palette. Store it under `frontend/admin/public/`, render
   it in both brand positions, and reference it as the favicon.
5. Update the NAS single-service Compose with both HTTPS and example LAN HTTP
   origins, and update security, Admin, deployment, requirements, and ADR text.

## Work breakdown

1. Add the plan and generate/inspect the logo asset.
2. Implement typed Origin security and dynamic cookie builders in `auth` with
   focused unit tests.
3. Wire login/logout behavior and private HTTP origin validation through
   `admin-api` and `server`, with integration/config tests.
4. Integrate the logo and update frontend tests and styles.
5. Update deployment/security/ADR documentation and the NAS Compose example.
6. Run Rust formatting, Clippy, full workspace tests, frontend lint/test/build,
   Compose validation, diff checks, and visual QA; then move this plan to
   `docs/exec-plans/completed/`.

## Progress

- [x] 2026-08-28 — Inspected current cookie, Origin, Admin UI, deployment, and
  security contracts; selected a private-origin conditional cookie design.
- [x] 2026-08-28 — Generated two transparent logo candidates, rejected the
  detailed 3D candidate, resized the final flat candidate to 512×512, and
  confirmed alpha and small UI rendering.
- [x] 2026-08-28 — Implemented typed HTTP/HTTPS cookie policy, private-origin
  startup validation, login/logout integration, and focused tests.
- [x] 2026-08-28 — Integrated the Logo into login, sidebar, favicon, and
  apple-touch metadata; browser preview confirmed the image loaded at its
  natural 512×512 size.
- [x] 2026-08-28 — Updated Compose, requirements, Admin, deployment, security,
  and ADR text; full Rust/frontend/Compose/diff validation passed.

## Decisions

- HTTP support is inferred from the exact accepted request Origin/Referer; it
  is not a global downgrade that changes HTTPS cookies.
- Cleartext Admin origins must be literal local/private addresses or localhost.
  DNS names are excluded because an apparently local hostname can resolve or
  rebind to a public address outside the application's view.
- The generated mark contains no letters so image-generation text accuracy is
  irrelevant and the asset remains language-neutral at favicon scale.

## Surprises and discoveries

- `url::Url` represents IPv6 hosts through structured `Host::Ipv6`; reparsing
  `host_str()` caused a legitimate `fd00::/8` HTTP origin to be rejected. The
  validator now matches `Host::Domain`, `Host::Ipv4`, and `Host::Ipv6`
  directly. The first focused server test exposed this and passed after the
  correction.
- The first frontend test invocation ran pnpm outside CI mode, attempted a
  registry metadata check, and aborted an interactive module-directory purge.
  `env CI=true pnpm --dir frontend/admin test` used the installed dependency
  graph and passed all 25 tests.
- The first generated Logo followed the semantic brief but retained 3D
  perspective and excessive vault hardware detail. A second constrained
  generation produced the selected flat, transparent, favicon-readable mark.

## Validation

- `cargo fmt --all --check` — passed after final formatting.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  passed.
- `cargo test --workspace --all-features` — passed outside the filesystem/
  socket sandbox, including 18 Admin, 22 Auth, 44 Server, all integration, and
  all doctests.
- `cargo test -p mcp-vault-server generated_logo_is_embedded_as_a_png_admin_asset`
  — passed after adding the final embedded-asset assertion; the current Server
  suite contains 45 tests.
- `env CI=true pnpm --dir frontend/admin lint` — passed.
- `env CI=true pnpm --dir frontend/admin test` — 25 passed.
- `env CI=true pnpm --dir frontend/admin build` — TypeScript and Vite
  production build passed.
- `docker compose -f deploy/existing-nginx/compose.yaml config --quiet` —
  passed with no `.env` dependency.
- `git diff --check` — passed.
- Built-in ImageGen plus `view_image` confirmed the final 512×512 PNG has
  alpha; a local Vite preview confirmed login-page rendering and favicon/image
  resource loading.

## Rollback and recovery

No schema or canonical Vault data changes are involved. Reverting the auth,
Admin, server-config, frontend, Compose, docs, and logo files restores the
prior HTTPS-only browser-cookie behavior. Existing sessions remain digest
records in SQLite; browser cookies retain their original attributes and expire
or can be revoked normally.

## Outcomes

MCP Vault now supports an exact private/loopback HTTP Admin Origin without
downgrading HTTPS sessions. HTTPS set/clear cookies remain Secure; accepted
local HTTP set/clear cookies omit only Secure while preserving session
HttpOnly, SameSite=Strict, CSRF, exact Origin/Referer, expiry, and revocation.
Public cleartext IPs and DNS names fail bootstrap validation, and the login UI
warns when transport is an unencrypted non-loopback HTTP connection.

The Admin UI now consumes the generated transparent MCP Vault mark on the
login shell, authenticated sidebar, favicon, and apple-touch metadata. The NAS
single-service Compose permits both the fixed HTTPS origin and an editable LAN
HTTP IP origin. No schema, canonical Vault data, or protocol endpoint changed.
