# Admin-owned Provider network access

- Owner: Codex Cloud
- Created / updated: 2026-10-09
- Status: implemented and default-feature checks passed; all-features gate blocked;
  paid M6 execution remains paused

## Purpose and user-visible result

The installation administrator chooses each Provider endpoint. HTTP clients use
normal direct or environment-proxy routing without application DNS pre-resolution,
destination-IP admission, or socket pinning. Provider calls have one Vault-scoped
enabled/disabled switch. LocalOnly and the private-network UI switch are removed.

## Governing requirements

The user approved this scope and explicitly approved removing LocalOnly on
2026-10-09 (Sentinel_c5ef0f37bb288191a71cc6e96ea7c920 and
Sentinel_db65b9d8c56c8191971d74e9d8c9ec93). See security §13–14,
architecture §12, interfaces Provider management, and ADR-0045. Other privacy,
Vault permissions, path eligibility, encryption, and request budgets remain enforced.

## Current repository state

Base HEAD is e5a47d643583ec1111e2cb46b126866b33b498d8. The previous 11-file
uncommitted proxy draft was copied and byte-verified at
/workspace/scratch/m6-cloud-e5a47d6-20261009/superseded-proxy-draft-20261009T085209Z.
Its replacement code was restored to HEAD; the existing M6 run evidence in
semantic-memory-implementation.md and scratch was retained. No main/PR/deployment
or actual Provider configuration is changed. The earlier non-streaming JSON fixes
are part of the base commit.

## Scope and non-scope

Change providers policy/transport, the local FastEmbed gate, mode serialization,
one forward State migration, Admin UI, related fixtures, regression tests, and
operator documentation. Do not build a trusted-proxy framework, alter network
permissions, edit actual run databases, or execute a paid Provider request.

## Invariants and risks

Only Admin APIs and trusted host-side tools configure Provider endpoints. Keep
URL validation, TLS verification, redirect denial, protected headers, encrypted
secrets, bounded resources, and per-request accounting. The administrator now
owns all target-network access; the application makes no target-IP/SSRF guarantee.
Legacy local_only MUST become disabled, never silently enabled. It has no enduring
runtime branch. Legacy remote_allowed becomes enabled. Missing configuration
remains disabled. Existing sealed evaluation snapshots must not be rewritten.

## Proposed design

ProviderMode has only Disabled and Enabled. Read aliases accept local_only as
Disabled and remote_allowed as Enabled. Migration 0044 canonicalizes stored mode
values and bumps only changed setting revisions. The obsolete
allow_private_networks JSON field remains readable and is ignored. HTTP adapters
validate URL syntax and then use reqwest's existing direct/environment-proxy
routing. FastEmbed follows the same total switch. Admin shows only the switch
and states that its configured address determines the destination.

## Work breakdown

1. Back up the replaced draft; read M6 modes without opening any credentials.
2. Change providers policy/transport and mode migration; update synthetic callers.
3. Change Admin UI and normative documentation, recording ADR-0045.
4. Add migration, permission, URL, redirect/credential, proxy-DNS and switch tests.
5. Run relevant backend/frontend gates, independent review, and unauthenticated
   network diagnostics. Report paid evaluation as blocked pending fresh authorization.

## Progress

- [x] 2026-10-09: backup verified; superseded code removed; M6 evidence preserved.
- [x] Read-only source and baseline State both store remote_allowed, not local_only.
- [x] Implement simplified transport, two-value switch and safe legacy migration.
- [x] Independent static review and incremental security-test review found no new
  code-level security defect; fix the identified operator-documentation omissions.
- [x] Complete default-feature tests, independent review and documentation.
- [ ] All-features tests/Clippy: existing official ort-sys CDN 403 prerequisite.

## Decisions

Use a one-time disable migration for legacy local_only, with an explicit normal
Admin enable action afterwards. Do not retain LocalOnly as a hidden compatibility
mode. Do not expand other privacy-policy removal beyond this authorization.

## Surprises and discoveries

LocalOnly was per-Vault and gated both HTTP target IP ranges and in-process
FastEmbed. It was not an independent file-permission or source-eligibility policy.
The source/baseline M6 configuration is remote_allowed; semantic arm State stores
do not own the shared Provider mode. All-features builds previously encountered
an official ort-sys CDN 403; do not disguise a skipped feature gate as passing.

The first complete default-feature workspace attempt reached the Eval tests and
failed 13 probe setup cases because the probe still pinned State migration 43.
Update the probe, live-runtime and prepare-CLI exact-version checks to 44, together
with State test expectations. Probe validation opens prepared State read-only and
rejects old schema versions. Live runtime retains its existing migration-before-
version-check order, with unchanged seal and runtime-snapshot checks; it does not
gain permission to reuse old sealed runs. No real run or database was upgraded.
Preserve the failed log as
`validation/admin-network-workspace-before-version-fix.log` and rerun the gate.

## Validation

Use existing Rust/Cargo 1.94, Node 24.19 and pnpm 11.19. Cargo home and target are
the existing scratch cargo-home/target directories. Commands run without
MIMO_API_KEY: cargo fmt --all --check; relevant locked offline Cargo tests and
Clippy; pnpm --dir frontend/admin lint/test/build; git diff --check. Add no tests
that call paid APIs. Record exact completed commands/results below.

All logs below are under
`/workspace/scratch/m6-cloud-e5a47d6-20261009/validation/`. The Cargo commands use
`CARGO_HOME=../cargo-home`, `CARGO_TARGET_DIR=../target`, `CARGO_BUILD_JOBS=3`
relative to that directory and unset `MIMO_API_KEY`.

| Check | Result | Log |
| --- | --- | --- |
| `cargo test --offline --locked --workspace` | exit 0; 702 passed, 0 failed, 0 ignored across 66 suites including doc tests | `admin-network-workspace-tests.log` |
| `cargo clippy --offline --locked --workspace --all-targets -- -D warnings` | exit 0 | `admin-network-clippy.log` |
| `cargo fmt --all --check` | passed | `admin-network-fmt.log` |
| `git diff --check` | passed | `admin-network-diff-check.log` |
| `pnpm --dir frontend/admin lint` | exit 0 | `admin-network-frontend-lint.log` |
| `pnpm --dir frontend/admin test` | exit 0; 42 passed, 10 existing skips | `admin-network-frontend-tests.log` |
| `pnpm --dir frontend/admin build` | exit 0 | `admin-network-frontend-build.log` |
| `cargo build --offline --locked -p mcp-vault-eval -p mcp-vault-providers --features reqwest/rustls-tls-native-roots --bins` | exit 0; compile only | `admin-network-native-roots-build.log` |

The passing workspace suite covers one-time migration and unchanged unrelated
Vault settings, Admin session/CSRF/Origin checks, disabled JSON/SSE zero I/O and
zero transport budget, proxy CONNECT for an unresolved synthetic target, untrusted
TLS rejection, and JSON/SSE credential/body containment for 301/302/303/307/308.
TLS and redirect fixtures isolate ambient proxy variables in subprocesses.
Independent read-only reviews of the implementation, these tests, documentation
and exact schema-version changes found no new code-level security defect.

The unauthenticated Cloud TLS diagnostic made two TLS handshakes and zero origin
application HTTP requests. WebPKI alone failed with `tls_unknown_issuer`; adding
the environment CA succeeded. This is handshake evidence only, not proof that a
Provider business request succeeds (`admin-network-tls-preflight.log`). Production
default Cargo features and persistent trust/network settings are unchanged.

All-features tests and Clippy remain unverified because the earlier standard
ort-sys 2.0.0-rc.13 download from cdn.pyke.io returned proxy 403. The preserved
failure and source-container log location are recorded in
`semantic-memory-implementation.md`, section "2026-10-09 06:17 UTC". There was no
network-policy change, so this task did not repeat that download. FastEmbed's
switch change received static review but no all-features runtime verification.

## Rollback and recovery

The archived draft is a review artifact, not the rollback target. A code rollback
uses the base commit and preserves unrelated evidence. Do not edit applied SQL
migrations or actual deployment databases. A user deliberately re-enables a
migrated Provider switch through the existing authenticated Admin endpoint.
Do not reuse, rewrite, or restart the interrupted M6 run.

## Outcomes

LocalOnly runtime/UI and the private-network UI option are removed. Legacy
local_only is disabled once on upgrade and may then be explicitly enabled by an
administrator; legacy remote_allowed maps to enabled. Administrator-owned
networking now works through normal reqwest routing with the retained URL, TLS,
redirect, credential, privacy and resource controls. ADR-0045 records the change.

Changes remain uncommitted on base e5a47d643583ec1111e2cb46b126866b33b498d8.
The superseded 11-file draft, frozen ADR evaluation inputs and existing M6 run
evidence remain preserved. The actual source/baseline Provider setting was
remote_allowed. No paid request, real-State migration, main/PR/push/deployment,
network permission change or persistent credential change was made during this
implementation. M6 quality remains not_evaluated; do not restart the interrupted
run or launch a fresh paid run without the parent's explicit start instruction.
