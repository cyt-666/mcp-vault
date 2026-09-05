# ADR-0022: Continuously verify memory sources with exact Vault-scoped evidence

- Status: Superseded by ADR-0026
- Date: 2026-09-02
- Amends: ADR-0007 and ADR-0016
- Replaced by: ADR-0026 stable File-ID/source-hash current-set validation.

## Context

Stable File IDs and one upgrade repair made ordinary renames recoverable, but
they did not define a continuing source-health contract. A memory could remain
`active` between a file mutation and a delayed job, explicit memories with note
sources did not follow the same lifecycle rule, stale memory could not recover,
and an old one-time repair mixed final-memory sources with Stage 1 rows in one
unactionable unresolved count.

Path matching alone is unsafe after deletion and recreation. Semantic, vector,
or LLM similarity is also not evidence of file identity. At the same time,
silently deleting unsupported memory would destroy useful history and make
Provider outages destructive.

## Decision

MCP Vault maintains a rebuildable, Vault-scoped `memory_source_health`
projection for every final note source. Its states are `unverified`, `current`,
`content_changed`, `deleted`, `identity_missing`, and
`identity_ambiguous`. A current row records the resolved File ID and path, the
checked revision, and the raw current file hash accepted by the exact evidence
check.

Every memory containing at least one note source requires at least one current
note source to remain active, regardless of whether its origin is extracted,
Agent, Admin, import, or managed Markdown. Explicit Agent/Admin memories with
no note source remain supported by the explicit assertion itself. When the last
current note source disappears, the memory becomes `stale` with
`status_reason: source_unavailable` and immediately leaves normal recall.
Historical recall remains available.

Normal recall additionally checks the live `file_entries.content_hash` against
the health row. This closes the interval between a file update and asynchronous
job completion. Unverified sources after upgrade fail closed.

File create, update, move, delete, restore, and external-change events enqueue
`memory.source_reconcile`. The reconciler commits source health and lifecycle
before it admits optional Phase 1 work. A same-File-ID move with unchanged
evidence updates navigation only and invokes no Provider.

Cross-File-ID relinking is permitted only inside the same Vault and only when
exact evidence has one candidate:

- a whole-note source requires the normalized full-note hash;
- an excerpt requires the same line anchor, optional heading path, and excerpt
  hash;
- zero candidates remain missing, multiple candidates remain ambiguous, and a
  truncated scan never binds.

No vector search, semantic similarity, filename guess, or LLM judgment may
establish identity. Exact recovery may reactivate only stale memories whose
reason is `source_unavailable`. Archived and superseded memories never revive
automatically. Unsupported memories are retained, never automatically deleted.

`memory.audit_sources` is repeatable and paged. Its deduplication key includes
the reconciliation or Admin audit generation. It runs after full Vault
reconciliation and post-restore reconciliation. Final sources, affected
memories, Stage 1 sources, and distinct File IDs are counted separately.

Phase 2 receives stale memories related to dirty sources and must update,
archive, or supersede each one. A Provider outage leaves the already stale row
safe and inspectable rather than restoring or deleting it.

## Consequences

- Normal recall may temporarily return less after upgrade until the first
  source audit proves legacy note evidence.
- File mutation safety no longer depends on automatic-memory configuration or
  Provider availability.
- Source health is rebuildable operational state, while lifecycle reason and
  provenance remain portable in canonical Markdown.
- Exact candidate scans are bounded. Event handling uses stable File IDs and
  the changed file first; Vault-wide candidate loading is reserved for audits
  and actual cross-identity recovery.
- Source replacement and lifecycle materialization add work to file events,
  but they eliminate stale paths and duplicate active memory caused by races.

## Rejected alternatives

### Keep a one-time repair job

Rejected because every later file mutation can invalidate or restore evidence.

### Match by path, filename, vector similarity, or LLM judgment

Rejected because none of those proves identity and duplicate content can exist
legitimately.

### Delete stale memory automatically

Rejected because stale history remains useful for audit and later exact or
Phase 2 recovery.

### Let Provider availability control source invalidation

Rejected because recall safety must remain deterministic and local.
