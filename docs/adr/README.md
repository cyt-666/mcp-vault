# Architecture Decision Records

Accepted ADRs are binding unless superseded by a later ADR.

The current sequence includes ADR-0043. ADR-0013 keeps ADR-0007 durable memories
canonical and provenanced while allowing `recall` to return separately typed,
rebuildable ordinary-note cues. ADR-0014 replaced model self-score/routine
review defaults with exact evidence and autonomous promotion. ADR-0015 retains
those trust rules but removes per-note service markers in favor of one
Vault-level automatic mode and a narrow local type allow-list. ADR-0016 replaces
automatic direct promotion with Codex-style raw-memory extraction followed by
separate global consolidation; evidence remains exact and independent from
final semantic content. ADR-0017 retains that architecture but changes
prerelease upgrades to discard every replaced memory generation and require
versioned fresh jobs instead of compatibility conversion.

ADR-0018 adds a complete built-in OAuth 2.1 authorization server as the default
self-hosted ChatGPT path while preserving external issuer validation as an
optional compatibility mode. Its 2026-08-31 amendment separates protocol-only
`offline_access`, uses a 180-day sliding refresh idle lifetime, and protects a
successful rotation with a bounded duplicate-request grace before replay-family
revocation.

ADR-0019 removes the WebDAV proxy socket-peer allow-list and moves protection
of the plaintext data listener entirely to deployment networking while
retaining the forwarded-HTTPS requirement.

ADR-0020 enables managed multi-Vault administration while retaining per-Vault
MCP/WebDAV endpoints, Vault-bound credentials, a stable legacy Admin default,
and isolated initialization, jobs, indexes, and memory.

ADR-0021 permits a serialized ordinary-`renameat` compatibility path when a
same-filesystem Unix mount rejects `RENAME_NOREPLACE`; it does not broaden the
temporary-file hard-link exception.

ADR-0022 replaces one-time memory-source repair with continuous exact source
health, fail-closed normal recall, event-ordered reconciliation, and repeatable
Vault-scoped audits.

ADR-0023 keeps canonical memory in its source language and adds persisted,
rebuildable source/`zh-Hans`/`en` retrieval metadata, explicit historical
backfill, CJK-aware lexical recall, and object-scoped vector ranking without a
query-time LLM call.

ADR-0024 replaces character-count note-vector chunks with a versioned bounded
UTF-8 input envelope, preserves redacted Provider failure categories, and
requires current-model note and memory vector scheduling without re-running
memory extraction.

ADR-0025 aggregates current note chunks by File ID before the final note Top-K,
uses only the highest non-negative cosine per note, and makes that cosine scale
the existing reciprocal-rank contribution.

ADR-0026 replaces the two-phase global consolidation, model-readable lifecycle
history, destructive pipeline resets, and continuous source-health graph with
current source-owned memory sets plus direct explicit memory. Forgetting is
deletion, note sets replace atomically under File-ID/hash/set-revision checks,
and all query paths are current-only. It retains canonical Markdown,
provenance, multilingual aliases, LLM-free recall, and object-scoped vector
validation while adding relevance, chunk-coverage, and budget requirements.

Status values:

```text
Proposed
Accepted
Superseded
Deprecated
Rejected
```

When architecture changes, do not rewrite historical rationale. Add a new ADR that supersedes the old one and update the old status/link.

ADR-0028 makes bundled evaluation diagnostic-only and adds optional persisted
model grouping of note source units, amending ADR-0026/0027 retrieval admission.
See [ADR-0028](0028-model-guided-chunks-and-diagnostic-evaluation.md).

- [ADR-0029: Actionable compact MCP tool results](0029-actionable-compact-mcp-results.md)

- [ADR-0030: Automatic memory equivalence](0030-automatic-memory-equivalence.md)
  amends current source ownership for verified multi-source formal memories;
  implementation and acceptance are tracked in the active deduplication ExecPlan.


- [ADR-0031: Incremental memory organization](0031-incremental-memory-organization.md)
  replaces all-pairs scheduling with bounded, persistent contribution work.
- [ADR-0032: Lossless memory consolidation](0032-lossless-memory-consolidation.md)
  adds per-fact provenance and directional support/coverage verification while
  retaining incremental scheduling and canonical source contributions.

- [ADR-0033: Source-preserving memory units](0033-source-preserving-memory-units.md)
  records the historical v3 source-unit contract. Its conflicting automatic-body and no-cross-source decisions, plus the old predecessor-memory discard authorization, are superseded by ADR-0035 for the new semantic-memory plan.
- [ADR-0034: Replayed create witness recovery](0034-replayed-create-witness-recovery.md)
  adds a transactionally verified `superseded` terminal state for a replayed
  create whose canonical result was later claimed by another File ID.
- [ADR-0035: Semantic memory cards and task packs](0035-semantic-memory-cards-and-task-packs.md)
  makes source-level semantic extraction, evidence-backed memory cards and
  task packs the new target. It requires a separate namespace and staged,
  explicitly authorized cutover; it does not authorize legacy-memory cleanup.
- [ADR-0043: Strict MiMo function output for all M6 stages](0043-strict-mimo-function-output-for-all-m6-stages.md)
  scopes strict non-streaming function output to the four versioned M6 stages
  and defines the relation-stage empty-string sentinel mapping.
- [ADR-0044: Bounded A80 extraction and isolated evaluation](0044-bounded-a80-extraction-and-isolated-evaluation.md)
  limits M6 source proposals to 80-block indexed batches, atomically publishes
  only fully validated source sets, and isolates item failures under a
  monotonic budget and same-root recovery fence.
- [ADR-0036: Two-stage semantic extraction and card composition](0036-semantic-memory-two-stage-composition.md)
  separates observation and composition contracts and preserves terminal cleanup.
- [ADR-0037: Provider structured streaming generation](0037-provider-streaming-structured-generation.md)
  defines bounded streaming generation and structured-output validation.
- [ADR-0038: Independent agent review for M6](0038-independent-agent-review-for-m6.md)
  permits independent agent review in place of manual M6 scoring while retaining
  all gates and explicitly distinguishing `agent_reviewed` from `human_reviewed`.
- [ADR-0039: Source-revision-bound model block IDs](0039-source-revision-bound-model-block-ids.md)
  binds transient evidence IDs to a Vault source revision and rejects namespace
  collisions without changing persistent evidence spans.
- [ADR-0040: Revision-bound indexed observation evidence](0040-revision-bound-indexed-observation-evidence.md)
  sends one revision namespace plus source-local integer block indices, then
  resolves them back to prepared block IDs before existing evidence fences.
- [ADR-0041: Canonical assertion-status tokens in observation prompts](0041-canonical-assertion-status-prompt-tokens.md)
  keeps the v6 status enum strict and repeats its exact vocabulary and meanings
  in the v10 prompt; invalid model values still fail closed.
- [ADR-0042: Strict MiMo function output for M6 observations](0042-strict-mimo-function-output-for-m6-observations.md)
  uses an evaluation-only strict function schema and validated SSE argument
  aggregation while retaining the semantic and source evidence fences.
