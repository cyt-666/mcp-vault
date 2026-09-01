# ADR-0020: Managed multi-Vault lifecycle preserves endpoint isolation

- Status: Accepted
- Date: 2026-09-01

## Context

MCP Vault already scopes canonical files, credentials, jobs, indexes, vectors,
memory, and audit by `VaultContext`, and its data-plane URLs already contain a
Vault slug. The shipped Admin surface nevertheless selected the first registry
row and could create only the setup-time default Vault. Enabling several
Vaults must not turn that implicit selection into an authorization parameter,
break existing single-Vault clients, or let one Vault's initialization or
failure stop another.

Allowing Admin to attach arbitrary server paths or delete a registered Vault in
the same work would add unrelated path-ownership, retention, and destructive
recovery decisions.

## Decision

One Admin owner may create several service-managed Vaults. New roots are fixed
at `<data-dir>/vaults/<slug>`; the validated slug is an immutable endpoint
identity. A Vault can be disabled and re-enabled but is not detached or deleted
by this lifecycle.

Every Vault receives distinct path-based data endpoints:

```text
/dav/v1/vaults/{vault_slug}/
/mcp/v1/vaults/{vault_slug}
```

WebDAV credentials, PATs, built-in OAuth tokens, and external OAuth grants
remain bound to the endpoint Vault. Ordinary MCP tools never accept
`vault_id`; an Agent uses another MCP connection to access another Vault.

Admin adds explicit `/api/v1/vaults/{vault_slug}/...` routes. Existing
unscoped Admin routes remain compatibility aliases to a persisted
`legacy_default_vault_id`; they never select the first row after another Vault
is created.

Managed admission atomically inserts the registry row, the Vault-scoped
initialization job, and the first legacy-default setting when necessary. The
new data endpoints remain unavailable until that job completes initial
reconciliation, index construction, and memory-generation initialization.
Terminal failure affects only that Vault. Job rows, not payload fields, own
Vault selection, and equal-priority job claiming is fair across Vaults.

Backups remain installation-global and include disabled Vaults. Restore still
requires the archive and current registry topology to match exactly.

## Consequences

Positive:

- existing single-Vault IDs, roots, URLs, credentials, and data remain valid;
- work/personal/research links and credentials cannot switch context;
- one failed or disabled Vault does not stop healthy Vaults or Admin;
- index and memory rebuild/reset work retains the same explicit Vault boundary;
- managed-only roots make path ownership and crash recovery deterministic.

Costs:

- old and explicit Admin routes must coexist;
- each Vault has initialization/readiness state and durable work;
- operators configure one WebDAV/MCP connection per Vault;
- installation-global backup maintenance still pauses all Vault writes.

## Rejected alternatives

- Add `vault_id` to MCP tools or infer Vault from note path prefixes.
- Use one global WebDAV/MCP credential and let requests choose a Vault.
- Keep Admin's lexicographic first-row selection after a second Vault exists.
- Accept arbitrary existing filesystem roots in the initial management UI.
- Add detach/content deletion without a separate retention and recovery design.
- Hide cross-Vault recall inside ordinary `recall`.
