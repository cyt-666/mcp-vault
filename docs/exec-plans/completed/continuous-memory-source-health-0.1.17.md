# MCP Vault 0.1.17 continuous memory source health

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-09-01
- Updated: 2026-09-02

## Purpose and user-visible result

Replace the one-time legacy source repair with a durable, event-driven source
health lifecycle. Normal recall must fail closed as soon as a note-dependent
memory has no verified current note source. Exact source recovery may reactivate
stale memory; semantic change proceeds through Phase 1/Phase 2; unsupported
memory remains inspectable but is not normally recalled. Admin must report exact
final-source, memory, Stage 1, and distinct-file counts instead of one mixed
"unresolved" number.

The release version becomes 0.1.17.

## Governing requirements

- Product requirements sections 3.5, 3.7, 3.9, and 4.1: sourced memory,
  event-driven automatic work, durable jobs, and restart-safe integrity.
- Architecture sections 4, 7, 9, and 11: Vault-scoped repositories,
  transactional outbox, derived health state, and two-phase memory.
- `docs/memory-system.md` lifecycle, provenance, and normal-recall contracts.
- ADR-0002, ADR-0007, ADR-0011, ADR-0016, ADR-0017, and the new continuous
  source-health ADR created by this plan.

## Current repository state

- The working tree contains the uncommitted but fully tested 0.1.16 rename and
  source-path repair; baseline commit `f6e4f96` contains managed multi-Vault.
- `memory.repair_sources:v2` runs once and combines unresolved final-memory
  source rows with unresolved unique Stage 1 File IDs.
- File update/delete/restore/move events enqueue `memory.revalidate` by FileId;
  FileCreated does not, missing identities are not reconsidered, and stale
  memories do not reactivate.
- Normal recall filters status but has no source-health/hash freshness
  predicate. Admin may currently restore stale memory without source proof.

## Scope

Included: migration 0013, source health/status reasons, event reconciliation,
exact cross-FileId proof, repeatable audits, Stage 1 withdrawal, affected-stale
Phase 2 decisions, recall fail-closed behavior, Admin APIs/UI, docs, tests, and
0.1.17 image validation.

Excluded: semantic/vector identity matching, automatic permanent deletion,
new environment variables, cross-Vault matching, and remote image publication.

## Invariants and risks

- Every health query/update remains Vault-scoped; cross-Vault candidates cannot
  contribute to uniqueness.
- Health is derived/rebuildable. Canonical memory content, lifecycle status,
  status reason, and provenance stay portable Markdown.
- Cross-FileId relinking requires one exact Vault-wide candidate: normalized
  full-note evidence, or exact anchored excerpt evidence. Ambiguous/truncated
  searches never bind.
- Any memory containing note sources requires at least one verified current
  note source, regardless of origin. Explicit memory without a note source is
  self-supported.
- Stale/archived/superseded records are never automatically deleted.
- Source reconciliation finishes before extraction admission for one event, so
  stale/update decisions cannot race into duplicate active memories.

## Proposed design

Migration 0013 adds `memory_source_health`, Vault/source/file/status indexes,
and optional memory `status_reason`/`status_changed_at`. Health records contain
resolution state, resolved current identity/path, verified file revision/hash,
last event, and check timestamp. Legacy note sources begin `unverified`.

Normal recall requires active status plus one `current` source whose verified
file hash still equals active `file_entries.content_hash`. A move with unchanged
hash remains available; changed/deleted/unverified sources fail closed before
the asynchronous worker finishes. Historical recall bypasses this eligibility
predicate deliberately.

New `memory.source_reconcile` jobs consume every file create/update/move/delete/
restore/external event. They reconcile final sources first, recalculate memory
lifecycle, then withdraw/rebind Stage 1 and admit extraction/consolidation.
Queued legacy `memory.revalidate` jobs remain supported during upgrade.

Exact same-FileId evidence is preferred. Missing identity may cross to a new
FileId only when one active Vault candidate exactly matches normalized full-note
evidence or the stored heading/line anchor plus excerpt hash. Stale records with
`source_unavailable` reactivate only on exact proof; archived/superseded records
stay historical. Phase 2 receives affected stale memories and must update,
supersede, or archive them.

A repeatable paged `memory.audit_sources` runs after initial/full Vault
reconciliation and restore, and via Admin. Its dedup key uses an audit
generation, not a permanent repair version. Admin exposes summary/detail and a
manual trigger. MCP source views add optional health metadata; existing request
shapes remain unchanged.

## Work breakdown

1. Add ADR, migration 0013, typed source-health/status-reason models,
   repositories, indexes, and migration tests.
2. Implement exact evidence verification, event source reconciliation,
   lifecycle recomputation, and deterministic extraction admission.
3. Add recall eligibility, stale reactivation/restore validation, affected
   stale Phase 2 input, and Stage 1 withdrawal/rebind behavior.
4. Add repeatable audit jobs, Admin summary/detail/trigger, precise progress,
   source-health UI, and remove misleading generic note-count copy.
5. Update specs/version/checksums and run full release validation.

## Progress

- [x] `2026-09-01` - Repository and lifecycle/event gaps inspected.
- [x] `2026-09-01` - Product decisions locked: immediate stale, exact unique
  cross-FileId relink, note-dependent explicit memory also stale, initial audit
  fail-closed, and no semantic matching/deletion.
- [x] `2026-09-02` - Migration 0013, stable source upsert, health/audit
  repositories, lifecycle reasons, and 0.1.16 upgrade coverage complete.
- [x] `2026-09-02` - Event-ordered source reconciliation, real-time recall
  fail-closed checks, exact unique cross-ID recovery, stale reactivation, and
  Stage 1 rebinding/withdrawal complete.
- [x] `2026-09-02` - Phase 2 affected-stale handling plus Admin source-health
  summary/detail/audit API and UI complete.
- [x] `2026-09-02` - Full Rust, frontend, migration, documentation, checksum,
  real HTTP, and linux/amd64 image validation complete. Litmus is explicitly
  blocked by the missing client and live endpoint credentials.

## Decisions

- Do not create `repair:v3`; preserve v2 only as historical job evidence.
- Do not add a `revalidating` memory status. Source-health/hash eligibility
  provides immediate fail-closed recall while existing lifecycle values remain
  compatible.
- Automatic cross-ID recovery uses exact evidence only and requires global
  Vault uniqueness.
- Explicit memories with note sources follow the same source-health rule;
  source-less explicit assertions remain active.
- First upgrade audit hides unverified note-dependent memory rather than
  trusting legacy active status.

## Surprises and discoveries

- The 0.1.16 unresolved count mixes every lifecycle's final note-source rows
  with unique inactive Stage 1 File IDs and can double-count one logical file.
- FileUpdated currently admits extraction and revalidation independently,
  creating an ordering race around stale memory and Phase 2.
- The existing Admin stale restore path does not verify source support.
- The initial pnpm command attempted dependency self-repair inside the network-
  restricted sandbox. The lockfile-backed node_modules tree was restored from
  the local content-addressed store with approved install scope; the exact
  pnpm lint/test/build commands then passed.
- Multi-line SQL added during implementation briefly retained patch-prefix
  characters inside string literals. The new migration test exposed the issue;
  all affected State SQL strings were mechanically cleaned and the migration
  test now passes.
- The first HTTP smoke attempt was blocked by the managed sandbox's loopback
  listener policy. The identical command passed outside that sandbox; this was
  an execution-environment restriction rather than a protocol failure.
- The local machine does not have the upstream `litmus` client installed and no
  live release WebDAV credentials were supplied, so Litmus remains a recorded
  interoperability blocker rather than a passing check.

## Validation

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
CI=true pnpm --dir frontend/admin lint
CI=true pnpm --dir frontend/admin test
CI=true pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
bash scripts/release/check-migrations.sh
bash scripts/interop/http-smoke.sh
bash scripts/interop/webdav-litmus.sh
docker build --platform linux/amd64 -t mcp-vault:0.1.17 -t mcp-vault:latest .
docker run --rm --platform linux/amd64 mcp-vault:0.1.17 --check-config
```

Litmus absence or missing live credentials must be recorded as a blocker, not
claimed as evidence.

Actual results:

- Rust formatting, Clippy with warnings denied, the complete workspace unit,
  integration, and documentation test suite, frontend lint/28 tests/build,
  documentation checks, migration fixtures 0009 through 0013, and checksums
  passed.
- The real HTTP smoke passed the built-in OAuth code/PKCE/offline-refresh flow,
  duplicate-refresh grace, authorization CSP, token Origin compatibility,
  metadata/discovery, Origin rejection, MCP routing, 50 concurrent WebDAV PUTs,
  revision preconditions, and Admin/data-plane separation.
- `mcp-vault:0.1.17` and `mcp-vault:latest` both resolve to
  `sha256:36709efe92bfce63bc1ac80493bdb6f6d29652946f5c0fe02c6fa2c67c301344`.
  The image is linux/amd64, runs as `mcpvault`, uses `SIGTERM`, and passed the
  read-only-root `--check-config` invocation.
- WebDAV Litmus is blocked because the local client and live endpoint
  credentials are unavailable. No remote image was published.

## Rollback and recovery

Rolling back the binary leaves additive migration 0013 tables/columns unused;
no canonical note or memory body is removed. Reconciliation/audit jobs are
idempotent and restart from durable job state. A failed initial audit leaves
unverified note-dependent memories safely absent from normal recall.

## Outcomes

MCP Vault 0.1.17 now continuously reconciles memory provenance from every
relevant file event and from repeatable paged audits. Note-dependent memory
fails closed until at least one source is proven current; exact same-Vault
evidence may safely rebind a changed FileId and reactivate only
`source_unavailable` stale memory. Content changes flow through Phase 2, while
deleted, missing, ambiguous, archived, and superseded history remains
inspectable without leaking into normal recall. Admin reports final sources,
affected memories, Stage 1 rows, and distinct file identities separately. The
upgrade is additive, Vault-scoped, compatible with legacy Canonical Markdown,
and requires no new environment variable or destructive memory reset.
