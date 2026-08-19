# ADR-0008: MCP credentials bind to one Vault

- Status: Accepted
- Date: 2026-08-19

## Context

A future service may host multiple Vaults. Allowing an Agent to choose a Vault in each tool call increases accidental writes and cross-domain memory leakage. Current MCP authorization also benefits from a clear protected resource/audience.

## Decision

Each MCP endpoint identifies a Vault and every PAT/OAuth grant is bound to that Vault.

Tool schemas do not contain `vault_id`.

An Agent needing access to two Vaults configures two MCP server connections/credentials. A future federated cross-Vault service requires a separate capability and explicit grants.

The server supports Vault-scoped PATs and standards-aligned OAuth resource-server mode.

## Consequences

Positive:

- simple model-facing tools;
- strong work/personal separation;
- clear OAuth resource and audit context;
- no accidental Vault switching.

Costs:

- multiple client configurations for multiple Vaults;
- grants and endpoints must be managed per Vault;
- future federation cannot be hidden inside ordinary recall.

## Rejected alternatives

- One global token that can access every Vault.
- `vault_id` argument on each tool.
- Infer Vault from note path prefixes.
