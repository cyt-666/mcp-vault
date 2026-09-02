# ADR-0011: Managed Memory Canonical Files Use an Explicit Vault Core Boundary

- Status: Accepted
- Date: 2026-08-21

## Context

Promoted durable memories must be ordinary Markdown that an owner can copy,
inspect, edit, and recover. The reserved _mcp-vault namespace is intentionally
hidden from ordinary WebDAV/MCP file operations and from normal Vault scans.
Writing memory files directly from the memory crate would bypass the canonical
write journal, atomic filesystem behavior, revision history, audit, and outbox
boundaries.

The existing Vault Core already exposes an explicit managed read path, but
ordinary write methods reject reserved paths. Memory materialization therefore
needs an explicit write capability without weakening user-path validation or
making managed files appear as ordinary notes.

## Decision

Add managed create/replace operations to Vault Core and storage-fs. They:

- require a validated VaultContext and a path inside the configured reserved
  namespace;
- use the same atomic temporary-file, fsync, journal, revision/history,
  audit, and outbox sequence as ordinary Core writes;
- persist stable file identity/revisions so memory metadata can reference the
  canonical file;
- remain unavailable through ordinary Core, WebDAV, MCP note paths, and
  storage directory listings;
- exclude managed entries from ordinary reconciliation deletion inference and
  Markdown extraction loops;
- allow the memory application service to perform only explicit managed
  operations after its own lifecycle/policy validation.

Memory projection tables remain the authoritative operational index for
memory-specific metadata. Canonical Markdown remains the portable knowledge
copy; deleting/rebuilding projections never deletes it.

Canonical note-source entries include optional stable `file_id`, current/last
known path, and evidence revision. A versioned idempotent repair rewrites legacy
managed records through this same Core boundary; it does not invoke a Provider,
reset the memory generation, or guess an identity from an unavailable path.

## Consequences

Positive:

- memory materialization preserves the canonical write boundary;
- managed files have revision/history and crash-recovery behavior;
- reserved namespace exposure remains explicit and auditable;
- direct projection rebuild can recover metadata from canonical Markdown.

Costs:

- Core/storage APIs have a second explicit managed operation path;
- reconciliation and indexer code must consistently exclude managed entries;
- memory projection and file revision state must be reconciled after crashes.

## Rejected alternatives

- Writing managed Markdown directly with tokio/fs from the memory crate.
- Storing promoted memory only in SQLite or a vector database.
- Exposing the reserved namespace through ordinary WebDAV/MCP path operations.
- Treating every low-confidence extraction candidate as a canonical file.
