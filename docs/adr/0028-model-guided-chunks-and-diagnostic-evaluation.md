# ADR-0028: Model-guided chunks and diagnostic-only retrieval evaluation

- Status: Accepted
- Date: 2026-09-06
- Amends: ADR-0026 retrieval admission, ADR-0027 automatic quality gate
- Authority: explicit user approval of revised retrieval design

## Decision

Bundled evaluation is a development/operator diagnostic, not a prerequisite for
production semantic retrieval. Startup and ordinary configuration reconciliation
must not initiate synthetic evaluation requests. Historical reports retain their
actual results; failed reports are not rewritten to passed. Current vector identity,
dimensions, source eligibility, permissions and bounded result ranking still apply.
Similarity is ranking evidence, not a probability that a passage answers a question.

An optional note_chunking generation binding may group numbered source units before
embedding. The model supplies boundaries only. Application code validates complete,
ordered, non-overlapping coverage and size, and materializes unchanged source text.
Invalid/unsupported proposals fall back to deterministic rules. Plans are derived,
Vault-scoped and persisted by source identity; queries never invoke generation.
Existing valid vector inputs are retained; enabling this role does not force a
whole-Vault rewrite. Source changes can produce a new plan. Changing embedding
output dimension is an explicit model setting, not an inferred maximum capability.

## Consequences

Semantic retrieval can return related passages that do not answer the question;
source evidence and bounded results make this limitation explicit. A passing
synthetic benchmark cannot establish private-Vault accuracy. Model grouping adds
optional ingestion cost and latency and does not guarantee better retrieval;
independent comparisons remain necessary before claiming quality improvements.

## Amendment — 2026-09-06: rule-only chunking

User explicitly withdrew model grouping after real comparisons failed to demonstrate
improvement. This supersedes the optional note_chunking decision above. Production
preparation, resolution and retrieval use bounded deterministic rule chunks; no
grouping generation or Admin role admission remains. Legacy plan/binding rows are
inert, retained for additive schema compatibility. Existing rule vectors remain
valid; model-grouped keys are not eligible and normal scheduling fills missing rule
vectors. Diagnostic-only evaluation policy remains unchanged.
