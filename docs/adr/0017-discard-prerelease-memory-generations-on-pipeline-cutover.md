# ADR-0017: Discard prerelease memory generations on pipeline cutover

- Status: Accepted
- Date: 2026-08-26
- Amends: ADR-0016 migration behavior

## Context

MCP Vault is still under active prerelease development. The first deployment
of ADR-0016 reused the durable job type `memory.extract` from the preceding
quote-as-memory design. Because persisted jobs carried no architecture
generation, an old full-Vault extraction at cursor 57 was reclaimed by the new
Phase 1 handler and continued at cursor 58. The former migration also attempted
to preserve explicit memories and could mistake any active extraction
singleton for the required clean regeneration job.

That compatibility behavior is harmful during development. A job cursor,
payload, candidate, raw input, proposal, or final memory produced by a replaced
architecture is not evidence that the new architecture has processed the same
source. Reinterpreting it can skip notes, apply obsolete validation rules, and
cause paid Provider calls under misleading progress.

Ordinary Vault notes remain the canonical source and can regenerate the memory
system. Managed memory Markdown has normal revision history and may exist in
backups, but its current prerelease representation does not need to survive an
architecture cutover.

## Decision

Until MCP Vault declares a stable memory-storage compatibility contract, every
memory architecture generation change is an explicit destructive cutover:

1. a forward migration deletes all persisted `memory.*` jobs;
2. every final memory projection, Stage 1 output, consolidation proposal,
   candidate, diagnostic, idempotency result, FTS row, and derived memory
   vector from the replaced generation is deleted;
3. a durable Vault-scoped reset job deletes every current managed file below
   `_mcp-vault/memory/` through Vault Core, retaining ordinary Core history;
4. empty current-generation aggregate/raw artifacts are recreated;
5. one fresh full-Vault Phase 1 job starts without an inherited progress
   cursor; and
6. all new memory jobs carry `pipeline_generation`. The Worker cancels a
   missing or mismatched generation before handler or Provider invocation.

The cutover never deletes ordinary notes, attachments, their revision history,
Provider/model configuration, credentials, audit records, backups, or
non-memory jobs. It does not attempt to preserve explicit Agent/Admin memories;
they belonged to the replaced prerelease generation and must be resubmitted if
still desired.

The extraction prompt/fingerprint version remains a separate number. It may
cause incremental re-evaluation within one compatible architecture generation;
`pipeline_generation` marks a larger incompatible state/job boundary.

## Consequences

- Upgrades during prerelease can intentionally erase current long-term memory
  output and incur a complete extraction/consolidation cost.
- Old memory jobs cannot silently run under new code, including jobs restored
  from a snapshot or manually retried.
- Recovery becomes simpler: current notes are the only migration input, and a
  new Phase 1 job always starts at note one.
- Managed memory file deletion remains auditable/recoverable through Vault Core
  history even though current projections and job history are discarded.
- Admin must clearly report that the old memory system is being discarded, not
  “migrated” or preserved.
- Before a stable release, the project must define an explicit compatibility,
  export, and upgrade policy in a new ADR rather than silently retaining this
  prerelease rule.

## Rejected alternatives

### Infer compatibility from `job_type` or payload shape

Rejected because the observed defect came from exactly that ambiguity. Missing
generation is obsolete, not a format to guess.

### Preserve explicit memories but delete extracted memories

Rejected for the current prerelease cutover. It leaves two semantics in one
generation and retains inputs shaped by superseded validation/promotion rules.

### Delete database rows but leave managed memory Markdown current

Rejected because a later projection rebuild could re-import obsolete memory.
Current managed files are removed through Vault Core as part of the same
recoverable cutover.
