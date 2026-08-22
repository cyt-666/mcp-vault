# ADR-0009: Archive overwritten WebDAV tombstones inside Vault Core

- Status: Accepted
- Date: 2026-08-20

## Context

The `dav-server` COPY/MOVE implementation applies `Overwrite: T` by deleting
the destination before invoking the project filesystem adapter. MCP Vault
retains deleted `file_entries` rows as tombstones for revision and history
retention, while the schema intentionally keeps `(vault_id, path)` unique.
Those constraints conflict with a MOVE that must preserve the source
`file_id`: the deleted destination row cannot remain at the destination while
the source row is moved there.

## Decision

When Vault Core commits an overwrite MOVE and finds a prior destination
tombstone, it moves that tombstone row to a deterministic path below the
Vault's reserved operational namespace in the same SQLite metadata
transaction. It then updates the source row to the requested destination and
records the normal MOVE revision, audit entry, and outbox event. The archive
path is never exposed by ordinary WebDAV listings or user-path validation.

Directory MOVE prepares one journaled operation per tracked descendant, so
the same rule applies to every overwritten destination under the tree.

## Consequences

- Source File IDs remain stable across WebDAV rename/overwrite operations.
- The existing unique path constraint and deletion history model remain
  unchanged; no migration is required for this behavior.
- The old destination's revisions remain attached to its File ID for recovery
  and audit, but the overwritten user path now resolves to the moved source.
- Recovery can finalize or roll back each prepared directory-entry operation
  from the durable journal after a process interruption.

## Rejected alternatives

- Drop the destination tombstone and lose its revision foreign-key history.
- Reuse the destination File ID and turn MOVE into an implicit delete/create.
- Remove path uniqueness with a schema migration before the protocol adapter
  is available.
- Bypass Vault Core and let the DAV adapter mutate the filesystem directly.
