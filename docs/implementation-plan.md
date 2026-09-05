# Complete-Service Implementation Plan

## 1. Intent

This is not an MVP plan. It decomposes the complete first-release service from `product-requirements.md` into implementable, testable work packages while preserving the final architecture from the first commit.

A package may deliver a temporarily incomplete runnable system, but it must not introduce shortcuts that require later architectural replacement.

For each substantial package, create an ExecPlan under `docs/exec-plans/active/` following `PLANS.md`.

## 2. Dependency graph

```text
WP-00 Foundation
   ├── WP-01 Domain and safe paths
   ├── WP-02 SQLite state and migrations
   └── WP-03 Filesystem storage
          │
          ▼
WP-04 Vault Core, revisions, history, recovery
   ├── WP-05 Authentication and secrets
   ├── WP-06 Outbox, jobs, watcher, reconciliation
   │
   ├── WP-07 WebDAV
   └── WP-08 MCP foundation
          │
          ├── WP-09 Markdown index and knowledge map
          ├── WP-10 Provider and embedding subsystem
          └── WP-11 Memory subsystem
                    │
                    ▼
              WP-12 Admin API/UI
                    │
                    ▼
WP-13 Backup, restore, observability, hardening
                    │
                    ▼
WP-14 Conformance, interoperability, release
```

Several packages can proceed in parallel after their interfaces are agreed, but integration acceptance remains mandatory.

## 3. WP-00 — Repository and build foundation

### Deliverables

- Rust workspace and pinned toolchain;
- frontend workspace and lockfile;
- task runner (`just`, `xtask`, or equivalent);
- CI skeleton;
- Docker multi-stage build;
- embedded/static Admin asset path;
- structured tracing setup;
- configuration bootstrap types;
- dependency/license policy;
- generated documentation/schema check.

### Required decisions

- exact crate split;
- UUIDv7 versus ULID;
- selected authenticated-encryption crate;
- selected Markdown AST library;
- `dav-server` version/adapter strategy;
- `rmcp` version matching current MCP spec.

### Acceptance

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
docker build .
```

A minimal process starts two listeners, but no placeholder handler may access user files directly.

## 4. WP-01 — Domain model and safe Vault paths

### Deliverables

- typed IDs;
- `VaultContext`;
- normalized `VaultPath`;
- permission/scope types;
- revision/precondition types;
- actor/source-plane model;
- stable domain errors;
- path and Unicode policy.

### Tests

- absolute/traversal/encoded traversal;
- separator and Unicode normalization;
- case-collision detection fixture;
- reserved paths;
- symlink/special-file policy;
- two Vault contexts never compare/interchange accidentally.

### Acceptance

All future application signatures can require `&VaultContext` and `&VaultPath`; no protocol string path leaks into storage code.

## 5. WP-02 — SQLite operational state

### Deliverables

- SQLx pool initialization and PRAGMAs;
- migrations for Vault, settings, identity, credentials, files/revisions, journal, outbox, jobs, providers, audit;
- repository layer;
- transaction/unit-of-work pattern;
- migration fixture tests;
- SQLite integrity diagnostics.

Memory/index tables may land in later migrations owned by their packages.

### Tests

- fresh migrate;
- upgrade prior fixture;
- foreign keys and uniqueness;
- Vault-scoped repository isolation;
- concurrent busy handling;
- backup API feasibility spike.

### Acceptance

No handler executes SQL. Repository APIs require Vault ID where user data is involved.

## 6. WP-03 — Safe filesystem and history store

### Deliverables

- safe root-relative resolution;
- streaming reads/writes;
- temporary files and atomic rename;
- fsync policy;
- hashing;
- metadata/stat;
- copy/move/delete primitives;
- history content-addressed blob store;
- file identity hints;
- disk-space checks.

### Tests

- large streaming file;
- partial write failure;
- rename/copy across paths;
- symlink escape;
- special files;
- history deduplication;
- read-only/full-disk simulation where practical.

### Acceptance

The implementation never exposes an unvalidated absolute path to protocol code.

## 7. WP-04 — Vault Core, revisions, history, and crash recovery

### Deliverables

- query and mutation application services;
- create/replace/patch/append/move/copy/delete/restore;
- expected revisions and DAV preconditions;
- stable File ID;
- per-path lock manager;
- operation journal;
- revision history;
- audit and outbox writes;
- startup recovery and maintenance mode;
- idempotency keys.

### Tests

- all mutation operations;
- concurrent MCP/DAV-style writes;
- patch exactness;
- move lock ordering;
- crash injection at every commit phase;
- restart recovery;
- restore creates a new revision;
- out-of-band file mismatch diagnosis.

### Acceptance

WebDAV and MCP can later call the same services and receive identical consistency/audit behavior.

## 8. WP-05 — Authentication, authorization, and secret storage

### Deliverables

- master-key loading/versioning;
- encrypted secret repository;
- Argon2id admin/WebDAV passwords;
- Admin session/CSRF;
- WebDAV credential verification;
- PAT generation/digest/rotation/revocation;
- OAuth resource-server configuration and validation;
- scope-to-application permission mapping;
- per-listener Origin policy;
- redaction types.

### Tests

- password rehash;
- session expiry/revocation;
- CSRF/Origin;
- PAT shown once and digest lookup;
- issuer/signature/time/audience/resource/scope/subject grants;
- token for Vault A rejected at Vault B endpoint;
- secret encryption/rotation/redaction.

### Acceptance

No public interface shares credentials with another plane. The service can authenticate every planned endpoint without implementing incomplete OAuth shortcuts.

## 9. WP-06 — Durable outbox, jobs, watcher, reconciliation

### Deliverables

- outbox dispatcher;
- persistent job queue with leases;
- worker supervisor;
- retry/backoff/dead-letter state;
- progress/cancellation;
- filesystem watcher;
- initial scan;
- periodic reconciliation;
- startup recovery integration.

### Tests

- restart while job leased;
- duplicate delivery;
- dedup keys;
- watcher event loss followed by reconciliation;
- direct external edit/delete/move;
- bounded concurrency;
- failed jobs visible and retryable.

### Acceptance

No durable background task depends solely on detached Tokio tasks or in-memory queues.

## 10. WP-07 — Integrated WebDAV

### Deliverables

- project adapter around `dav-server`;
- custom Vault-backed filesystem/guard;
- authentication;
- methods and properties;
- ETags/preconditions;
- ranges and streaming;
- lock abstraction;
- DAV error mapping;
- generated connection information.

### Tests

- unit/integration for every required method;
- Litmus;
- large binary;
- conflicts;
- credential expiry/revocation;
- path attacks;
- Sync Engine WebDAV scenario;
- Remotely Save scenario;
- desktop/mobile release checklist.

### Acceptance

A real Obsidian fixture can synchronize bidirectionally without any custom plugin, and every write appears in revision/audit/outbox.

## 11. WP-08 — MCP foundation and controlled Vault tools

### Deliverables

- official RMCP integration;
- 2026-07-28 stateless Streamable HTTP;
- supported older revision negotiation;
- `server/discover` instructions;
- deterministic authorization-dependent tools/resources;
- required transport headers and Origin validation;
- PAT/OAuth middleware;
- discovery/retrieval/mutation/history tools except memory-specific internals;
- structured output/error schemas;
- resource URIs and caching metadata;
- request IDs and tracing.

### Tests

- official conformance;
- discovery;
- header/body validation;
- scope-filtered tool list;
- PAT/OAuth;
- all tools normal/error/conflict;
- resource authorization;
- no arbitrary Vault selection;
- request-scoped SSE behavior where used.

### Acceptance

An MCP client can understand the server, explore current deterministic metadata, read, safely edit, and inspect history.

## 12. WP-09 — Markdown index, FTS, links, and knowledge map

### Deliverables

- Markdown/frontmatter/Obsidian syntax analyzer;
- notes/headings/tags/links/backlinks projection;
- FTS5;
- path/topic/time/tag scoped search;
- deterministic folder/tag/link index;
- `_mcp-vault/index.yaml` parser and validation;
- bounded overview and browse-index services;
- related-note scoring;
- rebuild and coverage status.

### Tests

- multilingual fixtures;
- fake links inside code;
- aliases/frontmatter;
- unresolved wikilinks;
- rebuild after DB projection deletion;
- FTS ranking;
- bounded overview;
- taxonomy overlay;
- external edit reindex.

### Acceptance

An Agent can obtain a useful Vault map without LLM/embedding and search exact source material.

## 13. WP-10 — Provider, embedding, and vector subsystem

### Deliverables

- provider/model/binding repositories;
- encrypted credentials;
- Admin-independent application configuration services;
- OpenAI Responses/compatible structured generation adapter;
- Anthropic-compatible adapter;
- first-class DeepSeek, MiMo, GLM, Kimi, Gemini, and Qwen compatibility
  presets on the shared OpenAI-compatible transport;
- local OpenAI-compatible adapter;
- embedding HTTP adapter;
- optional local `fastembed` feature;
- `VectorIndex` trait;
- pinned SQLite vector backend;
- exact-cosine fallback;
- provider health/test/model discovery;
- retry/timeout/concurrency/privacy/SSRF enforcement;
- re-embedding jobs and coverage.

### Tests

- local fake provider contract matrix;
- invalid schema/auth/rate limit/timeout/redirect;
- dimension mismatch;
- model binding inheritance;
- remote-disabled/local-only policy;
- provider outage does not affect core;
- vector Vault partition;
- model change re-embedding;
- ordinary-note chunk scheduling, stale-vector exclusion, and semantic/hybrid
  public retrieval.

### Acceptance

Semantic capability is fully optional, observable, and replaceable.

## 14. WP-11 — Complete memory subsystem

### Deliverables

All behavior from `memory-system.md`:

- current explicit and one-source-owned-set canonical Markdown;
- current-only ownership/query schema and projections;
- immediate explicit `remember` with omitted metadata fidelity;
- one-call source extraction using `memories[]` and application-owned identity;
- schema/prompt/profile versioning;
- exact File-ID/content-hash provenance separated from model content;
- whole-source-set replacement, source pause/resume, and actual deletion;
- FTS/vector/entity recall with relevance admission, calibrated semantic
  evidence, score/cosine separation, and one-object aggregation;
- continuity boosts applied only after relevance, temporal validity, diversity,
  and a whole-response budget;
- memory MCP tools/resources;
- separately typed ordinary-note recall cues backed by the Index service;
- Admin application services;
- rebuild behavior;
- canonical per-explicit and per-source-set files through Vault Core;
- Vault-level `automatic` source admission with no author-facing note metadata;
  legacy `explicit_only`/`all_notes` settings migrate to this mode;
- no candidate inbox, lifecycle state machine, global consolidation, source
  health engine, or query-time generative model;
- ordinary article knowledge remains available through `related_notes` while
  source extraction chooses only compact durable propositions;
- per-note Provider-output failure isolation, redacted schema-path diagnostics,
  bounded consecutive-failure circuit breaking, and paid-work cursor retention;
- durable Vault/source/hash/profile coverage with successful empty sets and an
  explicit `include_evaluated` override;
- persisted prepared source snapshots, optimistic source/set revalidation,
  byte-identical crash adoption, and redacted extraction/reconcile progress;
- additive migration 0015 with content-free preflight, exact Admin
  confirmation, safe explicit preservation, note regeneration, and no guessed
  handling of ambiguous/history rows;
- source-aware Admin edit/delete/pause/resume actions backed by Vault Core and
  audit records.

### Tests

Use the complete acceptance list from `memory-system.md`, plus provider/prompt
injection, ordinary unmodified-note admission, semantic content differing from
source text, empty-set/failure continuation, source hash/set CAS and restart,
move-without-model, change/delete fail-closed behavior, delete/pause/resume
concurrency, actual-delete audit, migration non-mutation, long-chunk coverage,
retrieval hard negatives, shared budgets, and two-Vault isolation.

### Acceptance

A semantically phrased task recalls a prior decision with provenance, without query-time LLM, and provider failure degrades safely.

## 15. WP-12 — Admin API and web console

### Deliverables

- separate Admin router/listener;
- setup/bootstrap;
- session/CSRF;
- all API groups from `interfaces.md`;
- React UI pages from `admin-and-configuration.md`;
- generated connection info;
- provider/model tests;
- index and memory management;
- jobs/audit;
- backup/restore workflows;
- diagnostics;
- accessibility and responsive layout.

### Tests

- backend API integration;
- frontend unit;
- Playwright setup and critical flows;
- Admin unavailable on data listener/public proxy;
- secret masking;
- confirmation/destructive flows;
- provider-disabled operation;
- memory review.

### Acceptance

The owner can fully configure and operate the service without editing SQLite or a long-lived configuration file.

## 16. WP-13 — Backup, restore, observability, and hardening

### Deliverables

- consistent backup/manifest/verification;
- staged restore and maintenance modes;
- key export guidance/tooling;
- health and readiness;
- metrics and OpenTelemetry opt-in;
- redacted diagnostics;
- rate/resource limits;
- disk-space behavior;
- history/trash/audit/job retention;
- container hardening;
- SBOM and image scan;
- upgrade/rollback documentation.

### Tests

- clean-host restore;
- backup corruption detection;
- archive traversal;
- low disk;
- graceful shutdown;
- proxy streaming;
- metrics/log redaction;
- no Admin public route;
- container runs non-root/read-only root.

### Acceptance

A verified backup restores a working service with content, operational state, history, credentials, and rebuildable projections.

## 17. WP-14 — Conformance, interoperability, and release readiness

### Deliverables

- official MCP conformance in CI and release;
- WebDAV Litmus;
- Obsidian plugin compatibility matrix;
- end-to-end scenarios;
- migration from prior prerelease fixtures;
- performance baseline;
- threat-model verification;
- release image signing/checksums/SBOM;
- operator documentation;
- first-release checklist.

### Release gates

- no known critical/high security defect;
- no failing isolation/recovery test;
- all advertised MCP revisions conform;
- supported Obsidian clients pass;
- backup/restore verified;
- Admin cannot be reached through reference public proxy;
- provider outage/degradation verified;
- full requirements traceability reviewed.

## 18. Managed multi-Vault enablement

ADR-0020 enables one Admin owner to create, select, disable, and re-enable
several service-managed Vaults. The Admin API/UI uses explicit Vault-scoped
paths, while existing unscoped routes retain a stable legacy-default binding.
Each Vault receives generated MCP/DAV endpoints, credentials, configuration
overrides, initialization/readiness, fair durable jobs, index/vector
partitions, and an independent memory pipeline.

The work does not delete/detach Vaults, attach arbitrary roots, or change
ordinary tool schemas to add `vault_id`.

Cross-Vault/federated recall is a separate future capability with explicit grants and a new security review.

## 19. Parallel work guidance

Safe parallelization after interfaces settle:

- frontend UI shell against OpenAPI mocks;
- Markdown analyzer;
- provider adapters;
- DAV compatibility fixture;
- MCP schemas/conformance harness;
- backup format design.

High-risk areas that require tight sequencing:

- Vault Core and operation journal;
- database migrations;
- auth/Vault binding;
- canonical memory materialization;
- restore.

## 20. Tracking completion

Each work package must have:

- an ExecPlan;
- requirements/ADR links;
- code and migration diff;
- test evidence;
- conformance evidence when relevant;
- documentation update;
- explicit remaining risks.

Do not mark the complete service finished merely because all routes exist. Completion is behavioral and operational.
