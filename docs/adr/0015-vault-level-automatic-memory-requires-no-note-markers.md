# ADR-0015: Vault-level automatic memory requires no note markers

- Status: Accepted
- Date: 2026-08-25
- Supersedes: ADR-0014's per-note `mcp-vault-memory` source-admission rule.
- Retains: ADR-0014's exact-evidence, no-self-score, autonomous-promotion
  rules.
- Amended by: ADR-0016 supersedes direct automatic promotion while retaining
  marker-free Vault-level admission and exact supporting evidence.
- Automatic knowledge classification superseded by ADR-0026, which retains
  marker-free Vault-level admission but allows useful sourced knowledge.

## Context

ADR-0014 stopped ordinary technical articles from becoming high-score memory
proposals by requiring a namespaced frontmatter boolean. That was an effective
circuit breaker but an unacceptable authoring contract: normal people should
not add service-control metadata while taking notes, and a system that depends
on per-note opt-in has merely moved its classification work onto the owner.

At the same time, processing every article proposition as durable memory
recreates the original noise problem. Ordinary Vault knowledge already has a
better representation: canonical notes plus `related_notes` retrieval. The
automatic-memory path must therefore operate without markers while remaining
much narrower than general knowledge extraction.

## Decision

Automatic memory is enabled once per Vault. There is one serialized source
mode, `automatic`; legacy `explicit_only` and `all_notes` values deserialize as
migration aliases for `automatic`. No note path, frontmatter key, tag, or
folder convention is required.

Every eligible non-managed Markdown create/update/move/restore may enter the
durable extraction worker. The Provider still must return an exact bounded
source quote and current line range, and all ADR-0014 local validation remains
mandatory.

Automatic note-derived materialization is additionally restricted to classes
that are intrinsically personal or temporal:

- owner identity, preference, or relationship;
- accepted project decision;
- current project progress;
- significant event.

General facts, project/software descriptions, component requirements,
procedures, inferred constraints, examples, and reference material are locally
rejected from automatic durable memory even if the Provider assigns a plausible
scope. They remain automatically available through ordinary note search and
`related_notes`. An Agent or Admin may still create any supported durable
memory type explicitly through `remember`; this restriction applies only to
background note mining.

The Chinese Admin UI says to write notes normally. Its one maintenance action
processes existing notes; it never asks users to mark notes or generate/review
candidates.

## Consequences

Positive:

- note authors use ordinary Markdown without MCP Vault metadata;
- automatic memory and recall require no repeated human intervention;
- technical reference knowledge remains discoverable without polluting durable
  behavioral context;
- exact-source and current-revision guarantees still prevent invented memory
  text.

Costs:

- when Vault-level automatic memory is enabled, eligible note changes can incur
  Provider calls even when the result is empty;
- the deliberately narrow automatic type allow-list can miss durable facts or
  constraints, which remain available as notes or can be captured by Agent
  `remember`;
- quality still requires extraction/consolidation evaluations and future
  deterministic conflict improvements.

## Rejected alternatives

- Require `mcp-vault-memory: true` in author-written notes.
- Require a dedicated folder or tag convention.
- Re-enable automatic durable extraction for every technical fact.
- Trust a Provider's scope label without a local automatic-type allow-list.
- Send uncertain results to a routine human queue.
