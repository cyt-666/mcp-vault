# Architecture Decision Records

Accepted ADRs are binding unless superseded by a later ADR.

The current sequence includes ADR-0019. ADR-0013 keeps ADR-0007 durable memories
canonical and provenanced while allowing `recall` to return separately typed,
rebuildable ordinary-note cues. ADR-0014 replaced model self-score/routine
review defaults with exact evidence and autonomous promotion. ADR-0015 retains
those trust rules but removes per-note service markers in favor of one
Vault-level automatic mode and a narrow local type allow-list. ADR-0016 replaces
automatic direct promotion with Codex-style raw-memory extraction followed by
separate global consolidation; evidence remains exact and independent from
final semantic content. ADR-0017 retains that architecture but changes
prerelease upgrades to discard every replaced memory generation and require
versioned fresh jobs instead of compatibility conversion.

ADR-0018 adds a complete built-in OAuth 2.1 authorization server as the default
self-hosted ChatGPT path while preserving external issuer validation as an
optional compatibility mode. Its 2026-08-31 amendment separates protocol-only
`offline_access`, uses a 180-day sliding refresh idle lifetime, and protects a
successful rotation with a bounded duplicate-request grace before replay-family
revocation.

ADR-0019 removes the WebDAV proxy socket-peer allow-list and moves protection
of the plaintext data listener entirely to deployment networking while
retaining the forwarded-HTTPS requirement.

Status values:

```text
Proposed
Accepted
Superseded
Deprecated
Rejected
```

When architecture changes, do not rewrite historical rationale. Add a new ADR that supersedes the old one and update the old status/link.
