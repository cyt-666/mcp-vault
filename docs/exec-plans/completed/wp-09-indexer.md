# WP-09 Markdown Index, FTS, Links, and Knowledge Map

- Status: Complete
- Owner/Agent: Codex
- Created: 2026-08-21
- Updated: 2026-08-21

## Purpose and user-visible result

Replace the WP-08 filesystem-only browse fallback with a rebuildable Markdown
projection. An authenticated MCP client will be able to browse a deterministic
folder/tag/link knowledge map and search exact source material lexically,
without an LLM, embedding provider, or unbounded Vault scan at request time.

## Governing requirements

- `AGENTS.md`: Markdown/filesystem content remains canonical; indexes are
  derived and rebuildable; all queries are Vault-scoped; protocol handlers do
  not execute SQL or filesystem I/O; query-time search must not scan the Vault.
- `docs/product-requirements.md` sections 3.2, 3.3, 3.9, 4.1-4.5, and 6:
  compact discovery, FTS/search, links/backlinks, bounded retrieval,
  provider-independent lexical behavior, rebuildability, and integrity.
- `docs/architecture.md` sections 3, 4.5-4.6, 6, 9-10, 13, 15-17:
  derived-state durability class, repository boundary, index services,
  transactional outbox/jobs, deterministic and semantic layers, and
  dependency direction.
- `docs/interfaces.md` sections 4.3-4.4, 6.1-6.5, 7-8, and 10:
  discovery/browse/search contracts, bounded snippets/cursors, resource links,
  structured errors, private cache metadata, and Admin index status/rebuild
  seams.
- `docs/data-model.md` sections 1, 10-12: reserved taxonomy path, note/
  heading/tag/link projections, FTS5, knowledge-map nodes/memberships, and
  rebuildability.
- `docs/security.md` sections 4, 9, and 11-13: Vault binding, untrusted
  Markdown/frontmatter/link parsing, parser limits, no plugin execution, and
  derived summaries not becoming authorization facts.
- `docs/development-and-testing.md` sections 5-8, 11-13: AST parsing,
  multilingual fixtures, code-block false-positive coverage, FTS reconciliation,
  rebuild tests, and bounded/index coverage checks.
- Accepted ADR-0001, ADR-0002, ADR-0005, ADR-0006, and ADR-0008: portable
  Markdown canonical state, Vault isolation, modular monolith, optional
  enrichment, and endpoint-bound MCP authorization.

## Starting repository state

- `crates/indexer/src/lib.rs` was a documentation-only stub even though
  `comrak` was pinned.
- `crates/state` had authoritative file/revision/job/outbox repositories but
  no note, heading, tag, link, FTS, index-node, or coverage tables.
- Migrations stopped at `0004_background_processing.sql`.
- WP-08 MCP `browse_index` and `vault_overview` used bounded Vault Core/state
  metadata, while `search_notes` was intentionally not advertised.
- Server startup performed Core reconciliation and generic outbox-to-job
  admission, but no index job handler or index coverage integration existed.
- `VaultCore::read` rejected the reserved namespace; taxonomy reads therefore
  required an explicit managed-read Core seam rather than direct storage access.

## Scope

### Included

- Add immutable migration `0005_index_projections.sql` for note metadata,
  headings, tags, links, FTS5, knowledge-map nodes/memberships, and per-Vault
  index coverage/status.
- Add Vault-scoped state repository records and transactional replacement/query
  methods. FTS writes and ordinary projection rows are reconciled together;
  SQL remains inside `crates/state`.
- Implement a bounded Comrak-based Markdown analyzer with frontmatter,
  headings/source anchors, aliases, tags outside code, Markdown links,
  Obsidian wikilinks, plain text, title, first paragraph, and content hash.
- Implement safe `_mcp-vault/index.yaml` taxonomy parsing through an explicit
  managed Core read, with bounded YAML input/depth and deterministic overlay.
- Implement `IndexService` rebuild/index/remove/query operations. It reads
  canonical bytes through Vault Core, persists derived projections through
  state, and never makes the index the source of truth.
- Integrate initial/reconciliation indexing and deterministic job/event seams
  without introducing provider or memory behavior prematurely.
- Add MCP lexical `search_notes`, switch overview/browse to the index service,
  include index revision/coverage and links/backlinks metadata, and preserve
  Vault-scoped authorization/private caching. Search falls back only within
  lexical/indexed behavior; semantic search remains WP-10.
- Add unit, multilingual parser, projection, FTS, link-resolution, rebuild,
  external-edit, bounded-query, and two-Vault isolation tests.

### Not included

- Embeddings, semantic/hybrid ranking, reranking, provider calls, or vector
  storage (WP-10).
- Durable memory extraction, memory recall, memory FTS, or memory resources
  (WP-11).
- Admin CRUD/UI implementation (WP-12); only state/service seams needed for
  later index status/rebuild routes are added.
- Automatic note movement/reorganization, taxonomy mutation through MCP, or
  an index-only copy of note content.

## Invariants and risks

- Every projection row, FTS row, node, membership, status record, query, and
  rebuild job is bound to the request or job's `VaultContext`.
- Canonical Markdown bytes remain under the Vault filesystem; deleting all
  derived rows must leave notes and history untouched.
- Indexing is asynchronous/rebuildable and never runs inside the canonical
  mutation transaction. A failed projection cannot roll back a successful
  file write.
- Query paths use FTS/state indexes with bounded limits/cursors and never walk
  the Vault or read note bodies on demand.
- Comrak AST traversal skips code blocks/inline code for tag/link extraction;
  links remain unresolved when safe Obsidian resolution cannot prove a target.
- Frontmatter/taxonomy parsing is size/depth/key bounded, treats content as
  data, and never executes Dataview, HTML, plugins, YAML aliases, or shell.
- FTS5 has no ordinary foreign-key enforcement; replacement, rebuild, and
  reconciliation tests must detect stale/orphaned rows.
- Taxonomy and index summaries are derived metadata and cannot grant access or
  change write permissions.

## Proposed design

```text
Core read(VaultContext, path)
  -> MarkdownAnalyzer (Comrak + bounded Obsidian extraction)
  -> IndexService command
  -> State IndexRepository transaction
       notes/headings/tags/links + FTS5 + map memberships + coverage

MCP request
  -> authenticated VaultContext
  -> IndexQueryService / State IndexRepository
  -> bounded overview, browse, or FTS result
```

`crates/indexer` owns analyzer/domain DTOs and the application service. It may
depend on `vault-core` and `state`; lower layers do not depend on the protocol
crate. `crates/state` owns all SQL and migration-specific row conversion.

The analyzer uses Comrak source positions (byte columns), enables frontmatter
and both Obsidian wikilink title forms, and derives stable heading paths. A
small bounded YAML decoder handles only taxonomy fields needed by the data
model (`topics`, `include`, `exclude`, `pinned`, `aliases`, `description`);
unknown fields are ignored as data, not executed.

The index service replaces one file's projection atomically in SQLite and
updates index coverage/revision. A full rebuild clears only derived rows for
one Vault, walks active file metadata through the repository, reads Markdown
through Core, and commits deterministic projections. Initial startup,
reconciliation, and durable `index.rebuild` jobs invoke the service.

Search input is converted to a quoted FTS5 AND query so user punctuation
cannot inject FTS operators. Optional path prefix, tag, topic, and modified
time filters are parameterized. Results include file/revision/title/path,
bounded snippets, heading/source anchor, tags/topic IDs, and resource URI.

## Work breakdown

1. Read the governing documents, inspect the stub and existing state/worker/
   MCP seams, and create this plan.
2. Add migration/repository records and tests for Vault-scoped projection
   replacement, FTS rows, index status, search filters, and isolation.
3. Implement the bounded Comrak/frontmatter/Obsidian analyzer and taxonomy
   parser with multilingual/code-block/false-positive fixtures.
4. Implement `IndexService` rebuild/index/remove/query and explicit managed
   taxonomy reads; test projection deletion/rebuild and external edits.
5. Integrate startup/reconciliation indexing and add deterministic index job
   admission/handler seams without blocking canonical writes.
6. Update MCP overview/browse and add `search_notes`, resource links, index
   revision/coverage, cursor validation, and public protocol tests.
7. Update docs/checksums, run all relevant Rust/frontend/docs checks, record
   conformance availability, and move this plan to completed.

## Progress

- [x] 2026-08-21 — Re-read root instructions and ordered product,
  architecture, implementation-plan, and PLANS documents; inspected WP-08,
  indexer stub, data model, Comrak, migrations, state, workers, and MCP seams.
- [x] 2026-08-21 — Create WP-09 ExecPlan before implementation.
- [x] 2026-08-21 — Add migration and Vault-scoped state projection/query
  repositories, including FTS, link metadata, backlinks, related-note
  scoring, and composite Vault foreign keys.
- [x] 2026-08-21 — Implement bounded Markdown/frontmatter/Obsidian and
  taxonomy analysis with multilingual/code-block fixtures.
- [x] 2026-08-21 — Implement IndexService rebuild/index/remove/query operations,
  managed taxonomy reads, coverage status, link resolution, and rebuild tests.
- [x] 2026-08-21 — Integrate startup/reconciliation indexing and durable
  `index.rebuild` worker admission/handler seams.
- [x] 2026-08-21 — Integrate indexed overview/browse and lexical
  `search_notes` into MCP with public protocol tests.
- [x] 2026-08-21 — Run frontend/docs/checksum checks and archive this plan.

## Decisions

- Use SQLite FTS5, already required by the data model, behind state repository
  methods; do not add a second search database or make FTS the canonical copy.
- Use Comrak AST/source positions plus project-owned Obsidian extraction rather
  than global regular expressions, so code blocks and inline code cannot create
  false tags/links.
- Use `yaml-rust` only for bounded taxonomy/frontmatter decoding; preserve raw
  frontmatter as JSON projection data and fail closed on malformed/oversized
  managed taxonomy without affecting canonical notes.
- Rebuild and replacement are Vault-scoped and derived-only. A rebuild can be
  retried or run after projection deletion without changing file revisions.
- Reserved taxonomy access is explicit through `VaultCore::read_managed` and
  `VaultStorage::open_read_managed`; ordinary user/WebDAV/MCP note reads still
  reject the managed namespace.
- File outbox delivery admits a Vault-scoped `index.rebuild` job while
  retaining the original `outbox.event` job for later consumers. Job payloads
  contain only bounded event metadata and the handler never logs note content.

## Surprises and discoveries

- Comrak source positions require arena-lifetime signatures during recursive
  AST traversal; the analyzer keeps those lifetimes local and does not store
  AST nodes.
- The ordinary Core read rejects the reserved namespace; taxonomy access now
  uses a deliberate managed-read Core boundary.
- File outbox delivery remains durable as the ordinary event job and
  additionally admits an index rebuild job; the index handler is independent
  of provider and memory workers.

## Validation

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p mcp-vault-indexer --all-features
cargo test -p mcp-vault-state --all-features
cargo test -p mcp-vault-mcp --all-features
cargo test -p mcp-vault-server --all-features
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
```

Official MCP conformance and external Obsidian interoperability remain
environment-dependent release checks; record exact availability rather than
claiming them from local unit tests.

## Rollback and recovery

Migration `0005_index_projections.sql` is forward-only. Removing derived rows
or disabling the index worker does not remove canonical content or history.
If projection rebuild fails, coverage/status remains non-complete and the job
is retryable; MCP search returns a safe unavailable/degraded result rather than
scanning the Vault. A later full rebuild reconstructs notes, links, FTS, and
knowledge-map rows from Core reads.

## Outcomes

- Added migration `0005_index_projections.sql` with rebuildable
  notes/headings/tags/links/FTS/map/status projections and composite
  Vault-scoped foreign keys.
- Added bounded Comrak/yaml-rust analysis, deterministic folder/tag/manual
  topic nodes, link resolution, backlinks, related-note scoring, and
  taxonomy validation.
- Added Core-managed reads, startup/periodic/job rebuild integration, and
  stateless MCP indexed browse/search with private cache scope and lexical
  degradation reporting.
- Unit, integration, two-Vault isolation, rebuild-after-deletion, and public
  MCP search tests pass. Frontend/docs/checksum validation is recorded in the
  final handoff before archiving this plan.
