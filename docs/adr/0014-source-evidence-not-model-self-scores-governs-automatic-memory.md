# ADR-0014: Source evidence, not model self-scores, governs automatic memory

- Status: Superseded by ADR-0015
- Date: 2026-08-25
- Refines: ADR-0007's candidate-first boundary; its canonical-Markdown and
  provenance requirements remain binding.

## Context

The prerelease extraction pipeline asked the same generative model to propose a
memory, label its durability, and assign confidence and importance numbers.
Deployment evidence showed high self-scores on ordinary implementation facts.
Multiple floating-point thresholds did not make those claims independently
verifiable, and routing normal output to a human review queue made the service
depend on ongoing manual labor.

OpenAI's documented local Codex memory behavior is background generation into
local files with supporting evidence and chat-level source controls, not a
per-memory approval workflow. OpenAI's memory-engineering guidance emphasizes
non-invention, source-grounded distillation, deduplication, conflict resolution,
forgetting, and evaluations. Confidence can be metadata for volatile signals;
it is not proof supplied by the same model that made the claim.

## Decision

The default and only supported source-admission mode is explicit note opt-in:
root YAML frontmatter must contain the boolean `mcp-vault-memory: true`. Legacy
`all_notes` settings deserialize to this safe mode.

The provider no longer emits `confidence`, `importance`, or a free-form
canonical memory sentence. It may select at most the configured number of
atomic statements and must return, for each one:

- an exact bounded `evidence_quote` copied from the note;
- the current source line range and heading;
- a typed memory category and durability scope;
- bounded entities/tags.

Local code verifies the marker, current Vault/file/revision, exact quote within
the declared line range, schema, category/scope compatibility, content bounds,
deduplication, and idempotency. The exact source quote—not a model paraphrase—is
the canonical proposition. Valid proposals are automatically materialized
through `MemoryService` and Vault Core. Invalid or non-admitted proposals are
automatically rejected with a bounded diagnostic. Retryable infrastructure
failures remain durable job failures and retry without requiring a human.

The SQLite candidate row remains a derived validation/audit seam: it is written
before canonical promotion so a crash can resume idempotently, then receives a
terminal `promoted` or `rejected` decision. Pending rows are exceptional legacy
or interrupted state, not the normal product workflow. Admin describes them as
problems requiring attention, never as a routine candidate inbox.

Stored memory `confidence` and `importance` fields remain for backward
compatibility, explicit Agent/Admin memories, and retrieval weighting. For this
automatic path they are deterministic provenance metadata and are not used as
trust thresholds or shown as model certainty in Admin.

## Consequences

Positive:

- ordinary operation needs no review click;
- every automatic memory is an exact, revision-bound source statement;
- hallucinated paraphrases and model self-scoring cannot become trust evidence;
- old `all_notes` deployments fail safe into explicit-only admission;
- the owner can still inspect, archive, restore, or delete canonical memories.

Costs:

- exact quotes are less polished than model-written summaries;
- semantic contradiction handling still requires deterministic consolidation
  work and evaluations; exact deduplication alone is not sufficient;
- a marked note must state durable information clearly enough to quote.

## Rejected alternatives

- Treat model confidence/importance numbers as calibrated probabilities.
- Add more overlapping score ranges.
- Require the owner to approve every extraction.
- Materialize an unsupported model paraphrase.
- Re-enable all-note extraction and rely on later cleanup.

## References

- [OpenAI: Codex memories](https://learn.chatgpt.com/docs/customization/memories)
- [OpenAI Cookbook: state management with long-term memory notes](https://developers.openai.com/cookbook/examples/agents_sdk/context_personalization)
