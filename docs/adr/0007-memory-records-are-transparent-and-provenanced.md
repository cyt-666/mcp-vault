# ADR-0007: Durable memories are transparent and provenanced

- Status: Accepted
- Date: 2026-08-19

## Context

A hidden memory database can drift, accumulate contradictions, and make it impossible for the owner to understand what an Agent believes. Storing every extracted candidate as a visible file would create noise and sync load.

## Decision

Automatic extraction first creates derived candidates in SQLite.

An accepted/promoted durable memory is materialized as one atomic Markdown record under the reserved Vault namespace. It contains lifecycle, temporal, confidence, source, entity, and extraction metadata.

Every memory has provenance or is explicitly marked as an assertion. LLM output is validated before promotion. Normal recall excludes stale, superseded, archived, and rejected records.

## Consequences

Positive:

- owner can inspect/edit/delete memory;
- memories survive index rebuild;
- contradiction and source invalidation are explainable.

Costs:

- canonical memory files add to the Vault;
- direct edits require validation/reconciliation;
- lifecycle and deduplication are more complex than a vector-store insert.

## Rejected alternatives

- Opaque memory rows only.
- Save every chat message.
- Materialize every low-confidence extraction candidate immediately.
- Return unsourced summaries as fact.
