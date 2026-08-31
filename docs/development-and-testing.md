# Development and Testing Guide

## 1. Toolchain

Use a pinned Rust stable toolchain through `rust-toolchain.toml`.

Recommended project tools:

```text
cargo fmt
cargo clippy
cargo nextest
cargo deny
cargo audit
cargo llvm-cov
sqlx-cli
cargo machete
pnpm
Node.js LTS
Playwright
Docker / Docker Compose
WebDAV Litmus
official MCP conformance suite
```

CI must use lockfiles.

## 2. Backend dependencies

Preferred categories:

| Capability | Direction |
|---|---|
| Async runtime | Tokio |
| HTTP | Axum, Hyper, Tower, tower-http |
| MCP | Official `rmcp` SDK |
| WebDAV | `dav-server` behind project adapter |
| SQLite | SQLx with SQLite |
| Serialization | Serde, serde_json, serde_yaml |
| Errors | thiserror; anyhow only at process/task boundary |
| Logging | tracing, tracing-subscriber |
| HTTP provider client | reqwest with controlled redirect/TLS policy |
| Passwords | argon2 |
| Secret encryption | chacha20poly1305 or aes-gcm, secrecy, zeroize |
| Markdown AST | Comrak plus project Obsidian syntax analysis |
| Local embedding | optional fastembed feature |
| Vector index | internal trait; pinned sqlite-vec backend plus exact fallback |
| IDs/time | UUIDv7 or ULID, chrono/time |
| Testing | tempfile, proptest, insta, wiremock or equivalent |

Dependency choices are reviewed for maintenance, licenses, security, and supported Rust version before adoption.

`sqlite-vec` is pre-1.0 and must remain behind an internal abstraction.

## 3. Frontend dependencies

Recommended:

- React + TypeScript;
- Vite;
- Ant Design;
- TanStack Query;
- React Router;
- Zod;
- Vitest + Testing Library;
- Playwright;
- MSW for API mocks where useful.

Use `pnpm` and commit `pnpm-lock.yaml`.

## 4. Workspace rules

Suggested workspace:

```text
crates/
├── domain
├── vault-core
├── storage-fs
├── state
├── auth
├── webdav
├── mcp
├── indexer
├── memory
├── providers
├── admin-api
└── server
```

### Dependency rules

- `domain` depends on no infrastructure crate.
- `vault-core` has no Axum, RMCP, DAV, or React concerns.
- protocol crates depend on application services, not the reverse.
- SQL is confined to `state`.
- filesystem primitives are confined to `storage-fs`.
- provider HTTP logic is confined to `providers`.
- frontend code never shares generated domain code unless schemas are intentionally generated and reviewed.

Use `cargo metadata` or a dependency-graph check in CI to prevent forbidden edges.

The server embeds the compiled Admin frontend from
`frontend/admin/dist/` with `rust-embed`. That directory is intentionally
ignored by Git, so any Rust build or test that exercises Admin assets must run
the frontend build first. The repository `make test` target and the CI/release
Rust gates perform this prerequisite explicitly; the separate frontend job
does not share its generated files with the Rust job.

## 5. Rust style

### Errors

Define stable domain errors:

```rust
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("entry not found")]
    NotFound,
    #[error("revision conflict")]
    RevisionConflict { expected: i64, current: i64 },
    #[error("invalid vault path")]
    InvalidPath,
}
```

Map errors at protocol boundaries.

Never return raw SQL, I/O, provider, or crypto errors to clients.

### Async

- Do not call blocking filesystem/model code on Tokio core threads.
- Use bounded `spawn_blocking` pools or dedicated workers.
- Propagate cancellation.
- Avoid detached tasks for durable work.
- Use persistent jobs for retryable work.

### Transactions

- Application services own transaction boundaries.
- Repositories do not secretly open nested independent transactions.
- Outbox insertion occurs in the same transaction as operational metadata.
- SQLite busy handling and connection-pool admission are separately bounded
  and observable.
- Bursty request paths queue their short SQLite write phases through the shared
  state write gate. Do not serialize file streaming or canonical filesystem
  work merely because SQLite permits one writer.
- Read-before-write metadata commits use an immediate transaction so they do
  not attempt to upgrade a stale WAL snapshot.

### Traits

Use traits for:

- storage;
- clock/ID source in deterministic tests;
- repositories or unit-of-work boundaries;
- LLM/embedding/reranker providers;
- vector index;
- lock/history abstractions where substitution is real.

Do not create “interface for every struct” boilerplate.

### Secrets

Secret-bearing types must redact `Debug` and avoid accidental serialization.

### API models

Define separate:

- HTTP/MCP input DTO;
- validated command/query;
- domain model;
- output DTO.

## 6. Markdown analysis

Use a real Markdown AST for CommonMark/GFM structure.

Obsidian extensions require project code:

- `[[wikilinks]]`;
- `![[embeds]]`;
- block references;
- tags outside code;
- YAML frontmatter;
- heading anchors.

Do not parse wikilinks/tags with a global regex that also matches fenced code.

Maintain fixtures containing:

- Chinese headings/tags/paths;
- aliases;
- code blocks with fake links;
- escaped syntax;
- nested parentheses;
- Unicode normalization;
- malformed frontmatter;
- large notes.

## 7. Database development

- SQLx migrations under `migrations/`.
- CI creates a new database and upgrades a prior fixture.
- Enable foreign keys in every connection.
- Test constraints and Vault predicates.
- Use repository query helpers that require `VaultId`.
- Avoid dynamic SQL assembled from untrusted sort/filter names.
- FTS and vector tables need reconciliation tests because extensions may not enforce ordinary foreign keys.
- Backup catalog migration, SQLite snapshot/restore, manifest checksums, and
  clean-host restore fixtures must run against real temporary roots.

## 8. Test pyramid

### 8.1 Unit tests

Fast tests for:

- path normalization;
- revision/precondition evaluation;
- content hashing;
- Markdown parsing;
- link resolution;
- memory normalization/lifecycle;
- rank fusion;
- temporal decay;
- permission/scope mapping;
- secret redaction;
- provider response validation.

### 8.2 Property and fuzz tests

Use property/fuzz testing for:

- path decode/normalization;
- DAV destination paths;
- unified-diff parser;
- Markdown/frontmatter parser;
- memory JSON schema handling;
- backup archive validation;
- duplicate/link/device/oversized archive entries and traversal rejection;
- header/body matching;
- ranking determinism.

### 8.3 Repository tests

Run against real SQLite with migrations.

Every repository that contains Vault data has isolation tests with Vault A and Vault B.

### 8.4 Integration tests

Test application services with real temporary filesystem + SQLite:

- create/edit/move/delete/restore;
- file/DB crash recovery;
- outbox/job idempotency;
- external edit reconciliation;
- index rebuild;
- FTS path/tag/topic/time filters and bounded cursors;
- taxonomy overlay validation and managed-namespace read isolation;
- resolved links, unresolved links, backlinks, and related-note scoring;
- deletion of derived rows followed by a Vault-scoped rebuild;
- public MCP lexical search with two-Vault authorization isolation;
- provider failure;
- provider adapter contracts use local fake HTTP servers only;
- SSRF/redirect/privacy/retry/concurrency and response-schema failures;
- embedding dimension/model/Vault partition and re-embedding job references;
- deterministic bounded note chunks, missing/stale vector scheduling, semantic
  paraphrase retrieval, lexical fallback, and two-Vault note-cue isolation;
- memory materialization;
- forced Unix `RENAME_NOREPLACE` capability failure followed by a complete
  same-directory link commit, destination-race preservation, temporary cleanup,
  and explicit failure when safe atomic creation is unavailable.

### 8.5 Protocol tests

#### WebDAV

- DAV method behavior;
- preconditions and ETags;
- locks;
- ranges;
- streaming;
- auth/revocation;
- path attacks;
- two-Vault endpoint/credential isolation;
- overwrite COPY/MOVE and recursive directory moves;
- interrupted staged PUT journal cleanup;
- Litmus.

#### MCP

- `server/discover`;
- protocol negotiation;
- required metadata/headers;
- deterministic tool list by scope;
- structured schemas/results;
- resource reads;
- PAT and OAuth;
- official conformance suite.

#### Admin

- setup;
- session/CSRF/Origin;
- automatic installation-key creation, restart reuse, password-only first-Admin
  setup, concurrent single-winner behavior, and missing-established-key
  rejection;
- Admin requests remain authenticated without application-owned source-CIDR
  admission;
- secret masking;
- one-time WebDAV/PAT response display without plaintext in list endpoints;
- successful mutation audit entries without password content;
- LAN/public listener separation;
- provider/config validation.

### 8.6 End-to-end

Use Docker Compose and actual HTTP.

Scenarios:

- password-only first-Admin setup through Obsidian credential creation;
- DAV sync fixture;
- MCP Agent-like discovery/search/read/edit;
- remember/recall;
- provider outage/degradation;
- backup/restore;
- upgrade migration;
- public proxy cannot access Admin.

## 9. WebDAV interoperability suite

Maintain fixture scripts for supported plugin versions.

Because Obsidian plugins run in a GUI environment, combine:

- automated DAV protocol simulation matching observed requests;
- manual release checklist on desktop/mobile;
- captured sanitized request-shape fixtures;
- Litmus.

Do not encode one plugin’s bugs as general behavior without tests and documentation.

The current automated adapter slice can be run with:

```bash
cargo test -p mcp-vault-storage-fs --all-features
cargo test -p mcp-vault-core --all-features
cargo test -p mcp-vault-webdav --all-features
cargo test -p mcp-vault-server --all-features
```

The WebDAV integration fixture exercises the public adapter boundary with
Basic challenge, ETags and ranges, streamed binary PUT, PROPFIND, MKCOL,
COPY/MOVE/DELETE, LOCK/UNLOCK, overwrite behavior, directory trees, path
attacks, expiry/revocation, forwarded-HTTPS transport, and two-Vault isolation.
Run Litmus and the sanitized Sync Engine/Remotely Save fixtures against a
real HTTP server before release; the in-process fixture is not a substitute
for those client checks.

The disposable real-process fixture and public smoke can be run with:

```bash
bash scripts/interop/http-smoke.sh
```

It creates a temporary SQLite/Vault root, starts the actual data and control
listeners, performs 50 concurrent nested WebDAV PUTs and reads every result
back, and deletes only that fixture after the check. Its outer MCP PAT injection
is test-only and does not alter production authentication. The in-process
WebDAV regression additionally asserts that the same burst leaves no
`prepared` or `file_committed` operation journal rows.

## 10. MCP conformance

Use the official conformance tooling compatible with the selected `rmcp` version.

Test all protocol revisions advertised by the server.

At minimum verify:

- 2026-07-28 stateless HTTP;
- discovery;
- tool/resource listing and caching fields;
- standard routing headers;
- structured output;
- authorization challenge behavior;
- backward compatibility negotiated by SDK.

Never advertise a revision not exercised in CI/release validation.

The repeatable WP-14 entry point is:

```bash
bash scripts/conformance/mcp.sh
```

The default run uses the fixed official conformance commit and target
`2026-07-28` scenarios for stateless discovery, tools/resources listing,
header validation, DNS rebinding, and caching. The committed baseline contains
only narrow checks for conformance-private diagnostic capabilities that MCP
Vault intentionally does not advertise. Set
`MCP_VAULT_CONFORMANCE_REQUIREMENTS=2026-07-28` for the full official
requirements set when its product-specific expected-failure review is ready;
do not replace that review with a broad scenario allow-list.

## 11. Provider contract tests

Create local fake HTTP servers for:

- success;
- invalid JSON/schema;
- auth failure;
- rate limit with retry hints;
- transient 5xx;
- timeout;
- oversized response;
- redirect to forbidden host;
- model-list unavailable;
- embedding dimension mismatch.

CI never uses real API keys or billable endpoints.

## 12. Recall quality tests

Maintain a versioned benchmark corpus with Chinese and English notes/memories.

Evaluate:

- lexical exact match;
- semantic paraphrase;
- preference/decision distinction;
- current project continuity;
- stale/superseded filtering;
- temporal validity;
- duplicate diversity;
- cross-topic false positives;
- source/provenance accuracy.

Ranking changes require benchmark comparison and recorded rationale.

Memory integration fixtures cover explicit `remember` staging, the exact Codex
Phase 1 three-field output, local whole-source provenance/revision checks,
durable `no_output` coverage, unchanged-note skip, forced re-extraction,
source update/withdrawal, Phase 2 create/update/archive/discard decisions,
semantic snapshot conflicts, revision-only drift, proposal reuse, canonical artifact rendering, lifecycle
filtering, FTS-only recall, optional vector degradation, recall budgets, MCP
memory scopes/resources, and two-Vault isolation. Tests use local Provider
fakes only; recall tests prove that no query-time LLM is required.

The public MCP suite also proves that `recall` can return an indexed ordinary
note without durable memory and that a `memory:read` credential lacking
`vault:read` receives no note cue. `remember` tests require a raw-input ID and
consolidation-job ID and prove the final memory is unavailable until Phase 2
commits. Provider fixtures assert stable schema category/path diagnostics
without response values; generic single-array-envelope repair remains bounded
to schemas where the direct item/array already validates, while the multi-field
Phase 1/Phase 2 contracts reject missing fields.

Worker fixtures prove a full-Vault Phase 1 job continues after one malformed
note output, checkpoints a bounded redacted failure, preserves its paid-work
cursor, and admits Phase 2. State fixtures prove one active consolidation per
Vault, exact raw-hash selection commit, idempotent applied proposals, and
cross-Vault rejection. Migration/reset fixtures upgrade from schema 9, remove
legacy quote-as-content extracted memory through Vault Core, stage explicit
memory, clear legacy candidate/note rows, and preserve source history. Admin
and frontend tests cover two-role readiness, active-run reuse, truthful
Phase 1/Phase 2 progress, archive/restore revision conflicts, consecutive
permanent deletes without refresh, and the absence of candidate-generation or
review controls. Artifact pagination tests prove `MEMORY.md` includes active
memory beyond one 200-row State page.

## 13. Crash and recovery tests

Inject failures after each write phase:

```text
journal prepared
temp file written
file fsynced
rename committed
metadata transaction started
outbox inserted
metadata committed
```

Restart and assert:

- canonical file is either old or new, never partial;
- journal resolves;
- revision metadata matches file;
- duplicate events/jobs are harmless;
- no cross-path corruption;
- audit outcome is accurate.

WP-06 additionally exercises SQLite lease reclaim and Vault-scoped job
deduplication, outbox admission into a durable job before acknowledgement,
bounded no-follow enumeration, scan-generation fencing, direct external
create/edit/delete import, unsafe-entry protection, and supervisor shutdown.
The polling reconciliation pass is the authority when a filesystem watcher
event is missed; native watcher acceleration must not replace this test path.

## 14. Security tests

Follow `security.md`, including:

- traversal/symlink/Unicode;
- CSRF/Origin;
- secret logs/API;
- OAuth claims/resource indicators;
- cross-Vault;
- provider SSRF;
- prompt injection;
- archive restore;
- clean-host restore, pre-restore rollback, low-disk rejection, and backup
  retention last-verified protection;
- resource limits;
- public Admin route absence.

## 15. Performance tests

Reference fixture:

```text
10,000 Markdown notes
50,000 attachments
100,000 memory/index records
mixed Chinese/English
```

Measure:

- initial scan;
- PROPFIND/listing;
- lexical and hybrid search;
- recall;
- write latency;
- worker throughput;
- memory use;
- database size;
- backup duration.

Do not optimize before measurement, but do not accept designs that require loading the entire Vault into memory per request.

WP-14 also provides a bounded public-listener regression smoke:

```bash
bash scripts/perf/baseline.sh
```

It emits a JSON report with fixture size, iteration count, p50/p95/max,
threshold, timestamp, and platform. The report is a regression tripwire, not
the full-scale 10,000-note/50,000-attachment/100,000-record acceptance
measurement; a release must attach the larger fixture report separately.

## 16. CI pipeline

Suggested jobs:

1. formatting and generated-file check;
2. Clippy all targets/features;
3. unit/repository tests;
4. frontend lint/unit/build;
5. migration tests;
6. integration tests;
7. MCP conformance;
8. WebDAV Litmus;
9. security/dependency/license checks;
10. coverage;
11. Docker build and end-to-end smoke;
12. SBOM/image scan.

The repository additionally runs the real HTTP smoke, the pinned official MCP
core scenarios, migration fixture test, threat/traceability review, and the
release artifact checks documented in `docs/release-readiness.md`.

Use caching without allowing stale generated schemas/migrations to pass.

## 17. Local commands

Expected developer entry points should be wrapped by `just`, `make`, or `cargo xtask`:

```text
just setup
just dev
just fmt
just lint
just test
just test-mcp
just test-webdav
just test-e2e
just migrate
just reset-dev-data
just build-image
just docs-check
```

Codex should use the repository’s actual task runner once created.

## 18. Definition of done

A change is done when:

- behavior matches the governing requirement;
- boundaries and Vault scoping are preserved;
- migrations and rollback/recovery implications are documented;
- tests cover normal, error, isolation, and recovery behavior;
- relevant conformance tests pass;
- logs/secrets are safe;
- docs and examples are updated;
- commands run and known limitations are reported.
