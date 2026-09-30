# Semantic Shadow and Rollback Runbook

This runbook describes the M7-A1 dry-run and M7-A2 local isolated fixture boundaries.
M7-A1 only copies explicitly allowlisted synthetic/temporary source bytes and does
not publish. M7-A2 publishes only inside a temporary synthetic source/shadow fixture
to exercise the normal path; it never publishes or modifies production source cards,
switches a read pointer, or cleans old data.

## Runtime failure and degradation policy

The semantic-card/MemoryPack reader is the default memory context. If a card or
pack cannot be read because its source, permission, revision, support, or rule
fence is not current, disable that card/pack read and degrade to the authorized
source retrieval path (`search_notes`/`read_note`). Do not fall back to the old v3
automatic memory reader, overview, extraction, or reconciliation jobs.

The service no longer admits `memory.*` automatic jobs at startup or during
periodic reconciliation, and file events enqueue only the semantic source path.
Historical v3 rows, managed Markdown, ordinary notes, revision history, and
SQLite job/audit records remain in place for explicit raw management and
storage/legacy diagnostics. This change performs no cleanup or data deletion.

## Local dry-run boundary

`mcp-vault-eval::prepare_shadow_dry_run` requires absolute, non-overlapping
`source_root`, `shadow_root`, `shadow_state`, `shadow_history`, and
`artifact_root` paths. Production and `vault` path components are rejected. The
manifest allowlist must contain exactly every source logical ID and file ID.

Only manifest paths are copied. Each source is read from the explicit temporary
source root, checked against its content hash, written with create-new semantics
to the shadow root, and read again to prove the source was unchanged. The shadow
plan records copied IDs, manifest hash, independent state roots, and whether
outbox/rules drift requires replay or rebuild. It never applies those changes.

The shadow Vault identity, SQLite state, history, and outbox/rules cursor are
separate planning inputs; the source database, jobs, cards, rules, and read
pointer are never reused. Mock/replay artifacts contain hashes and safe status
codes, not source bodies, prompts, tokens, or Provider responses.

## Local isolated execution fixture

The M7-A2 fixture additionally exercises the copied synthetic source in a separate
Vault Core/SQLite/history set. It creates the source revision and manifest fence
through Core/State, binds the shadow plan to its explicit state/history paths and
shadow `VaultContext`, then publishes a deterministic structure-only proposal through
`SemanticMemoryService`. Card listing and managed Markdown verification remain
Core-backed. Reopening the same shadow database with the same Vault identity proves
that `SuppressRead` survives restart: the managed card file remains present while
service reads return no card. Source file/card/revision/history observations are
compared before and after and must be unchanged. This is an isolation and rules
continuity test, not semantic-quality or production-shadow evidence.

## Local test-client simulation

M7-A3 adds a test-only `ShadowReadClient` to the isolated fixture. Its default target
is the source Vault; switching to the shadow target is explicit and uses the public
Core-backed semantic facade with `ReadMemory` and `ReadVault`. Each read/switch derives
the current source FileRecord revision/content hash, semantic source revision, manifest
hash, shadow rules revision, and a clearly named per-source-file aggregate outbox
checkpoint from the temporary fixture. This is not a claim to read a global production
outbox cursor. A real source-file mutation and a real shadow rule mutation make the old
client fail closed; a source-target client cannot switch after the shadow rule drift and
keeps its source target. After a verified fence refresh, the shadow read returns no card
when Suppression is active. Any drift rejects switching and reads without changing the
source default target. After shadow suppression and same-identity restart, the shadow
target remains unreadable while the source target remains readable. This is only a local
client simulation and does not create a production pointer or imply cutover readiness.

## Production authorization before any real shadow run

No production run is implied by the local harness. A future real shadow build
requires all of the following as a separate approved change:

- explicit written authorization for the named Vault, source allowlist, Provider,
  model, prompt/schema versions, and spending ceiling;
- a verified paired backup of the database, canonical Vault files, history,
  correction/suppression rules, and the relevant outbox cursor;
- a dedicated shadow database, history root, artifact directory, and service
  identity with no source-root overlap;
- a test client or isolated endpoint using the shadow read path while the normal
  pointer remains unchanged;
- a recorded source/rules/outbox fence and a review of drift before every replay
  or rebuild.

Missing authorization, allowlist coverage, budget, Provider/model binding, or
paired-backup evidence is a fail-closed stop before external work.

## Rollback and cleanup

Rollback is a pointer/configuration operation only after an authorized review:
stop shadow workers, preserve the shadow artifact and audit record, verify the
source and rules fences, and point the test client back to the prior reader. Do
not restore an old database over newer source changes. If current permissions or
suppression rules cannot be proven, disable semantic-card reads and fall back to
authorized source retrieval. Never restore the retired v3 automatic reader as a
compatibility fallback.

Shadow roots, artifacts, old cards, and old data are not automatically deleted.
Cleanup requires a separate scope, paired backup, review of audit/retention
requirements, and explicit authorization. M7 production shadowing, client
cutover, rollback rehearsal, and cleanup remain pending.
