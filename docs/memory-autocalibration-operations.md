# Automatic retrieval calibration and memory administration

This contract amends [ADR-0027](adr/0027-bounded-automatic-retrieval-calibration.md) with ADR-0028
within [ADR-0026](adr/0026-current-source-owned-memory-sets.md)'s current-set model.
Canonical Markdown, source ownership, physical deletion, and extraction pauses retain
their meaning. Calibration never migrates legacy memories, resumes sources, invokes
extraction, or rewrites business vectors.

## Preparation and startup

[ADR-0028](adr/0028-model-guided-chunks-and-diagnostic-evaluation.md) supersedes
the automatic quality gate in ADR-0027. Existing current vectors can participate in
semantic retrieval without a bundled benchmark report. Startup, role binding and
ordinary reconciliation no longer schedule synthetic evaluations. Schema 0019 retires
old automatically admitted jobs while retaining spent budgets, caches and reports;
explicit diagnostic jobs remain resumable. Model availability, permissions, vector
identity/dimension and current-source checks still apply.

Admin may explicitly run the bounded diagnostic. Its historical `active` field means
an applicable passing diagnostic report, not production semantic eligibility; the UI
labels it as benchmark success. Missing/failed reports do not disable normal retrieval.
The maintenance endpoint now permits/pauses diagnostic dispatch only. Neither endpoint
re-extracts memory or rewrites valid business vectors.

## Optional model grouping and default limits

Model grouping was withdrawn by user decision (ADR-0028 amendment). Preparation,
source resolution and retrieval always use deterministic rule chunks. Admin no
longer offers the note_chunking role. Legacy bindings and cached plans, including
pending records, are ignored; no grouping LLM request occurs.

Default bounds remain: note embedding chunks are at most 2048 UTF-8 bytes including
up to 512 context bytes and separators. Unconfigured embedding context retains the
8192-byte service ceiling; a configured context window is a conservative byte ceiling.
Vectors must have finite compatible dimensions; absent dimensions retain the 8192
resource ceiling, while configured dimensions require an exact output match. These
are validation limits, not instructions to generate maximum-dimensional vectors.

Existing valid rule vectors are reused. Legacy model-grouped keys are excluded from
current retrieval and normal scheduling fills missing rule vectors. No canonical
note rewrite, memory re-extraction, user pause removal or schema downgrade occurs.
Schema 19 and historical reports remain for compatibility. Rolling back only this
policy requires a prior compatible binary, not destructive database migration.

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

When both channels have reports, diagnostic status also reports their combined
no-answer failure IDs. A failed combined score remains a diagnostic failure; it
has no effect on production retrieval. The benchmark still evaluates its historical
threshold policy to preserve comparability and read-only cache replay. It does not
represent the new production ranking policy or prove private-Vault accuracy.

## Read-only diagnosis of an existing failed run

The `diagnose-calibration` operator command recomputes one exact existing run from
its saved synthetic vectors. It opens SQLite read-only without migrations, reads only
the selected Vault/channel/signature, and creates an in-memory synthetic FTS index.
It does not initialize Provider credentials, start workers/listeners, send model
requests, publish a threshold, or read canonical Vault files. Standard file-access
permissions authorize this local operator command; it is not an MCP tool.

The JSON contains raw answer ranks before admission, every calibration threshold trial
and its failed gates, one closest trial selected on calibration data only, and a replay
comparison with the original report. A closest trial is diagnostic evidence, not an
enabled or passing configuration. It exports neither raw vectors, arbitrary cache keys,
database/Vault paths, secrets, nor private notes. Unknown corpus hashes and incomplete
caches fail explicitly; the command never fetches missing embeddings.

This command was added after the original 0.2.2 commit `6500214`; an image built from
the updated source is required. After building that image, run a temporary command
container from the deployment's Compose directory (no service restart is needed):

```bash
docker compose run --rm --no-deps -T mcp-vault diagnose-calibration \
  --vault default --channel note \
  --signature sha256:700548b45805452e35a386e696b6ae753ee3c7f011721f214a4c1e50e8a29ace \
  > calibration-diagnostics.json
```

The signature above targets the user-reported failed note run. Use the actual Vault
slug instead of `default` if different, and use the exact `profile.signature` from
the relevant run for other diagnoses. The command uses the Compose service's existing
database configuration and mount. Check its exit status before sharing the JSON;
errors go to stderr and do not create a successful diagnostic report. The running
service and saved calibration result are not changed by this command.

Diagnosis limitations: the old failure report used 0.999999 when no threshold passed;
its zero pure-semantic score is not an unconstrained measurement of model ability.
Version 0.2.2 also ignored embedding response `index` values. Updated decoding restores
input order and rejects duplicate/missing/mixed/out-of-range indices; adapters omitting
every index keep their positional contract. Cached vectors alone cannot prove whether
an old Provider response was reordered. No automatic business-vector rebuild or paid
calibration retry is triggered by this parsing fix.

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
maintenance stops further calibration requests but retains prior diagnostic reports.
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
Related notes in recall use lexical evidence and current-vector similarity ranking
without a benchmark-derived semantic floor. General `search_notes` retains its discovery behavior.

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

## Explicitly authorized real generation evaluation

`crates/memory/examples/real_generation_eval.rs` exercises the 15 synthetic
`generation-coverage.json` sources through the normal extraction and Provider path.
It requires an explicitly authorized 15-request allowance and a `memory_extraction`
binding in the isolated local test installation. It is not a startup task.
Build with `cargo build --locked -p mcp-vault-memory --example real_generation_eval`,
then invoke `target/debug/examples/real_generation_eval --source-data LOCAL_TEST_DATA
--output NEW_OUTPUT_DIRECTORY --authorized-requests 15` (one command line).

The source database is read-only. Necessary Provider configuration is copied into
an ephemeral installation with a new key; synthetic canonical writes use its Vault
Core. The evaluator disables transport retries, charges its shared budget before
every dispatch, persists each result, and refuses an existing output directory.
An interrupted run retains evidence but must not be restarted as an assumed free
retry. Reports require source/output review; contract success alone is not a quality
score. See the [authorized run and review limitations](exec-plans/reports/memory-system-regression-20260906.md).

## Upgrade and rollback

1. Stop writes and take a coordinated backup of the database, canonical Vault and
   revision history; retain the separately protected installation master key. Record
   the application commit and current ProviderMode/role configuration.
2. Start the new binary with the existing state. Forward migrations 0016–0019 run
   through the normal migration runner. 0016 makes deletion-snapshot Provider/model
   references nullable while copying prepared snapshots unchanged. 0017 adds only
   operational calibration state and the calibration-job singleton index. 0018
   separates embedding identity revisions from Admin edit revisions, initialized
   to preserve existing fingerprints and vectors. It also closes unfinished
   calibration rows left by terminal jobs, retaining caches and spent requests;
   rows with an active job remain resumable. Later display-name/enabled-only edits
   and identical model saves preserve vector identity and calibration signatures.
   0019 adds derived note grouping and retires automatically admitted diagnostics.
3. Startup preserves configured embedding channels and does not run synthetic evaluations. No legacy-memory
   migration, extraction resume, full extraction or business-vector rebuild is part
   of this step. Inspect current vector coverage; diagnostic quality does not gate retrieval.
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
