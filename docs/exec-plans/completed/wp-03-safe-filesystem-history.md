# WP-03 Safe Filesystem and History Store

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Provide the low-level filesystem boundary that future Vault Core operations
can safely use for canonical Vault content and revision-history blobs. A
caller binds the storage object to one `VaultContext` and supplies only a
validated `VaultPath`; the implementation resolves paths relative to that
Vault root, refuses symlink/special-file traversal, streams content through a
temporary file, commits with an atomic rename, applies an explicit fsync
policy, and computes stable SHA-256 hashes. History content is stored outside
the Obsidian Vault under a Vault-scoped content-addressed blob layout.

This package deliberately does not decide revisions, permissions,
preconditions, audit facts, operation-journal state, or outbox events. Those
application-consistency responsibilities belong to WP-04 and later services.

## Governing requirements

- `AGENTS.md`: Vault isolation, canonical filesystem content, safe writes,
  validated paths, no unsafe symlink traversal, atomic writes, and protocol
  boundaries that never receive an unvalidated absolute path.
- `docs/implementation-plan.md` section 6 (WP-03): root-relative resolution,
  streaming I/O, temporary files, atomic rename, fsync, hashing, metadata,
  copy/move/delete, history blobs, identity hints, and disk-space checks.
- `docs/product-requirements.md` sections 3.1, 3.4, 4.1, and 6:
  portable canonical content, atomic/recoverable writes, concurrent-change
  safety, and revision/history acceptance criteria.
- `docs/architecture.md` sections 4.3-4.4, 7, and 8: Vault Core owns
  canonical behavior while `storage-fs` owns no-follow I/O, temp files,
  fsync, hashes, metadata, and content-addressed history.
- `docs/data-model.md` sections 1, 8, 9, and 18-19: Vault content/history
  roots, stable file identity hints, history blob naming, and rebuildability.
- `docs/security.md` sections 9-10 and 20: one decode/normalization boundary,
  reserved paths, symlink and special-file denial, atomic commits, and
  traversal/symlink/security tests.
- `docs/deployment-and-operations.md` sections 5-6 and 10: persistent
  `/data/vaults`, `/data/history`, and `/data/tmp` roots, safe startup root
  validation, and low-disk write rejection.
- `docs/development-and-testing.md` sections 4, 5, 8, 13-15: dependency
  direction, non-blocking filesystem work, storage test boundaries, crash
  phases, and large-file/security coverage.
- Accepted ADR-0001 and ADR-0002: Markdown remains canonical and Vault is the
  isolation boundary.

## Current repository state

WP-00 through WP-02 are present in the working tree and their completed
ExecPlans are under `docs/exec-plans/completed/`. `mcp-vault-domain` already
exports `VaultContext`, normalized `VaultPath`, `VaultPathPolicy`, filesystem
entry kinds/policy, typed IDs, and revisions. `mcp-vault-state` owns the first
SQLite migration and has `file_entries`, `file_revisions`, and history-related
operational columns, but WP-04 has not yet implemented the repository or
journal workflow around them.

`crates/storage-fs/src/lib.rs` now contains the low-level implementation and
`crates/storage-fs/tests/storage.rs` covers the storage boundary. The
`crates/vault-core` shell remains intentionally unchanged; no public protocol
path is allowed to use filesystem primitives until WP-04 wires this boundary
through Vault Core.

## Scope

### Included

- Add the `storage-fs` implementation and its explicit error/metadata/value
  types without depending on protocol, state, or Vault Core crates.
- Bind a `VaultStorage` instance to one `VaultContext` and a path policy.
- Validate and resolve root-relative paths using descriptor-relative,
  no-follow traversal on Unix and a checked portable fallback elsewhere.
- Validate the configured content root and provide safe root/directory
  creation, stat, streaming read, and metadata operations.
- Stream writes to same-directory temporary files, compute SHA-256, honor
  configurable disk headroom, fsync according to policy, and atomically rename.
- Provide explicit file copy, entry move, empty-file/empty-directory delete,
  and safe directory creation primitives.
- Return stable file metadata and best-effort filesystem identity hints without
  exposing absolute paths to callers.
- Add a Vault-bound `HistoryStore` under
  `<history-root>/<vault-id>/blobs/<hash-prefix>/<sha256>` with streaming,
  deduplication, safe open, and fsync behavior.
- Add unit/integration tests for large streaming content, partial source
  failure, rename/copy/delete, symlinks, special files, reserved paths,
  history deduplication, identity hints, and simulated low disk space.

### Not included

- File/revision repository APIs, revision increments, HTTP/MCP preconditions,
  audit rows, operation-journal states, transactional outbox insertion, or
  crash reconciliation (WP-04/WP-06).
- Protocol adapters, WebDAV directory listings, MCP tools, Admin routes, or
  direct listener changes.
- Recursive destructive deletion, archive restore, backup snapshots, or
  history retention/garbage collection (WP-04/WP-13).
- Symlink support, special-file exposure, or an arbitrary absolute-path API.

## Invariants and risks

- Every `VaultStorage` and `HistoryStore` is constructed with a
  `VaultContext`; the Vault ID is retained for diagnostics and history layout.
- Public content methods accept `VaultPath`, never raw absolute paths. The
  storage root and history root remain private implementation state.
- User-facing content paths use the domain `VaultPathPolicy`, so the managed
  `_mcp-vault` namespace remains unavailable to ordinary file primitives.
- Unix traversal opens each directory component with `O_NOFOLLOW` relative to
  an already-open directory descriptor. The final file is also opened with
  no-follow semantics; portable fallback code checks every component with
  `symlink_metadata` and rejects links before use.
- Only regular files and directories cross the storage boundary. Symlinks,
  sockets, FIFOs, devices, and unknown special entries are denied by default.
- A failed stream never renames its temporary file. The destination therefore
  remains the prior complete file; cleanup failures are observable as storage
  errors and never cause a partial destination to be reported as committed.
- Replacement policy is explicit at the primitive boundary. Expected revision
  and concurrent-write decisions remain in Vault Core, which will hold the
  operation lock and journal the filesystem phase.
- History hashes are content-addressed SHA-256 values. A repeated payload
  reuses one blob; the current Vault content remains the canonical copy.
- Filesystem errors are mapped to redacted typed errors without embedding
  absolute roots or raw request bodies in diagnostics.

## Proposed design

### Components and dependency direction

```text
storage-fs
├── VaultStorage       # one VaultContext + private content root
├── HistoryStore       # one VaultContext + private history blob root
├── ContentHash        # validated SHA-256 value
├── FileMetadata       # kind, size, mtime, identity hint
├── ReadFile           # streaming async reader without a path
└── platform helpers   # Unix *at/no-follow; checked fallback elsewhere
```

`mcp-vault-storage-fs` depends on `mcp-vault-domain` and filesystem/hash
libraries only. It must not depend on SQLx, `mcp-vault-state`, Axum, DAV,
RMCP, or `mcp-vault-core`.

### Write flow

```text
VaultContext + VaultPath
  → domain/path-policy validation
  → secure parent descriptor + disk-headroom check
  → create same-directory temp file (exclusive)
  → stream input while hashing
  → sync temp according to policy
  → atomic rename within the parent descriptor
  → sync parent directory when strict durability is selected
  → stat committed entry and return hash/metadata
```

`write_atomic` only reports success after the rename phase. The caller owns
the later SQLite/journal transaction and may use the returned hash, size, and
identity to record the canonical mutation.

### History flow

History writes use a unique temporary file under the Vault's private blob
directory, hash while streaming, then atomically install the generated
content-addressed path. Existing regular blobs are treated as deduplicated
successes; symlinks or special entries at a blob path are errors. History
reads validate the hash and open the file with no-follow semantics.

### Recovery boundary

WP-03 guarantees temporary-file cleanup on an ordinary failed call and leaves
the destination either old or new after an atomic rename. It does not inspect
or reconcile abandoned temporary files after a process crash. WP-04 will put
the temp name and phase in `operation_journal` and implement startup recovery.

## Work breakdown

1. Add pinned filesystem/hash/runtime dependencies and create the live plan;
   verify the crate dependency graph remains below Vault Core and state.
2. Implement typed storage errors, options, hashes, metadata, reader, and
   disk-space helpers in `crates/storage-fs`.
3. Implement Vault-bound secure root/path traversal, directory/stat/read,
   atomic streaming write, copy/move/delete, and fsync behavior.
4. Implement the Vault-bound content-addressed `HistoryStore` and its safe
   streaming/deduplication operations.
5. Add storage tests and fixtures for normal, failure, isolation, symlink,
   special-file, history, and resource-limit behavior.
6. Run formatting, Clippy, workspace tests, frontend/docs checks, and a
   container build; update this plan with evidence and move it to completed.

## Progress

- [x] 2026-08-20 — Read the root instructions, required document order, WP-03
  requirements, architecture/data-model/security/operations/testing guidance,
  and the current WP-00-WP-02 implementation.
- [x] 2026-08-20 — Confirmed WP-03 is limited to storage primitives and history
  blobs; Vault Core revision/journal/recovery behavior remains WP-04.
- [x] 2026-08-20 — Added pinned filesystem/hash/runtime dependencies and typed
  storage errors, options, hashes, metadata, readers, and disk diagnostics.
- [x] 2026-08-20 — Implemented secure Vault content operations with Unix
  descriptor-relative no-follow traversal and a checked portable fallback.
- [x] 2026-08-20 — Implemented Vault-scoped history blobs with safe relative
  directory traversal, streaming writes, deduplication, and reads.
- [x] 2026-08-20 — Added normal, large-stream, partial-failure, isolation,
  symlink, special-file, history, and low-disk tests (8 integration tests).
- [x] 2026-08-20 — Ran all required Rust, frontend, documentation, locked
  workspace-build, and Docker checks; completed the plan.

## Decisions

- Use a Vault-bound storage object rather than a global root or a helper shaped
  like `read(path)`. This makes the Vault identity explicit even before WP-04
  adds application services.
- Use Unix descriptor-relative `openat`/`renameat` operations with
  `O_NOFOLLOW` for component traversal. A portable fallback retains the same
  reject-by-default policy where descriptor APIs are unavailable.
- Keep atomic write replacement policy explicit. WP-03 exposes the primitive;
  WP-04 decides whether a mutation is permitted from expected revision and
  journal state.
- Use SHA-256 for both file write receipts and history blob addresses, matching
  the data-model and operational history layout. Hash formatting is implemented
  internally instead of adding a second hex-format dependency.
- Keep strict durability as the default: sync the temporary file and parent
  directory. Relaxed/no-sync modes exist for tests and explicitly configured
  environments.
- Simulate low disk space with a configurable minimum-free-byte headroom in
  tests; do not fill the developer or CI filesystem.

## Surprises and discoveries

- The existing domain crate already contains the required path, reserved
  namespace, and special-entry policy types, so no domain expansion is needed
  for the first storage slice.
- The existing workspace already resolves `rustix` transitively, but storage
  will declare the pinned version directly because descriptor-relative safety
  is a direct architectural dependency, not an incidental transitive API.
- `vault-core` is still an empty boundary crate. No Core API should be invented
  in WP-03 merely to exercise storage code; integration tests will call the
  storage boundary directly and leave mutation orchestration to WP-04.
- macOS temporary directories can contain a `/var` alias to `/private`; history
  root validation accepts trusted parent aliases while opening all generated
  Vault-relative history components through descriptor-relative no-follow
  traversal.
- Binding a Unix socket is restricted in the local sandbox, so the special-file
  test uses the portable `mkfifo` utility and validates FIFO rejection without
  requiring network permissions.

## Validation

```bash
cargo fmt --all --check
cargo test -p mcp-vault-storage-fs --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
make check
make build
docker build --tag mcp-vault:wp03 .
```

Expected results include large streaming and failed-write tests preserving the
old destination, symlink/special-file rejection, history deduplication, and
green workspace/frontend/docs/container checks. If platform-specific special
file or strict directory-fsync behavior is unavailable, the test must record
the exact platform limitation rather than silently skip the security claim.

Observed results:

- `cargo test -p mcp-vault-storage-fs --all-features` passes all 8 integration
  tests; the large stream, partial failure, two-Vault, symlink, FIFO, history,
  and low-disk cases are covered.
- Workspace Clippy and `cargo test --workspace --all-features` pass with all
  warnings denied.
- `make check` passes Rust formatting/lint/tests, frontend lint/test/build,
  documentation checks, and workspace documentation generation.
- `make build` passes frontend packaging and locked workspace compilation.
- `docker build --tag mcp-vault:wp03 .` passes and produces the release image;
  the builder compiles `mcp-vault-storage-fs` and the runtime image is emitted.

## Rollback and recovery

WP-03 adds no database migration and creates no persistent data in the
repository. A deployed rollback is a binary rollback; existing canonical
Vault content and history blobs remain untouched. Failed writes remove their
temporary file where possible and never delete the destination. Orphaned temp
files after process termination are intentionally left for WP-04's journaled
reconciler rather than blindly deleting user-looking files.

## Outcomes

WP-03 is complete. `storage-fs` now provides Vault-bound secure path
resolution, no-follow descriptor traversal on Unix, a checked portable
fallback, streaming reads/writes, SHA-256 receipts, metadata and identity
hints, configurable disk headroom, atomic rename, copy/move/delete primitives,
and a Vault-scoped deduplicated history Blob store. Failed streams preserve the
previous destination and clean temporary files in the ordinary error path.

The package adds no SQL migration and does not implement revision/journal/
outbox orchestration; those remain the WP-04 Vault Core boundary. The next
unfinished work package is WP-04, Vault Core, revisions, history integration,
and crash recovery. This plan is moved to `docs/exec-plans/completed/`.
