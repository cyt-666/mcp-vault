# Architecture

## 1) Architectural Style

- Primary style: modular monolith with one Rust server binary and an embedded Admin frontend.
- Why: Cargo packages domain and infrastructure responsibilities into separate crates while crates/server composes both HTTP planes and durable workers in process.
- Primary constraints: VaultContext is required for Vault data; protocol adapters call application services; canonical Markdown and attachments remain filesystem assets while SQLite owns operational state.
- Evidence: docs/architecture.md:3-31,77-126; docs/architecture.md:625-654.

## 2) System Flow

```text
process args/env -> AppConfig -> StateStore migrations/recovery -> services -> data/Admin listeners and workers
MCP request -> authenticated Vault binding -> MCP adapter -> MemoryService / IndexService -> State repositories and Vault Core -> structured response
source change -> outbox/job -> memory worker -> source selection -> validated complete source set -> canonical Vault Core publication -> current projection
recall -> Vault permission/current-source eligibility -> FTS/vector candidates -> ranking and budget -> response
```

The current memory flow stores complete source units. It is included to identify the reality the semantic-memory plan must replace behind an isolated implementation boundary; it is not the target design.

Evidence: crates/server/src/main.rs:5-53; crates/server/src/lib.rs:150-220; crates/server/src/workers.rs:1636-1665,1935-1958; crates/memory/src/v3/selection.rs:1-3,95-103; docs/architecture.md:524-538.

## 3) Layer/Module Responsibilities

| Layer or module | Owns | Must not own | Evidence |
|---|---|---|---|
| domain | Typed IDs, VaultContext, path/revision/permission values | Infrastructure | docs/architecture.md:128-157 |
| vault-core + storage-fs | File mutation, history/recovery and safe filesystem boundary | SQL outside repositories or protocol-specific policy | docs/architecture.md:158-215,320-406 |
| state | Vault-scoped SQL repositories, migration and persistent job state | HTTP response construction | docs/architecture.md:216-229,423-474 |
| memory | Application-level memory write, extraction, current eligibility and recall | MCP/Admin routing or direct filesystem mutation | crates/memory/src/v3/service.rs:108-140; docs/architecture.md:299-318 |
| providers | Generation/embedding/reranking and shared bounded transport | Canonical file or SQL writes | docs/architecture.md:544-591; docs/provider-compatibility.md:11-16 |
| protocol adapters | Authenticate, validate DTOs, call services and render responses | Memory business logic or direct repositories | docs/architecture.md:230-298 |
| server | Composition, listener separation, worker registration, startup/recovery | Cross-module domain ownership | crates/server/src/lib.rs:150-220,330-370 |

## 4) Reused Patterns

| Pattern | Where found | Why it exists |
|---|---|---|
| Application service | MemoryService and IndexService | Keep business operations independent of MCP/Admin adapters (crates/memory/src/v3/service.rs:108-140; docs/architecture.md:299-318) |
| Vault-scoped repository | StateStore::memory_units and UnitRepository methods | Enforce context and query predicates around SQLite state (crates/state/src/pool.rs:228-240; crates/state/src/units.rs:345-367,538-575) |
| Persistent worker/job | memory.extract handler and state jobs | Resume bounded work after cancellation or restart (crates/server/src/workers.rs:1636-1665; docs/data-model.md:543-595) |
| Provider adapter + shared transport | crates/providers | Apply common URL validation, redirect denial, timeout, size, concurrency and redaction controls; the administrator owns endpoint network access (docs/provider-compatibility.md) |

## 5) Known Architectural Risks

- Intent vs reality: the new plan requires paraphrased semantic Observations and MemoryCards with evidence per assertion; current v3 selects full original units and explicitly prohibits generated body rewriting or cross-source consolidation. Plan baseline: docs/semantic-memory-implementation-plan.md:24-55,150-210,541-551. Current contract: docs/product-requirements.md:107-125, docs/architecture.md:524-540, docs/adr/0033-source-preserving-memory-units.md:13-25.
- Current memory storage and read eligibility are source-set/FileId/hash based; they do not implement card revision/support graphs or source-permission propagation across multi-source cards (migrations/0028_source_preserving_memory_units.sql:3-30,88-110; docs/security.md:503-507).
- The M1 implementation should be added in an independently testable semantic path before any M7 cutover; new canonical persistence/derived-state classification needs an ADR because architecture currently identifies Markdown and generated memory artifacts as canonical (docs/architecture.md:81-126; plan: docs/semantic-memory-implementation-plan.md:595-601).

## 6) Evidence

- docs/architecture.md:3-31,77-126,128-318,522-591,625-654
- crates/server/src/main.rs, crates/server/src/lib.rs, crates/server/src/workers.rs
- crates/memory/src/v3/service.rs and crates/memory/src/v3/selection.rs
- crates/state/src/pool.rs and crates/state/src/units.rs
- docs/semantic-memory-implementation-plan.md:24-55,117-233,527-601
- docs/adr/0033-source-preserving-memory-units.md:13-39
