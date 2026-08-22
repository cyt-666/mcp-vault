# WP-14 — Conformance, interoperability, and release readiness

Status: In progress
Owner: Codex
Created: 2026-08-21
Last updated: 2026-08-22

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
- ADRs `0001`–`0011`, especially the Vault boundary, protocol/auth separation, MCP statelessness, safe writes, outbox/recovery, provider isolation, and canonical memory-file decisions.

## Current repository state

- Rust workspace crates include `server`, `mcp`, `webdav`, `admin-api`, `vault-core`, `storage-fs`, `state`, `auth`, `indexer`, `memory`, `providers`, `backup`, and `domain`.
- `crates/mcp` uses RMCP 3.x and advertises MCP `2026-07-28`; its current tests are primarily in-process router/service tests covering headers, authorization, deterministic discovery, tools, resources, and Vault isolation.
- `crates/webdav` has broad in-process DAV adapter tests for authenticated reads, writes, collections, copy/move/delete, locks, ranges, preconditions, credentials, and Vault isolation. WP-14 now adds the external Litmus entry point and a real-process compatibility fixture.
- `crates/server` composes separate data/control routers and listener configuration. The new disposable fixture binary prepares a temporary SQLite/Vault root, runs the real startup reconciliation path, binds both listeners, and publishes a 0600 manifest for scripts.
- WP-13 already provides backup/restore, maintenance gates, readiness/metrics/diagnostics, container non-root/read-only-root hardening, SBOM and image-scan CI hooks, and a recovery endpoint. WP-14 exercises these through the public deployment boundaries rather than bypassing them.
- `.github/workflows/ci.yml` now runs Rust, frontend, documentation, container, public HTTP smoke, pinned official MCP core scenarios, migration, and bounded performance checks. Litmus is an explicit opt-in job because the external binary/target credentials are not available in the ordinary repository runner.
- The repository now contains conformance/Litmus wrappers, a WebDAV/client compatibility matrix, a performance baseline policy/report script, requirements traceability, a release checklist, and a manual release workflow.

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

The deployment follow-up on 2026-08-22 removes manual secret generation and
reverse-proxy coupling from the application bootstrap path:

1. When no explicit master-key file is configured, MCP Vault owns
   `<data-dir>/secrets/master-key`. It atomically creates a random persistent
   key when the path and installation-key verifier are both absent, then reuses
   that file on restart. A persisted verifier or key-dependent record with a
   missing/different key still fails closed; startup never replaces a lost
   installation key.
2. When neither bootstrap-token environment input nor an explicit token file
   is configured, MCP Vault owns `<data-dir>/secrets/bootstrap-token`. It
   atomically creates a high-entropy token only while no Admin exists, exposes
   it through an explicit local CLI command, and removes the managed file after
   the first Admin is committed. The token is never emitted to ordinary logs or
   an unauthenticated HTTP response.
3. Explicit `MCP_VAULT_MASTER_KEY_FILE`,
   `MCP_VAULT_BOOTSTRAP_TOKEN_FILE`, and `MCP_VAULT_BOOTSTRAP_TOKEN` inputs
   remain operator-managed compatibility overrides. MCP Vault does not create
   or delete explicit files.
4. MCP Vault does not inspect, reject, or mutate filesystem permission bits on
   key/token files. Ownership, mode, ACL, volume protection, TLS, reverse
   proxying, firewall rules, and Admin source-network publication belong to the
   deployment environment.
5. The application keeps the separate control listener plus Admin username/
   password, session, CSRF, Origin, expiry, revocation, and login limiting. It
   removes application CIDR filtering; the safe default listener remains
   loopback and operators explicitly choose any LAN/VPN/public bind or proxy
   policy.

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
14. **Self-contained first-run provisioning.** Add managed secret paths and
    atomic create/reuse behavior in `auth`/`server`, a local bootstrap-token
    display command, managed-token cleanup after first-Admin commit, and remove
    application Admin CIDR enforcement. Update Compose/operator docs without
    requiring a particular reverse proxy. Validate fresh install, restart,
    concurrent creation, setup cleanup, explicit-file preservation, lost-key
    fail-closed behavior, and authenticated Admin route protection.

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
- [x] 2026-08-22 — Implement self-contained first-run master-key/bootstrap-token provisioning, remove permission-bit and Admin CIDR enforcement, retain application Admin authentication, and update deployment-independent tests/docs.
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

## Surprises and discoveries

- The initial repository had strong in-process MCP and WebDAV coverage but no real-process protocol harness; WP-14 added the fixture so route-level tests are no longer the only evidence.
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
- Existing explicit master-key/bootstrap-token environment and file inputs
  must remain operator-owned for upgrades. Only absent overrides select the
  application-managed paths, and a missing explicit file is never created or
  replaced by the automatic provisioning path.

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

Validation recorded on 2026-08-22 for self-contained first-run provisioning:

- `cargo fmt --all --check`, workspace all-target/all-feature Clippy with
  `-D warnings`, `cargo test --workspace --all-features`, and
  `cargo doc --workspace --no-deps` pass. Auth/Server tests cover concurrent
  atomic creation, restart reuse, broad Unix permission acceptance, managed
  first-Admin token cleanup, explicit-file preservation, missing established
  key failure without regeneration, and the local display boundary.
- Admin API tests prove missing socket-peer metadata is no longer an
  application network denial while strict Origin, password/session, and CSRF
  protections remain. Frontend lint/test/build pass with corrected setup copy.
- Base and optional Nginx Compose configurations pass `docker compose config
  --quiet`; neither requires pre-created MCP Vault key/token files. The Nginx
  example remains optional and keeps its own operator-selected source policy.
- `scripts/check-docs.sh`, documentation checksums, and `git diff --check`
  pass. The real HTTP `make e2e` smoke passes outside the restricted listener
  sandbox, including Admin/data-plane separation and 50 concurrent WebDAV
  writes. No Docker image/archive was built.

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

## Outcomes

Implementation and automated evidence are complete, but this plan remains
active because the release gates requiring external Litmus/client binaries,
full-scale/clean-host operational evidence, SBOM/scan/signature tooling, and
final capability-set review have not been performed. Move it to
`docs/exec-plans/completed/` only after those gates have concrete evidence.

The 2026-08-22 deployment follow-up additionally shipped application-owned
first-run key/token provisioning, local token retrieval, managed-token cleanup,
deployment-owned Admin source admission, and permission-bit-neutral secret-file
loading without weakening Admin authentication or installation-key identity.
