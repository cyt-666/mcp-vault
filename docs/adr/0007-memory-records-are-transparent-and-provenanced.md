# ADR-0007: Durable memories are transparent and provenanced

- Status: Accepted
- Date: 2026-08-19
- Continuous source-health behavior amended by: ADR-0022

## Context

A hidden memory database can drift, accumulate contradictions, and make it impossible for the owner to understand what an Agent believes. Storing every extracted candidate as a visible file would create noise and sync load.

## Decision

Automatic extraction first creates derived candidates in SQLite.

An accepted/promoted durable memory is materialized as one atomic Markdown record under the reserved Vault namespace. It contains lifecycle, temporal, confidence, source, entity, and extraction metadata.

Every memory has provenance or is explicitly marked as an assertion. LLM output is validated before promotion. Normal recall excludes stale, superseded, archived, and rejected records.

Note provenance uses stable Vault-scoped File ID plus an evidence revision.
Canonical Markdown stores that identity as an optional `sources[].file_id` so
renames survive projection rebuild. Outward source `path` is resolved from the
current active file and is absent after deletion; the evidence revision remains
historical until content equality proves it can advance. Legacy records without
an ID remain readable and are repaired only when path/revision state proves the
identity without guessing.

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
