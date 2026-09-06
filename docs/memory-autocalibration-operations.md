# Automatic retrieval calibration and memory administration

This contract implements [ADR-0027](adr/0027-bounded-automatic-retrieval-calibration.md)
within [ADR-0026](adr/0026-current-source-owned-memory-sets.md)'s current-set model.
Canonical Markdown, source ownership, physical deletion, and extraction pauses retain
their meaning. Calibration never migrates legacy memories, resumes sources, invokes
extraction, or rewrites business vectors.

## Preparation and startup

After recovery and worker registration, the first server reconciliation tick admits
`retrieval.calibrate` for each ready Vault with an allowed, enabled embedding role and
no applicable server report. Existing role bindings and vectors are sufficient: no
Admin visit, binding update, regeneration, or manual API request is required. Later
reconciliation ticks (normally every 300 seconds) compensate for missed events.
Admin embedding binding, Provider edits and Provider-mode changes, successful embedding
jobs, and new Vault initialization also call the same ensure service. A global binding
change checks each Vault independently. Reconciliation admits Vaults in bounded pages
of 64 and checks cancellation between pages; missed events are covered by later ticks.

Memory and note channels have separate signatures and quality gates. A missing note
role does not block the memory channel. An embedding role is sufficient; generation
and reranking roles are not required. Valid existing business vectors are reused.
Coverage remains independent of calibration. A passed report cannot repair missing
or stale vectors, and 100% vector coverage cannot replace calibration.

The signature includes Vault, role, embedding profile, document and query preparation,
lexical admission, object ranking policy, and the bundled corpus fingerprint. Reports
are stored under their complete signature, so an older task cannot overwrite a newer
configuration's publication. Reads only accept a matching passed server report. A
report saved before a crash is reused after checking the current signature again.
An endpoint silently replacing a model without changing any configuration cannot be
detected from its name; operators must change the model identity/profile to invalidate
that preparation.

## Evaluation and honest interpretation

The bundled `calibration.json` contains 40 synthetic source documents and 120 labeled
queries. Calibration and holdout splits have disjoint documents; each split contains
40 answerable and 20 no-answer queries, including 20 answerable cross-language queries
without lexical admission. Inputs use the production memory normalization or note
chunking and query envelopes. Short-lived in-memory FTS5 indexes reproduce production
candidate column/tokenizer shapes and BM25 ranks. Shared admission and object scoring
functions combine lexical and semantic evidence; exact cosine calculation is also shared
with the production vector backend. Synthetic vectors/checkpoints never
enter business vector collections, canonical files, or recall counters.

Threshold candidates are midpoints of calibration-split similarity boundaries. The
selected floor is frozen before holdout evaluation. Both splits require mean object
Recall@5 >= 0.70, mean returned Precision@5 >= 0.80, no-answer false-return rate <= 0.05,
and pure-semantic Recall@5 >= 0.70. Holdout recall cannot fall below its lexical control.
Empty returns have zero precision on answerable queries. Recall divides by all labeled
relevant objects rather than treating any hit as full recall. Invalid dimensions,
missing vectors, zero vectors and non-finite values fail evaluation rather than
becoming successful no-answer results.

When both configured channels have applicable reports, status and recall also check the
union of holdout no-answer failures. Two different false returns out of 20 fail the
combined 5% gate even if each channel individually passed. Both semantic paths then
remain inactive with `joint_no_answer_quality_failed`; reports are retained for diagnosis,
lexical retrieval remains available, and automatic compensation does not create retry
storms. Admin shows the combined sample count, failed case IDs and rate. Changing a
model/profile or explicitly retrying remains possible.

Reports identify `evaluation_scope=builtin_benchmark`, configured profile, corpus hash,
implementation package version, build commit and checkout state, split/subset metrics, failed case IDs, absolute
no-answer false-return counts, actual HTTP requests/bytes, and elapsed wall time.
One error out of 20 no-answer samples is already 5%; this small synthetic benchmark
is a gate, not a guarantee about private Vault quality. Release evidence must retain
the build commit alongside the report. Source archives should set `MCP_VAULT_BUILD_COMMIT`
at build time; missing provenance is reported as `unknown`, never invented. Dirty
worktree builds explicitly report `implementation_source_state=dirty`. Contract fake metrics in CI are never evidence
of a deployment model's semantic or multilingual quality.

## Bounded execution and controls

The normal ProviderService adapters perform actual requests, with existing encrypted
credentials, ProviderMode, endpoint restrictions, capabilities and dimension checks.
Transport accounting reserves every HTTP attempt, including internal retries, before
dispatch. A refused reservation makes no request. Completed batches persist a bounded
synthetic cache; worker/process retries cannot reset consumption or the deadline.
The serialized checkpoint limit is 32 MiB per channel/signature (report limit 256 KiB).
A 160-input, 3072-dimensional cache is covered by a database round-trip regression.
An oversized cache fails explicitly, rather than discarding already spent request counts.

Defaults per channel/signature round: 512 unique inputs, 2 MiB request bytes, 32 HTTP
attempts, batches of 16, 15 minutes. One active durable job per Vault handles its
channels serially; the registered worker allows at most two such jobs globally.
Transient failures use the existing bounded job retry mechanism. Quality failures and
terminal execution failures require an explicit retry. Admin cancellation also stops
a queued signature; graceful process shutdown preserves resumable work. Pausing
maintenance stops further calibration requests but keeps applicable query reports.
Already dispatched HTTP requests cannot be recalled.

The maintenance endpoint accepts an optional typed engineering `budget` for future
signatures: `max_requests` (1–128), `max_bytes` (1024–8388608), `max_inputs` (1–512),
`batch_size` (1–64), and `timeout_seconds` (1–3600). Omitted fields use defaults. Limits
are frozen when the run is admitted. Manual retry grants another bounded round using
those frozen limits, retains cumulative counters and completed synthetic batches, and
preserves the previous report. Quality thresholds cannot be supplied by a client.
The current signature plus eight other terminal signatures per channel are retained;
older terminal checkpoints/publications are pruned during subsequent execution.

## Admin operations and protocol mapping

All paths below are relative to `/api/v1/vaults/{slug}`. Session authentication is
required; state changes require CSRF and permitted Origin. GET requests never admit
calibration. Existing unscoped compatibility routes resolve the configured default
Vault and do not permit arbitrary cross-Vault access.

| User operation | Request | Application operation | UI evidence |
| --- | --- | --- | --- |
| Inspect legacy migration | POST `/memory/migration/preflight` | `migration_preflight_v2_1` | Classified report and fingerprint |
| Execute reviewed migration | POST `/memory/migration/execute` with fingerprint and `MIGRATE_MEMORY_V2_1` | `migrate_legacy_v2_1` | `migration.completed`, unresolved IDs and regeneration job; 409 requires a fresh preflight |
| Inspect preparation | GET `/memory/semantic-calibration` and `/memory/embeddings` | `calibration_status` / `embedding_status` | Independent channel status, report and coverage |
| Run/retry preparation | POST `/memory/semantic-calibration/run` with `{ "channel": "memory" }`, `note`, or `all` | `request_retrieval_calibration` | 202 and admitted/reused job ID; UI never submits quality metrics |
| Pause/resume maintenance | PUT `/memory/semantic-calibration/maintenance` with `enabled`, optional `budget` | Maintenance/budget application services | Separate from semantic eligibility |
| Cancel current work | POST `/jobs/{id}/cancel` | `cancel_retrieval_calibration` for calibration jobs | Durable cancellation in Jobs |
| Inspect paused sources | GET `/memory/extraction/sources?paused=true&limit=50&offset=0` | Current-source repository projection | Includes zero-item sets and missing-source eligibility |
| Resume one source | POST `/memory/extraction/sources/{id}/resume` with `expected_set_revision` | `resume_note_extraction` | Durable resume intent before canonical mutation; source-state update and extraction-job insert share one DB transaction |
| Edit explicit memory | PATCH `/memories/{id}` with `expected_revision` and changed fields | `update` | Omitted metadata preserved; explicit null clears supported optional fields |

The memory screen loads endpoints independently and exposes failures locally. Its
count describes loaded rows, not a fabricated global total. Both memory and paused
source lists have pagination. Deleting the last derived item leaves an empty paused
source visible for explicit recovery. Missing source notes cannot be resumed.
Resume returns `resume_accepted=true` and a `job_id`: normally the extraction job,
or the durable `memory.source_resume` job if reconciliation must finish the canonical
commit first. Its fixed request time and expected set revision make replay idempotent.
A crash after the canonical commit is recovered without overwriting a newer pause;
successful replay inserts exactly one extraction job. Resume itself does not call a
Provider; the subsequently authorized extraction job may do so.
Deletion after projection rebuild does not require the original Provider/model;
remaining items retain provenance and unchanged vectors.

## Retrieval, language and section diagnostics

All memory retrieval paths apply type, validity and importance filters again when
reading current bundles. Context contributes ranking only after query relevance has
admitted the object. Semantic rank counts valid unique objects after source/profile
and request filters; extra chunks of one object do not demote another object.
Related notes in recall use the same lexical gate and their independent calibrated
semantic floor. General `search_notes` retains its discovery behavior.

Recall budgets estimate complete serialized JSON bytes divided by four, rounded up.
They include escaped content, full headings, tags, paths, provenance, resource URIs,
score diagnostics and response metadata. This is an explicit conservative estimator,
not an exact tokenizer. Oversized objects are skipped; the first item is not exempt.
The final envelope is checked before recall counters are updated. A budget too small
to hold response metadata is rejected explicitly.

`matched_section` includes `chunk_key`, `heading_path`, `projection_start_byte`,
`projection_end_byte`, and `revision`. Offsets refer to the indexed plain-text
projection, not raw Markdown line numbers. Semantic snippets come from the winning
chunk; lexical section evidence is located within current derived chunks. Results
remain deduplicated by note. With `result_granularity=section`, a located section is
identified in each result; an unlocated result remains explicitly note-level. Use
`read_note` for canonical text. These locator additions do not change embedding text.

Extraction prompt `memory-current-set-v2-language-coverage` preserves the supporting
passage's language and literal identifiers. It explicitly covers source frontmatter,
completed work, study periods, environment, conditional experiments and future stages,
without turning tutorials or suggestions into adoption. The server sends the full
bounded source and validates the structured set. A prompt upgrade alone does not
re-extract unchanged current sources or discard recoverable prepared snapshots.
The 15-source `generation-coverage.json` fixture supplies required facts and forbidden
claims for separately authorized real-model review; offline transport assertions do
not claim generation quality.

## Upgrade and rollback

1. Stop writes and take a coordinated backup of the database, canonical Vault and
   revision history; retain the separately protected installation master key. Record
   the application commit and current ProviderMode/role configuration.
2. Start the new binary with the existing state. Forward migrations 0016 and 0017 run
   through the normal migration runner. 0016 makes deletion-snapshot Provider/model
   references nullable while copying prepared snapshots unchanged. 0017 adds only
   operational calibration state and the calibration-job singleton index.
3. Startup automatically prepares configured embedding channels. No legacy-memory
   migration, extraction resume, full extraction or business-vector rebuild is part
   of this step. Inspect channel reports and coverage independently. If the configured
   model fails quality, lexical recall remains available and diagnostics explain why.
4. Legacy memory conversion is a separate reviewed Admin preflight/execute operation.
   Resume individual paused sources only when regeneration is intended.

There is no destructive SQL down migration. For a binary rollback, stop the service,
preserve a backup of all post-upgrade writes, and restore the coordinated pre-upgrade
state/Vault/history with the matching old binary and key. Do not point an older SQLx
binary at a newer migrated database or copy only part of the canonical/operational
state. If keeping the new binary, pausing calibration is sufficient to stop new
maintenance work; it does not remove valid reports or alter source pauses. Interrupted
canonical snapshot publication uses existing reconciliation, and interrupted
calibration resumes from its persisted budget/cache.
