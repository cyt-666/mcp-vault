# WP-07 Integrated WebDAV

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Replace the WebDAV 501 fallback with a real RFC 4918 data-plane adapter. A
Vault-bound WebDAV app credential will authenticate at
`/dav/v1/vaults/{vault_slug}/`, and an existing Obsidian-compatible DAV client
will be able to list, read, stream, create, replace, move, copy, and delete
Markdown notes, binary attachments, and directories. DAV preconditions and
ETags will map to Vault Core revisions, so a stale client receives a conflict
instead of overwriting newer content.

## Governing requirements

- `AGENTS.md`: protocol adapters authenticate/translate/call application
  services; no direct filesystem or SQL access; safe atomic writes,
  preconditions, history, audit, outbox, and Vault isolation.
- `docs/product-requirements.md` sections 3.1, 3.4, 3.8, 4.2, 4.5, and 4.6:
  standard WebDAV for Obsidian, binary support, credentials, conflict safety,
  streaming, TLS, and interoperability.
- `docs/architecture.md` sections 4.3-4.5, 5.1, 7.1, 7.3, 8, 13, and 17:
  Vault Core boundary, custom `dav-server` adapter, write/revision flow,
  stable identity, lock ordering, and future multi-Vault binding.
- `docs/interfaces.md` sections 1-3 and 12: versioned Vault URL, required DAV
  methods, Basic authentication, ETags/preconditions, ranges, streaming,
  depth handling, and compatibility policy.
- `docs/security.md` sections 2, 4.2-4.3, 6, 9, 10, and 20: independent
  WebDAV credentials, secure Basic transport, trusted proxy handling, safe
  paths, no secret logging, and write integrity.
- `docs/deployment-and-operations.md` sections 3-4, 6-8, 14, 18-19:
  public data listener, reverse-proxy headers/TLS, readiness, maintenance,
  and WebDAV disaster-recovery smoke tests.
- `docs/development-and-testing.md` sections 2, 4-5, 8.5, 13-15: DAV
  adapter dependency direction, protocol tests, Litmus, streaming, recovery,
  and performance constraints.
- Accepted ADR-0001, ADR-0002, ADR-0003, and ADR-0005: Markdown remains
  canonical, Vault is the isolation boundary, standard WebDAV is used through
  a project-owned adapter, and the service remains a modular monolith.
- ADR-0009: overwrite MOVE tombstones are archived inside Vault Core without
  dropping revision history or weakening the unique path invariant.

## Current repository state

WP-00 through WP-06 supplied the state, storage, Core, authentication, and
server composition seams. WP-07 now provides the project-owned
`dav-server::GuardedFileSystem`, Core-backed metadata/listing/directory and
incremental staged-write services, Vault-bound Basic authentication, and the
stateful data-plane mount. The running server constructs the adapter with the
loaded master-key policy and exact trusted-proxy IP configuration; Admin
credential CRUD remains explicitly deferred to WP-12.

## Scope

### Included

- `dav-server` `GuardedFileSystem` adapter with `DavHandler`, request-scoped
  Vault credentials, and a project-owned path/permission translation layer.
- Basic authentication challenge/parsing, Vault slug/context resolution,
  credential expiry/revocation, permission checks, loopback/TLS/trusted-proxy
  transport checks, and redaction-safe failures.
- Core/storage query and staging seams needed by DAV: safe direct-child
  enumeration, metadata/ETag projection, durable directory create/delete,
  incremental atomic upload staging, and Core-backed copy/move.
- OPTIONS, PROPFIND, GET, HEAD, PUT, DELETE, MKCOL, COPY, MOVE, LOCK, and
  UNLOCK through `dav-server`, including ranges, depth limits, content types,
  streaming bodies, and conditional headers.
- Data-plane server composition at `/dav/v1/vaults/{vault_slug}/`, connection
  information helpers, request peer information, and trusted proxy scheme
  configuration.
- Unit, Core, WebDAV integration, two-Vault isolation, auth/revocation,
  precondition/conflict, path-attack, range/large-binary, and lock tests.
- WebDAV operations/deployment/security documentation and checksum updates.

### Not included

- Admin WebDAV credential CRUD endpoints/UI; WP-12 will expose the existing
  credential application service and generated connection information through
  Admin APIs.
- Obsidian plugin-specific automation or a custom plugin. Litmus and local
  protocol fixtures are included here; real Hēsperus/Remotely Save and
  desktop/mobile checklists remain release-gate work in WP-14.
- Durable distributed DAV locks. This package uses `dav-server`'s in-memory
  lock manager plus Vault Core's per-path locks; a later implementation may
  replace the lock manager without changing the adapter boundary.
- MCP, indexing, memory, provider, or Admin business logic.
- A direct TLS server. Basic authentication is accepted for loopback peers or
  for HTTPS as asserted by a configured trusted proxy; plaintext public
  transport remains rejected.

## Invariants and risks

- Every request derives one `VaultContext` from the URL slug and verifies the
  credential in that same context. No request or DAV operation accepts an
  arbitrary `vault_id`.
- The adapter never calls `std::fs`, `tokio::fs`, SQLx, or provider code. All
  canonical reads, listings, directory operations, and mutations go through
  Core/application services; SQL remains in state repositories and I/O in
  storage-fs.
- `LocalFs` is never used in production. A DAV write is staged through the
  Core journal/atomic-write path, then history/audit/outbox metadata commits in
  the same transaction as the revision.
- PUT commit uses the current File ID and expected revision captured at open;
  the final conditional metadata commit is authoritative if a concurrent
  Core/DAV write races the upload.
- Basic credentials, Authorization headers, passwords, note bodies, and
  absolute roots never appear in logs or error bodies.
- A trusted `X-Forwarded-Proto: https` assertion is accepted only when the
  socket peer is in the configured exact trusted-proxy IP set. Loopback is
  accepted for local operation; arbitrary forwarded headers are ignored.
- DAV path decoding happens once through `DavPath`, then the adapter converts
  only decoded UTF-8 bytes into `VaultPath`; reserved paths, traversal,
  symlinks, special files, and unsafe names remain rejected by lower layers.
- `dav-server` lock tokens are protocol coordination only. Core path locks and
  expected revisions remain the correctness boundary if the process restarts
  or the in-memory DAV lock table is lost.
- Request bodies are streamed into bounded storage staging. The adapter does
  not collect an attachment body into an unbounded `Vec<u8>`.

## Proposed design

```text
HTTP /dav/v1/vaults/{slug}/...
  → WebDAV adapter extracts slug and Basic credentials
  → Vault registry resolves VaultContext
  → AuthService verifies credential + permissions/expiry/revocation
  → rewrite request to Vault-relative DAV path
  → dav-server::DavHandler<DavCredentials>
  → GuardedFileSystem methods
  → VaultCore query/mutation/staged-write services
  → storage-fs + state repositories
```

`WebDavService` owns the state/auth/core factory and trusted transport policy.
`DavCredentials` is a request-scoped, cloneable value containing the validated
VaultContext, AuthPrincipal, and VaultCore handle. `CoreFileSystem` is a
project-owned `GuardedFileSystem<DavCredentials>`; it only translates DAV
operations and maps stable Core errors to `dav_server::fs::FsError`.

`DavHandler` is configured once with `CoreFileSystem`, a principal derived
from credential ID, and `MemLs`. The mount slug is removed from the request
URI and `Destination` header before dispatch, so the filesystem sees only a
validated Vault-relative DAV path. The original endpoint remains the security
binding; rewriting is not authorization.

### Core/storage seams

- `VaultStorage::list_directory` returns safe immediate-child metadata with
  deterministic ordering and no symlink/special-file exposure.
- `VaultCore::metadata` and `list_directory` return protocol-neutral metadata
  and revision-derived ETag values without exposing storage roots.
- `VaultCore::create_directory` journals and commits a directory entry through
  the same audit/outbox transaction boundary as files.
- `VaultStorage::AtomicWrite` gains incremental chunk/finalize support. A
  `VaultCore::StagedWrite` owns the journal intent, Core lock, atomic writer,
  and final commit; `DavFile::flush` completes it or marks it recoverable.
- Existing Core `read`, `copy`, `move_entry`, `delete`, and `restore` paths
  remain the only canonical file mutation implementations. Directory COPY
  recursion is delegated by `dav-server` to Core-backed `create_directory`
  and `copy`/`move_entry` calls.

### HTTP/security integration

`AppConfig` gains an exact `MCP_VAULT_TRUSTED_PROXY_IPS` list (empty by
default). The data listener uses `into_make_service_with_connect_info` so
loopback and trusted-proxy checks use the socket peer, not an untrusted header.
The public router keeps the existing no-state test seam and adds a stateful
data-router constructor used by `server::run`.

## Work breakdown

1. Read governing documents and create this plan; inspect `dav-server` 0.11
   traits and current Core/auth seams. Validate the design before code.
2. Extend storage/Core query and streamed-write boundaries; add directory
   operation tests and crash/precondition coverage.
3. Implement the guarded DAV filesystem, metadata, path/error mapping, and
   `DavHandler` integration with auth/locks; add protocol integration tests.
4. Integrate stateful WebDAV router and trusted peer/proxy configuration into
   server composition without exposing Admin routes.
5. Run Litmus if installed, focused/public WebDAV tests, security/isolation
   tests, workspace/frontend/docs checks, and update docs/checksums.

## Progress

- [x] 2026-08-20 — Re-read AGENTS.md, product requirements, architecture,
  implementation plan, PLANS, interfaces, security, deployment/testing, and
  ADR-0003; inspected current WebDAV/auth/Core/storage/server seams.
- [x] 2026-08-20 — Create WP-07 ExecPlan and settle adapter/staging/transport
  boundaries.
- [x] 2026-08-20 — Add storage/Core metadata, directory, and staged-write
  services, including directory-tree moves and overwrite tombstone handling.
- [x] 2026-08-20 — Implement guarded `dav-server` filesystem and auth/path
  translation.
- [x] 2026-08-20 — Integrate the stateful WebDAV data-plane router/server.
- [x] 2026-08-20 — Add protocol, isolation, conflict, range, lock, and recovery
  tests.
- [x] 2026-08-20 — Run final checks, update docs/checksums, and complete plan.

## Decisions

- Use `dav-server` 0.11's `GuardedFileSystem` instead of `LocalFs`; this
  preserves the project-owned Core boundary and lets the library supply RFC
  4918 method/conditional/range/lock parsing.
- Use the library's `MemLs` for protocol lock tokens in WP-07. Core's durable
  correctness still comes from path locks, expected revisions, and the
  operation journal; a durable lock repository can replace this seam later.
- Trust forwarded HTTPS only from exact configured proxy IPs. A header-only
  `X-Forwarded-Proto` check was rejected because it would make plaintext Basic
  authentication appear secure to an arbitrary client.
- Stage DAV chunks through storage-fs and commit through Core rather than
  buffering request bodies in the protocol crate or writing canonical files
  directly from `DavFile`.
- Preserve the existing unique `(vault_id, path)` file-entry invariant. When
  `dav-server` has already deleted an overwrite destination, Core archives
  that tombstone below the reserved namespace in the same SQLite metadata
  transaction so a MOVE still preserves the source File ID without a schema
  migration.

## Surprises and discoveries

- `dav-server`'s `GuardedFileSystem` receives request-scoped credentials and
  already handles RFC conditional headers, ranges, Depth, and DAV XML; the
  adapter should not duplicate those parsers.
- `DavHandler` formats `DavMetaData::etag()` as a strong quoted ETag, so the
  Core metadata seam supplies an unquoted revision/hash value and the adapter
  keeps directory tags projection/version based.
- The existing `AuthService` verifies WebDAV credentials but the server
  composition root did not retain a key ring or construct an AuthService;
  WP-07 adds the composition seam without sharing Admin/MCP credentials.
- `dav-server` applies overwrite by deleting the destination first. The
  existing unique path constraint therefore required a Core-level tombstone
  archive for stable-identity MOVE; this was recorded in ADR-0009 and covered
  by the overwrite and directory integration tests.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p mcp-vault-storage-fs --all-features
cargo test -p mcp-vault-core --all-features
cargo test -p mcp-vault-webdav --all-features
cargo test -p mcp-vault-server --all-features
cargo test --workspace --all-features
CI=true pnpm --dir frontend/admin lint
CI=true pnpm --dir frontend/admin test
CI=true pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
make check
```

`make check` passed, including workspace Clippy/tests, frontend lint/test/build,
documentation checks, and `cargo doc`. WebDAV's in-process public adapter
fixture covers Basic challenge, expired/revoked credentials, two-Vault binding,
PROPFIND depth, large binary PUT/GET/range, stale `If-Match`, MKCOL, COPY,
MOVE, DELETE, LOCK/UNLOCK, traversal, interrupted staging, and overwrite
directory behavior. No `litmus`, `dav-litmus`, `litmus.py`, or `cadaver`
executable is installed in this environment, so the external Litmus and real
Sync Engine/Remotely Save release fixtures remain WP-14 validation.

## Rollback and recovery

WP-07 adds no migration. If a staged DAV upload is interrupted, the existing
operation journal and startup recovery remove or finalize the safe old/new
state; the request never writes a non-atomic canonical file. If a directory
metadata commit is interrupted, the journal is recovered before readiness.
Reverting the adapter leaves the previous 501 router available; no canonical
content or operational rows need destructive rollback.

## Outcomes

WP-07 is complete. The data plane now serves Vault-scoped WebDAV through
Vault Core, with streaming safe writes, revision-derived ETags, DAV
preconditions/ranges/properties/locks, recursive directory operations,
overwrite-safe stable identities, connection URL generation, trusted-proxy
transport checks, and revocation/expiry enforcement. No database migration was
needed. Admin credential management and external client release fixtures are
the explicit follow-up work described under non-scope.
