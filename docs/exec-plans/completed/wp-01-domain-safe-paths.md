# WP-01 Domain Model and Safe Vault Paths

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-20
- Updated: 2026-08-20

## Purpose and user-visible result

Replace the WP-00 domain placeholder with the typed, protocol-independent
values that every later MCP Vault operation will require. The result is a
domain crate that can represent Vault identity, Vault-scoped context,
normalized relative paths, permissions/scopes, revisions/preconditions, and
actors/source planes without importing Axum, RMCP, WebDAV, SQLx, or filesystem
I/O.

Untrusted path input will be decoded at most once, normalized to NFC with `/`
logical separators, and rejected when it is absolute, traverses parents,
contains unsafe separators/control characters/platform names, exceeds hard
limits, or crosses the reserved `_mcp-vault` namespace. Filesystem entry
safety is represented as an explicit policy for later `storage-fs` code rather
than guessed from a path string.

## Governing requirements

- `AGENTS.md`: domain ownership, mandatory `VaultContext`, safe path/write
  invariants, explicit domain errors, and no protocol/storage dependencies.
- `docs/implementation-plan.md` section 4 (WP-01): typed IDs, context, path,
  permissions, revisions, actors, errors, policy, and required tests.
- `docs/architecture.md` sections 4.1-4.4 and 13: domain/Vault Registry
  ownership, context shape, safe resolution, and future multi-Vault boundary.
- `docs/data-model.md` sections 1-3, 8, and 19: canonical Vault roots,
  reserved namespace, normalized `/` + NFC paths, UUID/ULID identifiers, and
  Vault-scoped file/revision rows.
- `docs/security.md` sections 7.3, 9.1-9.2, 10, and 15: Vault binding,
  traversal/Unicode/platform/symlink/special-file rejection, and isolation.
- `docs/development-and-testing.md` sections 2, 4-5, and 8: dependency
  direction, explicit domain errors, and unit/property test expectations.
- Accepted ADR-0002 (`docs/adr/0002-vault-is-the-isolation-boundary.md`): the
  Vault is the isolation boundary even in a single-Vault deployment.

## Current repository state

WP-00 is complete and stored under
`docs/exec-plans/completed/wp-00-foundation.md`, but its implementation remains
an uncommitted working tree owned by the user. `crates/domain/src/lib.rs`
currently contains only crate-level documentation and already depends on the
workspace `uuid` package. `vault-core`, `storage-fs`, and `state` depend on the
domain crate as empty lower-level shells. No domain values, path parser, or
domain tests exist yet. The workspace uses Rust 1.94.0, edition 2024, and
`thiserror`/Serde dependencies are already available at the workspace level.

## Scope

### Included

- Add typed UUIDv7-backed identifiers for Vaults, files, revisions, memories,
  identities/credentials, jobs, operations, and events.
- Add validated `VaultSlug` and `VaultContext` with normalized absolute content
  root, settings revision, typed Vault identity, and same-Vault checks.
- Add `VaultPath` with root representation, logical segments, parent/join
  operations, one-time URL decoding, NFC normalization, hard size/depth
  limits, platform-safe segment validation, and collision keys.
- Add reserved namespace and case-sensitivity policies, including the default
  `_mcp-vault` root and collision detection over path fixtures.
- Add `Scope`, `Permission`, and set types with deterministic string forms and
  scope-to-permission mapping.
- Add numeric `Revision`, write preconditions, actor identity/type, source
  plane, filesystem entry kind, and explicit domain errors.
- Add unit/property-oriented tests for traversal, encoded traversal, Unicode,
  separators, reserved/platform names, collisions, policy behavior, revision
  preconditions, IDs, and two-Vault context isolation.
- Update the owning security/data-model documentation if the implemented
  path/slug policy needs a more explicit normative statement.

### Not included

- SQLx migrations/repositories or database serialization (WP-02).
- Root-relative filesystem resolution, no-follow descriptors, symlink checks
  against actual filesystem metadata, atomic writes, or history blobs (WP-03).
- Vault Registry persistence and setup UI (WP-02/WP-12).
- Vault Core mutation/query services (WP-04).
- Authentication or mapping credentials to scopes (WP-05).
- Protocol DTOs, URL routing, URL decoding outside the domain helper, or HTTP
  error mapping (WP-07/WP-08).

## Invariants and risks

- `VaultContext` is required by value-bearing future APIs; no global Vault
  singleton or unscoped `read(path)` helper may be introduced.
- `VaultId`, `FileId`, `MemoryId`, and other IDs are distinct Rust types, so a
  caller cannot accidentally use one identity class in another API.
- A `VaultPath` never stores an absolute host path and never contains `..`,
  backslashes, empty segments, NUL/control characters, or an encoded traversal
  marker after the single decode boundary.
- NFC normalization can make two different input strings equal. Collision
  detection must run under the selected case policy before a scan or registry
  accepts a set of paths.
- The domain crate performs no filesystem existence, symlink, canonicalization,
  or permission checks. Those belong to `storage-fs`; this package only models
  the safe policy and rejects impossible values.
- The default path policy reserves `_mcp-vault` and permits it only through an
  explicit managed-path operation. User paths cannot shadow the service
  namespace.
- Errors must not include note bodies or full sensitive paths by default;
  path errors use categorical reasons and collision errors contain only the
  two normalized path values needed by callers.

## Proposed design

### Components and dependency direction

`mcp-vault-domain` will be split into focused modules:

```text
domain
├── actor       # ActorId, ActorType, Actor, SourcePlane
├── error       # DomainError and PathError
├── id          # UUIDv7-backed typed identifiers
├── path        # VaultPath, policies, collision detection, entry safety
├── permission  # Scope, Permission, set types
├── revision    # Revision and write preconditions
└── vault       # VaultSlug and VaultContext
```

The crate depends only on standard library, `uuid`, Serde, `thiserror`,
`percent-encoding`, and `unicode-normalization`. No lower-level crate depends
on domain implementation details beyond its public value types, and domain
does not depend on any lower-level crate.

### Data and transaction flow

There is no database or filesystem transaction in WP-01. The pure flow is:

```text
external path bytes
    → one-time URL decode helper (when input is a URL path)
    → VaultPath parse + NFC normalization + hard validation
    → ReservedPathPolicy / PathCasePolicy checks
    → Vault Core or storage-fs receives (&VaultContext, &VaultPath)
```

`VaultContext::new` validates and normalizes the configured absolute content
root but never calls `canonicalize` or touches the filesystem. A later Vault
Registry will construct contexts from persisted Vault rows and storage-fs will
perform no-follow resolution beneath the context root.

### Public interfaces and schema changes

Core public values will include:

```rust
VaultContext::new(VaultId, VaultSlug, PathBuf, Revision) -> Result<_, DomainError>
VaultPath::parse(&str) -> Result<VaultPath, DomainError>
VaultPath::from_url_path(&str) -> Result<VaultPath, DomainError>
VaultPathPolicy::validate_user_path(&VaultPath) -> Result<(), DomainError>
WritePrecondition::check(Option<Revision>) -> Result<(), DomainError>
```

The path parser represents Vault root as an empty logical path (`as_str() ==
""`, `is_root() == true`) so WebDAV root listings do not need a fake filename.
`from_url_path("/")` maps to that root; `parse("/")` remains invalid as an
absolute path. No SQL schema or wire/API schema changes are made in this
package.

### Failure, retry, and recovery

Domain construction is synchronous and deterministic. Invalid values return a
typed `DomainError` immediately; no retry is meaningful and no state is
mutated. Later protocol adapters translate these errors to HTTP/DAV/MCP
responses, while Vault Core preserves the typed precondition and conflict
semantics.

## Work breakdown

1. Add dependency anchors and domain module declarations. Validate the domain
   crate still has no infrastructure dependency.
2. Implement typed IDs, `VaultSlug`, `VaultContext`, and root normalization.
   Add deterministic ID/context/isolation tests.
3. Implement `VaultPath`, URL decode-once behavior, NFC normalization, segment
   and platform-name validation, path operations, comparison keys, and
   collision detection. Add traversal/Unicode/case fixtures.
4. Implement reserved namespace, filesystem entry safety, scope/permission,
   actor/source-plane, revision/precondition, and domain errors. Add unit tests
   for each error and policy boundary.
5. Update security/data-model documentation where the exact chosen policy is
   normative, then run workspace formatting, Clippy, tests, docs, and the
   frontend checks required by the repository.
6. Record exact evidence and move this plan to `docs/exec-plans/completed/`
   only after WP-01 acceptance is genuinely satisfied.

## Progress

- [x] 2026-08-20 — Re-read root `AGENTS.md`, `docs/README.md`, the WP-01
  implementation-plan section, architecture/data-model/security/testing
  constraints, and `PLANS.md`.
- [x] 2026-08-20 — Inspected the actual WP-00 working tree and confirmed the
  domain crate is still an empty lower-level boundary.
- [x] 2026-08-20 — Added domain dependencies and focused module declarations.
- [x] 2026-08-20 — Implemented typed IDs, slug, context, and root normalization.
- [x] 2026-08-20 — Implemented path parsing, decode-once URL handling, NFC,
  policies, collision detection, reserved namespace, and entry safety.
- [x] 2026-08-20 — Implemented permissions/scopes, actors/source planes,
  revisions/preconditions, stable errors, and public integration tests.
- [x] 2026-08-20 — Ran the complete workspace/frontend/docs acceptance checks,
  locked build, and Docker image build.

## Decisions

- Keep UUIDv7 as the identifier strategy selected in WP-00. Each identifier
  category is a distinct newtype around `uuid::Uuid`; no string aliases are
  used for Vault/file/memory identity.
- Use NFC normalization and `/` as the canonical logical path representation.
  The path root is the empty logical path, while a leading slash is accepted
  only by the explicit URL-path helper and stripped exactly once.
- Use a conservative cross-platform filename policy: reject Windows reserved
  device names, trailing spaces/periods, control characters, and Windows
  punctuation even on Unix. This prevents a Vault copied between platforms
  from changing identity or becoming unaddressable.
- Make case sensitivity explicit (`Sensitive` or `Insensitive`) instead of
  guessing from the host OS. The default `VaultPathPolicy` is conservative
  `Insensitive`; scans and registry setup can explicitly select `Sensitive`
  and must run collision detection before accepting a path set.
- Model symlink/special-file handling as a default-deny `FilesystemPolicy` in
  domain. Actual no-follow descriptor behavior remains storage-fs.
- Reject the host filesystem root as a Vault content root; a Vault context must
  bind to a non-root absolute path without parent components.

## Surprises and discoveries

- WP-00 intentionally left `uuid` only as a dependency anchor; no existing
  code constrains serialization or context construction, so this package owns
  the first stable domain API.
- The architecture sample uses `settings_revision: i64`, while file revisions
  are monotonic and non-negative. The domain API uses a checked non-negative
  `Revision` and later repositories can convert it to SQLite integer storage.
- Clippy's `derivable_impls` lint required using Rust's enum/struct default
  derives after selecting the conservative insensitive path policy.
- The first final acceptance pass caught only rustfmt drift in the new public
  integration test; `cargo fmt --all` corrected it and the complete rerun was
  green.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p mcp-vault-domain --all-features
cargo test --workspace --all-features
make docs-check
make check
```

Expected results: domain tests cover all WP-01 acceptance bullets; no domain
dependency points at Axum/RMCP/DAV/SQLx; all workspace checks remain green.

Observed results so far:

- `cargo test -p mcp-vault-domain --all-features` passes 21 unit tests and 4
  public integration tests in `crates/domain/tests/safe_paths.rs`.
- `cargo clippy -p mcp-vault-domain --all-targets --all-features --
  -D warnings` passes.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` and
  `cargo test --workspace --all-features` pass with the WP-00 suites intact.
- `make check` and `make build` pass, including frontend install/lint/test/build,
  generated Rust docs, and the locked workspace build.
- `docker build --tag mcp-vault:wp01 .` passes with the new domain dependencies
  and release binary.

## Rollback and recovery

WP-01 changes only pure domain code and dependency lock metadata. Rollback is
reversible by restoring the domain crate files and Cargo manifest/lockfile;
there is no database migration or Vault content mutation. If a future caller
has already compiled against the new API, keep compatibility shims in domain
rather than weakening path validation; no protocol code should duplicate the
parser.

## Outcomes

WP-01 ships the complete protocol-independent Domain foundation: UUIDv7 typed
identifiers, validated Vault slugs and contexts, NFC-normalized safe logical
paths with one-time URL decoding, explicit case-collision policy, reserved
namespace enforcement, default-deny filesystem entry policy, scopes and
permissions, actor/source-plane provenance, monotonic revisions, and typed
write preconditions/errors. The security and data-model documents now state
the chosen path limits and reserved namespace behavior. No SQLite, filesystem
I/O, authentication, or protocol behavior was pulled forward from later work
packages. WP-02 should build the Vault-scoped SQLite state layer on these types.
