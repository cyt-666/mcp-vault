# ADR-0027: Bounded automatic retrieval calibration

- Status: Accepted
- Date: 2026-09-05
- Amends: ADR-0026 retrieval preparation and operational policy

## Decision

Configured embedding roles automatically calibrate against a versioned, bundled
non-private benchmark after startup and on configuration reconciliation. Memory
and note channels have separate signatures, thresholds and holdout reports.
When both have applicable reports, their no-answer failure IDs are unioned against
the same holdout gate. A failing combined result prevents semantic activation and
automatic retry storms while retaining both reports for diagnosis and explicit retry.
A passing benchmark enables semantic admission; vector coverage alone does not.
No generation model, private corpus labeling or Admin page visit is required.

The application uses the existing Provider authorization/transport boundary and
durable jobs. Requests, input bytes, time, retries and concurrency are bounded;
checkpoints and spent budget survive restarts. Publication rechecks the effective
signature. A failed candidate cannot replace an applicable passing report.
Benchmark results describe only that corpus, never measured private-Vault quality.
Reports record build provenance, actual dispatch counts and failed case identifiers.
Production and the isolated benchmark backend share input preparation, exact cosine,
lexical admission and object contribution functions; private-source eligibility and
response budgeting remain enforced by application services and tested through recall.

Canonical knowledge and existing valid business vectors remain unchanged.
Calibration cannot migrate legacy memory, resume paused sources or queue full
extraction. Admin may disable automatic maintenance while retaining an applicable
active calibration. GET is read-only; run/retry invokes the same worker engine.
Legacy imported reports retain their provenance and require server revalidation.

All retrieval channels apply current/request eligibility, then relevance admission,
then object-level ranking and a shared serialized-output budget. Related-note
candidates cannot bypass recall relevance gates. Search retains its own contract.

## Consequences

Upgrades may issue a small bounded number of embedding requests using already
configured authorization. Operators can inspect and stop maintenance. Unsupported
quality, authorization and budget failures are explicit and preserve local lexical
retrieval. No model-readable memory lifecycle or global consolidation is restored.

## Rejected alternatives

Manual metrics forms and bind-only triggers miss existing configured deployments.
Unconditional positive-cosine admission and fake quality reports do not establish
relevance. Rebuilding valid business vectors would spend resources without fixing
missing calibration.
