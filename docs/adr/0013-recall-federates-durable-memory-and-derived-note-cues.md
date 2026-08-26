# ADR-0013: Recall federates durable memory and derived note cues

- Status: Accepted
- Date: 2026-08-24
- Refined by: ADR-0014 for evidence/promotion policy and ADR-0015 for
  Vault-level source admission.

## Context

An Agent must be able to remember that relevant knowledge exists in ordinary
Vault notes even when the owner has not converted that knowledge into explicit
durable memory Markdown. Treating every article proposition as a memory
candidate creates an unbounded manual review queue and confuses source
retrieval with accepted owner/project state. Keeping `recall` limited to
promoted memories has the opposite failure: ordinary note knowledge is absent
from proactive recall and can be found only when the Agent guesses exact search
terms and calls `search_notes` separately.

## Decision

`recall` federates two separately typed, Vault-scoped projection classes:

1. `memories` are promoted durable propositions governed by ADR-0007. Active
   records have canonical Markdown, provenance, lifecycle, confidence,
   importance, and temporal validity.
2. `related_notes` are rebuildable retrieval cues derived from current
   canonical Markdown. They contain bounded source metadata/snippets and point
   to a note revision/resource. They are not durable facts and require
   `vault:read` in addition to the `memory:read` permission needed to call
   `recall`.

Ordinary notes are always available to lexical retrieval. When the
`embedding_note` role is configured, deterministic bounded note chunks receive
reference-only durable embedding jobs and participate in semantic/hybrid
ranking. Query-time recall may request a query embedding but never invokes a
generative LLM or scans the Vault filesystem. Stale vectors are excluded by
their source/chunk content hash and remain rebuildable.

Automatic extraction is not the ordinary-note indexing mechanism. It produces
at most a small number of high-leverage owner/project memories after structured
and deterministic admission checks. ADR-0014 removes model self-scores,
routine review, and generated canonical paraphrases: exact source quotes are
verified and automatically promoted or rejected. ADR-0015 removes per-note
markers and enables source admission once per Vault, while a local type/scope
allow-list excludes general article knowledge, project/software descriptions,
requirements, procedures, and inferred constraints. During prerelease testing,
unpromoted legacy/interrupted rows may be discarded and regenerated under a
new extraction pipeline without migrating their content.

## Consequences

Positive:

- an Agent can discover that a relevant article exists and then read the
  canonical source without manual memory promotion;
- ordinary knowledge does not create review-queue pressure;
- the author does not need to modify note syntax for automatic memory;
- callers can distinguish accepted durable context from retrieval hints;
- lexical behavior remains available without any provider;
- note and memory vectors remain optional, rebuildable, and Vault-scoped.

Costs:

- `recall` has a larger typed response and shared result/token budgets;
- note-vector scheduling, stale detection, and provider degradation need
  explicit operational evidence;
- MCP credentials with only `memory:read` receive no note cues, so trusted
  Agent profiles should normally also grant `vault:read`.

## Rejected alternatives

- Promote or review every extracted article fact.
- Make candidate confidence thresholds the only noise control.
- Trust a provider-generated durability label as proof of owner intent.
- Send every Markdown note to extraction by default and rely on later review.
- Keep `recall` memory-only and rely on every MCP Host to issue a second search.
- Return note snippets to a `memory:read` credential that lacks `vault:read`.
- Run a generative LLM over the Vault at recall time.
