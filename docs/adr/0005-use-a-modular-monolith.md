# ADR-0005: Use a modular monolith

- Status: Accepted
- Date: 2026-08-19

## Context

The service includes WebDAV, MCP, Admin, indexing, memory, providers, jobs, and SQLite/filesystem consistency. Splitting these into services would introduce deployment, authentication, distributed consistency, and queue infrastructure before scale requires it.

A single unstructured crate would make boundaries equally difficult to preserve.

## Decision

Build one Rust deployable with explicit modules/crates, two HTTP listeners, persistent jobs/outbox, and dependency rules.

Protocol adapters and workers remain separable behind application interfaces so a future deployment may split them without changing domain behavior.

## Consequences

Positive:

- simple Docker deployment;
- direct consistency and migration boundary;
- no mandatory broker or external database;
- strong internal boundaries remain testable.

Costs:

- one process contains several responsibilities;
- compile time and crate ergonomics require management;
- CPU-heavy local inference must be isolated on bounded worker pools.

## Rejected alternatives

- Microservices and a broker from the first release.
- One giant crate with handlers, SQL, filesystem, and AI logic mixed together.
