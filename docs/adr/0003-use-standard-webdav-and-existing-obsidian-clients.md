# ADR-0003: Use standard WebDAV and existing Obsidian clients

- Status: Accepted
- Date: 2026-08-19

## Context

Obsidian opens a local folder as a Vault. A remote HTTP URL is not directly opened as a Vault. Existing community plugins already synchronize a local Vault to WebDAV.

Maintaining a custom Obsidian plugin would add desktop/mobile compatibility and release burden unrelated to the core memory service.

## Decision

MCP Vault exposes standard WebDAV and tests existing plugins, primarily Hēsperus Sync Engine’s WebDAV backend and secondarily Remotely Save.

The server integrates WebDAV into the Rust service through a custom DAV filesystem/guard adapter that calls Vault Core. It does not expose the Vault with a separate generic WebDAV process that bypasses revisions, audit, and events.

## Consequences

Positive:

- no custom Obsidian client code;
- standard protocol and other DAV clients remain possible;
- canonical files stay server-visible.

Costs:

- WebDAV semantics and compatibility testing are substantial;
- sync conflict behavior partly belongs to clients;
- client-side WebDAV encryption cannot be used when server indexing is required.

## Rejected alternatives

- Custom Obsidian synchronization plugin/protocol.
- Self-hosted LiveSync/CouchDB as the canonical backend.
- Separate WebDAV container pointing directly at the content volume.
