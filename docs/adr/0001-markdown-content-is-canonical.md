# ADR-0001: Markdown content is canonical

- Status: Accepted
- Date: 2026-08-19

## Context

MCP Vault must remain compatible with Obsidian, preserve user ownership, and avoid a proprietary note database. At the same time, credentials, revision counters, jobs, and audit records cannot be reconstructed reliably from note files.

## Decision

Current user knowledge is canonical as ordinary files under the Vault content root:

- notes;
- attachments;
- Obsidian configuration;
- active durable memory Markdown.

SQLite is authoritative for operational state such as authentication, configuration, revisions, jobs, and audit.

FTS, embeddings, topic projections, link projections, and automatic extraction candidates are derived and rebuildable.

## Consequences

Positive:

- copying the content root yields a usable Obsidian Vault;
- indexes and AI providers can change without converting user data;
- memory is inspectable.

Costs:

- filesystem/SQLite consistency requires a journal and reconciliation;
- backup must include both content and operational state;
- “database is derived” cannot be used as an excuse to discard credentials, revisions, or audit.

## Rejected alternatives

- Store notes only as database rows and export Markdown on demand.
- Treat every SQLite table as disposable.
- Store durable memory only in a hidden vector database.
