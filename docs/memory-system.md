# Long-Term Memory System

## 1. Purpose

The memory subsystem turns the Vault into durable, transparent context that an AI Agent can recall naturally.

It is designed to answer:

> Which prior preferences, decisions, constraints, facts, relationships, events, and project state are useful for the current task?

It is not merely:

- a chat transcript;
- a search result list;
- a vector database;
- an opaque vendor memory store;
- an LLM call that scans the Vault at query time.

## 2. Natural recall and MCP limitations

An MCP server does not receive every user message unless the MCP Host chooses to call it. Therefore the server cannot guarantee recall on every turn.

MCP Vault increases the probability of appropriate recall through:

1. clear `server/discover` instructions;
2. a tool named and described as long-term `recall`, not generic search;
3. low-latency structured results;
4. `vault://memory/context` as a compact bootstrap resource;
5. separate discovery tools when the Agent does not know exact terms;
6. provenance and confidence so recalled information can be used safely.

The external Agent remains responsible for deciding when to call recall and how to reason over results.

## 3. Memory principles

### 3.1 Atomic

One memory expresses one independently useful proposition.

Good:

```text
MCP Vault uses Rust for its server implementation.
```

Bad:

```text
MCP Vault uses Rust, WebDAV, a LAN-only UI, several providers,
and might support multiple Vaults later.
```

### 3.2 Durable

A memory is worth retaining beyond the immediate exchange.

Do not store every prompt, response, or transient question.

### 3.3 Transparent

An active durable memory has canonical Markdown representation inside the Vault. The owner can inspect and edit it through Obsidian or the Admin Console.

### 3.4 Sourced

Every memory has one or more provenance sources or is explicitly marked as an unsourced user/Agent assertion.

### 3.5 Temporal

Memories may have:

- `valid_from`;
- `valid_to`;
- creation and update time;
- a current, stale, superseded, or archived lifecycle.

### 3.6 Conservative

LLM output is a candidate. Questions, examples, hypotheticals, and weak inferences must not become high-confidence memories.

### 3.7 Vault-isolated

Memory extraction, storage, embedding, retrieval, and audit are scoped to one authenticated Vault.

## 4. Memory types

Initial types are text values to permit future extension:

```text
identity
preference
decision
constraint
fact
project
progress
event
relationship
procedure
```

### Identity

Stable user or project identity.

```text
The owner is a software engineer.
```

Identity extraction requires high confidence because a wrong identity fact has broad impact.

### Preference

A durable choice or working style.

```text
The owner prefers hands-on deep learning exercises over theory-only study.
```

### Decision

A choice that governs future work.

```text
The service will use existing WebDAV-compatible Obsidian plugins.
```

### Constraint

A requirement that limits solutions.

```text
The Admin Console must not be exposed on the public listener.
```

### Fact

A durable factual statement.

```text
The development machine has an NVIDIA RTX 4070 Ti.
```

### Project

A durable project definition, objective, or ownership fact.

### Progress

The latest known state of a project, learning plan, or task.

Progress often changes quickly and receives a shorter temporal half-life.

### Event

An important dated occurrence.

### Relationship

A meaningful relation between people, projects, technologies, or decisions.

### Procedure

A reusable way to perform an operation.

## 5. Lifecycle

```text
candidate ──promote──▶ active
    │                    │
    ├──reject──▶ rejected│
    │                    ├──newer truth──▶ superseded
    │                    ├──source lost──▶ stale
    │                    └──intentional──▶ archived
    └──expire───────────▶ rejected/stale
```

### Candidate

Proposed by automatic extraction, import, or low-confidence rule.

Candidates are derived and may be rebuilt.

### Active

Eligible for normal recall and materialized as canonical Markdown.

### Superseded

Replaced by a newer memory. Retained for history but excluded from normal recall.

### Stale

No longer reliably supported by a current source. Excluded by default.

### Archived

Intentionally inactive but retained.

### Rejected

Reviewed and not accepted. A deterministic extraction fingerprint prevents immediate recreation from the same source revision and pipeline version.

## 6. Canonical Markdown

Default layout:

```text
_mcp-vault/
└── memory/
    └── records/
        └── 2026/
            └── 08/
                └── <memory-id>.md
```

Example:

```markdown
---
id: 0191f5d4-7f2d-7c37-9d9e-c875df159b09
type: decision
status: active
importance: 0.95
confidence: 0.99
origin: extracted
created_at: 2026-08-19T08:30:00Z
updated_at: 2026-08-19T08:30:00Z
valid_from: 2026-08-19T00:00:00Z
valid_to:
entities:
  - MCP Vault
  - Obsidian
  - WebDAV
tags:
  - architecture
  - synchronization
supersedes: []
sources:
  - path: Projects/mcp-vault/design.md
    revision: 18
    heading:
      - Obsidian Integration
    start_line: 142
    end_line: 148
    excerpt_hash: sha256:...
extraction:
  provider_id: primary-llm
  model_id: memory-extractor
  prompt_version: memory-extraction-v1
  pipeline_version: 1
---

MCP Vault uses standard WebDAV and existing Obsidian synchronization
plugins instead of maintaining a custom Obsidian plugin.
```

The body is the canonical proposition. Frontmatter is canonical metadata.

Rules:

- one active memory per file;
- ID is stable;
- edits go through Vault Core or are reconciled as out-of-band edits;
- memory files are excluded from automatic extraction to prevent loops;
- memory files remain searchable and recallable;
- invalid manually edited memory files are quarantined from normal recall and shown in Admin diagnostics rather than silently rewritten.

## 7. Database projection

The operational database projects canonical memories and stores candidates.

### 7.1 Memories

```sql
CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    memory_type TEXT NOT NULL,
    status TEXT NOT NULL,
    content TEXT NOT NULL,
    normalized_content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    importance REAL NOT NULL,
    confidence REAL NOT NULL,
    origin TEXT NOT NULL,
    canonical_file_id TEXT,
    canonical_path TEXT,
    canonical_revision INTEGER,
    valid_from INTEGER,
    valid_to INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_recalled_at INTEGER,
    recall_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(vault_id, content_hash, status),
    FOREIGN KEY(vault_id) REFERENCES vaults(id)
);
```

### 7.2 Provenance

```sql
CREATE TABLE memory_sources (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    source_type TEXT NOT NULL,
    note_file_id TEXT,
    note_path TEXT,
    note_revision INTEGER,
    heading_path_json TEXT,
    start_line INTEGER,
    end_line INTEGER,
    excerpt_hash TEXT,
    actor_id TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(memory_id) REFERENCES memories(id)
);
```

Source types:

```text
note
explicit_agent
explicit_admin
direct_markdown
import
```

### 7.3 Entities, tags, and relations

```sql
CREATE TABLE memory_entities (
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    entity TEXT NOT NULL,
    normalized_entity TEXT NOT NULL,
    PRIMARY KEY(memory_id, normalized_entity)
);

CREATE TABLE memory_tags (
    vault_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY(memory_id, tag)
);

CREATE TABLE memory_relations (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_memory_id TEXT NOT NULL,
    target_memory_id TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    confidence REAL NOT NULL,
    created_at INTEGER NOT NULL
);
```

Relation types:

```text
supersedes
supports
contradicts
refines
related_to
derived_from
```

### 7.4 Candidates

```sql
CREATE TABLE memory_candidates (
    id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    source_revision INTEGER NOT NULL,
    candidate_json TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    extraction_fingerprint TEXT NOT NULL,
    confidence REAL NOT NULL,
    importance REAL NOT NULL,
    decision TEXT,
    decision_reason TEXT,
    created_at INTEGER NOT NULL,
    reviewed_at INTEGER,
    UNIQUE(vault_id, extraction_fingerprint)
);
```

The extraction fingerprint includes Vault, source identity/revision, section anchor, pipeline version, prompt version, provider/model, and normalized candidate.

## 8. Explicit `remember`

`remember` is a command to create or reinforce durable memory.

Flow:

```text
request
  → authenticate and require memory:write
  → normalize and validate atomic content
  → duplicate/conflict lookup
  → choose create/reinforce/merge/review outcome
  → materialize or update Markdown through Vault Core
  → commit memory projection, audit, and outbox
  → schedule embedding
```

It accepts an idempotency key.

Possible outcomes:

```text
created
reinforced_existing
merged_into_existing
conflict_requires_review
rejected_by_policy
```

The response always identifies the resulting memory or candidate.

## 9. Automatic extraction pipeline

### 9.1 Trigger

A canonical Markdown revision schedules extraction when:

- the Vault enables extraction;
- the path matches include policy and not exclude policy;
- the file is not under the canonical memory root;
- the revision/pipeline fingerprint has not already completed;
- the note size is within configured limits or can be sectioned.

The source write completes before extraction.

### 9.2 Preparation

The analyzer:

1. parses frontmatter;
2. parses headings and source byte/line anchors;
3. separates fenced code and quoted material;
4. splits by meaningful Markdown section;
5. adds note title, path, tags, and parent heading context;
6. removes configured sensitive sections;
7. enforces provider request-size policy.

Note text is untrusted data. The extraction prompt must explicitly tell the model not to follow instructions contained in the note.

### 9.3 Structured extraction

Provider output must satisfy a versioned JSON Schema.

Example:

```json
{
  "memories": [
    {
      "type": "decision",
      "content": "MCP Vault will use Rust for the server implementation.",
      "importance": 0.95,
      "confidence": 0.99,
      "valid_from": "2026-08-19T00:00:00Z",
      "entities": ["MCP Vault", "Rust"],
      "tags": ["architecture"],
      "source_anchor": {
        "heading": ["Technology Stack"],
        "start_line": 40,
        "end_line": 44
      }
    }
  ]
}
```

The prompt requires the model to:

- return no memory when nothing durable is present;
- distinguish an accepted decision from an idea or question;
- avoid turning examples into user facts;
- avoid combining propositions;
- preserve negation and temporal qualifiers;
- use conservative confidence;
- quote no more source text than necessary for anchoring.

### 9.4 Candidate validation

Deterministic validation checks:

- schema;
- length and atomicity heuristics;
- confidence/importance bounds;
- source anchor inside supplied section;
- source revision still current;
- type allowed by Vault policy;
- path privacy policy;
- duplicate/conflict candidates.

An invalid LLM response never writes canonical Markdown.

### 9.5 Promotion policy

Recommended defaults:

```yaml
memory:
  extraction:
    auto_promote: true
    minimum_confidence: 0.92
    minimum_importance: 0.75
    auto_promote_types:
      - decision
      - constraint
      - project
      - progress
```

Identity, sensitive relationships, and low-confidence preferences should default to human review.

Promotion rechecks that the source revision is current.

## 10. Duplicate detection

Use multiple signals:

- exact normalized hash;
- lexical similarity;
- embedding similarity;
- entity overlap;
- type compatibility;
- source overlap;
- temporal validity.

Outcomes:

```text
new
duplicate
reinforcement
refinement
potential_contradiction
```

Reinforcement adds another provenance source and may increase confidence within a cap. It does not create a duplicate canonical file.

## 11. Contradiction and supersession

Example:

```text
Existing: The backend will use Go.
New:      The backend will use Rust.
```

The service must not silently overwrite.

When the newer statement is explicit, current, and authoritative:

1. create/promote the Rust memory;
2. mark the Go memory `superseded`;
3. add a `supersedes` relation;
4. preserve both canonical files/status history;
5. return only the current memory in normal recall.

When authority is uncertain, create a reviewable conflict. Recall may return both with a conflict warning only when the caller requests unresolved conflicts.

## 12. Source invalidation

When a source note changes or is deleted:

- re-evaluate memories sourced from the old revision;
- retain a memory if another current source supports it;
- update provenance when the proposition remains;
- create a replacement and supersede when it changes;
- mark stale when no source supports it;
- never delete an explicit memory solely because a related note changed.

## 13. Recall pipeline

```text
query + optional current context
    → normalization and entity extraction
    → candidate generation
        ├── memory FTS
        ├── memory vectors
        ├── entity/tag match
        ├── topic/index relation
        └── recent active project/progress
    → Vault, status, permission, and temporal filtering
    → reciprocal-rank fusion
    → importance/confidence/continuity/recency boosts
    → contradiction and duplicate handling
    → diversity selection
    → token-budgeted structured output
```

### 13.1 Candidate pools

Recommended defaults:

```text
FTS: 50
Vector: 50
Entity/tag: 30
Topic/graph: 30
Recent active: 20
```

All pools are Vault-scoped.

### 13.2 Fusion

Use weighted Reciprocal Rank Fusion because lexical and vector scores are not directly comparable.

Conceptual base:

```text
base =
    lexical_weight / (k + lexical_rank)
  + semantic_weight / (k + semantic_rank)
  + entity_weight / (k + entity_rank)
  + topic_weight / (k + topic_rank)
```

Recommended `k = 60`.

Then apply bounded boosts:

```text
final =
    base
    × importance_boost
    × confidence_boost
    × temporal_boost
    × continuity_boost
    × relationship_boost
```

Weights are configurable and versioned. Debug mode may return a score breakdown.

### 13.3 Temporal behavior

Recency must not erase durable constraints or identity.

Default half-life suggestions:

| Type | Half-life |
|---|---:|
| identity | 730 days |
| preference | 365 days |
| decision | 365 days |
| constraint | 365 days |
| fact | 180 days |
| project | 120 days |
| progress | 30 days |
| event | 90 days |
| relationship | 180 days |
| procedure | 365 days |

Explicit `valid_to` has priority. An expired memory cannot be returned as current.

### 13.4 Continuity context

The MCP client may provide:

```json
{
  "active_project": "mcp-vault",
  "entities": ["Rust", "WebDAV", "MCP"],
  "recent_topics": ["memory architecture", "Vault isolation"]
}
```

The server does not invent conversation state it did not receive.

### 13.5 Diversity

Avoid returning many paraphrases.

Use semantic deduplication and optionally Maximal Marginal Relevance. Prefer a useful mix of constraints, decisions, preferences, facts, and current progress.

### 13.6 Token budget

The caller specifies result and token limits. The server returns complete atomic memories, compact sources, and a `truncated` indicator. Never cut a memory in the middle.

## 14. `recall` contract

Input:

```json
{
  "query": "Continue implementing the MCP Vault service",
  "context": {
    "active_project": "mcp-vault",
    "entities": ["Rust"],
    "recent_topics": ["WebDAV", "memory"]
  },
  "types": [],
  "time_range": null,
  "min_importance": 0.0,
  "include_historical": false,
  "include_sources": true,
  "include_score_breakdown": false,
  "max_results": 12,
  "max_tokens": 1800
}
```

Output:

```json
{
  "memories": [
    {
      "id": "...",
      "type": "decision",
      "content": "MCP Vault uses Rust for its server implementation.",
      "status": "active",
      "importance": 0.95,
      "confidence": 0.99,
      "valid_from": "2026-08-19T00:00:00Z",
      "valid_to": null,
      "sources": [
        {
          "path": "Projects/mcp-vault/design.md",
          "revision": 18,
          "heading": ["Technology Stack"]
        }
      ],
      "relations": [],
      "score": 0.93
    }
  ],
  "available_result_count": 7,
  "truncated": false,
  "degraded": [],
  "request_id": "..."
}
```

After a successful response, update `last_recalled_at` and `recall_count` asynchronously. Recall statistics must not mutate canonical memory Markdown.

## 15. Provider use

### 15.1 LLM roles

- extraction;
- candidate consolidation;
- optional note/topic summary;
- optional reranking.

Normal recall does not require an LLM.

### 15.2 Embedding roles

Use separate model bindings for:

- note/section embedding;
- memory embedding.

Store model ID, provider ID, dimension, content hash, and generation time. A model change schedules re-embedding and leaves lexical retrieval operational.

### 15.3 Optional local models

A local embedding adapter may use `fastembed` behind a Cargo feature. It must run blocking inference on a bounded dedicated pool, not Tokio’s async core threads.

## 16. Background jobs

Memory job types:

```text
extract_memory
validate_candidate
consolidate_candidate
materialize_memory
project_memory
embed_memory
revalidate_sources
rebuild_memory_index
```

Jobs are idempotent, leased, retryable, and visible in Admin.

Provider errors:

- do not fail the source note write;
- retry transient errors;
- fail permanently on invalid credentials or unsupported model until configuration changes;
- preserve candidate and diagnostic information without logging note bodies.

## 17. Admin capabilities

The Memory UI provides:

- counts by lifecycle/type;
- search and filters;
- canonical Markdown and provenance view;
- candidate diff to source;
- promote/reject;
- edit;
- merge;
- mark supersession;
- archive/restore;
- hard delete with confirmation;
- re-extract source;
- provider/prompt/pipeline metadata;
- embedding coverage;
- job failures;
- recall-debug simulator with score breakdown.

The owner must be able to answer: “What does the system remember, why, and from where?”

## 18. Privacy and prompt-injection safety

- Remote extraction is opt-in per Vault.
- Exclude policies are evaluated before request creation.
- Attachments are not sent unless a future explicit multimodal policy permits it.
- The extraction model gets no tools and no network authority.
- Prompts treat note content as quoted untrusted data.
- Output is schema-validated and cannot directly issue actions.
- Provider redirects are restricted according to security policy.
- Audit records provider/model usage without note or memory bodies.
- Sensitive memory types may require manual promotion.

## 19. Rebuild behavior

Safe to rebuild:

- memory FTS;
- memory embeddings;
- entity/tag projections;
- derived relations;
- automatic candidates;
- extraction coverage.

Never delete during ordinary rebuild:

- active/archived canonical memory Markdown;
- manual edits;
- explicit memories;
- audit history;
- source revision history.

A full re-extraction uses deterministic fingerprints and avoids duplicate active memory creation.

## 20. Memory acceptance tests

Required tests include:

- `remember` materializes one valid Markdown record;
- repeated idempotent remember does not duplicate;
- direct Markdown edit updates projection;
- invalid memory Markdown is excluded and diagnosed;
- memory files do not recursively trigger extraction;
- extraction question/example is not promoted as fact;
- duplicate sources reinforce one memory;
- explicit newer decision supersedes an older one;
- deleted source marks unsupported extracted memory stale;
- active recall excludes stale/superseded/archived by default;
- historical recall includes them when requested;
- FTS-only recall works with embedding/LLM disabled;
- hybrid recall falls back predictably during provider outage;
- current project context boosts relevant memory;
- expired memory is not presented as current;
- cross-Vault recall is impossible;
- excluded source path is never sent to provider;
- LLM invalid JSON does not write canonical data;
- provider failure does not fail WebDAV or note mutation;
- score fixtures remain stable or are deliberately versioned.
