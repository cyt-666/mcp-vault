# PLANS.md

This file defines the execution-plan format used by Codex for substantial MCP Vault work.

An ExecPlan is a living, self-contained implementation document. It must enable another engineer or Agent to resume the work without relying on chat history.

Create plans under:

```text
docs/exec-plans/active/<descriptive-name>.md
```

Move them to `docs/exec-plans/completed/` after the acceptance criteria have been met and verified.

## When an ExecPlan is required

Use an ExecPlan when work:

- spans more than one crate or protocol surface;
- changes a database schema or migration;
- changes authentication, authorization, or network exposure;
- changes Vault write consistency or recovery;
- introduces an LLM, embedding, or vector provider;
- implements a complete work package from `docs/implementation-plan.md`;
- is likely to require several hours or multiple Codex tasks.

A tiny local fix with obvious tests does not require a new plan.

## Required plan sections

### Title and status

State the objective, owner/agent, creation date, last update, and status.

### Purpose and user-visible result

Describe the concrete behavior that will exist when the plan is complete.

### Governing requirements

Link the exact sections of the requirements, architecture, interfaces, security document, and ADRs that constrain the work.

### Current repository state

Describe the relevant modules, migrations, interfaces, and tests as they actually exist. Do not assume the repository still matches an older plan.

### Scope and non-scope

State what is included and what deliberately remains for another plan.

### Invariants and risks

List data-integrity, Vault-isolation, security, compatibility, and migration risks.

### Proposed design

Explain component boundaries, data flow, transaction boundaries, public interfaces, schema changes, and recovery behavior.

### Work breakdown

Use ordered, independently verifiable steps. Each step must name expected files/modules and its validation.

### Progress

Maintain a timestamped checklist. Split partially completed entries rather than marking them complete.

### Decisions

Record design decisions made while implementing, including alternatives rejected and why. If a decision is durable, create an ADR.

### Surprises and discoveries

Record unexpected repository behavior, dependency constraints, test failures, protocol quirks, and performance findings with evidence.

### Validation

List exact commands, conformance suites, manual checks, fixtures, and expected results.

### Rollback and recovery

Explain how to undo migrations or configuration changes and how interrupted work recovers safely.

### Outcomes

At completion, summarize what shipped, what differs from the original plan, and any follow-up work.

## Plan quality rules

- Plans must describe observable behavior, not vague activities.
- Do not use “implement the feature” as a step.
- Include file paths, symbols, endpoint names, migration identifiers, and test commands when known.
- Keep progress and decisions current during implementation, not only at the end.
- Do not mark acceptance complete when tests are skipped.
- A plan may evolve, but it must remain internally consistent.
