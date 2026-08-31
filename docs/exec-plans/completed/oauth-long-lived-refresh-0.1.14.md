# OAuth long-lived refresh interoperability for 0.1.14

- Owner: Codex
- Created: 2026-08-31
- Last updated: 2026-08-31
- Status: Complete

## Purpose and user-visible result

ChatGPT can authorize MCP Vault with `offline_access`, keep a connection active
through rotating refresh tokens, and recover from a concurrent refresh retry
without invalidating the token pair that already succeeded. Access tokens stay
short-lived at one hour. Each successful refresh receives a new 180-day idle
lifetime. Reuse of an already-rotated token is tolerated only as a rejected
duplicate for 60 seconds; a later replay revokes the full token family.

The service version and local Linux image become `0.1.14`. Existing grants that
do not contain `offline_access` remain readable and refreshable. A newly
reconnected ChatGPT client can request the advertised protocol scope.

## Governing requirements

- `docs/product-requirements.md` sections 3.8, 4.3, 4.5, and 4.6 require a
  self-contained, Vault-bound OAuth authorization server, rotating refresh
  tokens, redacted secrets, and standards-aligned MCP interoperability.
- `docs/architecture.md` sections 3.2, 4.5, 5.2, and 13 keep OAuth state in
  State repositories, business validation in Auth, handlers protocol-only,
  and every grant Vault/resource scoped.
- `docs/interfaces.md` section 9.2 owns the public metadata, authorization,
  token, and refresh contracts.
- `docs/security.md` section 7.2 requires exact client/resource/scope binding,
  keyed token digests, rotation, and replay-family revocation.
- ADR-0018 owns the built-in public-client authorization-server decision.

## Current repository state

`crates/auth/src/service/local_oauth.rs` issues one-hour access tokens and
30-day absolute refresh tokens. A refresh creates a new token but supplies the
old token expiry, and any observed rotation or failed compare-and-set revokes
the family immediately. Its strict scope parser accepts only domain `Scope`
values.

`crates/state/src/auth/local_oauth.rs` already serializes writes and rotates the
old refresh row plus the new access/refresh rows in one SQLite transaction.
The refresh-to-refresh insert currently selects the old row's `expires_at`
instead of binding the new record's expiry. Existing `rotated_at` timestamps
are sufficient for a grace-window decision, so no schema change is needed.

`crates/mcp/src/oauth_server.rs` advertises authorization code and refresh
grants, renders only business scopes, and returns only those scopes in token
responses. Protected-resource metadata correctly exposes only MCP permission
scopes and must remain unchanged.

## Scope and non-scope

In scope:

- `offline_access` authorization-server discovery, request validation,
  persistence, consent display, token responses, and refresh inheritance;
- 180-day sliding refresh expiry;
- 60-second duplicate-refresh grace followed by replay-family revocation;
- concurrent refresh, legacy grant, metadata, HTTP smoke, documentation,
  version, and image verification.

Out of scope:

- new database migrations or environment variables;
- changing PAT, external JWT issuer, Admin, WebDAV, or MCP tool permissions;
- deploying, committing, pushing, or publishing an image;
- recovering already expired or revoked refresh tokens.

## Invariants and risks

- `offline_access` is a protocol capability, never a domain permission. It
  must not enter `Scope`, `PermissionSet`, tool filtering, or protected-resource
  metadata.
- Existing `scopes_json` rows without `offline_access` remain valid. Unknown
  strings other than `offline_access` fail closed.
- Refresh cannot add business scopes or add `offline_access` to a legacy
  grant. Omitting refresh `scope` inherits the full frozen grant; an explicit
  scope can narrow business permissions while retaining an already-granted
  offline capability.
- Only a correctly bound token can trigger replay handling. Client, resource,
  Vault, revocation, and expiry checks remain fail closed.
- A loser in a concurrent rotation must re-read the durable old row before
  deciding whether the failure is a duplicate or delayed replay.
- Plaintext codes and tokens remain one-time response values only and never
  enter SQLite, logs, docs, or test failure output.

## Proposed design

Auth introduces a local-only grant representation containing a `ScopeSet` and
an `offline_access` boolean. Authorization parsing accepts the eight domain
scopes plus that one protocol value, serializes both deterministically into the
existing JSON column, and exposes the boolean separately on the consent prompt
and token issue. Access-token authentication parses this representation but
constructs its principal exclusively from the domain scopes.

Code exchange creates a refresh expiry at `now + 180 days`. Refresh rotation
does the same and State binds the supplied expiry in the inserted row. The old
row remains the immutable evidence of `rotated_at`.

Refresh reuse handling compares `now - rotated_at` with 60 seconds. At or
inside the boundary it returns the ordinary invalid-grant error without a
write. Beyond the boundary it revokes both access and refresh rows for the
Vault-bound family. A compare-and-set loser re-reads the old row and applies
the same decision, preventing a concurrent stale read from revoking the
winner's pair.

Authorization-server metadata adds `offline_access`; protected-resource
metadata stays limited to application scopes. The consent page displays
`offline_access` as “保持长期连接”, and token responses include it only when it
was granted.

## Work breakdown

1. Add the active ExecPlan and amend ADR-0018 with the protocol-scope, sliding
   lifetime, and bounded replay-grace decision.
2. Change Auth scope parsing/serialization and prompt/token DTOs without
   modifying domain `Scope`.
3. Change refresh issuance and replay handling, including a time-injectable
   internal path for deterministic boundary tests.
4. Bind the new refresh expiry in the existing State transaction.
5. Update authorization-server metadata, consent rendering, token response,
   MCP integration assertions, and real HTTP smoke behavior.
6. Add focused legacy/offline, expiry, concurrent retry, and delayed replay
   tests.
7. Update interface, security, data-model, deployment, compatibility,
   traceability, release, and version references; refresh documentation
   checksums through the repository task runner.
8. Run all Rust/frontend/protocol checks and build/smoke the local
   `linux/amd64` `mcp-vault:0.1.14` and `mcp-vault:latest` image.

## Progress

- [x] 2026-08-31 — Read governing requirements, architecture, current ADR,
  relevant protocol/security/operations docs, and PLANS.md.
- [x] 2026-08-31 — Inspected current Auth, State, MCP HTTP, frontend, smoke,
  version, and reference MoviePilotMCP implementation.
- [x] 2026-08-31 — Implement protocol scope and token lifecycle changes.
- [x] 2026-08-31 — Add/update tests and HTTP smoke.
- [x] 2026-08-31 — Update ADR, docs, version, and deployment examples.
- [x] 2026-08-31 — Run validation and record exact outcomes.
- [x] 2026-08-31 — Move this plan to `docs/exec-plans/completed/` after acceptance.

## Decisions

- Use a 180-day sliding refresh-token lifetime; active use can extend the
  connection indefinitely, while 180 days of inactivity requires reconnect.
- Use a 60-second duplicate-refresh grace. Duplicate requests always fail and
  never receive another token, but do not destroy the successful pair during
  the grace interval.
- Treat `offline_access` as local OAuth protocol state rather than a domain
  permission. This avoids granting any Vault capability and keeps Admin maximum
  scopes unchanged.
- Do not add a migration: existing JSON and `rotated_at` columns already carry
  the required durable state.
- Release and image version is `0.1.14` per the operator's correction.

## Surprises and discoveries

- State already has the required serialized compare-and-set transaction; no
  process-local OAuth mutex is needed in Rust.
- The refresh insert accepted a new expiry in its DTO but ignored it in SQL,
  copying the old absolute expiry instead.
- Immediate family revocation occurs both when a rotated row is observed and
  when two callers race and the transaction loser receives `InvalidInput`.

## Validation

Run and record:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/interop/http-smoke.sh
make docs-check
git diff --check
docker build --platform linux/amd64 -t mcp-vault:0.1.14 -t mcp-vault:latest .
docker image inspect mcp-vault:0.1.14
docker run --rm --platform linux/amd64 mcp-vault:0.1.14 --check-config
```

The Auth tests must deterministically prove both sides of the 60-second
boundary, exact 180-day refresh expiry, concurrent one-winner behavior, and
legacy rows without `offline_access`. The HTTP smoke must prove metadata,
consent, token scope, immediate duplicate rejection, and continued use of the
winner's access token.

## Rollback and recovery

There is no migration. Rolling back the binary leaves all rows readable because
business-only scope arrays retain their old shape; however, a grant containing
`offline_access` will be rejected by the old strict parser, so rollback should
reconnect affected clients or revoke/recreate the local OAuth user. Deployment
must therefore back up SQLite as usual before upgrading. Interrupted rotation
remains protected by the existing SQLite transaction.

## Outcomes

Implemented and verified on 2026-08-31. `cargo fmt`, workspace Clippy, the
full workspace test suite, frontend lint/test/build, migration checks, docs
checks, real HTTP smoke, and the fixed official MCP conformance scenarios all
passed. The local `linux/amd64` image was built
as `mcp-vault:0.1.14` and `mcp-vault:latest`, both tags point to
`sha256:d33565dd3d8fb1ab969f3cc1f0f1649146ce11a25d3eadcdbbbb0c6c7f7b858a` and
report user `mcpvault`, stop signal `SIGTERM`; the read-only
`--check-config` run passed. No database migration, Git push, or remote image
publication was performed.
