# WP-08 MCP Foundation and Controlled Vault Tools

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-21
- Updated: 2026-08-21

## Purpose and user-visible result

Replace the MCP 501 fallback with the official `rmcp` 3.0.1 server adapter at
`/mcp/v1/vaults/{vault_slug}`. An authenticated MCP client will be able to
discover server instructions, receive a deterministic scope-filtered tool and
resource view, browse bounded deterministic Vault metadata, read notes, make
revision-aware Core mutations, and inspect/restore history. Each request will
derive its Vault context from the URL and PAT/OAuth credential without placing
business state in a protocol session.

## Governing requirements

- `AGENTS.md`: stateless MCP boundary, official SDK, Vault isolation, separate
  protocol/application layers, safe writes, redacted errors, and no arbitrary
  `vault_id` tool argument.
- `docs/product-requirements.md` sections 3.2-3.5, 3.8-3.10, 4.1, 4.3-4.6:
  discovery, retrieval, controlled mutation/history, memory-facing
  instructions, independent authorization, portability, bounded results, and
  provider-independent core operation.
- `docs/architecture.md` sections 4.2-4.5, 5.2-5.3, 6-9, 13, 15-17: RMCP
  adapter boundary, application services, Core mutation sequence, durable
  events, dependency direction, and future multi-Vault binding.
- `docs/interfaces.md` sections 1-2, 4-9: endpoint binding, MCP 2026-07-28
  Streamable HTTP, required headers/body metadata, discovery instructions,
  deterministic tools, resources, structured errors, PAT/OAuth, and scopes.
- `docs/security.md` sections 2, 4.2-4.3, 7, 9-10, 20: independent agent
  authorization, Origin/Host/transport validation, OAuth resource-server
  checks, safe paths, destructive-tool annotations, redaction, limits, and
  public/Admin separation.
- `docs/development-and-testing.md` sections 2, 4, 8.5-8.6, 10, 13-16:
  official RMCP SDK, protocol tests, conformance, crash/isolation coverage,
  and bounded output.
- `docs/standards-and-references.md` section 2: MCP revision and pinned
  official Rust SDK direction.
- Accepted ADR-0002, ADR-0005, and ADR-0008: Vault is the isolation boundary,
  the service remains a modular monolith, and MCP credentials bind to one
  Vault.

## Current repository state

WP-00 through WP-07 are complete. `crates/mcp/src/lib.rs` is an explicit 501
fallback. `rmcp` 3.0.1 is pinned with the server feature but the streamable
HTTP server feature is not yet enabled. Auth already provides Vault-bound PAT
and OAuth principal derivation, including scope-to-permission mapping, but no
HTTP adapter consumes it. Vault Core provides canonical reads, mutations,
revisions, history, and recovery. Indexer and memory crates remain documented
stubs for WP-09 and WP-11. The server data router currently mounts WebDAV and
the MCP fallback separately.

## Scope

### Included

- Enable the official RMCP Streamable HTTP server transport and configure
  stateless behavior, supported protocol negotiation, request-size limits,
  exact Host/Origin policy, and request-scoped cancellation.
- Add MCP middleware that extracts one URL Vault slug, authenticates a
  Vault-bound PAT or configured OAuth resource-server token, inserts a typed
  request context, and returns safe Bearer challenges/errors.
- Implement `server/discover`, deterministic authorization-dependent tools and
  resources, private cache metadata, server instructions, request IDs, and
  structured tool/error envelopes.
- Implement deterministic Core-backed `vault_overview`, `browse_index`,
  `recent_changes`, `read_note`, `create_note`, `edit_note`, `move_note`,
  `delete_note`, `note_history`, and `restore_note_revision` behavior. All
  canonical mutations use `SourcePlane::Mcp`, expected revisions, and
  idempotency keys.
- Implement `vault://overview`, `vault://index/{node_id}`,
  `vault://recent`, and `vault://note/{path}` resources with read
  authorization and bounded content.
- Add a Vault-scoped recent-revision repository query and typed DTOs without
  moving SQL into the protocol crate.
- Integrate the stateful MCP router into the public data listener while keeping
  Admin routes on the control listener.
- Add unit, auth/isolation, public protocol, error/conflict, resource, and
  stateless transport tests.

### Not included

- FTS, Markdown AST/frontmatter/link analysis, semantic/hybrid search, topic
  projections, or `_mcp-vault/index.yaml`; WP-09 owns those capabilities.
- Durable memory commands/recall or memory resources; WP-11 owns them.
- Admin PAT/OAuth CRUD UI/API; WP-12 owns management surfaces. Tests create
  credentials through the existing AuthService/state boundary.
- Provider calls, LLM work, indexing, or memory work in request handlers.
- A custom JSON-RPC implementation, persistent MCP protocol session, or
  arbitrary `vault_id` tool parameter.
- Claiming official conformance without the external conformance suite being
  available; local RMCP/public HTTP tests remain required evidence.

## Invariants and risks

- The URL slug and authenticated principal must resolve to the same
  `VaultContext`; a second Vault credential must fail on the first endpoint.
- Every tool requiring user data obtains `McpRequestContext` from the
  authenticated request extension. The handler never trusts a tool-supplied
  Vault identifier or filesystem root.
- Tool lists are deterministic and scope-filtered, but every tool repeats the
  permission check at call time to prevent hidden-tool invocation.
- Reads and resource responses are bounded. Binary note content is rejected
  or represented as metadata/resource links rather than base64-expanded into
  unbounded tool output.
- `create_note`, `edit_note`, move, delete, and restore call Vault Core only;
  no MCP handler accesses SQL or filesystem primitives directly.
- MCP errors contain stable public codes and request IDs, never SQL, absolute
  roots, Authorization headers, tokens, or note bodies by default.
- PAT lookup uses the existing keyed digest/version path. OAuth validation
  uses issuer/signature/time/audience/resource/grant/scope checks already in
  AuthService; the adapter must not weaken them.
- RMCP's current revision is stateless. Legacy revisions are negotiated by
  the SDK configuration without reintroducing application state in sessions.

## Proposed design

```text
HTTP /mcp/v1/vaults/{slug}
  → MCP adapter extracts slug and Bearer credential
  → Vault registry resolves VaultContext
  → AuthService verifies PAT/OAuth + Vault grant
  → typed request extension { context, principal, core }
  → rmcp StreamableHttpService (stateless)
  → MCP handler/tool/resource translation
  → Vault Core / state repositories
```

`McpService` owns state/auth/Core construction and creates an RMCP
`StreamableHttpService` with `NeverSessionManager` and
`legacy_session_mode = false`. The handler receives the authenticated request
extension through RMCP's `RequestContext`/`Extension` extraction. `list_tools`
filters the static tool definitions in the documented order; tool calls still
check permissions. `server/discover`, list responses, and resource reads use
private cache scope because authorization changes their contents.

The first deterministic browse implementation uses state/Core metadata only.
It intentionally does not walk note bodies or claim to be FTS. WP-09 will add
an index application service behind the same protocol DTO/tool boundary.

## Work breakdown

1. Add this plan, inspect RMCP 3.0.1 transport/handler APIs, and settle
   stateless/auth/context boundaries.
2. Add typed data-host configuration and recent-revision repository query;
   add repository/isolation tests.
3. Implement `McpService`, Bearer middleware, RMCP stateless transport,
   `server/discover`, tool/resource models, scope filtering, and safe errors.
4. Implement Core-backed deterministic browse/read/mutation/history tools and
   resource reads with request IDs, preconditions, and bounded results.
5. Integrate the MCP service into `server::run` and public data composition;
   verify control-plane routes remain absent.
6. Add public-protocol tests for discovery, version/header/body validation,
   PAT/OAuth, scope filtering, Vault isolation, conflicts, resources, and
   stateless requests; run workspace/frontend/docs checks.
7. Record conformance availability and move this plan to completed only after
   all local acceptance checks pass.

## Progress

- [x] 2026-08-21 — Re-read root instructions and ordered architecture/
  interface/security/testing documents; inspected WP-07, RMCP 3.0.1, auth,
  Core, state, and server seams.
- [x] 2026-08-21 — Create WP-08 ExecPlan and settle stateless/context/tool
  boundary.
- [x] 2026-08-21 — Add `MCP_VAULT_DATA_HOSTS` configuration and the
  Vault-scoped recent-revision repository query with bounds/isolation tests.
- [x] 2026-08-21 — Implement RMCP stateless transport/auth/discovery,
  deterministic scope-filtered tools/resources, cache metadata, annotations,
  and safe structured errors.
- [x] 2026-08-21 — Implement Core-backed deterministic browse/read/history,
  historical reads, revision-aware mutations, idempotency, and bounded note
  resources.
- [x] 2026-08-21 — Integrate the stateful MCP service into the public data
  listener and add public discovery, auth, scope, conflict, resource, and
  Core round-trip tests.
- [x] 2026-08-21 — Run final checks, update docs/checksums, and complete plan.

## Decisions

- Use RMCP `StreamableHttpService` with `NeverSessionManager` and
  `legacy_session_mode = false`; no MCP protocol session may hold Vault or
  business state.
- Authenticate in an Axum middleware before RMCP dispatch and pass only a
  typed, Vault-bound request extension into the handler. This keeps auth and
  endpoint binding outside tool business logic while preserving request
  metadata access through RMCP.
- Do not implement an ad-hoc full-text scan in WP-08. Browse/read/mutation
  tools use deterministic Core/state metadata; WP-09 supplies indexed search
  through a service boundary.

## Surprises and discoveries

- RMCP 3.0.1 already contains `server/discover`, protocol negotiation, header
  validation, request-scoped SSE cancellation, and stateless HTTP behavior;
  duplicating these in Axum would create compatibility debt.
- RMCP's static tool router does not itself authorize `tools/list`; the project
  handler must filter the returned definitions and still enforce every call.
- RMCP 2026-07-28 discovery responses namespace `serverInfo` under
  `_meta`; public tests assert the wire shape rather than assuming the older
  initialize response shape.
- RMCP requires `MCP-Protocol-Version`, `Mcp-Method`, and operation-specific
  `Mcp-Name` headers for the current stateless request path; tests send these
  headers so protocol validation remains exercised at the public boundary.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p mcp-vault-state --all-features
cargo test -p mcp-vault-mcp --all-features
cargo test -p mcp-vault-server --all-features
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
```

When available, run the official MCP conformance suite for every advertised
revision and record its exact command/version. Do not advertise a revision
that has not been exercised.

## Rollback and recovery

WP-08 adds no database migration unless a repository query requires a future
schema change; the planned recent-revision query uses existing revision rows.
If MCP integration is reverted, the prior explicit 501 router can be restored
without touching canonical files. In-flight Core mutations retain the
existing operation-journal recovery behavior. Removing the MCP adapter does
not revoke or rewrite credentials; credential lifecycle remains an Admin/auth
operation.

## Outcomes

Implementation now provides a real `StreamableHttpService` at the versioned
MCP endpoint, with no protocol session state and no tool-supplied Vault ID.
PAT/OAuth requests are authenticated before RMCP dispatch; the typed request
extension binds the principal, Vault context, Core, and state repository.
Core-backed tools and resources expose only bounded metadata/content, return
private-cache results, preserve revision/idempotency preconditions, and map
business failures to stable structured envelopes. Search and memory features
remain intentionally absent from discovery until WP-09/WP-11 supply their
application services.

Final acceptance evidence and conformance-tool availability are recorded here
before moving this plan to `docs/exec-plans/completed/`.

Final acceptance on 2026-08-21:

- `cargo fmt --all --check` passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  passed.
- `cargo test --workspace --all-features` passed, including 10 MCP public
  protocol tests.
- `pnpm --dir frontend/admin lint` passed in CI mode after allowing the
  configured registry fetch; frontend test and build also passed.
- `bash scripts/check-docs.sh`, `cargo doc --workspace --no-deps`, and
  `shasum -a 256 -c SHA256SUMS` passed.
- No official MCP conformance executable or suite was present in the
  repository/environment (`mcp-conformance` and `rmcp-conformance` were not
  found). The local RMCP public tests cover the required headers, discovery,
  current revision, structured results, authorization, resources, and
  stateless requests; official conformance remains WP-14 release work.
