# ADR-0030: Automatic source contributions and equivalent formal memories

> Superseded for the current memory runtime by [ADR-0033](0033-source-preserving-memory-units.md). This document preserves historical rationale.

- Status: Partially superseded by ADR-0031 (incremental organization, automatic old-state reset, no inclusion deletion/sentence rewriting/profile-change dissolution). Formal publication and source-support invariants remain accepted.
- Date: 2026-09-06
- Authority: explicit execution request for the automatic memory deduplication plan.
- Amends: ADR-0026 only in its one-source formal ownership restriction and the
  upgrade of already classified current v2.1 source sets. Ambiguous prerelease
  lifecycle rows still require the existing non-destructive classification.
- Implementation status: implemented; the active ExecPlan records engineering evidence and the separately pending real-provider/production gate.

## Decision

Keep current source-owned sets as source contributions. A formal note-derived
memory may be supported by several current contributions only after each
independently supports its complete proposition. Explicit assertions do not
participate. Same-source full inclusion may remove redundancy; cross-source
inclusion, topic similarity and transitive similarity cannot establish support.
Case folding and Unicode compatibility normalization are lexical conveniences,
not proof that identifiers, formulas, quantities or propositions are equal.

Formal content and exact support references must be canonical managed Markdown,
with rebuildable Vault-scoped projections. Every public memory reader, count,
vector source and mutation must use one formal view. Absorbed IDs become absent;
write operations must never redirect an old ID into a larger support group.
Publication checks source hashes, revisions, pause state and formal revisions,
with durable preparation and recovery for all affected files. Forget removes all
known supporting contributions and pauses their sources without a model call.
Source invalidation removes only that source's support, and the last lost
support makes the formal object immediately unreadable.

Existing classified v2.1 source sets must be adopted automatically after upgrade,
without extracting source notes again, changing provider bindings, visiting the
Admin UI or invoking a migration endpoint. Subsequent source events, readiness
and provider recovery must resume incremental work. Compatible vectors
remain reusable. No synthetic calibration is an admission requirement.

Necessary judgments use the existing authorized extraction provider through the
shared transport. Structured request-local references and enumerated relations
are proposals, never IDs, paths or write commands. Cache identities include
exact inputs, necessary scope, model configuration and rule version. Uncertainty
is retained and cached. Actual dispatches, including retries, are recorded per
Vault before network I/O. Per the operator decision on 2026-09-07, daily request
and byte caps are removed; concurrency, slice bounds, timeouts and provider
backoff remain enforced. No private bodies enter logs.
Recall remains local and never calls the judgment model.

## Consequences and implementation boundary

Migration 0021 adds independent formal objects, exact contribution supports,
a single public view, durable publication operations and candidate checkpoints.
`memory.deduplicate` is admitted at startup, periodically and after source
publication/reconciliation. Local adoption and cleanup run without a model.
Semantic work uses the existing extraction binding, strict pair judgments and
bounded dispatch budgets. Canonical facts use `mcp-vault-memory-fact/v2.2`;
source contributions retain `mcp-vault-memory-set/v2.1`.

Formal deletion removes all known exact supporting contributions and pauses their
source sets. Absorbed IDs return not found, including for writes. A changed
judging rule/profile dissolves old groups into their exact current contributions
before revalidation. No lifecycle or transitive similarity graph is introduced.

This decision does not authorize development access to production credentials,
paid model calls, deployment, or rewriting a user's real Vault. Semantic quality
from a real provider remains a separate explicitly authorized acceptance step.

The 2026-09-07 amendment also prioritizes newly published contributions during
candidate discovery and unattempted comparison scheduling. Publication persists
that work atomically; interrupted pair attempts rotate durably without claiming
completion. Existing-source upgrade work continues in the background.

Further operator guidance on 2026-09-07: unchanged examined inputs must not keep
creating semantic jobs. Persist a metadata/profile coverage checkpoint, retaining
invalidation by new inputs and pending work. Proposal acceptance tolerates extra
response properties without executing them, while necessary fields and semantic
merge evidence remain mandatory. A malformed proposal is isolated to its pair
or body; provider availability failures still defer work.
