# ADR-0021: Serialize legacy no-replace move fallback

- Status: Accepted
- Date: 2026-09-01

## Context

MCP Vault requires move destinations to remain absent. Modern Unix mounts can
enforce that in one syscall with `renameat2(RENAME_NOREPLACE)`, but older Linux
kernels and several network/NAS filesystems return `EINVAL`, `ENOSYS`, or an
unsupported-operation error even though ordinary same-filesystem atomic rename
works. Returning that capability error makes MCP and WebDAV rename unusable.

Ordinary rename can replace an existing target. A check followed by rename is
therefore unsafe when multiple MCP Vault operations can claim the namespace at
the same time. Hard-linking the source is not an acceptable general move
implementation, especially for directories, and would broaden the deliberately
narrow temporary-file exception in the storage security policy.

## Decision

All Core operations that claim an absent target share a Vault-scoped in-process
namespace mutation lock. They acquire it before deterministic path locks and
hold it across destination validation, journal preparation, filesystem commit,
and metadata commit.

Unix move first attempts descriptor-relative `RENAME_NOREPLACE`. Only explicit
capability errors (`EINVAL`, `ENOSYS`, `EOPNOTSUPP`, or `ENOTSUP`) enter the
compatibility path. Storage then rechecks the target through the already-opened
destination directory descriptor and, if it remains absent, performs ordinary
descriptor-relative `renameat`. This fallback supports regular files and
directories on one filesystem. An existing target returns a conflict; unrelated
errors and cross-device moves fail without copying or deleting anything.

The move fallback never uses a hard link. The existing hard-link exception
remains limited to installing a service-created, synced, same-directory private
temporary regular file.

One MCP Vault process owns a Vault root. Direct host filesystem writers do not
participate in the process lock and must not race live protocol mutations;
reconciliation remains responsible for out-of-band changes observed outside
that commit window.

## Consequences

Positive:

- old kernels and NAS mounts can atomically rename files and directories;
- all MCP/WebDAV absent-target claims remain conflict-preserving;
- move journaling and recovery retain one atomic filesystem transition;
- no user entry becomes a hard-link alias.

Costs:

- absent-target namespace mutations serialize per Vault;
- the fallback depends on the documented single-process root ownership model;
- cross-device moves remain unsupported.

## Rejected alternatives

- Treat capability `EINVAL` as a malformed user path.
- Fall back to uncoordinated check-then-rename.
- Hard-link then unlink arbitrary source files.
- Copy then delete files, which is not one atomic move and cannot move a
  directory without a new recovery protocol.
- Add a database migration or operator environment switch for a kernel
  capability that can be detected at the syscall boundary.
