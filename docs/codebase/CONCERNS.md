# Codebase Concerns

## Acquisition Checklist

- [x] Phase 1: Run scan and read intent documents.
- [x] Phase 2: Investigate all seven documentation areas.
- [x] Phase 3: Populate the seven mapping documents.
- [x] Phase 4: Validate evidence sections and known intent/reality differences.

## 1) Top Risks (Prioritized)

| Severity | Concern | Evidence | Impact | Suggested action |
|---|---|---|---|---|
| High | Intent vs reality: the new semantic-memory plan is not implemented. The current product contract forbids generated body rewriting/cross-source consolidation and returns complete original source units; the new plan requires semantic Observations and MemoryCards with per-assertion support. | docs/semantic-memory-implementation-plan.md:24-55,150-210,527-601; docs/product-requirements.md:107-125; docs/architecture.md:524-540; docs/adr/0033-source-preserving-memory-units.md:13-25 | Existing schemas, public memory response semantics, eligibility rules and initialization behavior do not satisfy the new design. | Treat plan as the new baseline; write a superseding ADR and forward-only contract updates. Implement M1 in an isolated namespace before M2/M7 cutover. |
| High | Cross-source card permissions are not represented by current source-set eligibility. Current automatic-unit access is based on source identity/hash; new cards may depend on multiple sources and must fail closed when a required dependency is no longer authorized. | migrations/0028_source_preserving_memory_units.sql:3-30,88-110; docs/semantic-memory-implementation-plan.md:197-210,503-507; docs/security.md:605-635 | A private source could influence a card shown to a reader who cannot read that source. | Make current read eligibility check every required source dependency before async repair; add two-Vault and source-permission tests. |
| Medium | Current public recall/get/list contracts expose complete original units, not semantic card core, qualifiers and explicit evidence expansion. | docs/interfaces.md:601-619; crates/mcp/src/lib.rs:1558-1629; docs/semantic-memory-implementation-plan.md:478-500 | Reusing the old response schema would silently retain the rejected “return source section” behavior. | Keep existing v3 contract untouched while M1 is local; define a versioned card/read-evidence schema and match runtime validation before M5. |
| Medium | Current Admin source setup is Vault-level opt-in, and only Markdown note sources are implemented; conversation/tool-record sources in the new logical model have no adapters. | docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:27-57; crates/server/src/workers.rs:1636-1665; docs/semantic-memory-implementation-plan.md:117-148 | Source eligibility and trust roles may be misrepresented if future kinds are claimed before adapters and authorization exist. | M1 uses the existing Vault-level opt-in with authorized Markdown sources. Mark later SourceKinds pending until implemented. |
| Medium | The plan defines card revisions/support semantics but current persistence is current complete source sets under migration 0028; the two models are not structurally compatible. | docs/semantic-memory-implementation-plan.md:113-233; docs/data-model.md:777-797; migrations/0028_source_preserving_memory_units.sql:1-35 | Modifying old migrations or reinterpreting v3 tables risks upgrade/recovery correctness and violates immutable migration history. | Add a forward-only schema and explicit ownership/rebuildability contract; do not clear or rewrite existing memory data in M0/M1. |

## 2) Technical Debt and Change Surface

| Debt item | Why it matters | Where | Risk if ignored | Safe change strategy |
|---|---|---|---|---|
| Large protocol/composition files | A read-only source line-count command reported 9,141 lines in admin-api, 5,569 in MCP, 4,891 in server workers and 4,353 in the v3 memory service. | Command: rg --files crates frontend/admin/src | xargs wc -l | sort -nr | head -n 20; source files: crates/admin-api/src/lib.rs, crates/mcp/src/lib.rs, crates/server/src/workers.rs, crates/memory/src/v3/service.rs | New semantic paths can become entangled with protocol/runtime code. | Keep application DTO/service and State repository boundaries explicit; isolate new semantic tests. |
| High-churn memory-adjacent surfaces | Recent 90-day scan ranks docs/interfaces.md (19), admin-api/src/lib.rs (14), MCP src/lib.rs (14), server/workers.rs (13), memory-system.md (12) among high churn. | docs/codebase/.codebase-scan.txt:479-499 | Concurrent user changes may be overwritten; public contracts drift easily. | Recheck git status before edits and make narrow file-scoped patches; preserve existing uncommitted changes. |
| No dedicated semantic A/B/C output yet | Existing quality corpus and live examples implement the current v3 selection contract. | crates/memory/tests/fixtures/memory-quality/selection-v3.json; crates/server/examples/live_memory_v3_eval.rs; docs/semantic-memory-implementation-plan.md:683-739 | Engineering fakes could be mistaken for evidence that semantic extraction improves tasks. | Add a synthetic manifest and no-Provider dry-run in M0/M1; keep real semantic acceptance pending until authorized and run. |

## 3) Security Concerns

| Risk | Category | Evidence | Current mitigation | Gap |
|---|---|---|---|---|
| Cross-Vault card/support joins omit a Vault predicate | Access control | docs/security.md:605-635; docs/adr/0002-vault-is-the-isolation-boundary.md:10-30 | VaultContext, composite keys/foreign keys, Vault predicates and two-Vault tests are established requirements. | New tables, cache keys, jobs, embeddings and source-support joins do not exist yet and need matching controls. |
| Private-source contribution leaks through a multi-source card | Authorization/data exposure | docs/semantic-memory-implementation-plan.md:503-507; docs/security.md:605-635 | Existing v3 requires memory:read plus vault:read for automatic source content (docs/product-requirements.md:117-121). | New support-aware card eligibility must enforce permissions before query candidates, navigation, counts or output. |
| Model output or source text mistaken as trusted commands/evidence | Prompt injection / input trust | docs/semantic-memory-implementation-plan.md:247-257,509-513; docs/security.md:462-486 | Existing selection schema limits the model to service-provided IDs (crates/memory/src/v3/selection.rs:95-103). | New Observation parser must validate the evidence IDs/spans and preserve actor/status and conditions. |

## 4) Performance and Scaling Concerns

| Concern | Evidence | Current symptom | Scaling risk | Suggested improvement |
|---|---|---|---|---|
| CI performance test is a bounded health endpoint smoke, not memory-recall throughput | scripts/perf/baseline.sh:4-7,43-83; .github/workflows/ci.yml:93-105 | Scanner found no performance test config; repository does run a small health p95 tripwire. | It cannot predict cost or latency of source extraction, card retrieval or task-pack construction. | Add an isolated synthetic semantic-memory benchmark only when the stage requires it; record host/config. |
| No semantic A/B/C evaluation currently | docs/semantic-memory-implementation-plan.md:630-739; existing v3 fixture path above | No completed comparison is evidenced by current source fixtures or scan. | A new design might pass structure tests while losing task-relevant conditions. | Freeze source/task/model/index/budget manifest; report coverage, support, qualifiers, duplicates, no-answer, task outcome and cost separately. |
| Current extraction batches operate on complete source units and bounded per-source work | crates/memory/src/v3/selection.rs:48-92; docs/architecture.md:524-538 | This bounds old selection input; it does not define limits for semantic observations/cards. | Reusing these bounds without a model/evidence contract can truncate or omit relationships. | Set plan-owned input/output limits and test budget truncation before publication. |

## 5) Fragile/High-Churn Areas

| Area | Why fragile | Churn signal | Safe change strategy |
|---|---|---|---|
| docs/interfaces.md and crates/mcp/src/lib.rs | Public MCP contracts, generated schemas and runtime validation meet here. | 19 and 14 changes respectively in last 90 days (docs/codebase/.codebase-scan.txt:479-495). | Add contract tests for both serialized schema and runtime acceptance/rejection before publishing new fields. |
| crates/memory/src/v3/service.rs and crates/server/src/workers.rs | Current source-set publication and durable job flow are large and heavily coupled to v3. | Source line counts and server worker churn appear in scan output (docs/codebase/.codebase-scan.txt:479-499,504-526). | Add a separate semantic application path and keep v3 serving as the rollback reader until M7. |
| docs/memory-system.md and ADR sequence | The current authoritative contract has already changed several times; ADR-0033 supersedes several old memory decisions. | Scan reports 12 recent changes to memory-system.md; docs/adr/README.md:72-95. | Keep historical rationale immutable, record new decision in the next ADR, and update authoritative document links. |

## 6) [ASK USER] Questions

No unresolved M0 question blocks the authorized work. The maintainer has fixed M1 admission to the existing Vault-level opt-in plus authorized Markdown sources; additional SourceKinds remain later plan work. See intent/reality row 4 above.

## 7) Evidence

- docs/codebase/.codebase-scan.txt:454-526
- docs/semantic-memory-implementation-plan.md:24-55,113-233,247-257,503-523,527-739
- docs/product-requirements.md:105-132
- docs/architecture.md:77-126,522-542,625-654
- docs/adr/README.md:72-95 and docs/adr/0033-source-preserving-memory-units.md:13-39
- docs/security.md:462-516,605-650
- migrations/0028_source_preserving_memory_units.sql:1-35,88-110
