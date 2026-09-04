# Standards and Primary References

## 1. Verification date

This document was verified on **2026-08-24**.

External protocols and libraries evolve. Before implementing a protocol-sensitive change, confirm the current official specification and the selected dependency’s supported revision.

## 2. MCP

### Protocol target

- MCP Specification 2026-07-28
  https://modelcontextprotocol.io/specification/2026-07-28

Key design implications used by this project:

- `server/discover` is required and can return usage instructions.
- Streamable HTTP is stateless at the protocol level for 2026-07-28.
- The MCP endpoint accepts POST; the earlier separate GET stream endpoint is removed.
- Protocol-level sessions and `Mcp-Session-Id` are removed for this revision.
- request metadata and standard routing headers are required by the current transport rules;
- tool/resource list and read results carry caching metadata;
- tools should be returned in deterministic order;
- authorization-dependent tool sets are permitted because credentials are per request.

Relevant pages:

- Streamable HTTP
  https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http
- Discovery
  https://modelcontextprotocol.io/specification/2026-07-28/server/discover
- Tools
  https://modelcontextprotocol.io/specification/2026-07-28/server/tools
- Resources
  https://modelcontextprotocol.io/specification/2026-07-28/server/resources
- Authorization
  https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/authorization-server-discovery
- Authorization security considerations
  https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/security-considerations

### Rust SDK

Use the official Rust SDK:

- repository
  https://github.com/modelcontextprotocol/rust-sdk
- crate
  `rmcp`

At verification time, the official SDK documents stable 2026-07-28 support and compatibility with older revisions. Pin a tested 3.x release in the lockfile and run its matching conformance suites.

The official conformance framework is maintained at:

- repository: https://github.com/modelcontextprotocol/conformance
- WP-14 CI pin: `74edef34d674f563537be8c6587cebaa58e830ca`

The pin is used by `scripts/conformance/mcp.sh`; update it only after a
reviewed compatibility run. The conformance runner's expected-failure file is
strict: a new failure fails the job and a formerly failing check becoming
passing makes the baseline stale.

Do not use older blog examples that assume a persistent `Mcp-Session-Id` for the current revision.

## 3. Codex project instructions

Codex reads repository `AGENTS.md` files before work and resolves instructions from repository root toward the working directory.

Official documentation:

- https://developers.openai.com/codex/agent-configuration/agents-md
- https://developers.openai.com/codex/cloud
- https://developers.openai.com/cookbook/articles/codex_exec_plans

The root `AGENTS.md` is deliberately concise and points to detailed documents. `PLANS.md` defines the project’s execution-plan convention.

## 4. Obsidian Vault model

Official Obsidian developer documentation defines a Vault as a folder and its subfolders:

- https://docs.obsidian.md/Plugins/Vault
- https://docs.obsidian.md/Reference/TypeScript%2BAPI/FileSystemAdapter

Design consequence: MCP Vault exposes an ordinary folder tree and does not create a proprietary remote Vault format.

## 5. Obsidian WebDAV clients

Primary existing plugin target:

- Hēsperus Sync Engine
  https://github.com/hesprs/sync-engine

At verification time, Sync Engine is the successor to Hēsperus WebDAV Sync and provides a WebDAV backend module, bidirectional sync, conflict strategies, custom headers, and rate/memory controls.

Secondary target:

- Remotely Save
  https://github.com/remotely-save/remotely-save

These are unofficial Obsidian plugins. Compatibility must be demonstrated by project tests and release checks rather than assumed from protocol claims.

## 6. WebDAV

Primary standard:

- RFC 4918 — HTTP Extensions for Web Distributed Authoring and Versioning
  https://www.rfc-editor.org/rfc/rfc4918

Related HTTP semantics/preconditions/ranges should follow current HTTP RFCs.

Rust library direction:

- `dav-server` / `dav-server-rs`
  https://github.com/messense/dav-server-rs

The library exposes custom filesystem and guarded filesystem interfaces and reports passing core WebDAV Litmus suites. MCP Vault must wrap it behind a project-owned Vault Core adapter rather than using its local filesystem backend directly.

Conformance:

- WebDAV Litmus test suite
  http://www.webdav.org/neon/litmus/

The repository wrapper is `scripts/interop/webdav-litmus.sh`. A missing Litmus
binary is an explicit blocked gate; it is not treated as a passing test.

## 7. OAuth and protected resources

Relevant standards referenced by the MCP authorization specification:

- RFC 9728 — OAuth 2.0 Protected Resource Metadata
  https://www.rfc-editor.org/rfc/rfc9728
- RFC 8707 — Resource Indicators for OAuth 2.0
  https://www.rfc-editor.org/rfc/rfc8707
- RFC 8414 — OAuth 2.0 Authorization Server Metadata
  https://www.rfc-editor.org/rfc/rfc8414
- RFC 7591 — OAuth 2.0 Dynamic Client Registration
  https://www.rfc-editor.org/rfc/rfc7591
- RFC 7636 — Proof Key for Code Exchange (PKCE)
  https://www.rfc-editor.org/rfc/rfc7636
- RFC 9207 — OAuth 2.0 Authorization Server Issuer Identification
  https://www.rfc-editor.org/rfc/rfc9207
- OAuth 2.1 work and security best practices as incorporated by the current MCP specification.

The default standalone mode implements both the authorization-server and
resource-server roles for public clients: DCR, authorization code, PKCE
`S256`, resource indicators, issuer identification, opaque tokens, and refresh
rotation. Authorization-server metadata also advertises `offline_access` for
long-lived ChatGPT connections. That protocol scope is separated from MCP
resource permissions; active refresh rotation renews a bounded 180-day idle
lifetime. A configured external authorization server remains optional.

ChatGPT plugin interoperability is also checked against the official OpenAI
Plugins authentication guide:

- https://developers.openai.com/plugins/build/auth

The guide requires protected-resource and authorization-server metadata,
authorization code + PKCE `S256`, `resource` propagation, CIMD/DCR or a
predefined client, and issuer/audience/expiry/scope validation. MCP Vault's
built-in flow uses DCR and owns authorization, token, login, and consent
surfaces; the external issuer mode remains a separately tested compatibility
path.

## 8. SQLite and vector search

Core storage:

- SQLite
  https://sqlite.org/
- SQLx
  https://github.com/launchbadge/sqlx

Vector backend candidate:

- sqlite-vec
  https://github.com/asg017/sqlite-vec

At verification time, sqlite-vec remains pre-1.0. It must be pinned and isolated behind an internal `VectorIndex` interface with a non-extension fallback.

## 9. Local embeddings

Candidate optional Rust adapter:

- fastembed-rs
  https://github.com/Anush008/fastembed-rs

WP-10 pins the FastEmbed Rust crate to 5.17.4 and runs its synchronous ONNX
inference on a bounded blocking task. Provider HTTP uses reqwest 0.12.28 with
project-owned redirect, DNS, timeout, and response-size policy.

Run synchronous inference on a dedicated bounded blocking pool. Model choice and license must be visible to the administrator.

### 9.1 Remote structured generation

Primary request-contract references:

- OpenAI OpenAPI specification and official generated SDK list:
  https://github.com/openai/openai-openapi
- OpenAI Structured Outputs supported-schema rules, including required object
  fields and `additionalProperties: false`:
  https://developers.openai.com/api/docs/guides/structured-outputs
- DeepSeek JSON Output and thinking mode:
  https://api-docs.deepseek.com/guides/json_mode
  https://api-docs.deepseek.com/guides/thinking_mode
- Xiaomi MiMo structured output:
  https://mimo.mi.com/docs/en-US/quick-start/usage-guide/text-generation/structured-output
- Xiaomi MiMo OpenAI-compatible Chat Completions fields:
  https://mimo.mi.com/docs/en-US/api/chat/openai-api
- Zhipu structured output and OpenAI compatibility:
  https://docs.bigmodel.cn/cn/guide/capabilities/struct-output
  https://docs.bigmodel.cn/cn/guide/develop/openai/introduction
- Zhipu `embedding-3` request, dimension, batch, and per-input limits:
  https://docs.bigmodel.cn/api-reference/模型-api/文本嵌入
- Moonshot/Kimi Chat and platform overview:
  https://platform.kimi.ai/docs/api/chat
  https://platform.kimi.ai/docs/api/overview
- Google Gemini OpenAI compatibility:
  https://ai.google.dev/gemini-api/docs/openai
- Alibaba Qwen/DashScope OpenAI-compatible Chat, structured output, and
  thinking:
  https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions
  https://help.aliyun.com/zh/model-studio/qwen-structured-output
  https://help.aliyun.com/zh/model-studio/deep-thinking

OpenAI's published official SDK list does not include Rust at this verification
date. The user-proposed community `openai_rust_sdk` was evaluated but is not a
project dependency: its current release requires a newer Rust toolchain and
owns its HTTP client, so adopting it would bypass the Provider transport
controls above. Wire-format compatibility remains in project-owned adapters;
all network execution remains in `ProviderTransport`.

The normalized implementation matrix and verification boundary are maintained
in `provider-compatibility.md`. Provider documentation changes do not silently
change stored rows: update the typed preset, local fake contract, Admin text,
and release evidence together.

### 9.2 Automatic-memory trust references

Primary OpenAI references used by ADR-0014 and ADR-0015:

- Codex local memories, background generation, source controls, local files,
  and supporting evidence:
  https://learn.chatgpt.com/docs/customization/memories
- Long-term memory lifecycle guidance covering source-grounded distillation,
  non-invention, deduplication, conflict resolution, forgetting, and evals:
  https://developers.openai.com/cookbook/examples/agents_sdk/context_personalization

These references do not define MCP Vault's wire contract. They support the
design conclusion that model self-scored confidence is not trust evidence and
that routine memory generation must not depend on per-item human approval.

## 10. Markdown parsing

Candidate AST parser:

- Comrak
  https://github.com/kivikakk/comrak

Comrak covers CommonMark and GFM. Obsidian wikilinks, embeds, block references, and tag behavior require project-owned parsing and fixtures outside code spans/blocks.

## 11. Reference policy

- Prefer official protocol specifications and project repositories.
- Pin versions in lockfiles.
- Record a compatibility decision in an ADR when behavior differs from a standard or tested client.
- Never advertise protocol compatibility that is not exercised by CI/release tests.
- Update this document’s verification date when the protocol baseline changes.

## 12. Backup and observability libraries

WP-13 pins and uses the following primary library references:

- Rust `tar` 0.4.45 for the portable container format:
  https://docs.rs/tar/0.4.45/tar/
- OpenTelemetry Rust 0.31 and OTLP HTTP exporter 0.31:
  https://docs.rs/opentelemetry/0.31.0/opentelemetry/
  https://docs.rs/opentelemetry-otlp/0.31.0/opentelemetry_otlp/
- Prometheus text exposition format:
  https://prometheus.io/docs/instrumenting/exposition_formats/
- Syft SBOM generator:
  https://github.com/anchore/syft
- Trivy image scanner:
  https://github.com/aquasecurity/trivy

The tar container is only packaging. The project-owned backup service validates
every entry, manifest path, size, type, and checksum before extraction or
publication; it does not rely on archive extraction defaults for safety. OTLP
export remains disabled unless `MCP_VAULT_OTEL_ENDPOINT` is explicitly set,
and metrics labels are fixed rather than derived from request paths, Vault
identities, or credentials.
