# MCP Vault Documentation Map

This directory is the authoritative implementation specification for MCP Vault.

The set is intentionally finite. New documents should be added only when an existing document cannot own the information without becoming ambiguous.

## Reading order for a new implementation Agent

Read these first:

1. [`../AGENTS.md`](../AGENTS.md)
2. [`product-requirements.md`](product-requirements.md)
3. [`architecture.md`](architecture.md)
4. [`interfaces.md`](interfaces.md)
5. [`data-model.md`](data-model.md)
6. [`memory-system.md`](memory-system.md)
7. [`security.md`](security.md)
8. [`implementation-plan.md`](implementation-plan.md)

Then consult:

- [`admin-and-configuration.md`](admin-and-configuration.md) for the management plane and provider settings;
- [`provider-compatibility.md`](provider-compatibility.md) for first-class model providers and their tested wire contracts;
- [`deployment-and-operations.md`](deployment-and-operations.md) for Docker, backup, monitoring, and upgrades;
- [`development-and-testing.md`](development-and-testing.md) for workspace, coding, CI, and conformance practices;
- [`compatibility-matrix.md`](compatibility-matrix.md) for automated WebDAV/MCP evidence and manual Obsidian client records;
- [`requirements-traceability.md`](requirements-traceability.md) for requirement-to-test/release evidence mapping;
- [`release-readiness.md`](release-readiness.md) for first-release gates and operator handoff;
- [`standards-and-references.md`](standards-and-references.md) for the protocol versions and primary references used when this specification was written;
- [`adr/`](adr/) for accepted architectural decisions.

## Document ownership

| Document | Owns |
|---|---|
| `product-requirements.md` | Required behavior, scope, quality attributes, acceptance criteria |
| `architecture.md` | Components, dependencies, consistency, event/job flows, managed multi-Vault shape |
| `interfaces.md` | MCP, WebDAV, Admin HTTP contracts and authorization scopes |
| `data-model.md` | Filesystem layout, SQLite schema, revisions, migrations, rebuildability |
| `memory-system.md` | Durable memory, low-noise extraction, related-note recall, lifecycle, ranking, provenance |
| `admin-and-configuration.md` | Setup, UI pages, configuration hierarchy, provider management |
| `provider-compatibility.md` | OpenAI, Anthropic, DeepSeek, MiMo, GLM, Kimi, Gemini, and Qwen wire compatibility |
| `security.md` | Threat model, control/data/agent planes, secrets, path and provider safety |
| `deployment-and-operations.md` | Docker, networking, TLS, health, backup, recovery, upgrade |
| `development-and-testing.md` | Rust implementation rules, test strategy, CI and tooling |
| `compatibility-matrix.md` | MCP/WebDAV interoperability evidence and Obsidian client checklist |
| `requirements-traceability.md` | Release requirement-to-evidence mapping |
| `release-readiness.md` | First-release gates, artifact procedure, operator handoff |
| `implementation-plan.md` | Complete-service work packages and dependencies |
| `standards-and-references.md` | Versioned external standards and verified libraries |
| `adr/*.md` | Why durable architectural choices were made |

## Normative language

The words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** express requirement strength within this project.

When documents conflict:

1. accepted ADRs govern architectural decisions;
2. product requirements govern externally visible behavior;
3. security requirements override convenience;
4. newer explicitly versioned interface/schema text overrides older examples;
5. unresolved conflicts must be documented and corrected before implementation proceeds.

## Scope statement

These documents specify the intended complete service. `implementation-plan.md` breaks that target into work packages, but completion of an early package does not redefine the product as an MVP.
