# Atomic Create Compatibility for Filesystems Without RENAME_NOREPLACE

- Status: Completed
- Owner/Agent: Codex
- Created: 2026-08-31
- Updated: 2026-08-31

## Purpose and user-visible result

Allow MCP, WebDAV, and other Vault Core callers to create a complete canonical
file on a writable Unix filesystem that supports ordinary files and hard links
but rejects `renameat2(..., RENAME_NOREPLACE)` with an unsupported-operation
error. A create still fails when the destination already exists, never exposes
partial content, and never falls back to an overwrite-capable plain rename.

The observed user failure is
`filesystem operation atomically rename file failed (InvalidInput)` for every
new note on the deployed Vault mount. Replacement writes are not the affected
path because they use ordinary atomic replacement after revision validation.

## Governing requirements

- `AGENTS.md`: canonical writes are atomic, revision-aware, recoverable, and
  must never silently overwrite a concurrent change.
- `docs/product-requirements.md` sections 3.4 and 4.1: controlled mutations,
  atomic writes, conflict behavior, and crash reconciliation.
- `docs/architecture.md` sections 4.3-4.4 and 7: Vault Core owns mutation
  policy; `storage-fs` owns temporary files, atomic installation, fsync, and
  redacted filesystem errors.
- `docs/security.md` sections 9-10: descriptor-relative/no-follow operations,
  no externally controllable hardlink aliases, atomic commit, and conflict
  protection.
- `docs/development-and-testing.md` sections 8, 13, and 14: integration,
  crash/recovery, concurrency, and filesystem security tests.
- Completed plan `docs/exec-plans/completed/wp-03-safe-filesystem-history.md`:
  same-directory temporary files, explicit replacement policy, and atomic
  visibility remain binding.

## Current repository state

`VaultCore::content_mutation_with_policy` maps create/copy operations with
`require_absent=true` to `DestinationPolicy::MustNotExist`. On Unix,
`platform::commit_temp` implements that policy only with
`rustix::fs::renameat_with(..., RenameFlags::NOREPLACE)`. Rustix calls the
platform `renameat2`/exclusive-rename interface. The deployed filesystem
returns `EINVAL`, which Rust maps to `ErrorKind::InvalidInput`; Linux documents
that result when a filesystem does not support a rename flag.

The same 0.1.11 linux/amd64 probe succeeds on the container filesystem and an
OrbStack host bind mount, so the behavior is mount-specific rather than a
generic Docker or path/content failure. Existing tests run on filesystems that
support the flag and do not exercise the unsupported-capability branch.

## Scope and non-scope

### Included

- Keep `RENAME_NOREPLACE` as the preferred Unix create commit.
- On only `EINVAL`, `ENOSYS`, or `EOPNOTSUPP`, atomically link the
  service-created same-directory temporary regular file at the destination,
  then unlink the temporary name.
- Map a destination race to `StorageError::DestinationExists`.
- Return a stable explicit error when neither exclusive rename nor the safe
  hard-link installation is supported.
- Add deterministic Unix unit coverage for forced fallback success,
  destination-race preservation, cleanup, and non-capability error behavior.
- Remove a journal-owned linked temporary name during Vault Core recovery once
  the canonical target hash proves the new state.
- Document the deployment filesystem requirement and controlled internal
  hard-link exception.

### Not included

- No plain `stat` plus `renameat` fallback.
- No hard-link API exposed to MCP, WebDAV, Admin, or user paths.
- No directory-move fallback; directories cannot use this file-only strategy.
- No cross-filesystem commit, copy-based partial target, schema migration, or
  protocol contract change.

## Invariants and risks

- The fallback source is only a regular file created by MCP Vault with
  `O_CREAT|O_EXCL|O_NOFOLLOW` in the already-open destination parent.
- Source and destination use the same parent descriptor, so no cross-Vault or
  cross-filesystem alias is introduced.
- `linkat` atomically refuses an existing destination. A concurrent external
  creator wins with `EEXIST`; MCP Vault cleans its temporary file and reports a
  conflict.
- The target becomes visible only after the complete temporary payload has
  been written and synced.
- There can be a short interval where temporary and canonical names reference
  the same complete inode. The temporary name is removed before success and
  remains journal-addressable after an interrupted commit.
- If hard links are prohibited by the mount, fail explicitly rather than
  weaken no-clobber semantics.

## Proposed design

Add a private Unix no-replace commit helper. It receives the result of the
preferred exclusive rename so tests can deterministically force the capability
branch without depending on the host filesystem.

```text
validate destination
  -> renameat2(temp, target, NOREPLACE)
     -> success: sync parent
     -> EEXIST: destination conflict
     -> EINVAL/ENOSYS/EOPNOTSUPP:
          linkat(temp, target)
          -> EEXIST: destination conflict
          -> unsupported: explicit unsupported-filesystem error
          -> success: unlinkat(temp), sync parent
     -> any other error: preserve the original rename failure
```

The existing `commit_temp` owner performs error cleanup and directory fsync.
Portable non-Unix code and replacement commits remain unchanged.

## Work breakdown

1. Add the Unix helper and stable unsupported-filesystem error in
   `crates/storage-fs/src/platform/unix.rs` and `crates/storage-fs/src/error.rs`.
2. Add Unix unit tests with an injected exclusive-rename result, plus existing
   public storage integration coverage and a Vault Core linked-temp recovery
   test.
3. Update security, deployment, and compatibility documentation, then refresh
   document checksums.
4. Run formatting, focused storage tests, workspace Clippy/tests, frontend
   checks when affected, documentation checks, and a linux/amd64 image build
   if a release version is requested.

## Progress

- [x] 2026-08-31 — Traced MCP `create_note` through Vault Core and
  `storage-fs` to the exact `RENAME_NOREPLACE` call and redacted error mapping.
- [x] 2026-08-31 — Confirmed current container and OrbStack bind mounts support
  the flag; recorded that the production behavior is mount-specific.
- [x] 2026-08-31 — Implemented the safe file-only compatibility path, explicit
  unsupported error, destination-race mapping, and pre-validation cleanup.
- [x] 2026-08-31 — Added five deterministic Unix tests and a Vault Core test
  proving recovery removes a linked journal temp after canonical installation.
- [x] 2026-08-31 — Bumped the workspace, Admin package, and deployment defaults
  to `0.1.12`; built and smoke-checked the `linux/amd64` image.
- [x] 2026-08-31 — Completed Rust, frontend, documentation, checksum, and
  whitespace validation on the final versioned tree.

## Decisions

- Reject `stat`/existence-check plus ordinary rename: another protocol or
  external Obsidian writer can create the destination between the check and
  overwrite-capable rename.
- Use a hard link only as an internal commit primitive for one already-synced
  same-directory temporary regular file. This preserves atomic visibility and
  no-clobber behavior without creating a user-facing hard-link feature.
- Do not generalize the fallback to directories or moves because link/unlink is
  not an atomic directory rename.

## Surprises and discoveries

- The client-facing `InvalidInput` is the redacted standard `ErrorKind` for
  the filesystem's `EINVAL`; it does not identify invalid note content or a
  malformed Unicode path.
- All reported paths were creates, so they selected `MustNotExist`; that is why
  unrelated ASCII and Chinese filenames failed identically.
- Recovery previously treated an absent temp after rename as the normal case
  but did not remove a still-linked temp after proving the canonical new hash;
  the fallback required making that cleanup explicit.

## Validation

```bash
cargo fmt --all --check
cargo test -p mcp-vault-storage-fs --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
bash scripts/check-docs.sh
shasum -a 256 -c SHA256SUMS
git diff --check
```

Focused tests must prove fallback success installs complete bytes and removes
the temp name, a destination created after validation is preserved byte-for-
byte, unsupported hard-link behavior is explicit, and unrelated rename errors
do not enter the fallback.

Observed results on the final `0.1.12` tree:

- `cargo fmt --all --check` passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  passed.
- `cargo test --workspace --all-features` passed, including five forced Unix
  storage tests and the linked-temporary-name recovery test.
- `make frontend-lint frontend-test frontend-build` passed; Vitest ran 26
  tests and the production bundle completed.
- `bash scripts/check-docs.sh`, `shasum -a 256 -c SHA256SUMS`, and
  `git diff --check` passed.
- `docker build --platform linux/amd64 --tag mcp-vault:0.1.12 --tag
  mcp-vault:latest .` passed. Both tags resolve to image
  `sha256:697a2093ba6edcc8b686b19b6a839889270538e7e8bf4300af6e4e21dc58bb7e`;
  inspection reports `linux/amd64`, user `mcpvault`, and the expected
  entrypoint. `docker run --rm --platform linux/amd64 mcp-vault:0.1.12
  --check-config` passed.

## Rollback and recovery

The change has no schema or data migration. Rollback is a binary rollback.
Normal failures remove the temporary file and leave any pre-existing target
unchanged. A process interruption after the hard link but before temp unlink
leaves two names for the same complete inode; the durable operation journal
retains both canonical and temporary paths for recovery rather than exposing
partial bytes.

## Outcomes

MCP Vault now preserves the preferred exclusive-rename path while allowing
safe regular-file creation on a mount that rejects that interface but supports
same-directory hard links. The fallback never calls overwrite-capable plain
rename, maps a concurrent target creator to a conflict, cleans private temporary
names, and gives recovery enough information to finish an interrupted linked
commit. A mount lacking both primitives fails with a stable explicit storage
error. The final source and local Linux image are versioned `0.1.12`.
