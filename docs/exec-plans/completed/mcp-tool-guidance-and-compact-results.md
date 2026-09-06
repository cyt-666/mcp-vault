# MCP tool guidance and compact results

Owner: Codex. Date: 2026-09-06. Status: completed.

## Purpose / governing requirements
Audit all 17 tools for parameter meaning, next action and context cost. User explicitly
requested all-tool fixes. Governed by product requirements retrieval/controlled writes,
architecture protocol boundaries, interfaces sections 4–6, security Vault isolation,
ADR-0026 and amended ADR-0028. No business or canonical-state policy changes.

## Current state / scope
HEAD b5d4da0, clean worktree. MCP handlers expose full service records, including
internal IDs and graph metadata; recall strips sources by default. Descriptions still
mention the removed evaluation gate. Some advertised enum values are unsupported.
All 17 tool descriptions and input shapes, result presentation, discovery instructions,
public tests and documentation are in scope. No real model calls or deployments.

## Design / decisions
Default compact tool DTO presentation; include_details opt-in preserves full existing
metadata, with get_memory already a detailed single-record read. Preserve source paths,
revision preconditions, validity/confidence, cursors, truncation/degradation and deletion
side effects. recall include_sources defaults true; false is explicit opt-out. Do not
confuse managed memory canonical_path with readable source-note paths. Keep structured
and text compatibility output as required by SDK/MCP; reduce the shared payload.

## Invariants / risks / recovery
Do not bypass Vault/read permissions, expose history through recall, mutate LLM output,
or drop conflict/revision safety. Internal Admin service DTOs remain unchanged. No DB
migration. Rollback code only; clients relying on extended fields use include_details.

## Progress
- [x] Inventory all 17 tools and their DTO/description/return builders.
- [x] Implement consistent compact presentation and accurate input/action guidance.
- [x] Verify default source -> read_note, opt-outs, details, metadata, negative cases.
- [x] Run checks, measure payload reduction, document per-tool findings/results.

## Validation / outcomes
All 17 tools audited and descriptions rewritten; compact presentation with extended
compatibility mode, default source navigation, hidden unsupported enum choices,
history pagination/depth-0 semantics and historical read metadata repaired. No
business state or permission changes. ADR-0029 records the wire-contract decision.

327 all-feature workspace tests and workspace Clippy passed. Four official protocol
versions passed existing baseline without new exceptions. Public tests exercise all
17 routes via OAuth and the new direct source navigation/default/details/opt-out,
selected historical metadata and history pagination. A single recall sample measured
262 compact bytes versus 911 detailed bytes. SDK text mirror retained for compatibility.

See [all-tool report](../reports/mcp-tools-compact-results-20260906.md) for per-tool
findings, exact commands, failure corrections and limitations. Compiled server ready;
no production deployment, model calls or commit. Hosts must refresh tool discovery.

