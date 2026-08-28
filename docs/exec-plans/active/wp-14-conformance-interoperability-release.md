# WP-14 — Conformance, interoperability, and release readiness

Status: In progress
Owner: Codex
Created: 2026-08-21
Last updated: 2026-08-26

> Memory redesign notice (2026-08-26): all candidate-first, exact-quote-as-final,
> direct-promotion, marker, and candidate-review material below is retained only
> as an implementation-history record. ADR-0016 and
> `docs/exec-plans/completed/wp-14-codex-two-phase-memory.md` supersede those
> decisions. Current release validation must exercise Phase 1 raw-memory
> staging, Phase 2 consolidation, ADR-0017 destructive cutover/fresh
> regeneration, and the candidate-free
> Admin/MCP contracts described by that dedicated ExecPlan.

## Purpose and user-visible result

This work package turns the implemented MCP Vault service into a release candidate that has executable evidence for protocol compatibility, cross-client behavior, recovery, security boundaries, performance, and artifact integrity. The result is a repeatable set of local and CI commands that can be run against a fixture deployment, with explicit distinction between automated evidence, manually required client checks, and checks blocked by unavailable external tools.

The release process must not claim conformance or client compatibility merely because an in-process adapter test passes. It must expose a documented way to run the official MCP conformance suite against the real Streamable HTTP endpoint, a WebDAV Litmus entry point against a real HTTP deployment, public-protocol end-to-end scenarios, migration/recovery fixtures, and release artifact checks. Unsupported or unverified protocol revisions and client versions remain unadvertised.

## Governing requirements

- `AGENTS.md`: non-negotiable Vault isolation, protocol-layer boundaries, secret handling, safe writes, testing, and release review rules.
- `docs/product-requirements.md`: release-quality MCP/WebDAV/Admin behavior, recovery, security, operations, and acceptance requirements.
- `docs/architecture.md`: sections on protocol planes, Vault Core, auth separation, durable operations, derived projections, observability, deployment, and recovery.
- `docs/implementation-plan.md`: section 17, WP-14 deliverables and release gates; section 18, future multi-Vault enablement; section 20, completion evidence.
- `PLANS.md`: required living ExecPlan structure and completion/rollback rules.
- `docs/interfaces.md`: MCP target revision and headers, stateless Streamable HTTP, WebDAV semantics, Admin listener isolation, maintenance recovery, and metrics interfaces.
- `docs/standards-and-references.md`: MCP/RMCP, WebDAV/HTTP, Obsidian interoperability, security, and operational references.
- `docs/development-and-testing.md`: conformance, HTTP E2E, migration, performance, threat-model, CI, and release verification expectations.
- `docs/deployment-and-operations.md`: reference deployment, proxy exposure, startup/shutdown, backup/restore, diagnostics, upgrade/rollback, and operational acceptance.
- `docs/security.md`: threat model, secret/cookie/token handling, origin/proxy policy, rate limits, audit, and security release checks.
- ADRs `0001`–`0012`, especially the Vault boundary, protocol/auth separation, MCP statelessness, safe writes, outbox/recovery, provider isolation/presets, and canonical memory-file decisions.

## Current repository state

- Rust workspace crates include `server`, `mcp`, `webdav`, `admin-api`, `vault-core`, `storage-fs`, `state`, `auth`, `indexer`, `memory`, `providers`, `backup`, and `domain`.
- `crates/mcp` uses RMCP 3.x and advertises MCP `2026-07-28`; its current tests are primarily in-process router/service tests covering headers, authorization, deterministic discovery, tools, resources, and Vault isolation.
- `crates/webdav` has broad in-process DAV adapter tests for authenticated reads, writes, collections, copy/move/delete, locks, ranges, preconditions, credentials, and Vault isolation. WP-14 now adds the external Litmus entry point and a real-process compatibility fixture.
- `crates/server` composes separate data/control routers and listener configuration. The new disposable fixture binary prepares a temporary SQLite/Vault root, runs the real startup reconciliation path, binds both listeners, and publishes a 0600 manifest for scripts.
- WP-13 already provides backup/restore, maintenance gates, readiness/metrics/diagnostics, container non-root/read-only-root hardening, SBOM and image-scan CI hooks, and a recovery endpoint. WP-14 exercises these through the public deployment boundaries rather than bypassing them.
- `.github/workflows/ci.yml` now runs Rust, frontend, documentation, container, public HTTP smoke, pinned official MCP core scenarios, migration, and bounded performance checks. Litmus is an explicit opt-in job because the external binary/target credentials are not available in the ordinary repository runner.
- The repository now contains conformance/Litmus wrappers, a WebDAV/client compatibility matrix, a performance baseline policy/report script, requirements traceability, a release checklist, and a manual release workflow.
- The Admin API registers `DELETE /api/v1/providers/{id}`, but the current
  repository deletes only the Provider row. Registered models, role bindings,
  and Vault-scoped embedding rows retain foreign keys to that Provider/model,
  so an ordinary configured service cannot be deleted. The Chinese Admin UI
  exposes model discovery but has no deletion control or impact explanation.

## Scope and non-scope

### Included

1. A versioned MCP conformance harness and CI/release job that starts or targets a real HTTP endpoint, supplies test-only authentication through a narrowly scoped fixture boundary, runs official core scenarios for the advertised revision, and uses a reviewed expected-failures baseline whose stale/new failures fail the job. Full frozen requirements runs remain an explicit release review command.
2. A real HTTP WebDAV smoke fixture plus a Litmus entry point, with request fixtures for the supported sync-client behaviors and a compatibility matrix for Hēsperus Sync Engine and Remotely Save/Obsidian integration.
3. Public-protocol E2E scenarios for Vault isolation, safe writes/preconditions, MCP discovery/search/memory, WebDAV mutation, Admin listener isolation, provider outage degradation, and backup/restore/reconciliation.
4. A migration fixture check from the prior prerelease schema/data shape, including rollback/recovery evidence.
5. A deterministic performance baseline harness with bounded fixture data, explicit environment metadata, and non-flaky threshold reporting.
6. Threat-model verification and requirements-to-test/evidence traceability.
7. Release checklist, image digest/checksum/SBOM verification, optional signing verification hooks, operator documentation, and first-release evidence policy.

### Not included

- Adding a second production Vault or changing public tool schemas to accept arbitrary `vault_id`.
- Replacing the MCP/WebDAV/Admin business-service boundaries with test-only shortcuts.
- Claiming that a GUI client was tested when its binary/plugin, credentials, or environment were unavailable.
- Adding new product features from future WP-15+ work; only small seams needed to exercise existing behavior are in scope.
- Signing a release with a private key stored in this workspace. CI may verify a supplied signature or use an external OIDC signing job, but secrets remain outside the repository.

## Invariants and risks

- Every fixture request still resolves a `VaultContext`; test authentication is injected at the fixture boundary and cannot disable production authentication or cross-Vault predicates.
- No conformance, Litmus, proxy, benchmark, or diagnostics artifact may contain passwords, bearer tokens, API keys, note bodies, memory contents, or authorization headers. Captures must be sanitized before persistence.
- MCP conformance is evaluated only for revisions the server actually advertises and supports. A failed official scenario is visible; expected failures are explicit, versioned, reviewed, and strict about regression/stale-baseline changes.
- Admin routes remain on the control listener and must be unreachable through the reference public/data proxy.
- E2E writes use preconditions and verify revisions/history/outbox/reconciliation; no test may normalize a lost update into success.
- Performance thresholds are evidence thresholds for a documented fixture/environment, not universal SLAs. They must fail on obvious regressions while tolerating normal CI variance.
- Migration tests run on copies of fixtures and preserve the original fixture for repeatability. A failed upgrade leaves the source recoverable and reports the exact migration boundary.
- Release manifests are generated from immutable image/artifact digests and sanitized metadata. A checksum verifies the bytes actually shipped; an SBOM is attached to the same digest.

## Proposed design

### External fixture boundary

Add a test-support path that composes the existing `server` routers with a temporary SQLite database, Vault root, generated test credentials, and an ephemeral data/control listener. The fixture exposes the same `/mcp`, WebDAV, Admin, health, metrics, and recovery routes as production. A loopback-only test proxy may inject a fixture PAT for external conformance/Litmus processes, but the application continues to enforce normal authentication and Vault binding. The fixture writes a redacted connection manifest for scripts and deletes only its own temporary directory on exit.

Prefer a Rust fixture binary or integration test harness over a shell mock so the HTTP path, middleware ordering, and storage/recovery services are real. Keep protocol DTOs and business services unchanged.

### MCP conformance

Provide a script/Make target and CI job that:

1. builds the workspace and launches the fixture;
2. waits for readiness and verifies the advertised protocol revision and required headers;
3. runs the official MCP conformance CLI in server mode against the real endpoint for `2026-07-28`;
4. passes a committed expected-failures file only for reviewed, known SDK/feature limitations;
5. fails on new failures, a scenario unexpectedly becoming passing while still listed as expected, unsupported advertised revisions, or missing artifacts;
6. uploads sanitized JSON/HTML evidence.

The harness must also retain project-specific tests for deterministic discovery, `_meta` consistency, origin/header rejection, auth/Vault binding, and statelessness because official conformance cannot prove all domain invariants.

### WebDAV and client interoperability

Provide a real HTTP WebDAV target and a Litmus command wrapper. Keep Litmus optional locally when the external binary is absent, but make CI/release behavior explicit: either run it and upload results or mark the release gate blocked rather than passing silently. Add sanitized method/condition fixtures for PROPFIND, MKCOL, GET/HEAD, PUT with `If-Match`/`If-None-Match`, COPY/MOVE, DELETE, LOCK/UNLOCK, ranges, and 4xx/5xx behavior. Document the exact tested matrix for Hēsperus Sync Engine and Remotely Save, with manual steps/check boxes for GUI/plugin versions.

### E2E, migration, performance, and security evidence

Use the public HTTP endpoints and a disposable Compose/test environment for scenario tests. Use provider fakes for outage/degradation and never paid provider APIs. Keep migration fixtures under `tests/fixtures` with checksums. Benchmark a bounded Vault fixture using Criterion or a dedicated harness, emit environment/commit metadata, and compare only to the checked-in baseline policy. Add a threat-model checklist that maps each high-risk assertion to an automated test or an explicitly required manual review. Generate requirements traceability from a maintained table rather than relying on route existence.

### Release assets

Add a release checklist and scripts that validate version/config/lockfile consistency, docs/checksums, image non-root/read-only behavior, image digest, SBOM presence, vulnerability scan result, and optional cosign signature/attestation verification. CI should build once, record the digest, and make all subsequent checks reference that digest. Signing remains an external credentialed step; verification is required when a signed artifact is declared.

### Review-remediation hardening

The deployment review on 2026-08-22 found release-blocking defects behind
otherwise green happy-path tests. Remediation keeps the existing crate
boundaries and adds shared runtime coordination rather than client-side
throttling:

1. `vault-core` owns an explicit cloneable runtime shared by every Core built
   by the composition root. It contains the process path-lock registry and the
   maintenance coordinator, so WebDAV, MCP, Admin, reconciliation, memory, and
   workers cannot accidentally create isolated lock domains.
2. Maintenance admission uses counted RAII request/write guards. Changing to
   read-only rejects new mutations and waits for admitted writes; changing to
   offline additionally waits for active protocol requests before roots or
   SQLite state are swapped.
3. SQLite restore removes every service-owned object from `main`, not only
   objects known to an older snapshot, before recreating the restored schema
   and applying forward migrations.
4. Installation-key identity is persisted as a non-secret keyed check through
   a forward migration. Startup requires and verifies a persistent key when
   encrypted secrets or durable keyed PATs exist. Symmetric OAuth JWKs are not
   stored as ordinary JSON; this release accepts normalized public RSA keys.
5. Password hashing/verification runs in a shared bounded blocking pool.
   Provider transports and their concurrency permits are cached and shared by
   the process-level Provider service rather than recreated per call.
6. Every admitted job reaches a terminal state. Generic outbox jobs have a
   handler, Admin retry resets an exhausted attempt budget, running jobs receive
   a task cancellation signal, and event-specific index jobs coalesce at
   execution so only the newest pending generation rebuilds a Vault.
7. Admin exposes audited Vault-scoped Provider privacy-mode and OAuth subject
   grant management. Manually supplied normalized JWKS remain valid until an
   Admin update; the service does not impose a 24-hour outage without a refresh
   worker.

### Self-contained first-run provisioning

The final deployment follow-up on 2026-08-22 removes manual secret generation
and reverse-proxy coupling from the application bootstrap path:

1. When no explicit master-key file is configured, MCP Vault owns
   `<data-dir>/secrets/master-key`. It atomically creates a random persistent
   key when the path and installation-key verifier are both absent, then reuses
   that file on restart. A persisted verifier or key-dependent record with a
   missing/different key still fails closed; startup never replaces a lost
   installation key.
2. While no Admin exists, the browser presents password-only first-Admin setup
   and the Auth repository atomically accepts one claim. No setup token or
   default account exists.
3. `MCP_VAULT_MASTER_KEY_FILE` remains an operator-managed compatibility
   override. Obsolete bootstrap-token settings are rejected with migration
   guidance rather than ignored.
4. MCP Vault does not inspect, reject, or mutate filesystem permission bits on
   key files. Ownership, mode, ACL, volume protection, TLS, reverse
   proxying, firewall rules, and Admin source-network publication belong to the
   deployment environment.
5. The application keeps the separate control listener plus Admin username/
   password, session, CSRF, Origin, expiry, revocation, and login limiting. It
   removes application CIDR filtering; the safe default listener remains
   loopback and operators explicitly choose any LAN/VPN/public bind or proxy
   policy.

### Admin usability and Chinese localization

The operator review on 2026-08-22 found that the first Admin shell exposes too
many peer-level destinations, expands advanced OAuth/recovery controls by
default, and uses English plus raw JSON as its primary presentation. The
release UI is revised without changing Admin API/Vault boundaries:

1. All operator-facing labels, help text, status messages, errors, and empty
   states use Simplified Chinese while protocol names, scopes, IDs, and URLs
   remain exact technical values.
2. Navigation is grouped into overview, connection, intelligence, and
   operations sections. Dashboard and each page show a concise status summary
   before management controls.
3. Common actions remain immediately discoverable. Advanced OAuth issuer/
   grant management, restore/recovery controls, and raw JSON diagnostics are
   collapsed until explicitly opened.
4. WebDAV permissions and common MCP PAT scopes use labeled choices instead of
   comma-delimited free text. Existing server validation and one-time secret
   semantics remain authoritative.
5. Page-specific lists replace the always-visible JSON dump for credentials,
   providers, memories, jobs, audit, backups, index, and system state. Raw
   sanitized responses remain available in an explicit diagnostic disclosure.
6. Responsive and keyboard-visible behavior is preserved and visually checked
   at desktop and narrow viewport sizes.
7. The unauthenticated Admin shell reads a non-secret setup-availability flag.
   A fresh installation shows only first-Admin creation; after the atomic first
   Admin commit it shows only login. The status is advisory UI state: the Auth
   service's create-only transaction remains the authoritative race-safe guard.
8. Every password-creation form explains the actual default policy beside the
   input, including minimum length, absence of composition requirements, and
   rejected placeholder examples. A policy error repeats actionable guidance
   instead of asking the operator to guess.
9. A valid Admin session survives page reload. The session bearer remains only
   in an HttpOnly/SameSite=Strict cookie; a separate SameSite=Strict CSRF
   cookie is readable by the Admin frontend solely to reconstruct the required
   mutation header before `GET /session` confirms the server-side session.
   HTTPS origins add `Secure`; the 2026-08-28 ADR-0004 amendment permits an
   explicitly configured literal private/loopback HTTP Origin to omit only
   that attribute while retaining the remaining controls.

### Password-only first-Admin initialization

The operator decision on 2026-08-22 supersedes the bootstrap-token portions of
the earlier self-contained provisioning design. MCP Vault now uses a
first-claim setup model:

1. While `admin_users` is empty, `GET /api/v1/setup` reports setup available and
   `POST /api/v1/setup` accepts only the desired username and password.
2. The Auth service performs password validation/hashing and the existing
   atomic `INSERT ... WHERE NOT EXISTS` remains the sole first-Admin commit
   boundary. Concurrent claims still produce exactly one winner.
3. No bootstrap token is generated, persisted, accepted by HTTP, exposed by a
   CLI command, or requested by the frontend. The installation master key
   remains automatically generated and independently required for encrypted
   operational secrets and durable keyed credentials.
4. Setup authorization is therefore reachability plus the empty-Admin state.
   The Admin listener remains loopback by default. An operator who publishes an
   uninitialized listener to LAN/VPN delegates first-claim trust to that
   network/deployment boundary; ordinary authenticated Admin sessions remain
   mandatory after setup.
5. Removed bootstrap-token environment variables are rejected with migration
   guidance rather than silently ignored, so an old deployment cannot retain a
   false belief that the token still protects setup.

### Provider lifecycle deletion

Deleting an AI service is one application operation, not a React-owned series
of API calls. The Provider service passes an optional optimistic revision to a
single State transaction. That transaction removes every global and
Vault-specific role binding for the Provider's models, deletes only derived
embedding rows (letting vector rows cascade), deletes the model inventory and
health/configuration rows, and removes all encrypted secrets owned by that
Provider. Canonical Vault files, durable memories/candidates, job history, and
audit history are retained. A redacted deletion summary reports only counts.

The Admin card shows a destructive action with a Chinese confirmation naming
the service and the known model/binding impact. It sends the displayed Provider
revision, refreshes all Provider/model/binding projections after success, and
explains that derived vectors are removed while notes and memories remain. A
stale revision returns the existing `revision_conflict` contract. In-flight
provider HTTP work is not replayed or force-aborted; once deletion commits, no
new role resolution can select the removed models and queued work fails through
the existing redacted configuration path.

The same card exposes an ordinary edit disclosure for name, first-class service
type, Base URL, and enabled state. The stored secret is never returned; an empty
replacement field preserves it and a non-empty value rotates it through the
Auth boundary. PATCH carries the displayed optimistic revision, so two Admin
tabs cannot silently overwrite each other. Editing does not recreate models or
role bindings merely because the display name or endpoint changes.

### Durable task execution logging

The Worker supervisor emits redaction-safe structured `tracing` records for
job start, completion, retry, cancellation, permanent failure, missing
handler, and durable transition failures. Memory extraction additionally emits
one progress record when a note starts and when it completes or fails, with
ordinal/total, candidate/skipped counters, elapsed milliseconds, and a hash of
the current path. Logs never include job payloads, note bodies, prompts,
provider responses, credentials, or raw error text. The existing SQLite job
progress remains the Admin/API source of current state; these records make
`docker compose logs` and OTLP collection useful for live diagnosis without
turning logs into a second knowledge store.

### Ordinary-note recall and extraction reset

The deployment usability review found that ordinary Markdown knowledge and
durable memory were incorrectly coupled at the review queue. The current
`recall` implementation searches only promoted memory FTS/vector rows, while
`search_notes` advertises semantic/hybrid modes but degrades every hybrid call
to lexical and rejects semantic mode. As a result, an ordinary article is
either invisible to semantic recall or exploded into many manually reviewed
memory candidates.

The corrected design keeps two distinct persistence classes behind one useful
Agent workflow:

1. Every indexed Markdown note automatically contributes a rebuildable,
   Vault-scoped note recall projection. Lexical search is always available;
   when the `embedding_note` role is bound, current note projections receive
   reference-only embedding jobs and semantic/hybrid retrieval. No review is
   required because these rows are derived pointers back to canonical source.
2. `recall` federates high-value durable memories with bounded `related_notes`
   cues. A cue contains only path/revision/title, a bounded matching snippet,
   structural metadata, score/provenance, and a note resource URI. The Agent
   follows it with `read_note` for exact content. Normal recall still performs
   no query-time generative-model call and never scans the filesystem.
3. Automatic memory extraction remains candidate-first but is narrowed to at
   most three high-leverage durable propositions per note. The prompt and local
   policy distinguish reusable owner/project state from ordinary article
   coverage; low-value model output does not enter the review queue.
4. Because this behavior is still under deployment testing, Admin exposes an
   audited reset-and-rerun operation that removes only unpromoted derived
   candidates and starts a fresh full-Vault extraction under the new pipeline
   version. It does not migrate or reinterpret the old candidate rows, delete
   canonical notes/memories, or erase job/audit history.
5. Deployment evidence from pipeline v2 showed that prompt/schema labels are
   not proof of source intent: a model labelled ordinary implementation facts
   as `owner_environment` and `project_state`. Pipeline v3 therefore defaults
   automatic candidate extraction to explicit source opt-in. A note must carry
   boolean frontmatter `mcp-vault-memory: true` before it is sent to the
   extraction model. The subsequent v4 trust correction removes the
   `all_notes` override entirely; legacy values normalize to explicit-only.
   Ordinary unmarked notes remain fully available through the lexical/semantic
   related-note index and produce no proposal/provider cost.
6. Deployment interaction after v3 showed that successful delete/restart
   responses were visible only in a notice above the current scroll position.
   Memory/candidate cards continued rendering the old parent snapshot until a
   four-request page reload completed, and manual backfill plus reset/backfill
   appeared as two overlapping actions. The page must apply successful
   lifecycle/candidate changes locally, surface the returned durable job beside
   the action, admit at most one ordinary manual backfill while one is active,
   and expose one context-sensitive “process marked notes” action. Job progress
   must distinguish notes evaluated by the model from notes skipped by the
   explicit source marker so a zero-candidate completion is explainable.
7. Human review cannot be a normal dependency. Under the default
   `explicit_only` source mode, the note marker is the owner's extraction
   authorization; locally valid, source-anchored, non-conflicting proposals
   that meet automatic policy become canonical memories without a click.
   Proposals that fail admission are automatically rejected with a bounded
   diagnostic rather than accumulated as pending work. The candidate table
   remains a derived validation/audit seam required by ADR-0007, but the Admin
   product language and default workflow expose automatic processing, durable
   memories, and exceptional diagnostics—not “generate candidates”. The noisy
   `all_notes` compatibility mode may retain opt-in review behavior, but it
   cannot make the default service depend on an operator.
8. Per-note frontmatter opt-in is also an unacceptable human dependency. It was
   a temporary false-positive circuit breaker, not a viable product contract.
   Automatic memory is enabled once per Vault and then evaluates ordinary note
   changes without requiring authors to alter their Markdown. Exact source
   evidence remains mandatory, while automatic materialization is restricted
   to intrinsically personal/temporal classes: identity, preference, accepted
   decision, progress, relationship, and significant event. General facts,
   project/software descriptions, procedures, and inferred constraints remain
   ordinary `related_notes` unless an Agent explicitly calls `remember`.

This correction preserves ADR-0007: promoted durable memory remains canonical
Markdown with provenance, while related-note cues and extraction candidates
remain rebuildable SQLite/vector projections. A new ADR records the federated
recall boundary and prevents note-search hints from silently becoming durable
facts.

## Work breakdown

1. **Plan and boundary audit.** Create this plan, inspect the current server/MCP/WebDAV composition and CI, and record exact unsupported external checks. Validation: plan review and clean diff scope.
2. **Fixture and protocol harness.** Add a disposable real-HTTP fixture/startup seam, readiness/cleanup behavior, sanitized connection metadata, and MCP wire/header assertions. Validation: fixture smoke and targeted MCP tests.
3. **Official MCP conformance gate.** Add the pinned CLI invocation, advertised-revision check, expected-failures policy, CI artifact upload, and Make target. Validation: official suite when network/tooling is available; otherwise a deterministic “not installed/not configured” failure path plus local wire smoke.
4. **WebDAV interoperability.** Add the Litmus wrapper, HTTP method/condition fixtures, compatibility matrix, and documented manual client runbook. Validation: real HTTP smoke; Litmus result or an explicit blocked gate.
5. **E2E and migration evidence.** Add disposable end-to-end scenarios, two-Vault isolation, provider-fake outage, public proxy/Admin separation, backup/restore/reconciliation checks, and prior-fixture migration validation. Validation: `cargo test`, Compose/test script, fixture integrity checks.
6. **Performance and threat/traceability evidence.** Add bounded benchmark/report tooling, threat-model verification checklist, and requirements traceability table. Validation: benchmark report generation and automated security/traceability checks.
7. **Release readiness.** Add operator/first-release docs, manifest/checksum/SBOM/signature verification, CI/release gates, update repository indexes/checksums, run all available checks, and archive this plan only after all acceptance criteria and documented exceptions are reviewed.
8. **Filesystem and Core concurrency remediation.** Make missing-parent
   creation idempotent on Unix/portable backends; introduce the shared Core
   runtime; cover same-parent, same-path, cross-Core, and cross-protocol races.
9. **Maintenance and restore remediation.** Add counted admission guards,
   drain in-flight mutations/requests before backup/restore, and replace all
   live schema objects when restoring an older snapshot. Validate concurrent
   backup/write and old-schema restore scenarios.
10. **Authentication and key remediation.** Add installation-key verification
    state, atomic first-Admin insertion, and a bounded blocking Argon2 executor.
    Validate PAT restart persistence, wrong-key rejection, setup races, and
    runtime responsiveness. The later first-run provisioning decision
    supersedes permission-bit enforcement without weakening key identity.
11. **Durable job and index remediation.** Terminally consume generic outbox
    jobs, reset retries, propagate task cancellation, and coalesce same-Vault
    full rebuilds without losing a newer dirty event. Validate restart, retry,
    cancellation, queue drain, and bounded rebuild counts.
12. **Provider and OAuth remediation.** Share Provider concurrency gates,
    expose Provider mode and subject-grant Admin APIs, normalize public JWKS,
    reject plaintext symmetric keys, and remove unsupported cache expiry.
    Validate Vault scope, concurrency, secret-at-rest, and end-to-end auth.
13. **Regression and release revalidation.** Update architecture/security/
    operations docs and checksums; run all Rust/frontend/public-protocol/
    migration/recovery checks. Do not rebuild deployment images until these
    gates pass.
14. **Initial self-contained first-run provisioning.** Add managed secret paths
    and atomic create/reuse behavior in `auth`/`server`, and remove application
    Admin CIDR enforcement. Item 17 supersedes this item's temporary setup-token
    mechanism. Validate fresh install, restart, concurrent creation,
    explicit-master-key preservation, lost-key fail-closed behavior, and
    authenticated Admin route protection.
15. **Chinese Admin usability redesign.** Refactor `frontend/admin/src/App.tsx`
    and `app.css` into grouped navigation, Chinese authentication/dashboard/
    management copy, page-specific summaries and lists, progressive disclosure
    for advanced/destructive controls, and choice-based permission/scope
    inputs. Extend frontend tests and update Admin documentation. Validate
    lint/test/build plus browser visual/interaction checks without changing
    backend routes or secret exposure.
16. **First-Admin setup visibility.** Expose a read-only setup-availability
    projection from the Auth service through the public Admin API, drive the
    unauthenticated shell from that state, and fail closed to login if status
    cannot be confirmed. Validate the fresh-to-initialized transition, a
    second setup rejection, concurrent setup safety, and frontend conditional
    rendering.
17. **Password-only first-Admin claim.** Remove bootstrap-token configuration,
    storage, CLI, Auth digest checks, Admin DTO fields, and frontend guidance;
    keep setup availability and atomic first-user insertion. Update ADR-0004,
    security/interface/deployment documentation and compatibility guidance.
    Validate fresh setup with username/password only, concurrent single-winner
    behavior, obsolete-variable rejection, no token artifacts, and restart
    login-only behavior.
18. **Password-policy guidance.** Add persistent, accessible Chinese guidance
    to first-Admin and WebDAV password inputs, make the translated policy error
    concrete, and document the exact default UTF-8-byte/placeholder behavior.
    Validate both forms and error translation through frontend assertions.
19. **Relative default-data setup regression.** Resolve the configured data
    directory against the process working directory before constructing the
    default Vault context, and remove the obsolete service-owned bootstrap-token
    path left by prerelease builds. Validate that the default `./data` path
    produces an absolute Vault root without clearing partially initialized
    operational state.
20. **Advertised data endpoint correctness.** Separate the canonical endpoint
    origin shown in Admin connection cards from Host/Origin allow-list policy.
    Add an optional typed public-origin setting, preserve configured reverse
    proxy HTTPS origins, and otherwise derive a direct-listener URL that
    includes the actual data bind port. Validate local `:8080`, explicit HTTPS,
    IPv6, MCP, and WebDAV URL shapes.
21. **Refresh-safe Admin session restoration.** Issue and clear a dedicated
    non-HttpOnly CSRF cookie alongside the HttpOnly session cookie, recover it
    into the in-memory API client on page load, verify the current session
    before rendering authenticated pages, and preserve mutation CSRF checks.
    Validate reload restoration, expired/missing-cookie fallback, logout
    cleanup, and a post-restore state-changing request.
22. **Truthful progress projections.** Make terminal jobs report complete
    progress even when a handler has no incremental counter, preserve unknown
    progress as unknown for non-terminal jobs, and calculate index coverage
    from the current Vault's authoritative Markdown file count rather than a
    missing UI field. Validate empty, partial, complete, and zero-note states.
23. **Complete model configuration.** Expose provider model discovery, manual
    provider-specific model registration for endpoints without a usable model
    list, model inventory, and Vault-scoped role bindings through Admin API and
    the Chinese console. Keep provider definitions global, resolve bindings
    through `ProviderService`, validate capability metadata, and never put API
    keys or arbitrary provider calls in React.
24. **Operable memory extraction.** Add a typed Vault-scoped extraction policy
    with an explicit enabled state, hard per-note bounds, and an observable
    effective `memory_extraction` model binding. Markdown
    create/update/move/restore events enqueue durable `memory.extract` jobs only when the
    policy is enabled. Admin can enqueue a bounded full-Vault extraction for
    existing current Markdown notes; workers still re-read through Vault Core,
    validate the current revision, and treat LLM output as an untrusted
    exact-source selection before automatic promotion/rejection.
25. **Memory extraction UI and diagnostics.** Show whether extraction is ready,
    why it is blocked, which model is bound, source-evidence policy, and the
    latest extraction jobs. Provide one coherent existing-note operation,
    immediate card/job feedback, and exceptional diagnostics while normal
    canonical Markdown promotion remains autonomous.
26. **Cost-safe extraction progress.** Publish a redacted phase, current note
    path, note ordinal, completed/total counts, model-evaluated notes, unmarked
    skips, source-ingestion failures before a Provider call, and generated-
    output failures after a Provider call while extraction runs. Render those
    fields in Admin instead of a
    bare `0%`. Treat a successful HTTP response whose body stream is interrupted
    as an ambiguous, non-retryable provider outcome because the model may
    already have consumed billable tokens; require an explicit Admin retry.
    Validate single-note and full-Vault progress, Chinese diagnostics, and the
    absence of note bodies/provider responses from persisted progress.
27. **OpenAI-compatible wire profiles.** Replace the assumption that every
    Chat Completions endpoint accepts strict OpenAI `json_schema` output with a
    typed model compatibility profile. Preserve strict OpenAI behavior, add
    MiMo's documented `json_object`, `max_completion_tokens`, and thinking
    controls, validate all returned JSON locally, and project redacted failure
    categories such as token-limit truncation, missing final content, invalid
    HTTP JSON, and invalid structured JSON. Evaluate the user-requested
    `openai_rust_sdk` without allowing it to bypass the existing SSRF, timeout,
    response-size, redaction, and retry-safety transport boundary.
28. **First-class multi-provider compatibility.** Extend the typed Provider
    adapter/profile catalog beyond OpenAI and Anthropic with documented presets
    for DeepSeek, Xiaomi MiMo, Zhipu GLM, Moonshot/Kimi, Google Gemini, and
    Alibaba Qwen/DashScope. For each preset, record and test the official Base
    URL pattern, authentication style, model-list behavior, structured-output
    mode, token-limit field, thinking/reasoning extension, and finish/error
    semantics. Reuse `ProviderTransport`; keep provider-specific translation in
    the Provider crate and expose a Chinese Admin preset selector without
    moving secrets or provider calls into React.
29. **Rebuildable ordinary-note semantic retrieval.** Complete the missing
    `embedding_note` vertical slice in State/Indexer/Provider/Server: resolve
    current note projections by reference, schedule only missing or stale
    Vault-scoped vectors in bounded durable batches, prune stale derived note
    vectors, and implement lexical/semantic/hybrid note ranking with explicit
    degradation. Validate provider-free lexical behavior, semantic paraphrase
    retrieval, model/Vault isolation, stale-revision exclusion, and restartable
    embedding jobs through local fakes.
30. **Federated recall cues.** Extend the Memory application result and MCP
    `recall` contract with bounded `related_notes` selected from the Index
    service, while retaining separately typed durable `memories`, provenance,
    result/token budgets, deterministic ordering, and no query-time LLM.
    Update discovery instructions so an Agent can recall that relevant source
    exists and then read it. Add public MCP tests proving ordinary notes are
    recallable without promotion and remain Vault-scoped.
31. **Low-noise extraction restart.** Version and constrain extraction to zero
    through three durable candidates per note, add deterministic review-admission
    thresholds and exact normalized deduplication, and add an authenticated
    Admin reset-and-rerun operation plus Chinese UI. The operation deletes only
    unpromoted candidate projections, retains canonical notes/memories and
    operational history, and audits counts. Validate zero-result articles,
    candidate caps, duplicate suppression, reset scope, and fresh backfill.
32. **Explicit extraction source intent.** Add a typed Vault-scoped source mode
    whose safe default is `explicit_only`, parse the namespaced boolean
    `mcp-vault-memory` frontmatter through the bounded Markdown analyzer before
    provider/model resolution, and normalize legacy `all_notes` to this safe
    mode. Bump the extraction pipeline/prompt version, expose the
    Chinese control and marker example, and prove that an unmarked article
    makes zero provider calls while a marked note still yields bounded
    candidates. A reset under v3 discards the v2 false positives rather than
    migrating them.
33. **Operable durable-memory lifecycle.** Expose the existing revision-aware
    MemoryService archive/restore/permanent-forget boundary through complete
    Admin routes and Chinese controls. Active memories can be archived;
    archived memories can be restored; permanent deletion requires a warning
    that names the memory and explains canonical Markdown/history impact. All
    operations carry the displayed revision, mutate canonical managed Markdown
    through Vault Core, append redacted audit facts, and refresh the list.
    React never deletes projection rows directly. Validate stale-revision
    conflicts, archive/restore/delete lifecycle, candidate independence, and
    confirmation copy.
34. **Coherent Memory-page operations.** Optimistically reconcile successful
    memory deletes/archive/restore and candidate decisions against the current
    card lists; merge the returned restart/backfill job into the visible task
    list; replace the two overlapping run/reset buttons with one
    context-sensitive action; disable it while a memory extraction job is
    active; and make repeated Admin run admission return the existing active
    job instead of enqueuing another full-Vault scan. Persist evaluated-note and
    source-policy-skip counters so the UI can explain why an explicit-only run
    called no model. Validate two sequential deletes without a parent refresh,
    immediate reset feedback, active-run deduplication, and zero-marked-note
    progress.
35. **Autonomous explicit-note promotion.** Make `explicit_only` extraction
    self-operating: validate source anchors/schema/scores, automatically promote
    every admitted memory type through the existing MemoryService policy, and
    automatically reject below-policy or conflict outcomes with diagnostic
    reasons. Remove the routine auto-promotion toggle and candidate-generation
    language from the Chinese UI; show pending rows only as exceptional
    “needs attention” diagnostics, and make ordinary operation require no
    review click. Preserve candidate-first validation internally and prove
    marked-note extraction creates canonical Markdown with zero pending rows.
36. **Remove author-facing note markers.** Replace the v4 per-note marker gate
    with a Vault-level `automatic` source mode that accepts legacy
    `explicit_only`/`all_notes` values as migration aliases. Process normal
    Markdown changes automatically, keep exact-quote/current-revision
    validation, and locally reject automatically unsafe classes such as facts,
    project descriptions, procedures, and inferred constraints. Update Admin
    to say “write normally”, rename manual backfill to “process existing
    notes”, bump pipeline/prompt version, and prove unmarked personal/decision
    evidence promotes while a generic technical statement is rejected.
37. **Isolate per-note extraction output failures.** Preserve a redacted schema
    violation category and trusted schema path instead of collapsing every
    mismatch to `provider_schema_invalid`; keep nonessential metadata optional;
    and make a full-Vault backfill checkpoint an isolated model-output failure
    before continuing with later notes. Persist bounded failed-note diagnostics,
    complete a mixed run with warnings, and open a cost-safety circuit only
    after repeated consecutive output-contract failures. Configuration,
    authentication, endpoint, state, lease, and retryable transport failures
    remain job-level outcomes. Manual retry of a failed `memory.extract` job
    must retain its durable cursor rather than rebilling completed notes.
38. **Skip already evaluated note revisions before Provider calls.** Add a
    Vault/file-scoped durable evaluation ledger keyed by source revision and a
    deterministic profile of pipeline/prompt, extraction policy, binding,
    model, and Provider output-affecting configuration values. Successful zero-result,
    promoted, and locally rejected evaluations all become current coverage;
    failed or interrupted calls do not. Automatic events and the default Admin
    backfill skip current coverage before reading/sending note content. Add an
    explicit `include_evaluated` task option, off by default, so an operator can
    knowingly re-evaluate unchanged notes at additional token cost. Preserve
    candidate/content idempotency as a second application-layer guard.
39. **Make extraction rejection and JSON-envelope behavior truthful.** Split
    locally rejected proposals into source-evidence, durability-policy, and
    other local-validation counters instead of labelling every rejection as an
    evidence failure. Narrow the extraction schema to combinations the local
    promotion policy can actually admit. For prompt-constrained JSON-object
    Providers, include an exact root-envelope template and apply only a bounded
    deterministic repair when an otherwise schema-valid candidate or candidate
    array omitted the single required array envelope; always run the complete
    schema validator after repair and never turn an empty/unknown object into a
    successful zero-result response.
40. **Separate source-ingestion failures from generated-output failures.** Give
    Phase 1 source read, size-bound, and UTF-8 failures typed stable categories
    that prove no Provider call occurred. Give post-Provider Stage 1 decoding,
    semantic bounds, and evidence-anchor failures a separate typed generated-
    output category. Persist independent cumulative counters and at most 20
    redacted note diagnostics for each category, including only ordinal, path,
    stable code, and elapsed time. The Admin UI must explain the two outcomes in
    Chinese and must not reuse the ambiguous `skipped`/“格式或读取跳过” label.
    Generated-output validation failures participate in the existing
    consecutive-failure cost circuit; source failures do not, and both remain
    note-local so later notes continue.
41. **Remove the first-configuration regeneration dead zone.** Move required
    fresh-regeneration admission into the shared Memory application service.
    Saving an enabled extraction policy or either memory model binding must
    immediately attempt the same Vault-scoped singleton admission used by
    startup/reconciliation, while the 300-second reconciliation remains only a
    recovery fallback. A pending fresh regeneration with ready configuration
    must not leave the Admin page with no task and both manual actions disabled.
42. **Derive provenance locally without model coordinates.** Stop asking a
    Provider to count lines, echo source text, or select evidence ranges. Match
    the Codex `raw_memory`/`rollout_summary`/`rollout_slug` Phase 1 contract and
    bind a non-empty output to file/path/revision plus a normalized whole-source
    hash locally. Explicit/imported provenance may still carry caller-validated
    line or heading anchors.

## Progress

- [x] 2026-08-21 — Read `AGENTS.md`, the ordered product/architecture/implementation/plan documents, and WP-14-specific testing, interface, standards, security, deployment, and ADR documents.
- [x] 2026-08-21 — Audited the existing server composition, MCP/WebDAV tests, CI, Makefile, Dockerfile, and WP-13 release assets.
- [x] 2026-08-21 — Researched the official MCP conformance CLI and RMCP support expectations; recorded expected-failure semantics and version constraints.
- [x] 2026-08-21 — Add `mcp-vault-fixture`, real HTTP MCP/WebDAV/Admin smoke, and loopback-only generated PAT injection at the fixture boundary.
- [x] 2026-08-21 — Fix MCP list-result `ttlMs` and dated-schema `ToolEnvelope.data` object shape; unsupported prompts/completion are now method-not-found rather than silently handled.
- [x] 2026-08-21 — Add pinned official MCP conformance wrapper/baseline and run target core scenarios successfully; retain exact output in the local validation record.
- [x] 2026-08-21 — Add WebDAV Litmus entry point and compatibility matrix; actual external plugin/Litmus execution remains a release-environment item.
- [x] 2026-08-21 — Add public HTTP E2E smoke, migration fixture command, bounded performance baseline, threat-model verification table, and requirements traceability.
- [x] 2026-08-21 — Add release checklist, image digest/checksum/SBOM/signature verification hooks, CI job, and manual release workflow.
- [x] 2026-08-21 — Run Rust/frontend/docs/migration/HTTP/performance/container validation; record external-tool and GUI-client blockers below.
- [x] 2026-08-21 — Package the latest image as a loadable Linux/amd64 archive and add a tested Nginx HTTPS Compose bundle with separate public data-plane and LAN-only Admin listeners.
- [x] 2026-08-21 — Fix real Sync Engine first-upload concurrency failures with a shared short-lived SQLite write gate and immediate metadata transactions; 32 concurrent in-process PUTs and 50 concurrent public HTTP PUTs now commit and read back without incomplete journals or client-side throttling.
- [x] 2026-08-22 — Resolve the deployment-observed durable-job backlog: `outbox.event` has a terminal compatibility handler; full-Vault index jobs are Vault-scoped generation-coalesced, renewable, cancellable, and periodic reconciliation admits one durable rebuild instead of racing a worker rebuild.
- [x] 2026-08-22 — Remediate all actionable review findings across filesystem/Core concurrency, maintenance/restore, installation keys/Auth, Jobs/indexing, Provider limits, and OAuth/Admin wiring; negative, concurrent, restart, Vault-isolation, public HTTP, and frontend tests pass without replacing the deployment image.
- [x] 2026-08-22 — Initially implement self-contained first-run master-key/bootstrap-token provisioning; the later password-only decision supersedes the token portion while retaining automatic master-key provisioning.
- [x] 2026-08-22 — Simplify and fully localize the Admin console in Chinese, retain all control-plane capabilities, and complete responsive visual QA.
- [x] 2026-08-22 — Show first-Admin creation only while setup is available, while retaining the backend atomic one-time guard.
- [x] 2026-08-22 — Make the production `mcp-vault` binary the Cargo default and the explicit `make run` target after the fixture binary made local startup ambiguous.
- [x] 2026-08-22 — Replace manual bootstrap-token handling with password-only atomic first-Admin initialization and update the security/deployment contract.
- [x] 2026-08-22 — Explain the actual password policy inline and in validation errors instead of requiring trial and error.
- [x] 2026-08-22 — Fix first-Admin setup with the default relative `./data` directory and clean the obsolete managed token artifact.
- [x] 2026-08-22 — Correct Admin-generated WebDAV/MCP addresses so local direct-listener URLs include `:8080` and proxy URLs use their configured public origin.
- [x] 2026-08-23 — Restore valid Admin sessions and CSRF capability after a browser page refresh without exposing the session bearer token.
- [x] 2026-08-24 — Correct completed-job and index-coverage progress projections observed in the amd64 deployment.
- [x] 2026-08-24 — Expose model discovery/manual registration and role bindings in the Admin API/UI.
- [x] 2026-08-24 — Make memory extraction explicitly configurable, event-driven, manually backfillable, and observable through durable jobs and the Admin UI.
- [x] 2026-08-24 — Make extraction progress describe its current safe unit of work and prevent automatic rebilling after an interrupted successful response body.
- [x] 2026-08-24 — Add typed OpenAI-compatible wire profiles and repair MiMo structured extraction without weakening the Provider transport boundary.
- [x] 2026-08-24 — Add documented, locally contract-tested DeepSeek, MiMo, Zhipu GLM, Kimi, Gemini, and Qwen provider presets on the shared secure transport.
- [x] 2026-08-24 — Make AI service editing and deletion operable from Admin:
  revision-aware secret-preserving PATCH, one transactional dependent
  model/binding/vector and owned-secret deletion, redacted audit counts, and
  backend/frontend regression tests.
- [x] 2026-08-24 — Add redaction-safe structured Worker lifecycle, progress,
  and stable-error logs for durable jobs, including per-note memory extraction
  progress without note bodies or provider payloads.
- [x] 2026-08-24 — Make ordinary Markdown automatically recallable as derived
  related-note cues, narrow durable extraction, and support discarding the
  current test candidates before a clean re-extraction.
- [x] 2026-08-25 — Require explicit note-level source intent by default after
  pipeline-v2 models misclassified ordinary technical facts as durable
  environment/project memories.
- [x] 2026-08-25 — Add missing Admin controls and route coverage for archiving,
  restoring, and permanently deleting a durable memory.
- [x] 2026-08-25 — Make Memory-page mutations immediately coherent and collapse
  manual extraction/reset into one observable, active-job-safe operation.
- [x] 2026-08-25 — Remove routine human review from explicit-note extraction;
  automatically promote valid results and reject non-admitted proposals.
- [x] 2026-08-25 — Remove the `mcp-vault-memory` authoring requirement and make
  Vault-level automatic memory safe for ordinary unmodified Markdown.
- [x] 2026-08-25 — Diagnose the deployment-observed
  `provider_schema_invalid` failure, expose its redacted structural cause, and
  continue a full-Vault run after isolated per-note output failures.
- [x] 2026-08-25 — Make manual extraction incremental by default with durable
  note-evaluation coverage and an explicit option to include already evaluated
  unchanged notes.
- [x] 2026-08-26 — Remove Worker batch head-of-line blocking, make legacy
  memory reset exclusive with old Phase 1/2 jobs, and replace the Admin's
  bounded mixed task list with a Vault-scoped active/waiting/terminal overview.
  Detailed evidence is in
  `docs/exec-plans/completed/wp-14-worker-fairness-and-job-visibility.md`.
- [x] 2026-08-26 — Replace prerelease memory compatibility with ADR-0017:
  migration 0011 deletes all old memory rows/tasks, current jobs carry a hard
  pipeline generation, obsolete cursors are cancelled before handler/Provider
  calls, managed memory files are cleared through Vault Core, and one fresh
  Phase 1 job starts at note one. Detailed evidence is in
  `docs/exec-plans/completed/wp-14-prerelease-memory-pipeline-cutover.md`.
- [ ] 2026-08-25 — Correct misleading rejection progress and harden the
  MiMo/JSON-object root-envelope contract observed in deployment logs.
- [x] 2026-08-26 — Split Phase 1 source-ingestion failures from post-Provider
  output/evidence validation failures in typed errors, durable progress, logs,
  Admin copy, tests, and interface documentation.
- [x] 2026-08-26 — Admit required fresh regeneration immediately after the
  final policy/model configuration becomes ready instead of waiting up to the
  300-second reconciliation interval.
- [x] 2026-08-26 — Replace model-echoed evidence quotes with server-derived
  evidence from model-selected, explicitly numbered source-line ranges.
- [ ] Complete release-environment Litmus, named Obsidian plugin/client matrix, full-scale performance, clean-host restore, and signed-artifact verification; then review whether the full frozen MCP requirements report is applicable to the advertised capability set.

## Decisions

- The official MCP conformance suite is an external release gate, while project-specific Rust tests remain necessary for Vault isolation, auth, safe writes, and discovery policy.
- A test fixture may inject only a generated test credential at the loopback boundary; it must not add an unauthenticated production mode or let protocol clients choose `vault_id`.
- External GUI/plugin compatibility is represented as a versioned matrix with reproducible manual steps and evidence links; unavailable clients are marked unverified rather than inferred from HTTP tests.
- Release signing is verification-first in this repository. Private signing material belongs to CI/release infrastructure, never source control or local fixtures.
- The Nginx deployment fixture uses a fixed private Compose subnet so `MCP_VAULT_TRUSTED_PROXY_IPS` can name the exact proxy peer. Public port 443 proxies only `/mcp/`, `/dav/`, and public health routes. Admin is a separate HTTPS listener bound to one configured LAN address, restricted by a Nginx source CIDR, and still authenticated/CSRF-protected by the control-plane application.
- Deployment archives use the image tag `mcp-vault:0.1.0` and include the architecture in each filename. A Docker `save` archive is preferred over an OCI-only archive so ordinary `docker load` works on the target host.
- SQLite's one-writer constraint is handled inside the state boundary: WebDAV
  credential touches and file-journal/metadata writes share one fair admission
  gate, metadata commits start with `BEGIN IMMEDIATE`, and pool admission has a
  separate 30-second bound from the 5-second SQLite busy timeout. The gate does
  not cover upload streaming, fsync, history materialization, or atomic rename.
- Every Core constructed by a protocol, Admin service, reconciler, memory
  service, or worker receives one composition-owned runtime. It shares weakly
  retained per-path locks and counted maintenance admissions across planes.
- Backup/restore mode changes precede draining: read-only waits for active
  writes, offline waits for active requests, and only an explicit runtime-minted
  capability allows Core journal recovery while restore owns the offline gate.
- Provider privacy mode and OAuth subject grants are Vault-scoped Admin
  resources. One process Provider service owns revision-aware transport caches;
  manually supplied normalized RSA JWKS have no artificial expiry until an
  SSRF-safe automatic refresh worker exists.
- Migration 0009 stores only a one-way installation-key verifier and clears/
  disables prerelease JWKS caches that cannot be proven public-only. Legacy
  issuers must be reconfigured with public RSA material after upgrade.
- First-run secret generation is an MCP Vault startup responsibility, not an
  Nginx/Compose responsibility. Default managed files live below the service
  data root and are excluded from ordinary service backup contents; explicit
  secret inputs remain operator-owned.
- File permission/ACL policy and Admin source-network admission are deployment
  responsibilities. MCP Vault keeps application-layer Admin authentication and
  a loopback default bind but no longer interprets source CIDRs.
- Simplified Chinese is the first-release Admin UI language. Information
  architecture uses grouped navigation and progressive disclosure; advanced
  controls and raw API payloads remain available without dominating the
  ordinary operator path. This is a presentation-layer change and does not
  merge protocol/authentication domains or move business logic into React.
- Job percentages are server projections: terminal success is `1.0`, measured
  structured progress is normalized, and absent progress stays null. Index
  coverage is indexed eligible Markdown divided by the current non-managed
  Markdown population; an empty Vault has no percentage.
- Provider edit/delete is an application lifecycle boundary. Edits carry the
  Admin-visible revision, preserve an omitted secret, and retain only the
  replacement ciphertext after a successful rotation. Deletion uses explicit
  ordered SQL in one immediate transaction rather than schema-wide cascades so
  it can return redacted counts and prove that only bindings, models, derived
  embeddings/vectors, health/configuration, and Provider-owned secrets were
  removed. Canonical files, memories/candidates, jobs, and audit are retained.
- Provider records, model inventory, and model-role bindings remain separate.
  Discovery is best effort and manual registration accepts the exact external
  model ID when `/models` is absent or incomplete. The first-release UI writes
  Vault overrides so future multi-Vault defaults do not require a schema or API
  redesign.
- Memory extraction is disabled by default and controlled by a typed
  Vault-scoped policy. Enabled semantic Markdown events and explicit Admin
  backfills create durable work; workers re-check policy before sending content
  and only validated candidates can reach promotion. Reconciled revisions keep
  `external_change` provenance but emit semantic file lifecycle events, while
  consumers retain compatibility with already-persisted legacy event labels.
- Structured memory extraction has a typed request-specific total response
  deadline (300-second default, 30-1800 second range) rather than inheriting the
  Provider's 30-second default used by ordinary operations. Once an HTTP
  success status has arrived, a body
  timeout/interruption/read failure is an ambiguous terminal result and is not
  automatically replayed because remote billable work may already have run.
- Memory extraction checkpoints the current path/ordinal, per-note timing,
  prior completed-path cursor, processed/candidate/skipped counters, and stable
  error code without persisting note or response content. Reclaimed full-Vault
  jobs resume after the cursor; a post-provider checkpoint failure stops for
  explicit review instead of rebilling automatically.
- OpenAI-compatible request dialect is typed configuration, not an unvalidated
  binding payload. First-class Provider kinds select DeepSeek, MiMo, GLM, Kimi,
  Gemini, or Qwen presets; model settings independently override output mode,
  token field, thinking, and one-call generation limit. Legacy generic rows may
  migrate from exact official hosts, but model names never select a vendor
  because local/proxy runtimes can serve the same IDs with different dialects.
  The application always validates JSON locally.
- Do not adopt `openai_rust_sdk` for this remediation. [OpenAI's published
  official SDK list](https://github.com/openai/openai-openapi#generated-sdks)
  has no Rust client, while the evaluated [community
  crate](https://docs.rs/openai_rust_sdk/latest/openai_rust_sdk/) is
  unofficial, requires Rust 1.97.1 rather than this workspace's 1.94 toolchain,
  constructs its own `reqwest` client, and does not preserve MCP Vault's
  endpoint-resolution, redirect, response-size, shared-concurrency, or
  post-success no-replay controls. Provider wire serialization therefore stays
  in the project-owned adapter behind `ProviderTransport`.
- Ordinary note knowledge is automatically recallable derived state, not a
  reason to materialize every article proposition as durable memory Markdown.
  `recall` returns durable memories and related-note cues as separate typed
  collections so an Agent can discover source knowledge without confusing a
  search hint with an accepted fact. The reset path intentionally discards
  unpromoted prerelease candidates rather than migrating test output produced
  by the overly broad pipeline.
- A model-provided durability scope is untrusted classification, not evidence
  that a statement should become durable memory. The initial per-note marker
  circuit breaker was superseded by ADR-0015: ordinary notes require no MCP
  Vault metadata, and exact evidence plus a local automatic-type allow-list
  controls materialization.
- Human review is an exceptional diagnostic, not a runtime dependency. The
  Vault-level enable switch authorizes automatic extraction; local validation
  and promotion policy decide whether a proposal becomes canonical or is rejected.
  The candidate projection remains internal derived state and never becomes a
  second canonical knowledge store.
- Note authors never add MCP Vault control metadata. The one Vault-level enable
  switch authorizes automatic evaluation; ordinary knowledge remains
  `related_notes`, and only locally allow-listed durable classes can be
  materialized from note evidence.
- A generated-output contract error is note-local inside a full-Vault backfill.
  Checkpoint it once, retain at most 20 redacted Admin diagnostics, and continue
  later notes without replaying the same paid request. Three consecutive output
  failures indicate probable model/profile incompatibility and open a fixed
  cost-safety circuit. Configuration, authentication, endpoint, state, lease,
  and retryable transport failures remain job-level. Manual retry preserves the
  memory extraction cursor.
- Extraction v6 exposes only type, exact evidence, durability scope, and source
  line bounds. Decorative model-generated entities, tags, validity, and heading
  metadata are absent rather than optional: strict Structured Outputs requires
  every declared object field to be required, while JSON-object Providers were
  observed omitting empty decorative fields.
- Automatic extraction call idempotency is note-level successful evaluation
  coverage, keyed by Vault/file identity and compared by source revision plus a
  deterministic effective extraction-profile hash. Candidate/content hashes
  remain a second post-Provider write guard. Valid zero-result responses count
  as coverage; failures do not. Default automatic and manual work is
  incremental, while the explicit `include_evaluated` task mode invalidates
  coverage before knowingly re-evaluating unchanged notes.
- Phase 1 failure classification follows the Provider boundary, not a broad
  `InvalidInput` catch. Failures before `generate_structured` that are caused by
  source bytes are source-ingestion failures and prove zero model cost.
  Failures decoding or validating the returned Stage 1 object are generated-
  output failures and use the same bounded continuation/cost-safety policy as
  Provider schema failures. Internal state/Core/configuration failures remain
  job-level outcomes rather than being silently counted as skipped notes.
- MiMo's default `json_object` mode cannot enforce required root properties.
  Phase 1 therefore includes an exact four-key object template as well as the
  JSON Schema. If and only if `raw_memory` is already a valid returned string
  and `source_summary` alone is absent, the Provider boundary may copy that
  string verbatim into the auxiliary summary and rerun the complete schema plus
  Memory evidence validator. It never repairs an empty object, missing core
  memory/evidence, wrong types, or invented anchors, and it never makes a
  second paid request.
- Required fresh-regeneration admission belongs to `MemoryService`, not only
  the Server reconciliation loop. Admin policy/model-binding writes and manual
  run admission invoke the same Vault-scoped singleton operation as startup;
  the 300-second loop remains an idempotent crash-recovery fallback.
- Phase 1 evidence is authoritative-source derived. The Provider sees stable
  `L<number>:` labels and selects bounded start/end lines; MCP Vault validates
  the current revision and computes the excerpt hash itself. Model-echoed text
  is not provenance. This contract is extraction pipeline 9 and memory pipeline
  generation 2, so prerelease Stage 1 state is reset instead of mixed.

## Surprises and discoveries

- The initial repository had strong in-process MCP and WebDAV coverage but no real-process protocol harness; WP-14 added the fixture so route-level tests are no longer the only evidence.
- The 2026-08-24 amd64 deployment exposed incomplete WP-10/WP-12 vertical
  slices: Provider records could be created and models could be discovered by
  a backend test call, but the console neither listed/registered models nor
  bound roles. The memory page reviewed candidates but could not enable or
  backfill extraction, while file events admitted extraction without checking
  the documented opt-in policy. Completed jobs also rendered their empty
  incremental progress as `0%`, and the dashboard expected an index coverage
  field that its endpoint did not supply. These are release-blocking behavior
  gaps rather than operator configuration mistakes.
- Real browser validation found why enabled extraction still had no task after
  a Vault rescan: reconciliation emitted the literal `external_change` event,
  while the worker admitted only `FileCreated`/`FileUpdated`. Core now emits
  semantic create/update/delete/restore event types for reconciled revisions,
  and worker coverage proves both the corrected contract and legacy event
  compatibility.
- OpenAI-compatible model-list responses commonly expose only an ID. Treating
  an absent capability document as proof that a discovered model cannot serve
  a role broke the existing local-fake contract, so role selection remains an
  explicit operator decision. Manually registered models retain validated
  capability metadata without inventing capabilities for discovered records.
- A deployed full-Vault extraction showed `0%` while the model console recorded
  23,943 consumed tokens. The durable progress contained only
  `{completed:0,total:N}`, so Admin could not distinguish idle work from an
  in-flight first-note provider call. The retained previous-attempt error was
  `provider_response_read_failed`; transport had already accepted a successful
  HTTP response and then marked its body-stream failure retryable, allowing the
  same billable note to be submitted up to the job attempt limit.
- The supplied `mcp-vault-memory-failure.log` contained only five INFO records
  for startup/listener readiness and periodic reconciliation. It had no
  provider or job-failure record, confirming that the old generic code could
  not be diagnosed further from application logs. The repaired path persists
  a bounded failure category and per-note duration in authenticated job
  progress while continuing to exclude note bodies and provider responses.
- The later container log for job `01a0376f-8c80-74c2-bf6d-95db7cf6bc0e`
  proves the first nine notes completed and the tenth returned
  `provider_schema_invalid` after about 30 seconds. HTTP JSON and final model
  text had already parsed; the failure was the local schema subset. The old
  unit error discarded whether the mismatch was a missing field, type, enum,
  bound, or unexpected property, then the full-Vault loop returned immediately.
  The exact old field is therefore unrecoverable from that log. v6 preserves a
  trusted schema category/path without response values and continues the next
  note.
- The later `provider_response_invalid` screenshot proved that the earlier body
  stream failure was not the only defect. Source tracing found that every
  OpenAI-compatible model received strict `json_schema`, `max_tokens: 2048`,
  and `temperature: 0`, while [MiMo v2.5 documents JSON Object structured
  output](https://mimo.mi.com/docs/en-US/quick-start/usage-guide/text-generation/structured-output)
  plus [`max_completion_tokens` and separately controlled
  thinking](https://mimo.mi.com/docs/en-US/api/chat/openai-api). The old single
  error code also collapsed invalid HTTP JSON, absent final content, malformed
  structured JSON, and provider-declared truncation, preventing diagnosis from
  the Admin job record.
- Multi-provider research found a second generic-adapter defect: the old URL
  helper inserted `/v1` into every configured path that did not already end in
  `/v1`. That turns Zhipu `/api/paas/v4/` into
  `/api/paas/v4/v1/` and Gemini `/v1beta/openai/` into
  `/v1beta/openai/v1/`. Base URLs are now exact API roots; suffixes append
  directly, with `/v1` supplied only for backward-compatible host-only rows.
- “Thinking” is not one interoperable field. DeepSeek, MiMo, GLM, and Kimi
  document a `thinking` object, DashScope Qwen uses `enable_thinking`, and
  Gemini compatibility uses `reasoning_effort`. Structured-output and token
  fields also differ. The final design composes typed axes behind one adapter
  rather than scattering vendor conditionals through memory or Admin code.
- The Provider DELETE route existed before the UI action, but its repository
  issued only `DELETE FROM providers`. SQLite correctly rejected every normal
  configured Provider because models, bindings, and embeddings retained
  foreign keys. PATCH also had no Admin-visible revision field, and successful
  secret replacement retained superseded ciphertext. The lifecycle fix needed
  coordinated Provider/State/Admin/UI behavior rather than a button-only patch.
- Source tracing after the 15-note/41-candidate deployment showed that
  `MemoryService::recall` queried only memory FTS/entity/recent pools and
  accepted vector hits only when `object_type == "memory"`. MCP
  `search_notes` rejected semantic mode and always marked hybrid mode degraded.
  The extraction schema allowed an unbounded memory array and the service took
  up to 64 entries per note. This made ordinary knowledge semantically
  invisible unless it first became review noise, contrary to the discovery,
  retrieval, and memory requirements.
- The first pipeline-v2 deployment still produced two candidates from an
  ordinary server-upgrade design article: one infrastructure dependency was
  labelled `owner_environment`, and one MySQL struct description was labelled
  `project_state`, both with 0.90 model confidence. This proves that a typed
  scope plus score threshold cannot establish source intent and requires a
  deterministic note-level opt-in boundary before any provider call.
- The same Admin screenshot exposed that durable-memory cards were read-only.
  Although MemoryService already had revision-aware archive/permanent forget
  behavior, the route set and Chinese UI did not expose a complete lifecycle,
  leaving the owner unable to remove an incorrect promoted memory without a
  direct API/database workaround.
- The first v3 interaction test showed that the backend returned accepted
  delete/restart operations, but the only immediate acknowledgment was rendered
  above the operator's scroll position. Because the card lists and job list
  remained props until a four-endpoint refresh completed, the current viewport
  looked unchanged and permitted another manual backfill. This is a frontend
  state-coherence and admission problem, not evidence that the DELETE or
  restart routes were absent.
- The official conformance tooling has strict expected-failure baseline semantics: new failures fail, and an old expected failure becoming passing also requires baseline cleanup. This is preferable to a permanently permissive allow-list.
- RMCP's generated `ToolEnvelope` schema originally advertised `data` as unconstrained JSON and list responses omitted `ttlMs`; the official wire-schema checks caught both. The implementation now advertises object-shaped tool payloads and a private 1-second cache hint on list/read projections.
- The conformance suite's generic diagnostic tools are intentionally absent from MCP Vault. The baseline covers only the resulting untestable checks and the absent prompts cache check; tools/resources/header/DNS/caching wire checks remain enforced.
- A full frozen `2026-07-28` requirements run is useful diagnostic evidence but reports 67 failures from generic echo/media/prompt/task/elicitation tools that MCP Vault does not advertise or implement. The release CI gate therefore runs the official scenarios that map to the advertised tools/resources/stateless transport/security behavior; the full report remains a required review artifact before claiming a full suite score.
- WP-13’s full workspace test run required an escalated environment because concurrent loopback listener creation was denied by the default sandbox; this is an environment limitation to document if it recurs, not a reason to weaken network tests.
- The first hardened Nginx smoke exposed an official-image startup dependency on
  `chown` for default cache directories after all capabilities were dropped.
  Running Nginx directly as UID/GID `101:101` and moving PID/request temporary
  paths to a UID-owned tmpfs preserved the capability-free/read-only design.
- A real Sync Engine first sync exposed a server-side concurrent-write defect.
  A disposable fixture reproduced it with 16 parallel nested PUTs: 75 of 114
  returned/read back successfully while 39 returned 500 and were absent from
  state-backed reads. A serial control completed 20 of 20, while concurrency 2
  still produced 3 failures out of 20. SQLite remained integral, but every
  failed request had advanced its operation journal to `file_committed`; this
  localizes the defect after canonical atomic rename and before metadata/outbox
  commit rather than to Nginx, authentication, paths, or file content.
- A file-only write gate was insufficient because every authenticated WebDAV
  request also updates credential `last_used_at`; those concurrent updates held
  the small SQLx pool while waiting for SQLite's writer and surfaced as
  `PoolTimedOut`. Sharing the state write gate with that authentication update,
  reserving the metadata writer before precondition reads, and separating pool
  admission from lock wait removed both failure modes.
- The same deployment showed 1,595 pending jobs after the initial sync. The
  dashboard correctly counts `queued`, `running`, and `retry_wait`, but source
  inspection found two independent backlog causes: the outbox dispatcher
  creates a durable `outbox.event` job without registering a handler, and it
  creates one event-specific `index.rebuild` job whose handler rebuilds the
  entire Vault. This is not canonical data loss, but it is not an acceptable
  steady state. The 2026-08-22 remediation drains generic jobs and coalesces
  each Vault's active full-rebuild generations without dropping a newer dirty
  event.
- A repeated all-feature WebDAV test disproved the initial green concurrency
  conclusion: 31 of 32 distinct nested PUTs returned 201 and one returned 500.
  The operation failed before a durable journal remained because both Unix and
  portable missing-parent loops treat the losing concurrent directory creator's
  `AlreadyExists` as fatal.
- A real HTTP same-path fixture produced three 204 responses and thirteen 500
  responses from sixteen unconditional PUTs. Each request constructed a fresh
  `VaultCore`, so `PathLockManager` was shared only by clones within one request
  and not across WebDAV/MCP/Admin/worker operations.
- Review also found that the mode-only maintenance gate cannot drain writes,
  old-snapshot restore leaves newer live tables behind, PAT-only installations
  can silently rotate an ephemeral digest key, Argon2 blocks Tokio workers,
  failed-job retry/cancellation are ineffective, Provider limits are per call,
  and Provider/OAuth control-plane APIs cannot enable their existing services.
- Counted offline admission initially also blocked the authenticated
  `/maintenance/recover` route. The final design permits login/recovery only
  after the serialized restore operation has exited, while data-plane and
  ordinary Admin work remain unavailable.
- Removing application Admin CIDR enforcement cannot silently ignore the old
  `MCP_VAULT_ADMIN_ALLOWED_CIDRS` variable: doing so would leave operators with
  a false belief that a security policy remains active. Configuration now
  rejects the obsolete variable with migration guidance, while listener bind,
  firewall/VPN, and optional proxy policy stay deployment-owned.
- Existing explicit master-key inputs remain operator-owned for upgrades. Only
  an absent override selects the application-managed path, and a missing
  explicit key file is never created or replaced. Bootstrap-token variables
  are rejected because setup no longer consumes them.
- The existing `rust-embed` macro did not cause an incremental Cargo rebuild
  after `frontend/admin/dist` changed, so a locally rebuilt binary could serve
  stale UI assets even though a clean CI image was correct. A small Server
  build script now declares the dist directory as a Cargo rerun input.
- The existing extraction fingerprint is computed from generated candidate
  content after the paid Provider call. It can stop a duplicate canonical write
  but cannot stop the call itself or guarantee that a nondeterministic model
  selects the same statements. A separate successful-evaluation ledger is
  therefore required before the Provider boundary.
- The deployed “格式或读取跳过” counter was not merely imprecise copy. The
  Worker incremented it for every `MemoryError::InvalidInput` or
  `MemoryError::Markdown`, while `extract_note_with_options` also used
  `InvalidInput` after a paid Provider call for Stage 1 decoding, output bounds,
  and evidence/source mismatch. The aggregate therefore could not tell an
  unreadable note from an invalid generated result and retained no per-note
  cause. The fix must separate these cases at the Memory error boundary before
  changing the UI.
- The reported `required_property_missing` at `$.source_summary` matches the
  configured MiMo preset: `Auto` resolves to `json_object`, which guarantees
  JSON syntax but not the prompt-described field hierarchy. The request already
  included the schema text, so the parser did not discard the field; the
  generated root object omitted it. Response bodies remain intentionally
  unlogged, so the model's internal motive is unknowable, but the enforceability
  gap and missing root property are both established.
- Live local diagnosis found the first-run dead zone directly: reset completed
  at 16:57:54, policy became enabled at 17:00:23, but the full extraction was
  not admitted until periodic reconciliation at 17:02:55. During that interval
  `regeneration_pending` made combined readiness false and disabled both manual
  buttons. The task eventually appeared at 2/178 only because the default
  300-second recovery loop ran.
- The first live MiMo result then failed because the model had to count lines in
  unnumbered Markdown and echo an exact quote; one line-number or character
  drift produced `memory_phase1_evidence_mismatch`. Requiring the model to
  reproduce authoritative source text was the wrong boundary. Numbered range
  selection keeps provenance while removing that brittle comparison.

## Validation

Expected final commands, plus their exact outcomes:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
bash scripts/check-docs.sh
git diff --check
make conformance
make webdav-litmus
make e2e
make migration-check
make perf-baseline
make release-check
```

External tools such as Node-based MCP conformance, Litmus, Docker, Syft, Trivy, and Cosign must report “not available” distinctly. They may not produce a false green release gate. Evidence files must be sanitized and tied to the source/image digest under test.

Validation recorded on 2026-08-24 for structured extraction and first-class
multi-provider compatibility:

- Official DeepSeek, MiMo, Zhipu, Moonshot/Kimi, Google Gemini, and Alibaba
  Qwen/DashScope documentation was reviewed for API roots, Bearer
  authentication, structured output, token fields, thinking controls, model
  listing, and finish semantics. The normalized matrix and source links are in
  `docs/provider-compatibility.md`.
- Provider unit tests assert exact Generic, DeepSeek, MiMo, GLM, Kimi, Gemini,
  and Qwen request bodies, independent output/token/thinking overrides,
  official-host legacy migration, and the rule that a local model name cannot
  select a vendor. A transport-backed integration creates all six first-class
  Provider kinds and receives locally schema-validated results through the
  shared SSRF-safe client.
- `cargo fmt --all --check`, workspace all-target/all-feature Clippy with
  `-D warnings`, all 199 Rust tests, and `cargo doc --workspace --no-deps`
  pass. The Rust run includes the prior memory, Vault isolation, WebDAV,
  Admin/auth, recovery, and worker suites as well as the new Provider contracts.
- Frontend lint, all 15 Vitest tests, TypeScript checking, and the Vite
  production build pass. The Admin evidence covers all first-class Chinese
  provider options, typed model settings, effective MiMo thinking/budget text,
  and the official Gemini API-root template.
- `bash scripts/check-docs.sh`, full `SHA256SUMS` verification, and
  `git diff --check` pass with ADR-0012 and the provider-compatibility guide.
- No paid provider endpoint was called and no Docker image was built. Live
  account/model checks remain release-environment evidence rather than being
  inferred from local serialization tests.

Validation recorded on 2026-08-24 for extraction response diagnosis and
cost-safe progress:

- The supplied deployment log was inspected and contains only five INFO
  startup/listener/reconciliation records; it has no provider or job-failure
  detail. Source tracing confirmed that the old
  `provider_response_read_failed` was emitted only after a successful HTTP
  status while consuming `reqwest::Response::bytes_stream()`, where the
  underlying error category was discarded.
- Provider transport regression tests now send a successful status followed by
  a delayed body and by an interrupted body. They prove request-specific
  timeout override, `provider_response_timeout` versus
  `provider_response_incomplete`, and exactly one HTTP submission despite a
  Provider retry setting greater than zero after response-body failure.
- `cargo fmt --all --check`, workspace all-target/all-feature Clippy with
  `-D warnings`, all 190 Rust tests, and `cargo doc --workspace --no-deps`
  pass. The complete test run used the approved non-sandboxed loopback
  environment because provider/WebDAV fixtures bind ephemeral local ports.
- Frontend lint, all 14 Vitest tests, TypeScript checking, and the Vite
  production build pass. The UI checks cover current-note/ordinal progress,
  previous-attempt labelling, Chinese response-read diagnostics, and the typed
  30-1800 second extraction deadline.
- `bash scripts/check-docs.sh`, full `SHA256SUMS` verification, and
  `git diff --check` pass. The image build was recorded separately below.

Validation recorded on 2026-08-24 for the deployable amd64 image:

- `docker buildx build --platform linux/amd64 --tag mcp-vault:latest
  --tag mcp-vault:0.1.0 --load .` passed. The resulting image ID is
  `sha256:83b54afe6e7cc1721dc7aea51e3ab6674814aab8f4d4bdfaf4e94fce35fcaa66`,
  architecture `linux/amd64`, size `99,345,541` bytes, and runtime user
  `mcpvault` (UID/GID 999).
- A container-side read-only check confirmed the embedded binary is executable,
  `/data` exists, and the container does not run as root. The image carries the
  response-read and extraction-progress remediation described above.

Validation recorded on 2026-08-24 for progress, model configuration, and
memory extraction remediation:

- `cargo fmt --all --check`, workspace all-target/all-feature Clippy with
  `-D warnings`, all 190 Rust tests, and `cargo doc --workspace --no-deps`
  pass. Coverage includes Vault-scoped typed extraction policy, safe
  auto-promotion types, model registration/binding and stale-revision
  conflicts, exact job-type filtering, full-backfill progress, missing-model
  failure visibility, reconciled semantic events, legacy `external_change`
  admission, and the rule that malformed optional AI policy does not block
  index/outbox work.
- Frontend lint, 14 Vitest assertions, TypeScript, and the Vite production
  build pass. Tests cover completed/partial/unknown progress, authoritative
  index coverage, model inventory/role binding, extraction blockers, and recent
  durable extraction jobs.
- A disposable real process and authenticated browser configured a local
  Provider, manually registered and bound `fixture-memory-model`, enabled the
  extraction policy, and submitted a backfill. Adding a Markdown file directly
  to the Vault and using Admin rescan produced a visible `memory.extract` job;
  the intentionally absent provider entered retry with a Chinese connection
  diagnostic, completed jobs showed 100%, and the dashboard reached `2 / 2`
  indexed Markdown at 100%. AI and Memory pages had no horizontal overflow at
  1280px or an explicit 390x844 viewport. The memory crate's local fake proves
  the successful provider-to-candidate-to-promotion path without a paid API.
- The final embedded frontend and Server build completed. A last redundant
  browser reload timed out in the browser-control surface after restart; the
  Server remained running without an application error, and the final bundle
  is covered by the successful build/tests above. The disposable process and
  its exact 4.8 MB temporary data root were then stopped and removed.
- `bash scripts/check-docs.sh`, full `SHA256SUMS` verification, and
  `git diff --check` pass. No Docker image/archive was rebuilt; the previously
  packaged image therefore does not contain this remediation.

Validation recorded on 2026-08-22 for review remediation:

- `cargo fmt --all --check`, all-target/all-feature Clippy with `-D warnings`,
  `cargo test --workspace --all-features`, and `cargo doc --workspace --no-deps`
  pass. The full Rust run includes same-parent and same-path WebDAV races,
  backup/write/request draining, old-schema replacement, wrong/missing key,
  setup race, job retry/cancel/lease/coalescing, Provider process concurrency,
  RSA OAuth, and two-Vault grant isolation.
- Frontend lint/test/build pass with the Provider privacy-mode and OAuth issuer/
  subject-grant controls. `scripts/check-docs.sh`, `git diff --check`, and
  `make migration-check` pass at schema version 9.
- `make e2e` passes outside the restricted listener sandbox: real MCP
  discovery/Origin rejection, Admin-plane separation, revision precondition,
  and 50 concurrent public WebDAV PUT/read-backs are green. The first local
  attempt exhausted the fixture's 30-second first-link window and the second
  was denied listener creation by the sandbox; neither was a service failure.
- Per operator instruction, no Docker image/archive was rebuilt. External
  Litmus/client GUI, signing, and clean-host release gates remain the same
  explicit WP-14 blockers recorded below.
- `GET /api/v1/setup` exposes only a non-secret `setup_available` projection
  on the Admin listener. The UI uses it to choose exactly one unauthenticated
  flow and fails closed to login on status errors; the Auth repository's
  atomic first-user insert remains the authorization and race boundary.
- Password-only setup deliberately uses Admin-listener reachability as its
  first-claim trust boundary. Source-keyed setup limiting plus the bounded
  Argon2 pool constrain abuse, exact Origin remains mandatory, and obsolete
  token environment variables/HTTP fields are rejected rather than ignored.
- Connection cards use one canonical advertised data origin; Host and Origin
  allow-lists remain validation policy. Without an external origin, direct
  URLs are derived from the configured host plus actual listener port rather
  than assuming HTTP 80.
- Admin reload restoration keeps authentication and CSRF capabilities
  separate. The opaque session bearer remains only in its Secure/HttpOnly/
  SameSite=Strict cookie. Login also issues the session-bound CSRF value in a
  Secure/SameSite=Strict non-HttpOnly cookie so the Admin frontend can
  reconstruct `X-CSRF-Token`; the UI still calls `GET /api/v1/session` before
  treating the browser as authenticated, and the CSRF value alone grants no
  access. This avoids persisting the session bearer or rotating one shared
  CSRF digest on every tab reload.

Validation recorded on 2026-08-23 for reload-safe Admin sessions:

- Auth/Admin API tests prove login emits distinct session and CSRF cookies,
  the session bearer remains HttpOnly, and logout expires both cookies. The
  frontend tests recover the CSRF value after reload, confirm the server-side
  session, attach the restored value only to mutations, clear stale state on
  HTTP 401, and bypass the login/setup UI only after validation.
- A disposable real process and browser on `http://127.0.0.1` completed first
  setup, hard-loaded the Admin URL again into the authenticated dashboard,
  created a WebDAV credential through a protected mutation after reload, then
  logged out and remained on the login page after another hard load. The
  temporary process, credential, and data root were removed afterward.
- `cargo fmt --all --check`, workspace all-target/all-feature Clippy with
  `-D warnings`, and all 185 Rust tests pass. Frontend lint, 11 Vitest
  assertions, TypeScript, and the Vite production build pass. Documentation
  consistency and checksum checks are rerun after this record. No Docker image
  or archive was built.

Validation recorded on 2026-08-22 for self-contained first-run provisioning:

- `cargo fmt --all --check`, workspace all-target/all-feature Clippy with
  `-D warnings`, `cargo test --workspace --all-features`, and
  `cargo doc --workspace --no-deps` pass. Auth/Server tests cover concurrent
  atomic creation, restart reuse, broad Unix permission acceptance,
  explicit-master-key preservation, missing established key failure without
  regeneration, and absence of a setup-token artifact.
- Admin API tests prove missing socket-peer metadata is no longer an
  application network denial while strict Origin, password/session, and CSRF
  protections remain. Frontend lint/test/build pass with corrected setup copy.
- Base and optional Nginx Compose configurations pass `docker compose config
  --quiet`; neither requires pre-created MCP Vault key files. The Nginx
  example remains optional and keeps its own operator-selected source policy.
- `scripts/check-docs.sh`, documentation checksums, and `git diff --check`
  pass. The real HTTP `make e2e` smoke passes outside the restricted listener
  sandbox, including Admin/data-plane separation and 50 concurrent WebDAV
  writes. No Docker image/archive was built.

Validation recorded on 2026-08-22 for Chinese Admin usability:

- Frontend lint, four Vitest assertions, TypeScript, and Vite production build
  pass. Tests cover Chinese login/password-only setup copy,
  Chinese job summaries, and collapsed raw diagnostics.
- Workspace formatting, all-target/all-feature Clippy with `-D warnings`, and
  `cargo test --workspace --all-features` pass after updating the embedded
  index-title assertion. The Server build script recompiles embedded assets
  when the frontend dist directory changes.
- Real browser checks against both Vite and an isolated, authenticated MCP
  Vault process cover login, first-run instructions, grouped navigation,
  dashboard, WebDAV credentials, endpoint copy fields, PAT presets, collapsed
  OAuth/raw diagnostics, and a 390-by-844 responsive viewport. The narrow view
  starts at scroll position zero and retains a visible logout action.
- Documentation checksums, `scripts/check-docs.sh`, and `git diff --check`
  pass. No backend route/schema/auth behavior or Docker image was changed.

Validation recorded on 2026-08-22 for conditional first-Admin setup:

- Auth and Admin API tests prove setup availability starts true, exactly one of
  concurrent first-Admin requests succeeds, and availability then becomes
  false. The atomic insertion check remains authoritative.
- Frontend tests cover initialized-login-only and fresh-setup-only rendering;
  the API client reads status with `GET` and no CSRF mutation token. Frontend
  lint, test, TypeScript, and production build pass without warnings.
- Workspace formatting, all-target/all-feature Clippy with `-D warnings`, and
  `cargo test --workspace --all-features` pass. Documentation and checksum
  validation are rerun after recording this behavior. No Docker image/archive
  was built.

Validation recorded on 2026-08-22 for password-only first-Admin setup:

- Auth/Admin tests prove a username/password-only request creates the first
  Admin, concurrent claims retain one winner, setup password work is
  source-rate-limited, and an obsolete `bootstrap_token` HTTP field is
  rejected. Server tests prove a fresh secrets directory contains only the
  managed `master-key`; both obsolete token environment variables are rejected.
- Frontend lint, six Vitest assertions, TypeScript, and production build pass.
  The setup form and API client contain no token input or payload field.
- A real isolated process on loopback ports returned setup available, accepted
  only the test username/password, then returned setup unavailable; a second
  claim returned HTTP 409. The temporary secrets directory contained only
  `master-key`, and the fixture process/data were removed afterward.
- Workspace formatting, all-target/all-feature Clippy with `-D warnings`, and
  `cargo test --workspace --all-features` pass. Documentation checksums and
  consistency checks are recorded after this plan update. No Docker image was
  built.

Validation recorded on 2026-08-22 for password-policy guidance:

- The first-Admin and WebDAV forms now show the default policy beside the
  password input with `aria-describedby`: pure English needs at least 12
  characters, Chinese recommends a longer phrase, composition classes are not
  mandatory, and rejected placeholders are named.
- The `password_policy` translation repeats those requirements instead of the
  former generic “longer and uncommon” message. Frontend lint, eight Vitest
  assertions, TypeScript, and production build pass.
- Admin/security/product documentation states the exact 12-UTF-8-byte default,
  placeholder list, and visible-guidance requirement. Documentation checksum
  and consistency checks are rerun after this update.

Validation recorded on 2026-08-22 for relative default-data setup:

- The Server composition resolves `AppConfig.data_dir` against the process
  working directory before passing it to Admin. Unit coverage proves
  `./data/vaults/default` becomes an absolute, valid `VaultContext` root.
- A real process launched with
  `MCP_VAULT_DATA_DIR=./data/codex-relative-setup-smoke` reported setup
  available and completed password-only initialization. Its response exposed
  the expected absolute project-local Vault root; the old
  `vault_setup_failed` path did not recur.
- Startup removes only the former service-owned
  `<secrets-dir>/bootstrap-token` path and preserves `master-key`; a focused
  test covers both outcomes. The isolated relative-path process and its exact
  test directory were stopped and removed after validation.

Validation recorded on 2026-08-22 for advertised data endpoints:

- Admin connection info now prefers the typed
  `MCP_VAULT_DATA_PUBLIC_ORIGIN`, falls back to a configured data Origin for
  compatibility, and otherwise combines the first allowed direct host with the
  actual `MCP_VAULT_DATA_BIND` port.
- Admin API tests assert default direct WebDAV/MCP URLs include `:8080`, an
  explicit HTTPS public origin wins over policy origins, `:8443` is retained,
  and a direct IPv6 URL is bracketed with its port. The authenticated
  connection-info round trip verifies both complete Vault-scoped paths.
- The optional Nginx Compose example derives the public-origin setting from
  its existing public hostname, so port 443 remains implicit under HTTPS.
  Targeted Admin/Server tests, formatting, Clippy, Compose parsing, docs, and
  checksums are rerun after this update.

Validation recorded on 2026-08-21:

- `cargo fmt --all --check`, Clippy, `cargo test --workspace --all-features`,
  `cargo doc --workspace --no-deps`, frontend lint/test/build,
  `scripts/check-docs.sh`, checksum verification, and `git diff --check` pass.
- `scripts/interop/http-smoke.sh` passes against the real disposable data and
  control listeners, including 50 concurrent nested WebDAV PUTs and a GET
  verification of every uploaded object.
- `cargo test -p mcp-vault-state -p mcp-vault-core -p mcp-vault-webdav`
  passes. The WebDAV package includes a 32-request concurrent PUT regression
  against file-backed multi-connection SQLite; all requests return 201/204,
  every path is readable, and the incomplete-journal set is empty. Existing
  deterministic recovery tests still finalize a canonical write interrupted
  at metadata transaction/outbox phases.
- Official MCP core scenarios pass for target `2026-07-28` with five narrow
  baseline checks; tools/resources/header/DNS/caching wire checks are enforced.
  Core tools/resources/DNS compatibility also passed for `2025-11-25`;
  target-only scenarios are not silently counted for that older revision.
- `scripts/release/check-migrations.sh` runs both selected migration tests and
  reports one passing test for each, not an empty filtered suite.
- `scripts/perf/baseline.sh` passed with the disposable fixture
  (`p95=0.000660s` on the recorded macOS host; threshold `0.5s`).
- `docker build --tag mcp-vault:wp14 .` passed. The image ID is
  `sha256:d474a0489398e9c9ac9e919ef7ad2848f175013b682c43a95cf598bc6eec03ca`,
  runtime user is `mcpvault`, stop signal is `SIGTERM`, and read-only-root
  non-root smoke/config validation passed.
- `docker buildx build --platform linux/amd64 --tag mcp-vault:0.1.0 --load .`
  passed. The image ID is
  `sha256:3f642f845fe79356a31c222ddf25056ec2e1d47e29844319f46e01fd61a12bd4`,
  the runtime reports `linux/amd64`, UID `999`, and the image size is
  98,681,445 bytes. Its Docker-load archive is 101,782,528 bytes with SHA-256
  `a72d872099ad3fcaaa78c362d933c33325cfeac7be568c2069921e7300e3110d`.
- The latest worktree image build on 2026-08-24 used
  `docker buildx build --platform linux/amd64 --tag mcp-vault:latest
  --tag mcp-vault:0.1.0 --load .` and passed. Both tags point to image ID
  `sha256:6eb293d9f4a1bc7d45bf7c114be5ed6eb94102480ccee9bc75efdbcd93d58b58`;
  Docker reports `linux/amd64`, runtime user `mcpvault`, UID `999`, stop signal
  `SIGTERM`, and image size `99,408,741` bytes. The host is arm64, so Docker
  emitted the expected cross-platform warning when the non-root smoke ran
  without an explicit `--platform`; the check still returned UID `999`.
- The latest post-lifecycle-fix image build on 2026-08-24 used the same
  `linux/amd64` build with `mcp-vault:latest` and `mcp-vault:0.1.0` tags. Both
  tags point to image ID
  `sha256:594fabaa6f0973c08e3be3eeb4527525b0dbde6aa7d71bf1d45728888651f177`;
  Docker reports `linux/amd64`, runtime user `mcpvault`, UID `999`, stop signal
  `SIGTERM`, and image size `99,452,229` bytes. An explicit-platform,
  read-only-root smoke returned UID `999`.
- The standalone Nginx deployment archive is 5,326 bytes with SHA-256
  `24872660ee2a79d41c1cb1a7fbc36a2d63d751a21f7c9a8447ac50b2022b8c24`.
- The Nginx HTTPS Compose bundle passed `docker compose config --quiet`,
  `nginx -t`, and a real two-container HTTPS smoke. Public liveness/readiness
  returned 200, the public `/api/v1/system` returned 404, the separate Admin
  listener returned 200 for an allowed source CIDR and 403 for an excluded
  source CIDR, and the data plane remained ready during the Admin rejection.
- Local `litmus`, `syft`, `trivy`, and `cosign` are not installed. The release
  script exits blocked for missing SBOM/scan/signature tooling; CI/workflow
  hooks are present but no signed artifact is claimed.
- Hēsperus Sync Engine, Remotely Save, Obsidian Desktop/Mobile GUI runs and a
  full-scale performance/clean-host restore report remain unverified in this
  environment.
- Provider lifecycle validation on 2026-08-24:
  - the Provider integration test rotates a secret, proves the old ciphertext
    is gone, creates global and second-Vault bindings plus embeddings in two
    Vaults, rejects a stale delete, removes only the target Provider's
    dependent state, preserves an unrelated Provider, rejects a stale
    post-delete update without creating an orphan secret, and finishes with no
    foreign-key violations;
  - the authenticated Admin integration test rejects stale PATCH/DELETE,
    preserves the secret when PATCH sends `null`, returns deletion counts,
    removes visible models/bindings, and exposes the redacted deletion audit;
  - `cargo fmt --all --check`, full-workspace Clippy with `-D warnings`, and
    `cargo doc --workspace --no-deps` pass;
  - `cargo test -p mcp-vault-server --all-features` passes all 30 Server tests,
    including the path-hash redaction assertion for task progress logs;
  - `cargo test --workspace --all-features` passes all 202 Rust tests. The
    first full run observed the existing asynchronous staged-PUT cleanup test
    before its background rollback completed; that exact test passed in
    isolation and the complete workspace rerun passed;
  - The first full run after adding task logs also observed the existing
    WebDAV concurrent-PUT test with one transient 500/incomplete journal;
    the isolated test and the complete workspace rerun both passed.
  - Admin lint/build and 17 Vitest tests pass, including edit payload/settings,
    secret-preservation messaging, deletion impact confirmation, revision, and
    result-count assertions;
  - documentation/checksum validation and `git diff --check` pass.

Validation recorded on 2026-08-24 for ordinary-note recall and extraction v2:

- The Indexer local-fake embedding test rebuilds two canonical notes, derives
  deterministic bounded chunks, embeds them through the shared Provider
  transport, recalls a semantically phrased quarantine/recovery query, proves
  second-Vault isolation, excludes stale hashes, and schedules exactly the
  changed chunk after a note revision. Note semantic status reports separate
  current/expected/stale vector counts without calling the provider.
- Public MCP tests prove that `recall` returns an ordinary indexed note in
  `related_notes` without any promoted memory, including a revision-bound
  resource URI. A credential with `memory:read` but no `vault:read` receives no
  note cue. Existing memory and discovery ordering tests remain green.
- Extraction contract tests assert prompt/schema pipeline v2, a default
  `maxItems: 3`, typed durability scope, rejection of an ordinary mismatched
  technical procedure, review thresholds, same-pending-content deduplication,
  and clearing only unpromoted candidates while retaining a promoted record.
  The authenticated Admin integration cancels the prior extraction run,
  clears the review projection, queues a fresh pipeline-v2 backfill, and keeps
  both job rows visible.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, all 208 Rust tests, and `cargo doc --workspace --no-deps`
  pass. The run includes the existing Vault/Core/WebDAV/auth/recovery/provider
  suites in addition to the new note-retrieval and reset coverage.
- Admin lint, all 19 Vitest tests, TypeScript checking, and the Vite production
  build pass. The UI distinguishes lexical note coverage from optional
  `embedding_note` coverage, explains ordinary-note versus durable-memory
  behavior, exposes review limits, and confirms the destructive scope of a
  clean rerun.
- `bash scripts/check-docs.sh`, complete `SHA256SUMS` verification, and
  `git diff --check` pass with ADR-0013 and the updated public contracts.
- No paid provider was called and no Docker image was built for this change.
  A new deployment image remains a separate explicit packaging action.

Validation recorded on 2026-08-25 for extraction source intent and durable
memory lifecycle:

- Extraction pipeline v3 defaults every absent/legacy policy to
  `explicit_only`. The Memory integration test proves that an unmarked
  ordinary Markdown note returns before Provider/model resolution, while an
  explicit `all_notes` policy reaches the expected missing-model boundary and
  a boolean-frontmatter-marked note still completes the bounded fake-provider
  extraction contract.
- The authenticated Admin integration creates a canonical memory through
  `MemoryService`, archives it, rejects a stale restore revision, restores it,
  permanently deletes it through `DELETE /memories/{id}`, proves both the
  current managed Markdown and projection are gone, and finds the redacted
  `admin.memory.deleted` audit record. Archive now carries the caller's
  revision through Vault Core materialization rather than refetching and
  accepting a later concurrent revision.
- The Chinese Memory UI exposes archive, restore, and confirmed permanent
  delete actions with revision-bearing requests, disables duplicate lifecycle
  clicks, labels their audit records, and explains that current Markdown and
  projection are deleted while retained history/backups follow their own
  policy. Vitest verifies all three request contracts and destructive copy.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, all 210 Rust tests, and `cargo doc --workspace --no-deps`
  pass.
- Admin lint, all 20 Vitest tests, TypeScript checking, and the Vite production
  build pass. Documentation checks, every `SHA256SUMS` entry, and
  `git diff --check` pass.
- No paid Provider was called and no Docker image was built for this follow-up.

Validation recorded on 2026-08-25 for coherent autonomous memory v4:

- The official Codex memories documentation confirms background generation
  into local files with source controls and supporting evidence rather than a
  per-memory approval workflow. The OpenAI long-term-memory cookbook emphasizes
  non-invention, source-grounded distillation, deduplication, conflict
  resolution, forgetting, and evals; it does not establish model self-scores as
  trust evidence. ADR-0014 records the project decision derived from this
  evidence and the observed high-score false positives.
- The Provider schema no longer contains `content`, `confidence`, or
  `importance`. It returns exact `evidence_quote`, current source line range,
  typed category/scope, and bounded metadata. Local validation rejects an
  invented/out-of-range quote and materializes the exact source statement,
  never a generated paraphrase.
- The marked-note integration test proves one Provider result automatically
  creates canonical managed Markdown, terminally marks its derived candidate
  row `promoted`, leaves zero pending rows, and remains idempotent on a repeat.
  An unmarked note resolves no Provider; legacy `all_notes` deserializes to
  `explicit_only`.
- Job progress now distinguishes `notes_evaluated`,
  `source_policy_skipped`, `memories_promoted`, `proposals_rejected`, and
  format/read `skipped`. Admin run admission returns an existing active job
  instead of creating another full-Vault scan; the State test proves the lookup
  is type- and Vault-scoped and ignores terminal rows.
- The Memory page applies archive/restore/delete and proposal reset results to
  its local cards before the four-endpoint refresh. One frontend test performs
  archive, restore, and two sequential permanent deletes without a parent
  rerender. Reset immediately removes the exceptional row, shows the returned
  durable job beside the action, and can intentionally cancel/restart an active
  job. Ordinary manual backfill is disabled while a job is active and repeated
  admission visibly reuses that job.
- The Chinese UI exposes “自动生成长期记忆”, one collapsed maintenance action,
  hard bounds only, and exceptional “需要处理的问题”. It contains no routine
  candidate-generation action, score-threshold controls, or per-result review
  requirement.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, all 211 Rust tests, and `cargo doc --workspace --no-deps`
  pass. Admin lint, all 21 Vitest tests, TypeScript checking, and the Vite
  production build pass. Documentation checks, every `SHA256SUMS` entry, and
  `git diff --check` pass.
- No paid Provider was called and no Docker image was built for this follow-up.

Validation recorded on 2026-08-25 for marker-free automatic memory v5:

- ADR-0015 supersedes the per-note source marker while retaining ADR-0014's
  exact-evidence, no-self-score, and autonomous-promotion controls. The one
  Vault-level `automatic` mode requires no frontmatter, tag, path, or folder
  convention; legacy `explicit_only` and `all_notes` values deserialize as
  migration aliases.
- The ordinary-note integration fixture reaches Provider/model resolution
  without modifying the note and proves one exact-source decision becomes
  canonical managed Markdown with zero pending review rows. A repeat remains
  idempotent. Unit fixtures reject invented evidence, generic procedures, and
  a component requirement even when the Provider labels it a commitment.
- The Provider prompt/schema and extraction fingerprint are version 5. Local
  automatic materialization admits only identity, preference, relationship,
  accepted decision, current progress, and significant event. General facts,
  project/software descriptions, requirements, procedures, and inferred
  constraints remain ordinary `related_notes` knowledge.
- Admin sends fixed `source_mode: "automatic"`, tells the owner to write notes
  normally, and offers incremental processing plus an explicit full
  re-evaluation option instead of marker or candidate-generation controls.
  API, security, operations, data model, testing, traceability, and
  memory-system documents describe the same contract.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, all 211 Rust tests, and `cargo doc --workspace --no-deps`
  pass. Existing local frontend dependencies pass ESLint, all 21 Vitest tests,
  TypeScript checking, and the Vite production build.
- The canonical `pnpm --dir frontend/admin lint` wrapper did not reach the
  project script: Corepack's cached pnpm first attempted a registry metadata
  fetch and then aborted its own non-interactive dependency-directory reinstall.
  Running the exact underlying project binaries from `frontend/admin` avoided
  changing dependencies and all four frontend checks passed.
- `bash scripts/check-docs.sh`, every `SHA256SUMS` entry, and
  `git diff --check` pass. No paid Provider was called and no Docker image was
  built for this change.

Validation recorded on 2026-08-25 for per-note output isolation and extraction
pipeline v6:

- The supplied container log proves notes 1–9 completed, note 10 spent 30,014
  ms in the Provider call, and the adapter then emitted
  `provider_schema_invalid`; the worker immediately returned a permanent
  failure at 9/178. Because the old unit variant retained no schema category or
  path, the exact mismatching field cannot be reconstructed from that historical
  log.
- Provider schema validation now distinguishes type, enum, missing required
  property, unexpected property, and array-bound failures. It persists/logs
  only the stable category and a path built from trusted schema keys and array
  indexes; response values and arbitrary unexpected keys remain absent.
- Extraction v6 removes decorative generated metadata and keeps four required
  candidate fields. The official OpenAI Structured Outputs guide confirms that
  every declared object field must be required and optionality must use a null
  union, so merely making omitted fields optional would break strict-schema
  compatibility. The MiMo-compatible JSON-object path no longer has to invent
  empty entity/tag/heading arrays.
- A transport-backed Worker test gives `a.md` a missing `source_anchor`, then
  returns a valid empty result for `b.md`. Both Provider calls occur; the job
  finishes `completed_with_errors` at 2/2 with one bounded failure containing
  `required_property_missing` at `$.memories[0].source_anchor`. A separate
  test proves one or two consecutive failures do not open the circuit and the
  third does. State coverage proves explicit retry preserves a failed
  `memory.extract` cursor.
- Admin renders a terminal mixed job as “完成但有失败”, shows the failed-note
  count, latest source path, stable Chinese reason, and schema category/path.
  Frontend assertions cover that projection and the three-failure circuit
  message.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, all 214 Rust tests, and `cargo doc --workspace --no-deps`
  pass. Existing local frontend dependencies pass ESLint, all 21 Vitest tests,
  TypeScript checking, and the Vite production build.
- `bash scripts/check-docs.sh`, every `SHA256SUMS` entry, and
  `git diff --check` pass. No paid Provider was called and no Docker image was
  built for this change.

Validation recorded on 2026-08-25 for incremental automatic-memory extraction:

- Migration 0010 adds one Vault/file-scoped successful-evaluation row containing
  source revision, effective extraction-profile hash, redacted configuration
  identities/revisions, bounded outcome counters, and timestamp. It contains no
  note body, prompt, Provider response, credential, or canonical knowledge.
- A transport-backed Memory test proves the first extraction calls the fake
  Provider once, an unchanged default repeat calls it zero additional times, a
  same-revision legacy terminal candidate seeds coverage without a call, and an
  explicit full re-evaluation calls again. The forced failure invalidates old
  coverage, so the next default run retries rather than skipping. Policy and
  source-revision changes also call again, and another Vault cannot read the
  evaluation row.
- A full-Vault Worker test proves a second incremental job retries the prior
  failed note but skips the unchanged successful note, while a third
  `include_evaluated: true` job calls the fake Provider for both. Persistent
  progress reports the skip separately as `already_evaluated_skipped`.
- Admin accepts an optional `include_evaluated` request field that defaults to
  false; restart/reset always uses true. The Chinese UI labels the ordinary
  action “处理新增或有变化的笔记”, keeps the full re-evaluation option off by
  default, and warns about nondeterministic output and additional Token cost.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, all 216 Rust tests, and `cargo doc --workspace --no-deps`
  pass. Existing local frontend dependencies pass ESLint, all 22 Vitest tests,
  TypeScript checking, and the Vite production build.
- `bash scripts/release/check-migrations.sh`, `bash scripts/check-docs.sh`, every
  `SHA256SUMS` entry, and `git diff --check` pass. No paid Provider was called
  and no Docker image was built for this change.

Validation recorded on 2026-08-26 for truthful Phase 1 failures and the MiMo
missing-summary contract:

- A transport-backed full-Vault Worker test processes one invalid UTF-8 note,
  one Provider-schema failure, and one successful note. It proves the source
  failure makes no Provider call, both categories retain independent bounded
  path/code diagnostics, later notes continue, and the mixed task completes.
- The successful fake MiMo-style response deliberately omits
  `source_summary`; the shared adapter copies the returned `raw_memory`, reruns
  full validation, and Stage 1 persists it. Provider unit coverage proves an
  empty object is not repaired. Memory unit coverage proves an out-of-range
  evidence selection remains a typed generated-output failure.
- The Chinese Jobs/Memory projection distinguishes “源文件无法处理（模型未调用）”
  from “模型输出校验失败（模型已调用）”, shows the latest stable reason for
  each, and contains no “格式或读取跳过” fallback.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, and `cargo test --workspace --all-features` pass. Existing
  local frontend dependencies pass ESLint, all 23 Vitest tests, TypeScript
  checking, and the Vite production build.
- `bash scripts/check-docs.sh`, every `SHA256SUMS` entry, and
  `git diff --check` pass. No paid Provider was called and no Docker image was
  built.

Validation recorded on 2026-08-26 for immediate first-run admission and
server-derived evidence:

- Two authenticated Admin tests prove that enabling the ready policy and
  saving the final consolidation binding each immediately create the required
  fresh singleton, clear `regeneration_pending`, and do not wait for periodic
  reconciliation. A frontend test proves a ready pending state retains an
  enabled “立即开始全量生成” action when no job exists.
- The Phase 1 unit/integration contract sends `L1:`-numbered Markdown, exposes
  no generated `quote` property, accepts bounded line ranges, derives the
  excerpt hash from authoritative source, and rejects an out-of-range anchor.
  Full-Vault continuation and two-phase consolidation tests pass under pipeline
  9/generation 2.
- `cargo fmt --all --check`, full-workspace all-target/all-feature Clippy with
  `-D warnings`, and `cargo test --workspace --all-features` pass. Existing
  local frontend dependencies pass ESLint, all 24 Vitest tests, TypeScript
  checking, and the Vite production build.
- `bash scripts/check-docs.sh`, every `SHA256SUMS` entry, and
  `git diff --check` pass. No paid Provider was called and no Docker image was
  built.

## Rollback and recovery

Migration 0009 is forward-only. Before upgrade, retain a verified state/Vault
backup and the separate master-key file. The migration adds the non-secret key
verifier and deliberately disables/clears prerelease OAuth JWKS caches; restore
service requires re-saving public RSA keys through Admin. Rolling back to a
binary that expects schema 8 requires restoring the pre-upgrade backup rather
than manually dropping the verifier table. Remove only fixture-owned temporary
directories after validating their paths; never reset unrelated worktree
changes or target canonical Vault roots/backups during cleanup.

For a pre-self-provisioning deployment, retaining the existing explicit
master-key mount/environment is a valid rollback and upgrade path. To adopt the
managed location, stop the service and copy the exact existing key bytes before
removing the override. Never delete an established key expecting startup to
regenerate it. Move any old Admin CIDR rule to the deployment layer before
removing the now-rejected environment variable.

No migration is added for Provider lifecycle operations. Rolling back the code
restores the older API/UI behavior without changing the schema. A completed
Provider deletion is intentionally irreversible for configuration, encrypted
Provider secrets, model inventory, bindings, and derived vectors; recreate the
service, rediscover/register its models, and rebind roles. Canonical Vault
files, durable memories/candidates, job history, and audit facts remain
available throughout that recovery.

No schema migration is required for extraction source admission or memory
lifecycle controls. Current code serializes `automatic`; existing
`explicit_only` and `all_notes` values deserialize as aliases for that mode.
Rolling back to a marker-gated build would silently skip ordinary notes, so an
operator should either disable extraction or understand that changed behavior
before rollback. Permanent memory deletion is intentionally not reversible
through Admin; recover retained content only through the documented
revision-history or backup workflow.

Automatic-memory v4 also requires no schema migration. Prerelease score fields
in policy JSON are ignored, and legacy `all_notes` is accepted as an alias for
`explicit_only`. Existing promoted canonical memories remain unchanged;
pending legacy candidates can be discarded through the one reset/reprocess
action. Rolling back to v3 would restore model self-score/manual-review
behavior, so disable extraction before such a rollback if that behavior is not
acceptable.

Automatic-memory v5 also requires no schema migration. It changes the source
admission meaning of legacy policy values to Vault-level automatic processing
and bumps the prompt/fingerprint pipeline so test output can be regenerated
cleanly. Existing canonical memories remain unchanged. Rolling back to v4
restores the author-facing marker requirement, so disable extraction before
rollback if skipping unmarked changes would be surprising.

Automatic-memory v6 requires no SQL migration. Job progress gains additive
JSON fields and old jobs remain readable. Rolling back to v5 removes detailed
schema diagnostics, per-note continuation, and paid-cursor retention; a new or
retried full-Vault run can therefore stop on one malformed output and restart
from the beginning. Existing canonical memories and source notes are unchanged.

Migration 0010 is forward-only operational state. Before upgrading, retain a
verified schema-9 state backup if rollback to a pre-0010 binary may be needed;
restore that backup rather than manually dropping the table. Losing only the
evaluation rows does not lose canonical notes or memories, but it removes the
pre-Provider skip record and can cause unchanged notes to consume model calls
again. A forced evaluation marks coverage stale before the call, so its failure
is safely recoverable through a later default incremental task.

Migration 0011 is an intentionally destructive prerelease memory cutover.
Rollback requires restoring a matching pre-0011 state backup and binary; do not
manually reinsert old memory jobs. Ordinary Vault notes, revisions, Provider
configuration, audit, backups, and non-memory jobs are preserved, while current
memory output is regenerated from source notes.

No migration is required for the Phase 1 diagnostic split. Job progress is
operational JSON; prerelease jobs written with the old ambiguous `skipped`
field are not reclassified because the Provider-call boundary cannot be
reconstructed truthfully. Restarting extraction produces categorized progress.
The `source_summary` fallback affects only newly generated Phase 1 results and
does not rewrite existing Stage 1 or final-memory rows.

Pipeline generation 2 intentionally discards generation-1 prerelease memory
jobs, Stage 1 rows, and final projections on the next restart, then regenerates
from canonical Vault notes using server-derived evidence ranges. Rolling back
to generation 1 requires restoring a matching state backup; merely running the
older binary would cause its own generation check/reset behavior.

## Outcomes

Implementation and automated evidence are complete, but this plan remains
active because the release gates requiring external Litmus/client binaries,
full-scale/clean-host operational evidence, SBOM/scan/signature tooling, and
final capability-set review have not been performed. Move it to
`docs/exec-plans/completed/` only after those gates have concrete evidence.

The 2026-08-22 deployment follow-up ships application-owned installation-key
provisioning, password-only atomic first-Admin claim, deployment-owned Admin
source admission, and permission-bit-neutral key loading without weakening
post-setup Admin authentication or installation-key identity.

The Admin follow-up ships a Simplified Chinese, task-grouped console with
page-specific summaries, guided WebDAV/PAT choices, one-time-secret copy UX,
and progressive disclosure for OAuth, restore, and raw diagnostics. All prior
application services remain behind the same authenticated Admin API boundary.
The unauthenticated shell additionally chooses first-Admin creation or login
from server state, never presents registration after initialization, and still
relies on the atomic Auth transaction rather than browser state for safety. It
does not request or transmit a bootstrap token.
Password-creation forms also expose the real default policy before submission
and return the same concrete guidance on rejection.

The 2026-08-24 deployment-feedback follow-up ships truthful Admin progress,
complete model inventory/registration/role selection, and an opt-in,
event-driven memory extraction control surface with durable backfill progress.
It also repairs reconciled file-event semantics so Obsidian/direct filesystem
changes reach the extraction pipeline without discarding `external_change`
revision provenance or stranding legacy outbox rows.

The extraction cost-safety follow-up classifies response-body timeout and
interruption separately, raises the memory request deadline to five minutes,
and makes all post-success body failures explicit non-retryable outcomes. Jobs
now expose and auto-refresh their current Markdown path, ordinal, elapsed model
time, counters, and previous-attempt diagnostic; full-Vault retries resume from
the last completed path instead of starting over.

The provider follow-up adds first-class DeepSeek, MiMo, GLM, Kimi, Gemini, and
Qwen service types without creating parallel security boundaries. Model
configuration composes provider preset, structured-output dialect, token field,
thinking control, and bounded generation limit. Official API roots are appended
correctly, legacy generic rows recognize only exact official hosts, and all
provider output remains locally validated before memory lifecycle policy.

The Provider lifecycle follow-up adds Chinese edit/delete controls backed by
revision-aware application operations. Edits preserve an omitted secret and
clean superseded ciphertext on rotation. Deletion now succeeds for configured
services by atomically removing dependent bindings, models, derived vectors,
health/configuration, and owned encrypted secrets while preserving every form
of canonical knowledge and durable operational/audit history that should
survive the configuration change.

The task-observability follow-up adds structured `mcp_vault::jobs` lifecycle and
per-note progress/error events. Operators can follow live work with the normal
container log command while the Admin job snapshot remains authoritative after
restart. Logs contain only stable identifiers, counters, durations, error
codes, and path hashes; no note or Provider data is emitted.

The ordinary-note recall follow-up removes the false choice between an
invisible article and dozens of review candidates. Current Markdown now feeds
automatic lexical and optional `embedding_note` semantic cues, and `recall`
returns those cues separately from canonical durable memories. Extraction v6
requires no note marker: once enabled for the Vault, eligible ordinary
Markdown changes are evaluated automatically. Exact source evidence is
mandatory, model self-scores are absent, and a local allow-list materializes
only identity, preference, relationship, accepted decision, current progress,
and significant event. Normal operation has no human review queue; pending
rows are exceptional legacy/interrupted diagnostics. Admin can discard those
rows and start a clean, single-admission backfill without deleting source
notes, promoted memory Markdown, jobs, or audit. Its cards update immediately
for archive, restore, and audited permanent deletion through the existing
MemoryService/Vault Core boundary. One malformed generated result is now a
checkpointed note-local failure rather than the end of the batch; later notes
continue, Admin exposes a bounded redacted structural cause, and repeated
consecutive failures open a cost-safety circuit whose explicit retry preserves
the paid-work cursor.

The incremental-extraction follow-up stops treating post-Provider candidate
fingerprints as call idempotency. Successful note/profile evaluations, including
zero-result work, are now durable Vault-scoped operational coverage. Automatic
events and ordinary manual backfills skip unchanged current coverage before the
Provider call; operators can explicitly include already processed notes when a
deliberate full re-evaluation is worth its nondeterministic output and Token
cost. Failed or interrupted work remains eligible for the next incremental run.

The Phase 1 diagnostics follow-up replaces “格式或读取跳过” with two truthful
outcomes: source files that could not be ingested before any model call, and
generated results that failed schema/bounds/evidence validation after a model
call. Both retain bounded per-note diagnostics and continue later notes; only
generated failures enter the three-call cost circuit. The MiMo-compatible
request now provides an exact Stage 1 object template and safely tolerates a
lone missing auxiliary `source_summary` by copying returned `raw_memory`
before full revalidation.

The first-run admission/evidence follow-up removes two deployment-visible dead
ends. A ready policy or final model binding immediately admits the required
fresh task, while manual admission remains available during a ready pending
state. Phase 1 no longer asks a model to echo exact source text: it sends stable
line labels, accepts bounded ranges, and derives provenance from the canonical
note revision. Generation 2 cleanly restarts the prerelease memory corpus under
that contract.

The 2026-08-27 Codex-alignment correction supersedes that automatic-note line-
range contract. Live MiMo rejected otherwise valid notes on evidence range
bounds, while upstream Codex Phase 1 uses only `raw_memory`,
`rollout_summary`, and `rollout_slug`. Extraction pipeline 10 now uses that
exact wire shape and derives whole-note file/path/revision/hash provenance
locally. Phase 2 also stops treating projection-only revision churn as semantic
snapshot change: rebuild admission is canonical-record-only, identical
canonical revisions are no-ops, unrelated active extraction delays
consolidation without consuming attempts, and old prompt-version proposals are
rejected before reuse.
