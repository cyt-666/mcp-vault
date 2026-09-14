# Source-preserving memory units

This is the normative v3 memory contract. [ADR-0033](adr/0033-source-preserving-memory-units.md) supersedes the generated-body, consolidation and conversion contracts in ADRs 0026–0032 where they conflict. Historical SQL migrations remain unchanged; the runtime does not read predecessor memory formats or tables.

## Ownership and canonical content

An explicit `MemoryUnit` stores the authorized submitted body without trimming, case folding, translation or model rewriting. Optional type, importance, confidence, tags, entities, validity and sources remain optional. Updating it requires its expected revision. Idempotency compares the exact input, including whitespace and letter case.

An automatic unit belongs to one current source set, identified by Vault and stable source File ID. Its body is a complete span from the original Markdown plus necessary ancestor headings and introductory context. The server creates candidate identities, byte and line coordinates, full-source hashes and provenance. A model can choose candidate IDs, one of `preference`, `constraint`, `decision`, `experience`, `procedure`, `state`, and an auxiliary retrieval hint. It cannot generate or edit the memory body, fabricate IDs, or supply canonical paths or source identity.

Automatic selection targets actual preferences, constraints, adopted decisions, task state and practical experiences or procedures. Generic tutorials, third-party architecture descriptions, reference material and reading directories remain ordinary knowledge retrieval inputs. A one-off delivery detail (including presentation layout, slide copy, asset lists, file naming, or a purely presentation-specific step) remains in its source note even when it really happened or is phrased as “decided”. A mixed source is judged unit by unit: a durable decision or experience with an explicit adoption relationship, reason, and scope may be selected beside rejected delivery specifications. Important safety prerequisites, blockers, and committed next steps for a real task remain eligible even when they only need to be executed once. The file type and path are signals only; they are never a blacklist or a per-source quota. Facts from a reference architecture are not current project conventions without explicit current adoption evidence. Preserve time, version, and scope that the source actually states; a document's continued existence or a “current” title does not establish present validity, and missing dates must not be guessed. Historical author experience may still be selected within its stated scope. A complete selected unit retains the premise, scope, order, exception, and verification steps needed to interpret it. Selection quality is not guaranteed by schema validation: real-model evaluation and human review remain required.

The first pass keeps the `memory-source-unit-selection-v3` admission contract and its concrete boundaries. The active extraction profile is `memory-source-unit-selection-v3-minimal-review-v1`: minimal source units are selected first, then the independent completeness-review contract may keep, add displayed siblings, omit incomplete candidates, or explicitly replace them with a complete parent. Dates, versions, scope, safety prerequisites, exceptions, order, and verification steps remain part of a selected unit when the source provides them; a title or “current” wording does not prove present validity. Retrieval hints are short search aids and cannot add facts. The profile identity deliberately differs from previously active v4 and earlier selection-only checkpoints, so stale batches and published sets are not reused.

Canonical files are below the configured reserved root:

```text
memory-v3/explicit/{memory_id}.md    schema mcp-vault-memory-unit/v3
memory-v3/sources/{source_file_id}.md  schema mcp-vault-memory-units/v3
```

Explicit files preserve the submitted body exactly after the canonical separator. Source collections carry structured provenance and readable complete bodies. Parser validation checks hashes, rendered-body consistency and source coordinates against current original bytes. The source Markdown remains canonical user knowledge; FTS, vectors, caches and overviews are rebuildable. Canonical writes, deletions and recovery use Vault Core, revision history and durable outbox events.

## Selection and publication

Markdown parsing respects headings and complete nested sections, lists, tables, quotes and code blocks. Necessary ancestor introductions accompany child sections. A parent already selected with its complete descendant range subsumes that child's presentation within the same source publication; this does not authorize cross-source merging.

The first pass uses leaf or parent-introduction minimal units, then builds deterministic bounded completeness-review scopes from the source-owned full units. Both passes use at most 32 units and 60 KiB of final serialized input. Review can keep or omit first-pass units, add displayed minimal siblings, or explicitly replace them with a complete parent; byte containment never implies replacement. Sensitive and unreviewable oversized content is omitted with a stable diagnostic, never shortened or regenerated. Each first-pass ID is classified exactly once as review-owned, deterministic omission, or unrelated passthrough. Review checkpoints use the same Vault/source-hash repository with a distinct stage/schema/profile/input hash, and publication waits for every review scope.

Each source-processing invocation makes at most one uncached selection request; a backfill worker yields between unfinished batches. Validated results are checkpointed by Vault, source identity, full source hash, model/provider configuration, prompt/schema/batching profile and input hash. The v3 selection prompt/profile identity includes the complete selection instruction and its version, so changing the selection boundary cannot reuse an old published set or batch checkpoint. All batches must complete before one atomic source-set replacement. A crash or retry reuses matching validated batches. A changed profile causes a fresh evaluation; an explicit re-evaluation also changes the expected set generation used by the cache. Empty successful selections publish an empty evaluated set; skipped content is reported separately from a successful empty selection.

Prepared publication checks source identity/hash/revision, expected current set revision, canonical revision, source pause and global generation pause. Source changes immediately disqualify old automatic units through repository eligibility, before asynchronous rebuilding. Same-ID/same-hash moves update navigation without model generation. A delete/recreate is a new identity. Partial extraction is never exposed as a finished set.

Deleting an automatic unit removes it through a whole-set canonical rewrite and pauses that source. Other current units in the source remain readable. Only an explicit authenticated resume with the expected set revision clears the pause and requests evaluation. Deletion does not create a model-visible archive or supersession graph. History and backups remain operational recovery data.

## Retrieval and authorization

Normal `recall` does not call a generative model or scan the Vault. It uses eligible current projections with lexical, entity, tag and optional current-vector evidence. Original unit bodies receive more lexical weight than repeated ancestor context; auxiliary retrieval hints have a smaller separate ranking contribution. A soft source-diversity penalty favors distinct sources when relevance is similar. Explicit source filters are applied to the note lexical pool before its bound. Embedding and reranking are optional; provider failure degrades to lexical retrieval. A score orders candidates and does not establish factual correctness or semantic entailment. Exact body/context equality may fold presentation; semantic similarity never permits canonical deletion or rewriting.

The default budget is 4096 estimated Tokens, 12 complete units and 4 related-note cues; the maximum Token budget is 32000. The estimate uses serialized UTF-8 size divided by four, rounded up; it is an engineering output bound rather than a model-specific tokenizer count. Complete bodies, provenance, scope, diagnostic metadata and navigation are budgeted together. Bodies are never clipped. A unit too large for the remaining budget may contribute a `pointers` entry with a `get_memory`/resource read target; later smaller units are still considered.

Task `context.paths`, `context.entities` and `context.recent_topics` affect ranking without excluding other contexts. An explicit `source_path` is an exact filter. There is no manually maintained project registry. Related notes are separately typed current revision-bound navigation cues, not durable memory evidence.

Explicit memory reads require `memory:read`. Automatic bodies, their pointers and related-note cues additionally require `vault:read`. Filtering occurs before candidate counting/ranking, and the same eligibility applies to recall, get, list, resources and overview. A legacy, deleted, cross-Vault or stale source ID is not readable. Runtime state and all operational data are Vault-scoped.

## Overview

`get_memory_overview` and `vault://memory/context` return bounded navigation over current units. Inputs include exact `source_path`, directory `path_prefix`, `topic_ids`, `after_id`, `limit` (default 40), and a Token budget. Directory prefixes respect path boundaries. Topic filters resolve through the current knowledge map.

Generated sections contain a label, a navigation description and referenced current unit IDs. They are clearly marked `generated_navigation`, independent of original memory bodies. The generation service validates allowed IDs and stores dependencies on revisions, hashes and current source eligibility. Any dependency change invalidates the cache. Reads use current deterministic entries when the generated cache is missing or stale; they never wait for a model. Scoped queries can always use deterministic navigation. The background job builds bounded pages for the global overview and each current folder/topic in the existing knowledge map, using `memory_overview` with fallback to `memory_extraction`. One model request is allowed per overview invocation. Checkpoints record the current scope and page; source/index or model configuration changes restart the affected dependency generation. Stable job keys prevent repeated failed calls on unchanged inputs; an explicit run/resume can retry a failed overview.

Overview descriptions and retrieval hints are not sources for later automatic extraction and do not replace body evidence. No generated global description is a truth-maintenance system.

## Jobs and administration

- `memory.extract`: durable source selection, one uncached batch per invocation, complete-set publication.
- `memory.source_reconcile`: source movement, invalidation and deletion.
- `memory.overview`: bounded current-unit navigation generation with dependency checks.
- `embedding.rebuild`: independent note or `memory_unit` vector generation.

Admin exposes current units, explicit editing, automatic-unit copying, source pause/resume, extraction readiness and progress, skipped units, generation pause/resume/run, overview navigation and vector coverage. `/memory/generation` controls only the new work. No merge, candidate review, semantic calibration, legacy conversion or organization route is registered.

Fresh installations keep automatic extraction disabled until configured. Offline cutover enables the new policy but installs a new maintenance pause for protocol checks. Resuming generation admits a full ordinary-note backfill; it does not inherit old exclusions or source pauses.

## Cutover

Migration 0028 creates independent new tables and marks pre-existing Vaults as requiring offline initialization. It copies no legacy memories. Migration 0029 adds current knowledge-map dependency invalidation for directory/topic navigation. `mcp-vault initialize-memory --discard-legacy-memory` requires a process lock and exclusive SQLite ownership. It records a durable content-free manifest, terminalizes only incomplete Core intents proven to remain wholly inside the predecessor `memory/` namespace as `discarded` without replaying them, retires manifest files through Vault Core with exact hash guards, removes allow-listed Vault-scoped old memory rows/jobs/vectors/settings, and marks initialization ready. Cross-boundary or unclassifiable journals fail closed. Ordinary notes, attachments, unrelated managed files, accounts, credentials, Provider/model bindings and file history are preserved.

Interrupted initialization resumes the same manifest with exact hash guards. A completed initialization is a no-op on repeat and cannot clear newly created v3 units. Disabled Vaults retain their status. Existing historical migration files and their checksums are preserved. Pair the database, Vault content and history backup before cutover; see the operational cutover runbook for deployment-specific verification.

## Acceptance evidence

Engineering tests establish exact-byte preservation, validation, permissions, source invalidation, budget bounds, atomicity and recovery. Fake providers test this calling contract; they do not simulate language understanding. Real configured generation and embeddings must be tested against frozen original sources, with baseline note retrieval, difficult negatives and manual review of actual outputs. Report engineering, real-model and production acceptance separately, retaining failures and limitations.
