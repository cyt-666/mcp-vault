# ADR-0002: Vault is the isolation boundary

- Status: Accepted
- Date: 2026-08-19

## Context

The first release may configure one Vault, but a future service may host personal, work, or research Vaults. Adding a global singleton now would make later isolation risky and invasive.

## Decision

Every user-data operation requires a `VaultContext`.

Storage roots, database rows, credentials, permissions, jobs, events, caches, FTS, vectors, memory, providers overrides, and audit are Vault-scoped.

The first release maintains two-Vault isolation tests even when the Admin UI permits one configured Vault.

## Consequences

Positive:

- future multi-Vault management is an enablement feature rather than a redesign;
- accidental work/personal memory mixing is structurally prevented.

Costs:

- more explicit parameters and predicates;
- composite constraints and test fixtures;
- careful cache/vector partitioning.

## Rejected alternatives

- Global `/data/vault` singleton.
- Let every MCP tool accept `vault_id`.
- Add Vault scoping only when the second Vault feature is implemented.
