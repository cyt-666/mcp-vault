# ADR-0029: Actionable compact MCP tool results

- Status: Accepted
- Date: 2026-09-06
- Authority: explicit user request to fix all tool descriptions, parameters and context cost

## Decision

MCP tools default to compact presentation retaining the information needed to choose
the next action and perform safe writes. All 17 tools accept include_details for the
extended previous metadata; get_memory remains a full single-record drill-down.
Search/recall score diagnostics also select detailed presentation. Service and Admin
records remain unchanged; protocol presentation contains no business logic.

Recall includes source-note paths by default, with explicit include_sources=false
opt-out. A source-note path is distinct from managed-memory canonical_path. Descriptions
state exact field-to-argument transitions and preserve permissions and revision safety.
Unsupported parameter enum choices are hidden from discovery but old calls retain
explicit unsupported errors. History responses are paginated. The SDK text mirror and
structuredContent remain compatible representations of one logical result.

## Consequences

Clients needing extended fields must request include_details. Tool descriptions may be
cached by hosts, requiring reconnection to refresh discovery. Compact output does not
prove an Agent will follow instructions; protocol tests prove the actionable navigation
path, while actual Host/model behavior remains a separate observation. No migration,
canonical rewrite, query-time generation, or permission expansion is introduced.
