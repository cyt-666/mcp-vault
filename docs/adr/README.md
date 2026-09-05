# Architecture Decision Records

Accepted ADRs are binding unless superseded by a later ADR.

The current sequence includes ADR-0026. ADR-0013 keeps ADR-0007 durable memories
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

ADR-0020 enables managed multi-Vault administration while retaining per-Vault
MCP/WebDAV endpoints, Vault-bound credentials, a stable legacy Admin default,
and isolated initialization, jobs, indexes, and memory.

ADR-0021 permits a serialized ordinary-`renameat` compatibility path when a
same-filesystem Unix mount rejects `RENAME_NOREPLACE`; it does not broaden the
temporary-file hard-link exception.

ADR-0022 replaces one-time memory-source repair with continuous exact source
health, fail-closed normal recall, event-ordered reconciliation, and repeatable
Vault-scoped audits.

ADR-0023 keeps canonical memory in its source language and adds persisted,
rebuildable source/`zh-Hans`/`en` retrieval metadata, explicit historical
backfill, CJK-aware lexical recall, and object-scoped vector ranking without a
query-time LLM call.

ADR-0024 replaces character-count note-vector chunks with a versioned bounded
UTF-8 input envelope, preserves redacted Provider failure categories, and
requires current-model note and memory vector scheduling without re-running
memory extraction.

ADR-0025 aggregates current note chunks by File ID before the final note Top-K,
uses only the highest non-negative cosine per note, and makes that cosine scale
the existing reciprocal-rank contribution.

ADR-0026 replaces the two-phase global consolidation, model-readable lifecycle
history, destructive pipeline resets, and continuous source-health graph with
current source-owned memory sets plus direct explicit memory. Forgetting is
deletion, note sets replace atomically under File-ID/hash/set-revision checks,
and all query paths are current-only. It retains canonical Markdown,
provenance, multilingual aliases, LLM-free recall, and object-scoped vector
validation while adding relevance, chunk-coverage, and budget requirements.

Status values:

```text
Proposed
Accepted
Superseded
Deprecated
Rejected
```

When architecture changes, do not rewrite historical rationale. Add a new ADR that supersedes the old one and update the old status/link.
