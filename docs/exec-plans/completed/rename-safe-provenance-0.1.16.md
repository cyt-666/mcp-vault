# MCP Vault 0.1.16 rename compatibility and source-path coherence

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-09-01
- Updated: 2026-09-01

## Purpose and user-visible result

MCP and WebDAV file/directory moves must succeed on same-filesystem Unix
mounts that support ordinary atomic rename but reject `RENAME_NOREPLACE`.
After a note is moved, search and memory provenance must expose its current
readable path rather than a stale path that makes a later `read_note` fail.
The release version becomes 0.1.16.

## Governing requirements

- `docs/product-requirements.md` sections 3.3-3.5: current retrieval,
  revision-safe moves, durable sourced memory, and no silent overwrite.
- `docs/architecture.md` sections 4.4 and 7: storage boundaries, journaling,
  locking, derived projections, and recovery.
- `docs/interfaces.md` sections 6.12 and memory source contracts.
- `docs/security.md` section 9.2: descriptor-relative paths and constrained
  hard-link behavior.
- ADR-0002, ADR-0005, ADR-0007, ADR-0011, and ADR-0021.

## Current repository state

- Baseline commit `f6e4f96` contains the reviewed managed multi-Vault work.
- Unix `move_entry` always calls `renameat_with(NOREPLACE)` and exposes an
  unsupported filesystem's `EINVAL` as `InvalidInput`; only private temporary
  regular-file installation currently has a compatibility fallback.
- index rows store a path snapshot and are corrected only by a durable full
  rebuild, while stable `file_entries.id` already survives moves.
- memory source rows retain `note_file_id`, path, and evidence revision, but
  canonical Markdown omits the file ID and the revalidation worker currently
  skips every non-delete event.

## Scope

### Included

- Same-filesystem file and directory move fallback without move hard links.
- Vault-scoped namespace serialization for absent-target claims.
- Current-path resolution for outward note retrieval and memory provenance.
- Source revalidation/repair, backward-compatible canonical `file_id`, docs,
  versioning, tests, and local image validation.

### Not included

- Cross-filesystem copy/delete moves, multi-process shared-root coordination,
  arbitrary hard links, database schema changes, or destructive memory reset.

## Invariants and risks

- Existing destinations must win with a conflict; no service-mediated writer
  may be silently overwritten.
- Namespace and path locks use one acquisition order to avoid deadlock.
- Every lookup/update remains Vault-scoped and stable IDs are never inferred
  from a path in another Vault.
- Evidence revision remains the revision that supports a memory. A path may be
  rebound without claiming changed content still supports it.
- Canonical memory Markdown stays the portable source of durable memories.

## Proposed design

### Components and dependency direction

- Vault Core owns namespace serialization and continues to call storage-fs.
- storage-fs owns descriptor-relative rename capability fallback.
- state/index repositories join stable file state for outward current paths.
- memory owns provenance repair/materialization; workers only translate file
  events into durable memory work.

### Data and transaction flow

1. Absent-target Core operations acquire the Vault namespace lock before
   sorted path locks, validate state/filesystem absence, journal, and commit.
2. Unix storage tries exclusive rename. On an explicit capability error it
   revalidates destination absence and performs ordinary atomic `renameat`.
3. File move metadata commits preserve File ID and emit the existing event;
   index reads resolve that ID through current active `file_entries`.
4. Memory revalidation resolves every source by File ID. Matching current and
   evidence hashes advance path/revision; changed or deleted sole support makes
   extracted memory stale. Changes are materialized through Vault Core.
5. A versioned singleton repair job upgrades existing source rows and managed
   Markdown without Provider calls or a pipeline reset.

### Public interfaces and schema changes

- MCP `move_note` and WebDAV MOVE wire shapes do not change.
- `MemorySourceView.path` means current readable path and is null when absent.
- Canonical `sources[].file_id` is optional; legacy Markdown remains valid.
- No SQLite schema or environment-variable change.

### Failure, retry, and recovery

- `DestinationExists` maps to the public conflict path.
- Non-capability rename errors do not fall back. `EXDEV` remains unsupported.
- The physical fallback is still one atomic rename, so existing move journal
  recovery remains authoritative.
- Source repair is idempotent, Vault-scoped, bounded, and safely retryable.

## Work breakdown

1. Add ADR-0021, namespace locking, rename fallback, and concurrency tests.
2. Rebind outward index paths to current stable file state.
3. Add canonical source IDs, revalidation, runtime resolution, and repair job.
4. Update public/operational documentation and release version to 0.1.16.
5. Run full Rust/frontend/protocol/document/image validation and record results.

## Progress

- [x] `2026-09-01` - Committed reviewed multi-Vault baseline as `f6e4f96`.
- [x] `2026-09-01` - Implemented rename compatibility and namespace locking.
- [x] `2026-09-01` - Implemented index and memory path coherence.
- [x] `2026-09-01` - Completed docs, release artifacts, and validation.
- [x] `2026-09-01` - Reopened after deployment feedback exposed a gated source
  repair admission and a Phase 2 request-index contract failure.
- [x] `2026-09-01` - Made source repair independent of pending automatic-memory
  regeneration, observable in Admin, and able to recover exact historical
  path/revision identities lost by a legacy Markdown projection rebuild.
- [x] `2026-09-01` - Constrained Phase 2 discard indexes per request and made
  generated-output validation consume the durable retry budget.
- [x] `2026-09-01` - Re-ran full validation and replaced the superseded image
  evidence with the final 0.1.16 build.

## Decisions

- Compatibility covers both regular files and directories.
- Ordinary `renameat` is permitted only after the preferred primitive reports
  unsupported and while Core holds the Vault namespace claim lock.
- Move compatibility never hard-links a user entry.
- Current path is resolved by stable File ID; unresolved legacy identity is
  retained as history and never guessed.
- A legacy source lacking File ID may recover through one unique Vault-scoped
  `file_revisions` match on its exact path and evidence revision. Ambiguous
  path reuse remains unresolved rather than guessed.
- Phase 1 keeps processing later notes after an isolated source/model-output
  failure. Phase 2 remains an atomic global batch, but generated bookkeeping
  failures are retryable and the schema only permits request-valid indexes.
- Index results use the current readable path but retain the projection's
  analyzed revision until rebuild, so a stale snippet cannot authorize a write
  against a newer canonical revision.

## Surprises and discoveries

- The pre-implementation baseline passed Rust fmt/clippy/tests and frontend
  lint/test/build. A first non-CI pnpm invocation stopped before tests because
  it wanted an interactive modules-directory confirmation; `CI=true` passed.
- Returning the current file revision with a pre-rebuild snippet would weaken
  optimistic concurrency. Only the path is rebound synchronously; revision and
  content metadata advance together during the durable index rebuild.
- The sandbox denied the real HTTP fixture's local listeners and Docker socket;
  the same reviewed commands passed with those host capabilities enabled.
- Litmus is not installed and no real WebDAV URL/credential was supplied, so
  that external suite remains explicitly blocked rather than claimed.
- The first source-repair admission incorrectly required
  `regeneration_pending = false`, which meant disabled or not-yet-configured
  automatic memory could suppress the task completely.
- The Phase 2 schema allowed any bounded input index while local validation
  accepted discards only from `dirty_input_indexes`; the model could therefore
  produce schema-valid output that the application immediately rejected.
- One first full-workspace run observed the WebDAV forwarded-HTTPS test return
  500 instead of 401. The exact test, the complete WebDAV crate, and a second
  unchanged full-workspace run all passed; this is recorded as a transient
  test observation rather than hidden or attributed to the memory changes.
- An extra `mcp-vault --version` probe returned `unknown mcp-vault command`;
  version reporting is not an implemented CLI command or a release acceptance
  criterion. Cargo build output and immutable image tags identify 0.1.16.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
CI=true pnpm --dir frontend/admin lint
CI=true pnpm --dir frontend/admin test
CI=true pnpm --dir frontend/admin build
bash scripts/check-docs.sh
bash scripts/interop/http-smoke.sh
bash scripts/interop/webdav-litmus.sh
docker build --platform linux/amd64 -t mcp-vault:0.1.16 -t mcp-vault:latest .
docker run --rm --platform linux/amd64 mcp-vault:0.1.16 --check-config
```

Expected results: all available checks pass; Litmus absence is reported as an
environmental blocker rather than claimed as evidence; both image tags resolve
to one non-root linux/amd64 image.

Actual results:

- Rust fmt, Clippy with warnings denied, the second complete
  workspace/unit/integration/doc run, frontend lint/28 tests/build, docs,
  checksums, migration fixtures, and the real HTTP OAuth/MCP/WebDAV smoke
  passed. The first workspace run's transient WebDAV result is recorded above.
- `mcp-vault:0.1.16` and `mcp-vault:latest` both resolve to
  `sha256:d976e85bdf2ad89905addf93d33f700371b79d2dfb2421c6e277efeec0034b15`;
  image metadata is linux/amd64, user `mcpvault`, stop signal `SIGTERM`, and
  read-only-root `--check-config` passed.
- The reference Compose files validate when their documented required example
  variables are supplied.
- Litmus remains blocked by missing binary and live endpoint credentials.

## Rollback and recovery

Revert the 0.1.16 changes to restore native-exclusive moves. There is no schema
migration to roll back. Source repairs are forward-compatible Markdown/row
normalization: older readers ignore optional `file_id`, and evidence content is
not deleted or regenerated.

## Outcomes

MCP Vault 0.1.16 now supports serialized same-filesystem file/directory moves
on mounts that reject exclusive rename, without hard-linking user entries or
weakening service-mediated target conflicts. Search and related-note results
resolve stable IDs to current readable paths before rebuild. Durable memory
sources persist stable IDs, rebind move-only revisions without Provider work,
retain evidence on content changes, hide deleted paths, and repair legacy
projections/Markdown through an idempotent Vault-scoped job. No database schema,
environment variable, or destructive memory-generation migration was added.
