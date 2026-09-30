# M6 Holdout 30 独立智能体 pre-run gold 复核包

> 状态：未复核、未冻结；供独立复核者使用。生成包不代表任务、gold 或 M6 已通过。
> 复核主体：独立主代理；candidate producer 不得自评。输出标记 `agent_reviewed`，`human_review=false`。

- candidate SHA-256：`bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2`
- 固定 commit：`e046ceb59393db9a4977ba55899eba301ad28005`
- 任务数：30 holdout（development 30 不进入当前 live task set）
- 所有摘录均由该 commit 的 Git blob 生成；长行截断仅用于阅读，必须按 blob/SHA 回看原行。
- v4 按独立复核意见补充状态、来源证据和限定；全部仍 pending independent-agent-review，本包不作批准，ADR-0038 要求 `human_review=false`。

## 评审记录规则

每题分别判断 query 可回答性、重点/限定/禁止推断/状态、关系类型与方向、严重度及证据覆盖。对无答案任务，必须逐行检查列出的整个 ADR；标题和章节导航摘录不能证明不存在答案。证据不足标 `insufficient_evidence`，不得猜测。记录建议修订但不要改写冻结候选。

## Holdout 任务

### T-S16-1

- 风险标记：severity-high, qualifier-negation-review
- 来源：S16, S17；split：holdout；severity：`high`
- query：Under ADR-0015's historical classification rule, which categories could be materialized without note markers?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：At that time, automatic materialization was limited to intrinsically personal or temporal classes (owner identity/preference/relationship, accepted project decisions, current project progress, significant events); exact source evidence checks still applied.
  - 必须保留：At that time, automatic materialization was limited to intrinsically personal or temporal classes (owner identity/preference/relationship, accepted project decisions, current project progress, significant events); exact source evidence checks still applied.；This describes ADR-0015's historical classification rule; ADR-0026 later superseded that classification while retaining marker-free Vault-level admission and source-evidence constraints.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:29-48`; blob `8cf902e61afe0eba7aabd5767a1fb755c7c8e85c`; SHA-256 `4a25ea5436e2042e11778c927649b3c05faafbfa68355d231767529f4fed4b4c`; basis `direct_text`
  - 引用范围还有 8 行未摘录；须回看该范围。
> L29: Automatic memory is enabled once per Vault. There is one serialized source
> L30: mode, `automatic`; legacy `explicit_only` and `all_notes` values deserialize as
> L31: migration aliases for `automatic`. No note path, frontmatter key, tag, or
> L32: folder convention is required.
> L33:&#32;
> L34: Every eligible non-managed Markdown create/update/move/restore may enter the
> L35: durable extraction worker. The Provider still must return an exact bounded
> L36: source quote and current line range, and all ADR-0014 local validation remains
> L37: mandatory.
> L38:&#32;
> L39: Automatic note-derived materialization is additionally restricted to classes
> L40: that are intrinsically personal or temporal:
- `docs/adr/0016-use-codex-style-two-phase-memory.md:3-9`; blob `064b2d135106a5412d6fa706d4c2f296bb2015e1`; SHA-256 `03240f67dfc6b1b3881d50829627b9cca11236bc299c298db342c936989905a0`; basis `B_comparison_source_context`
> L3: - Status: Superseded by ADR-0026
> L4: - Date: 2026-08-25
> L5: - Supersedes: the direct automatic-promotion portions of ADR-0014 and ADR-0015
> L6: - Upgrade behavior amended by: ADR-0017
> L7: - Source-health and stale-input behavior amended by: ADR-0022
> L8: - Current memory ownership, extraction, deletion, and retrieval superseded by:
> L9:   ADR-0026
- `docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:10-11`; blob `8cf902e61afe0eba7aabd5767a1fb755c7c8e85c`; SHA-256 `4a25ea5436e2042e11778c927649b3c05faafbfa68355d231767529f4fed4b4c`; basis `classification_superseded_by_adr26`
> L10: - Automatic knowledge classification superseded by ADR-0026, which retains
> L11:   marker-free Vault-level admission but allows useful sourced knowledge.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S16-2

- 风险标记：cross-source-relation-review, qualifier-negation-review
- 来源：S16, S17；split：holdout；severity：`medium`
- query：Did ADR-0015 retain per-note markers, and which part of it did ADR-0016 supersede?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：No note markers were required. ADR-0016 superseded only ADR-0015's direct automatic-promotion portions, while retaining marker-free Vault-level admission and exact supporting evidence.
  - 必须保留：No note markers were required. ADR-0016 superseded only ADR-0015's direct automatic-promotion portions, while retaining marker-free Vault-level admission and exact supporting evidence.；Keep the narrow supersession scope: direct automatic promotion was superseded; marker-free Vault-level admission and exact supporting evidence were retained.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - kind `supersedes`；candidate 端点顺序 `S17 → S16`（该顺序本身不是证据）；须从 ADR 声明核对方向、范围及任务用途。
- 冻结来源证据：
- `docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:5-11`; blob `8cf902e61afe0eba7aabd5767a1fb755c7c8e85c`; SHA-256 `4a25ea5436e2042e11778c927649b3c05faafbfa68355d231767529f4fed4b4c`; basis `direct_text`
> L5: - Supersedes: ADR-0014's per-note `mcp-vault-memory` source-admission rule.
> L6: - Retains: ADR-0014's exact-evidence, no-self-score, autonomous-promotion
> L7:   rules.
> L8: - Amended by: ADR-0016 supersedes direct automatic promotion while retaining
> L9:   marker-free Vault-level admission and exact supporting evidence.
> L10: - Automatic knowledge classification superseded by ADR-0026, which retains
> L11:   marker-free Vault-level admission but allows useful sourced knowledge.
- `docs/adr/0016-use-codex-style-two-phase-memory.md:3-9`; blob `064b2d135106a5412d6fa706d4c2f296bb2015e1`; SHA-256 `03240f67dfc6b1b3881d50829627b9cca11236bc299c298db342c936989905a0`; basis `explicit_adr_relation_header`
> L3: - Status: Superseded by ADR-0026
> L4: - Date: 2026-08-25
> L5: - Supersedes: the direct automatic-promotion portions of ADR-0014 and ADR-0015
> L6: - Upgrade behavior amended by: ADR-0017
> L7: - Source-health and stale-input behavior amended by: ADR-0022
> L8: - Current memory ownership, extraction, deletion, and retrieval superseded by:
> L9:   ADR-0026
- `docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:3-9`; blob `8cf902e61afe0eba7aabd5767a1fb755c7c8e85c`; SHA-256 `4a25ea5436e2042e11778c927649b3c05faafbfa68355d231767529f4fed4b4c`; basis `explicit_adr_relation_header`
> L3: - Status: Accepted
> L4: - Date: 2026-08-25
> L5: - Supersedes: ADR-0014's per-note `mcp-vault-memory` source-admission rule.
> L6: - Retains: ADR-0014's exact-evidence, no-self-score, autonomous-promotion
> L7:   rules.
> L8: - Amended by: ADR-0016 supersedes direct automatic promotion while retaining
> L9:   marker-free Vault-level admission and exact supporting evidence.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S17-1

- 风险标记：severity-high, qualifier-negation-review
- 来源：S17；split：holdout；severity：`high`
- query：What two phases did ADR-0016 specify?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Phase 1 creates sourced raw-memory inputs; Phase 2 is a separate Vault-scoped durable consolidation job validating create/update/keep/supersede/archive/drop decisions.
  - 必须保留：Phase 1 creates sourced raw-memory inputs; Phase 2 is a separate Vault-scoped durable consolidation job validating create/update/keep/supersede/archive/drop decisions.；These are the two phases specified by historical ADR-0016; ADR-0026 later superseded its current memory ownership/extraction/deletion/retrieval architecture.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0016-use-codex-style-two-phase-memory.md:3-13`; blob `064b2d135106a5412d6fa706d4c2f296bb2015e1`; SHA-256 `03240f67dfc6b1b3881d50829627b9cca11236bc299c298db342c936989905a0`; basis `status_and_scope`
> L3: - Status: Superseded by ADR-0026
> L4: - Date: 2026-08-25
> L5: - Supersedes: the direct automatic-promotion portions of ADR-0014 and ADR-0015
> L6: - Upgrade behavior amended by: ADR-0017
> L7: - Source-health and stale-input behavior amended by: ADR-0022
> L8: - Current memory ownership, extraction, deletion, and retrieval superseded by:
> L9:   ADR-0026
> L10:&#32;
> L11: &gt; ADR-0017 supersedes the prerelease preservation/conversion paragraph below.
> L12: &gt; The two-phase architecture remains accepted; current cutovers discard all old
> L13: &gt; memory state and jobs before fresh regeneration.
- `docs/adr/0016-use-codex-style-two-phase-memory.md:39-58`; blob `064b2d135106a5412d6fa706d4c2f296bb2015e1`; SHA-256 `03240f67dfc6b1b3881d50829627b9cca11236bc299c298db342c936989905a0`; basis `direct_text`
  - 引用范围还有 8 行未摘录；须回看该范围。
> L39: MCP Vault will use the Codex two-phase memory architecture.
> L40:&#32;
> L41: Phase 1 distills one eligible Vault note revision or explicit remember input
> L42: into a sourced raw-memory record containing semantic `raw_memory`, a detailed
> L43: summary, a stable slug, and application-derived source provenance. It may
> L44: produce a no-op. Phase 1 never creates or updates final `memories` rows, final
> L45: FTS, or final canonical memory content.
> L46:&#32;
> L47: Phase 2 runs as a separate Vault-scoped durable job using the
> L48: `memory_consolidation` model binding. It consumes dirty Phase 1 records together
> L49: with current global memory and proposes a complete set of validated create,
> L50: update, keep, supersede, archive, and drop decisions. It performs semantic
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S17-2

- 风险标记：qualifier-negation-review
- 来源：S17；split：holdout；severity：`medium`
- query：Is ADR-0016's old global consolidation architecture current, and what does its own status say was superseded?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：ADR-0016's current memory ownership, extraction, deletion, and retrieval architecture is superseded by ADR-0026, as its status lines 3 and 8 say. Lines 11-13 are an older note about ADR-0017 superseding a prerelease paragraph; they do not establish the two-phase architecture as current.
  - 必须保留：Keep ADR-0016's chronology clear: its status at lines 3 and 8 supersedes current memory ownership/extraction/deletion/retrieval by ADR-0026; the lines 11-13 note concerns ADR-0017 and a prerelease paragraph, not a current-runtime exception.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0016-use-codex-style-two-phase-memory.md:3-13`; blob `064b2d135106a5412d6fa706d4c2f296bb2015e1`; SHA-256 `03240f67dfc6b1b3881d50829627b9cca11236bc299c298db342c936989905a0`; basis `direct_text`
> L3: - Status: Superseded by ADR-0026
> L4: - Date: 2026-08-25
> L5: - Supersedes: the direct automatic-promotion portions of ADR-0014 and ADR-0015
> L6: - Upgrade behavior amended by: ADR-0017
> L7: - Source-health and stale-input behavior amended by: ADR-0022
> L8: - Current memory ownership, extraction, deletion, and retrieval superseded by:
> L9:   ADR-0026
> L10:&#32;
> L11: &gt; ADR-0017 supersedes the prerelease preservation/conversion paragraph below.
> L12: &gt; The two-phase architecture remains accepted; current cutovers discard all old
> L13: &gt; memory state and jobs before fresh regeneration.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S18-1

- 风险标记：severity-high, qualifier-negation-review
- 来源：S18；split：holdout；severity：`high`
- query：Under the superseded ADR-0022 source-health design, what happened when a memory lost its last current note source?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Under that historical design, the health projection marked the memory stale/source_unavailable and removed it from normal recall; historical recall remained available.
  - 必须保留：Under that historical design, the health projection marked the memory stale/source_unavailable and removed it from normal recall; historical recall remained available.；This describes superseded ADR-0022 behavior, not the current runtime policy.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0022-continuous-exact-memory-source-health.md:1-7`; blob `e7d17d46f5f3a50ea20310495c9f24ba1c07f61f`; SHA-256 `3fff6b508a349b956d5dc3496b73b0715110509e7b1df0f23b0a45ef0a56901b`; basis `status_and_supersession`
> L1: # ADR-0022: Continuously verify memory sources with exact Vault-scoped evidence
> L2:&#32;
> L3: - Status: Superseded by ADR-0026
> L4: - Date: 2026-09-02
> L5: - Amends: ADR-0007 and ADR-0016
> L6: - Replaced by: ADR-0026 stable File-ID/source-hash current-set validation.
> L7:&#32;
- `docs/adr/0022-continuous-exact-memory-source-health.md:24-41`; blob `e7d17d46f5f3a50ea20310495c9f24ba1c07f61f`; SHA-256 `3fff6b508a349b956d5dc3496b73b0715110509e7b1df0f23b0a45ef0a56901b`; basis `direct_text`
  - 引用范围还有 6 行未摘录；须回看该范围。
> L24: MCP Vault maintains a rebuildable, Vault-scoped `memory_source_health`
> L25: projection for every final note source. Its states are `unverified`, `current`,
> L26: `content_changed`, `deleted`, `identity_missing`, and
> L27: `identity_ambiguous`. A current row records the resolved File ID and path, the
> L28: checked revision, and the raw current file hash accepted by the exact evidence
> L29: check.
> L30:&#32;
> L31: Every memory containing at least one note source requires at least one current
> L32: note source to remain active, regardless of whether its origin is extracted,
> L33: Agent, Admin, import, or managed Markdown. Explicit Agent/Admin memories with
> L34: no note source remain supported by the explicit assertion itself. When the last
> L35: current note source disappears, the memory becomes `stale` with
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S18-2

- 风险标记：no-answer-full-document-check, severity-high, qualifier-negation-review
- 来源：S18；split：holdout；severity：`high`
- query：Which post-event retry or backoff sequence does ADR-0022 specify for memory.source_reconcile?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：（空）
  - 必须保留：ADR-0022 says source events enqueue reconciliation but gives no retry or backoff sequence.；Source invalidation remains local and exact rather than depending on Provider availability.；ADR-0022 is superseded by ADR-0026 for current memory ownership and source-health behavior; this question is limited to the older ADR-0022 retry/backoff details.
  - 禁止推断：ADR-0022 does not specify the retry/backoff count, delay, or failure-trigger conditions after an event has enqueued reconciliation.
  - 正确状态：`no_answer`；no-answer：`true`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0022-continuous-exact-memory-source-health.md:43-46`; blob `e7d17d46f5f3a50ea20310495c9f24ba1c07f61f`; SHA-256 `3fff6b508a349b956d5dc3496b73b0715110509e7b1df0f23b0a45ef0a56901b`; basis `direct_text`
> L43: File create, update, move, delete, restore, and external-change events enqueue
> L44: `memory.source_reconcile`. The reconciler commits source health and lifecycle
> L45: before it admits optional Phase 1 work. A same-File-ID move with unchanged
> L46: evidence updates navigation only and invokes no Provider.
- `docs/adr/0022-continuous-exact-memory-source-health.md:101-103`; blob `e7d17d46f5f3a50ea20310495c9f24ba1c07f61f`; SHA-256 `3fff6b508a349b956d5dc3496b73b0715110509e7b1df0f23b0a45ef0a56901b`; basis `direct_text`
> L101: ### Let Provider availability control source invalidation
> L102:&#32;
> L103: Rejected because recall safety must remain deterministic and local.
- `docs/adr/0022-continuous-exact-memory-source-health.md:1-103`; blob `e7d17d46f5f3a50ea20310495c9f24ba1c07f61f`; SHA-256 `3fff6b508a349b956d5dc3496b73b0715110509e7b1df0f23b0a45ef0a56901b`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0022: Continuously verify memory sources with exact Vault-scoped evidence
> L8: ## Context
> L22: ## Decision
> L71: ## Consequences
> L85: ## Rejected alternatives
> L87: ### Keep a one-time repair job
> L91: ### Match by path, filename, vector similarity, or LLM judgment
> L96: ### Delete stale memory automatically
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S19-1

- 风险标记：severity-high, qualifier-negation-review
- 来源：S19；split：holdout；severity：`high`
- query：What language is canonical and what is the role of search aliases?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Canonical memory keeps the supporting source’s primary language. Validated aliases are derived search-only projections, not facts/provenance, and normal recall does not return them.
  - 必须保留：Canonical memory keeps the supporting source’s primary language. Validated aliases are derived search-only projections, not facts/provenance, and normal recall does not return them.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md:26-45`; blob `f7f82abbf94a4365ce8a54f0f41e3c5c16e13a6d`; SHA-256 `10b5beda5638bee9de42b7231be7ab9c75c26dfc065f7e0f1860735353e67e08`; basis `direct_text`
  - 引用范围还有 8 行未摘录；须回看该范围。
> L26: Canonical memory content remains one concise proposition in the primary
> L27: language of its supporting source. Phase 1 preserves the note's primary
> L28: language, and Phase 2 preserves the language of current memory on updates while
> L29: using the supporting raw inputs' primary language for creates.
> L30:&#32;
> L31: MCP Vault maintains Vault-scoped, derived multilingual retrieval metadata for
> L32: each eligible memory/content hash. The metadata contains a validated BCP-47
> L33: source language and bounded search aliases for the source language, Simplified
> L34: Chinese (`zh-Hans`), and English (`en`). Aliases are search-only projections:
> L35: they are not facts, provenance, confidence, or canonical memory body content.
> L36: They are never returned by normal recall and may be regenerated.
> L37:&#32;
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S19-2

- 风险标记：no-answer-full-document-check, qualifier-negation-review
- 来源：S19；split：holdout；severity：`medium`
- query：What production evaluation result in ADR-0023 demonstrates that alias enrichment reduced real cross-language recall misses after rollout?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：（空）
  - 必须保留：ADR-0023 gives the retrieval problem, alias-enrichment design, and expected consequences, but no measured post-rollout result.
  - 禁止推断：Do not treat an accepted design or anticipated consequence as evidence of deployment, causality, or measured recall improvement.
  - 正确状态：`no_answer`；no-answer：`true`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md:9-16`; blob `f7f82abbf94a4365ce8a54f0f41e3c5c16e13a6d`; SHA-256 `10b5beda5638bee9de42b7231be7ab9c75c26dfc065f7e0f1860735353e67e08`; basis `direct_text`
> L9: Phase 1 and Phase 2 use English system prompts without an output-language
> L10: contract. Providers may therefore turn Chinese source material into English
> L11: durable memories. Normal `recall` forwards the caller's natural-language query
> L12: unchanged, builds a whitespace-delimited FTS expression, and optionally adds a
> L13: query embedding. A Chinese request can consequently miss an English memory
> L14: when memory embeddings are unavailable or not reliably cross-lingual. The same
> L15: FTS shape also treats an unspaced Chinese sentence as one token, so paraphrases
> L16: can miss even when query and memory use the same language.
- `docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md:38-42`; blob `f7f82abbf94a4365ce8a54f0f41e3c5c16e13a6d`; SHA-256 `10b5beda5638bee9de42b7231be7ab9c75c26dfc065f7e0f1860735353e67e08`; basis `direct_text`
> L38: Alias generation runs as a separate durable `memory.enrich_retrieval` job using
> L39: the existing `memory_consolidation` binding. New or changed memories admit the
> L40: job automatically. Existing active, stale, and superseded memories are
> L41: backfilled only after an authenticated Admin explicitly requests it. Normal
> L42: recall never waits for this job and never invokes an LLM.
- `docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md:63-76`; blob `f7f82abbf94a4365ce8a54f0f41e3c5c16e13a6d`; SHA-256 `10b5beda5638bee9de42b7231be7ab9c75c26dfc065f7e0f1860735353e67e08`; basis `direct_text`
  - 引用范围还有 2 行未摘录；须回看该范围。
> L63: ## Consequences
> L64:&#32;
> L65: - Cross-language recall can work while the embedding Provider is unavailable,
> L66:   once persisted aliases cover the current memory hash.
> L67: - Alias coverage is asynchronous and explicit in recall/Admin diagnostics;
> L68:   missing coverage degrades retrieval but never hides or corrupts canonical
> L69:   memory.
> L70: - Existing backfill incurs bounded paid model calls only after Admin
> L71:   confirmation. New memory enrichment adds a separate batch call whose failure
> L72:   cannot roll back Phase 2.
> L73: - Search metadata is derived SQLite state and can be rebuilt. Canonical memory
> L74:   bodies and their revisions remain portable Markdown.
- `docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md:1-99`; blob `f7f82abbf94a4365ce8a54f0f41e3c5c16e13a6d`; SHA-256 `10b5beda5638bee9de42b7231be7ab9c75c26dfc065f7e0f1860735353e67e08`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0023: Preserve source language and persist multilingual retrieval metadata
> L7: ## Context
> L24: ## Decision
> L63: ## Consequences
> L78: ## Rejected alternatives
> L80: ### Store every memory body bilingually
> L85: ### Translate each recall query with an LLM
> L90: ### Depend only on multilingual embeddings
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S20-1

- 风险标记：severity-high, qualifier-negation-review
- 来源：S20；split：holdout；severity：`high`
- query：How are note-vector inputs bounded?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：The Index service creates versioned text-v2 chunks bounded by UTF-8 bytes and valid character boundaries; adapters do not split an oversized logical source into multiple billable calls.
  - 必须保留：The Index service creates versioned text-v2 chunks bounded by UTF-8 bytes and valid character boundaries; adapters do not split an oversized logical source into multiple billable calls.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:24-43`; blob `bb6f8e5b54d8a3c29c604c7c32bc3ce741ad13b3`; SHA-256 `71fbe1fa6574d1174739a51fe0e6ae597d8385e26878af32a19a7b5c4be69723`; basis `direct_text`
  - 引用范围还有 8 行未摘录；须回看该范围。
> L24: The Index application service owns deterministic versioned note-vector chunks.
> L25: The complete text passed for each `text-v2` input, including bounded metadata
> L26: context, is limited by UTF-8 bytes and snapped only at valid character
> L27: boundaries. Provider adapters do not hide an oversized logical source by
> L28: issuing multiple billable calls and pooling their vectors.
> L29:&#32;
> L30: Embedding job identities include a project-owned projection version. An
> L31: incompatible derived-profile change therefore admits new jobs without mutating
> L32: or silently repurposing historical terminal jobs. Job payloads remain
> L33: reference-only and resolve current source text at execution.
> L34:&#32;
> L35: Embedding workers retain stable redacted Provider error categories. Admin job
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S20-2

- 风险标记：no-answer-full-document-check, qualifier-negation-review
- 来源：S20；split：holdout；severity：`medium`
- query：Which post-rollout field evidence in ADR-0024 shows that the UTF-8 byte envelope eliminated embedding failures without degrading note recall?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：（空）
  - 必须保留：ADR-0024 records a pre-change failure and specifies a byte-bounded design, but reports no post-rollout cross-Provider failure or retrieval evaluation.
  - 禁止推断：Do not infer that the implementation eliminated failures across Providers or preserved recall quality from the design and expected consequences alone.
  - 正确状态：`no_answer`；no-answer：`true`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:9-20`; blob `bb6f8e5b54d8a3c29c604c7c32bc3ce741ad13b3`; SHA-256 `71fbe1fa6574d1174739a51fe0e6ae597d8385e26878af32a19a7b5c4be69723`; basis `direct_text`
> L9: The original note-vector projection bounded chunks by Unicode character count.
> L10: A chunk could contain 6,000 characters plus 2,048 context characters. Zhipu
> L11: `embedding-3` accepts no more than 3,072 tokens for one input, and a live Vault
> L12: note reproduced a non-retryable Provider failure while a short query succeeded.
> L13: One over-limit member rejects the complete embedding batch.
> L14:&#32;
> L15: The Admin index rebuild is deliberately separate from its asynchronously
> L16: scheduled vector jobs. It may therefore complete while semantic coverage stays
> L17: empty. Failed jobs expose only `embedding_rebuild_failed`, and their persisted
> L18: model ID does not change when an Admin selects a different binding. Note model
> L19: binding schedules current chunks, but memory model binding has no equivalent
> L20: existing-record backfill.
- `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:24-28`; blob `bb6f8e5b54d8a3c29c604c7c32bc3ce741ad13b3`; SHA-256 `71fbe1fa6574d1174739a51fe0e6ae597d8385e26878af32a19a7b5c4be69723`; basis `direct_text`
> L24: The Index application service owns deterministic versioned note-vector chunks.
> L25: The complete text passed for each `text-v2` input, including bounded metadata
> L26: context, is limited by UTF-8 bytes and snapped only at valid character
> L27: boundaries. Provider adapters do not hide an oversized logical source by
> L28: issuing multiple billable calls and pooling their vectors.
- `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:56-65`; blob `bb6f8e5b54d8a3c29c604c7c32bc3ce741ad13b3`; SHA-256 `71fbe1fa6574d1174739a51fe0e6ae597d8385e26878af32a19a7b5c4be69723`; basis `direct_text`
> L56: - Long multilingual notes generate more, smaller vector chunks and remain
> L57:   semantically searchable with lower-limit Providers.
> L58: - A chunk-profile upgrade invalidates only derived vector identity; canonical
> L59:   notes, memories, revisions, aliases, and FTS remain untouched.
> L60: - Model changes and explicit rebuilds use the newly selected model rather than
> L61:   retrying an old task with its persisted model ID.
> L62: - More chunks may increase embedding request count and vector storage. Bounded
> L63:   batches and the existing maximum-chunk cap contain that cost.
> L64: - The UTF-8 byte envelope is conservative because vendor tokenizers differ; it
> L65:   favors reliable offline rebuild over maximum per-request utilization.
- `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:1-83`; blob `bb6f8e5b54d8a3c29c604c7c32bc3ce741ad13b3`; SHA-256 `71fbe1fa6574d1174739a51fe0e6ae597d8385e26878af32a19a7b5c4be69723`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0024: Bound embedding inputs and rebuild current-model vectors
> L7: ## Context
> L22: ## Decision
> L45: ## Consequences
> L67: ## Rejected alternatives
> L69: ### Retry the old job after changing the binding
> L75: ### Split oversized inputs inside the Provider adapter and average vectors
> L80: ### Re-run all memory extraction to obtain vectors
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S21-1

- 风险标记：qualifier-negation-review
- 来源：S21, S20；split：holdout；severity：`medium`
- query：How does note ranking stop many chunks from one note dominating results?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：The service over-fetches a bounded pool, validates current source identity/hash, and aggregates by File ID; only the first valid in-scope chunk supplies the note result.
  - 必须保留：The service over-fetches a bounded pool, validates current source identity/hash, and aggregates by File ID; only the first valid in-scope chunk supplies the note result.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0025-aggregate-note-vector-chunks-before-ranking.md:24-43`; blob `e004ea2b38a24c0b4bafc405cd1d95d4e10065ef`; SHA-256 `7d5fea2c72c9e5bf30acb464b41f3f821d4caafeedfdb761496a0dda89b493f6`; basis `direct_text`
  - 引用范围还有 8 行未摘录；须回看该范围。
> L24: Ordinary-note semantic retrieval treats vector rows as an over-fetched,
> L25: bounded candidate pool and performs current-source validation plus object
> L26: aggregation in the Index application service. The candidate limit is
> L27: `min(10_000, requested_note_pool * MAX_NOTE_EMBEDDING_CHUNKS)`. Vault, model,
> L28: dimension, and exact `object_type = note` filtering remain below this boundary.
> L29:&#32;
> L30: Candidates are processed in descending cosine order. Negative similarities are
> L31: discarded. A candidate must match the current `FileId`, chunk key, and chunk
> L32: content hash. The first valid, in-scope candidate for a File ID is that note's
> L33: winning chunk; subsequent chunks for the same note neither consume a result
> L34: slot nor advance semantic rank. Equal vector scores use object ID, chunk key,
> L35: then embedding ID as deterministic tie breakers.
- `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:3-9`; blob `bb6f8e5b54d8a3c29c604c7c32bc3ce741ad13b3`; SHA-256 `71fbe1fa6574d1174739a51fe0e6ae597d8385e26878af32a19a7b5c4be69723`; basis `B_comparison_source_context`
> L3: - Status: Accepted
> L4: - Date: 2026-09-04
> L5: - Amends: ADR-0010, ADR-0013, and ADR-0023
> L6:&#32;
> L7: ## Context
> L8:&#32;
> L9: The original note-vector projection bounded chunks by Unicode character count.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S21-2

- 风险标记：cross-source-relation-review, qualifier-negation-review
- 来源：S21, S20；split：holdout；severity：`medium`
- query：For note ranking, what does ADR-0025 refine about ADR-0024, and what happens to negative-cosine chunks? Does this replace ADR-0024's UTF-8 input cap?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：ADR-0025 refines ranking by aggregating current-valid chunks per File ID in a bounded candidate pool; negative similarities are discarded and cannot advance rank. It does not replace ADR-0024's UTF-8 input cap.
  - 必须保留：ADR-0025 refines ranking by aggregating current-valid chunks per File ID in a bounded candidate pool; negative similarities are discarded and cannot advance rank. It does not replace ADR-0024's UTF-8 input cap.；Distinguish the ADR-0025 ranking refinement from ADR-0024's retained UTF-8 input bound; do not treat negative cosine as a positive ranking contribution.
  - 禁止推断：不得把“refines”解释为取代 ADR-0024 的 UTF-8 输入上限；不得把负 cosine 当作正向排名贡献。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - kind `refines`；candidate 端点顺序 `S21 → S20`（该顺序本身不是证据）；须从 ADR 声明核对方向、范围及任务用途。
- 冻结来源证据：
- `docs/adr/0025-aggregate-note-vector-chunks-before-ranking.md:5-5`; blob `e004ea2b38a24c0b4bafc405cd1d95d4e10065ef`; SHA-256 `7d5fea2c72c9e5bf30acb464b41f3f821d4caafeedfdb761496a0dda89b493f6`; basis `explicit_adr_relation_header`
> L5: - Refines: ADR-0010, ADR-0013, ADR-0023, and ADR-0024
- `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:24-28`; blob `bb6f8e5b54d8a3c29c604c7c32bc3ce741ad13b3`; SHA-256 `71fbe1fa6574d1174739a51fe0e6ae597d8385e26878af32a19a7b5c4be69723`; basis `relation_endpoint_context`
> L24: The Index application service owns deterministic versioned note-vector chunks.
> L25: The complete text passed for each `text-v2` input, including bounded metadata
> L26: context, is limited by UTF-8 bytes and snapped only at valid character
> L27: boundaries. Provider adapters do not hide an oversized logical source by
> L28: issuing multiple billable calls and pooling their vectors.
- `docs/adr/0025-aggregate-note-vector-chunks-before-ranking.md:24-35`; blob `e004ea2b38a24c0b4bafc405cd1d95d4e10065ef`; SHA-256 `7d5fea2c72c9e5bf30acb464b41f3f821d4caafeedfdb761496a0dda89b493f6`; basis `direct_refinement_and_ranking_rule`
> L24: Ordinary-note semantic retrieval treats vector rows as an over-fetched,
> L25: bounded candidate pool and performs current-source validation plus object
> L26: aggregation in the Index application service. The candidate limit is
> L27: `min(10_000, requested_note_pool * MAX_NOTE_EMBEDDING_CHUNKS)`. Vault, model,
> L28: dimension, and exact `object_type = note` filtering remain below this boundary.
> L29:&#32;
> L30: Candidates are processed in descending cosine order. Negative similarities are
> L31: discarded. A candidate must match the current `FileId`, chunk key, and chunk
> L32: content hash. The first valid, in-scope candidate for a File ID is that note's
> L33: winning chunk; subsequent chunks for the same note neither consume a result
> L34: slot nor advance semantic rank. Equal vector scores use object ID, chunk key,
> L35: then embedding ID as deterministic tie breakers.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S22-1

- 风险标记：qualifier-negation-review
- 来源：S22；split：holdout；severity：`medium`
- query：How does note-derived ownership differ from explicit memory ownership?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：A note-derived item belongs to one source/current set. Explicit memory belongs to the authenticated assertion and survives a referenced note’s change/deletion.
  - 必须保留：A note-derived item belongs to one source/current set. Explicit memory belongs to the authenticated assertion and survives a referenced note’s change/deletion.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0026-current-source-owned-memory-sets.md:38-57`; blob `6d9aa9816fd808ecf5ee7a71fea3a047f8c4ef54`; SHA-256 `2a36b0c8cebf0e80c021ea607eb733901fbb1ce0e69677b472ca77fbff234afd`; basis `direct_text`
  - 引用范围还有 8 行未摘录；须回看该范围。
> L38: MCP Vault has two current memory ownership classes.
> L39:&#32;
> L40: 1. A note-derived memory item belongs to exactly one source note. All items for
> L41:    that note form one current memory set identified by Vault and stable File ID,
> L42:    bound to the exact current source-content hash. The set is materialized as
> L43:    one managed Markdown file and replaced in full after a successful bounded
> L44:    extraction.
> L45: 2. An explicit memory belongs to the authenticated user/Agent assertion. It is
> L46:    immediately materialized as its own Markdown record and does not disappear
> L47:    when any source note changes or is deleted. Attaching a note reference does
> L48:    not change this ownership. Converting a derived item to explicit ownership
> L49:    is a separate authorized operation.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S22-2

- 风险标记：no-answer-full-document-check, severity-critical, qualifier-negation-review
- 来源：S22；split：holdout；severity：`critical`
- query：What recovery-retention period and restore procedure does ADR-0026 guarantee for a forgotten current memory in offline backups?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：（空）
  - 必须保留：ADR-0026 says forget deletes current memory and projections; Core revisions, audit, and offline backups are separate controls, not memory query sources.；The ADR specifies no backup retention period or restore guarantee for forgotten memory.
  - 禁止推断：Do not promise a retention duration, snapshot count, or user restore procedure from the mention of separate backup controls.
  - 正确状态：`no_answer`；no-answer：`true`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0026-current-source-owned-memory-sets.md:51-57`; blob `6d9aa9816fd808ecf5ee7a71fea3a047f8c4ef54`; SHA-256 `2a36b0c8cebf0e80c021ea607eb733901fbb1ce0e69677b472ca77fbff234afd`; basis `direct_text`
> L51: There is no model-readable memory history. Normal and detail reads, list,
> L52: recall, resources, known IDs, indexes, vectors, and context summaries expose
> L53: only current published data. A successful forget operation deletes the current
> L54: canonical memory and its projections rather than transitioning to archived.
> L55: It returns identifiers and effects, not deleted content. Retained Vault Core
> L56: revision history, audit, and offline backups are separate recovery controls and
> L57: are never memory query sources.
- `docs/adr/0026-current-source-owned-memory-sets.md:137-143`; blob `6d9aa9816fd808ecf5ee7a71fea3a047f8c4ef54`; SHA-256 `2a36b0c8cebf0e80c021ea607eb733901fbb1ce0e69677b472ca77fbff234afd`; basis `direct_text`
> L137: Canonical cleanup and conversion run only after a backup/preflight confirmation
> L138: and use Vault Core. Old `MEMORY.md`, `memory_summary.md`, `raw_memories.md`,
> L139: source summaries, Stage 1 rows, consolidation proposals, source-health rows,
> L140: supersession relations, and legacy jobs never become current-query inputs after
> L141: the cutover. The old tables may temporarily remain as isolated migration input
> L142: but are not a second runtime engine.
> L143:&#32;
- `docs/adr/0026-current-source-owned-memory-sets.md:1-202`; blob `6d9aa9816fd808ecf5ee7a71fea3a047f8c4ef54`; SHA-256 `2a36b0c8cebf0e80c021ea607eb733901fbb1ce0e69677b472ca77fbff234afd`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0026: Use current source-owned memory sets
> L13: ## Context
> L36: ## Decision
> L126: ## Migration and recovery
> L149: ## Consequences
> L174: ## Rejected alternatives
> L176: ### Keep lifecycle history and only change default filters
> L181: ### Keep Phase 2 but constrain its actions
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S23-1

- 风险标记：cross-source-relation-review, qualifier-negation-review
- 来源：S23, S24；split：holdout；severity：`medium`
- query：How does ADR-0028 amend ADR-0027's automatic activation gate, and what is bundled evaluation's current role?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：ADR-0027 historically made a passing bundled benchmark a semantic-admission prerequisite. ADR-0028 amends that gate: bundled evaluation is a development/operator diagnostic, not a production prerequisite; current source eligibility, permissions, vector identity/dimensions, and bounded ranking still apply.
  - 必须保留：ADR-0027 historically made a passing bundled benchmark a semantic-admission prerequisite. ADR-0028 amends that gate: bundled evaluation is a development/operator diagnostic, not a production prerequisite; current source eligibility, permissions, vector identity/dimensions, and bounded ranking still apply.；This is the ADR-0028 amendment to ADR-0027; a bundled benchmark pass is no longer a production activation prerequisite, while retrieval safety constraints remain.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - kind `amends`；candidate 端点顺序 `S24 → S23`（该顺序本身不是证据）；须从 ADR 声明核对方向、范围及任务用途。
- 冻结来源证据：
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:5-5`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `explicit_adr_relation_header`
> L5: - Amends: ADR-0026 retrieval admission, ADR-0027 automatic quality gate
- `docs/adr/0027-bounded-automatic-retrieval-calibration.md:9-16`; blob `58a06463c2cffe11462e5e996ab612c5f32491db`; SHA-256 `e1a568e4ed2a53397e67c9be2993a89feffa1f0e2fbe70af0702dea2c574d55c`; basis `amended_historical_gate`
> L9: Configured embedding roles automatically calibrate against a versioned, bundled
> L10: non-private benchmark after startup and on configuration reconciliation. Memory
> L11: and note channels have separate signatures, thresholds and holdout reports.
> L12: When both have applicable reports, their no-answer failure IDs are unioned against
> L13: the same holdout gate. A failing combined result prevents semantic activation and
> L14: automatic retry storms while retaining both reports for diagnosis and explicit retry.
> L15: A passing benchmark enables semantic admission; vector coverage alone does not.
> L16: No generation model, private corpus labeling or Admin page visit is required.
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:10-15`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `current_diagnostic_policy`
> L10: Bundled evaluation is a development/operator diagnostic, not a prerequisite for
> L11: production semantic retrieval. Startup and ordinary configuration reconciliation
> L12: must not initiate synthetic evaluation requests. Historical reports retain their
> L13: actual results; failed reports are not rewritten to passed. Current vector identity,
> L14: dimensions, source eligibility, permissions and bounded result ranking still apply.
> L15: Similarity is ranking evidence, not a probability that a passage answers a question.
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:34-42`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `current_retrieval_constraints`
> L34: ## Amendment — 2026-09-06: rule-only chunking
> L35:&#32;
> L36: User explicitly withdrew model grouping after real comparisons failed to demonstrate
> L37: improvement. This supersedes the optional note_chunking decision above. Production
> L38: preparation, resolution and retrieval use bounded deterministic rule chunks; no
> L39: grouping generation or Admin role admission remains. Legacy plan/binding rows are
> L40: inert, retained for additive schema compatibility. Existing rule vectors remain
> L41: valid; model-grouped keys are not eligible and normal scheduling fills missing rule
> L42: vectors. Diagnostic-only evaluation policy remains unchanged.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S23-2

- 风险标记：cross-source-relation-review, qualifier-negation-review
- 来源：S23, S22；split：holdout；severity：`medium`
- query：Before the later ADR-0028 amendment, what did ADR-0027's historical amendment to ADR-0026 specify as the gate for semantic admission?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：ADR-0027 amended ADR-0026's retrieval preparation and operational policy: it separated memory/note calibration reports, unioned applicable no-answer failures, and blocked semantic activation when the combined holdout gate failed. This is ADR-0027's historical policy, not a claim about current policy.
  - 必须保留：ADR-0027 amended ADR-0026's retrieval preparation and operational policy: it separated memory/note calibration reports, unioned applicable no-answer failures, and blocked semantic activation when the combined holdout gate failed. This is ADR-0027's historical policy, not a claim about current policy.；Keep the amendment relation and historical scope explicit: this reports ADR-0027's gate, not current production policy.
  - 禁止推断：Do not infer from ADR-0027 alone that its historical bundled-benchmark gate remains current; do not treat vector coverage alone as sufficient under that stated policy.
  - 正确状态：`historical_amended`；no-answer：`false`
  - 预期来源关系：
    - kind `amends`；candidate 端点顺序 `S23 → S22`（该顺序本身不是证据）；须从 ADR 声明核对方向、范围及任务用途。
- 冻结来源证据：
- `docs/adr/0027-bounded-automatic-retrieval-calibration.md:5-5`; blob `58a06463c2cffe11462e5e996ab612c5f32491db`; SHA-256 `e1a568e4ed2a53397e67c9be2993a89feffa1f0e2fbe70af0702dea2c574d55c`; basis `explicit_adr_relation_header`
> L5: - Amends: ADR-0026 retrieval preparation and operational policy
- `docs/adr/0026-current-source-owned-memory-sets.md:114-124`; blob `6d9aa9816fd808ecf5ee7a71fea3a047f8c4ef54`; SHA-256 `2a36b0c8cebf0e80c021ea607eb733901fbb1ce0e69677b472ca77fbff234afd`; basis `relation_endpoint_context`
> L114: Recall remains a local indexed operation and retains separately typed ordinary
> L115: note cues. Candidate generation and relevance acceptance are distinct. A
> L116: candidate must have calibrated semantic evidence or strong normalized lexical/
> L117: entity evidence before rank, recency, or importance boosts apply. Unrelated
> L118: queries may return no results. Public fused `score`, raw BM25/cosine, and each
> L119: RRF contribution keep distinct meanings. Vectors must match Vault, object
> L120: type, model/profile/dimension, current object/source hash, and exact embedding
> L121: input hash. Section-aware chunks carry local headings, cover supported input
> L122: from start to end or report an explicit limit, and contribute once per object.
> L123: All returned text and metadata share one estimated output budget; oversized
> L124: items are skipped while later items are still considered.
- `docs/adr/0027-bounded-automatic-retrieval-calibration.md:9-16`; blob `58a06463c2cffe11462e5e996ab612c5f32491db`; SHA-256 `e1a568e4ed2a53397e67c9be2993a89feffa1f0e2fbe70af0702dea2c574d55c`; basis `direct_amendment_policy`
> L9: Configured embedding roles automatically calibrate against a versioned, bundled
> L10: non-private benchmark after startup and on configuration reconciliation. Memory
> L11: and note channels have separate signatures, thresholds and holdout reports.
> L12: When both have applicable reports, their no-answer failure IDs are unioned against
> L13: the same holdout gate. A failing combined result prevents semantic activation and
> L14: automatic retry storms while retaining both reports for diagnosis and explicit retry.
> L15: A passing benchmark enables semantic admission; vector coverage alone does not.
> L16: No generation model, private corpus labeling or Admin page visit is required.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S24-1

- 风险标记：qualifier-negation-review
- 来源：S24；split：holdout；severity：`medium`
- query：Under ADR-0028's current amendments, what is bundled evaluation's role, and may a model still choose chunk groups?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Bundled evaluation is a development/operator diagnostic, not a production prerequisite. The later rule-only amendment withdrew model grouping; bounded deterministic rule chunks are used, with no grouping generation or Admin role admission.
  - 必须保留：Bundled evaluation is a development/operator diagnostic, not a production prerequisite. The later rule-only amendment withdrew model grouping; bounded deterministic rule chunks are used, with no grouping generation or Admin role admission.；Apply ADR-0028's lines 34–42 amendment: model grouping is withdrawn and rule-only chunking is current; diagnostic-only evaluation remains.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:10-15`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `diagnostic_only_policy`
> L10: Bundled evaluation is a development/operator diagnostic, not a prerequisite for
> L11: production semantic retrieval. Startup and ordinary configuration reconciliation
> L12: must not initiate synthetic evaluation requests. Historical reports retain their
> L13: actual results; failed reports are not rewritten to passed. Current vector identity,
> L14: dimensions, source eligibility, permissions and bounded result ranking still apply.
> L15: Similarity is ranking evidence, not a probability that a passage answers a question.
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:34-42`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `rule_only_amendment`
> L34: ## Amendment — 2026-09-06: rule-only chunking
> L35:&#32;
> L36: User explicitly withdrew model grouping after real comparisons failed to demonstrate
> L37: improvement. This supersedes the optional note_chunking decision above. Production
> L38: preparation, resolution and retrieval use bounded deterministic rule chunks; no
> L39: grouping generation or Admin role admission remains. Legacy plan/binding rows are
> L40: inert, retained for additive schema compatibility. Existing rule vectors remain
> L41: valid; model-grouped keys are not eligible and normal scheduling fills missing rule
> L42: vectors. Diagnostic-only evaluation policy remains unchanged.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S24-2

- 风险标记：no-answer-full-document-check, severity-high, qualifier-negation-review
- 来源：S24；split：holdout；severity：`high`
- query：What empirical answerability probability does ADR-0028 report for passages in its highest similarity-ranked group?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：（空）
  - 必须保留：ADR-0028 treats similarity as ranking evidence rather than answer probability and reports no measured answerability rate.；A related passage may still fail to answer the query.
  - 禁止推断：Do not invent a calibrated probability, similarity band, or measured production/heldout result from ranking similarity.
  - 正确状态：`no_answer`；no-answer：`true`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:10-15`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `direct_text`
> L10: Bundled evaluation is a development/operator diagnostic, not a prerequisite for
> L11: production semantic retrieval. Startup and ordinary configuration reconciliation
> L12: must not initiate synthetic evaluation requests. Historical reports retain their
> L13: actual results; failed reports are not rewritten to passed. Current vector identity,
> L14: dimensions, source eligibility, permissions and bounded result ranking still apply.
> L15: Similarity is ranking evidence, not a probability that a passage answers a question.
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:28-32`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `direct_text`
> L28: Semantic retrieval can return related passages that do not answer the question;
> L29: source evidence and bounded results make this limitation explicit. A passing
> L30: synthetic benchmark cannot establish private-Vault accuracy. Model grouping adds
> L31: optional ingestion cost and latency and does not guarantee better retrieval;
> L32: independent comparisons remain necessary before claiming quality improvements.
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:1-42`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0028: Model-guided chunks and diagnostic-only retrieval evaluation
> L8: ## Decision
> L26: ## Consequences
> L34: ## Amendment — 2026-09-06: rule-only chunking
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S25-1

- 风险标记：qualifier-negation-review
- 来源：S25, S24；split：holdout；severity：`medium`
- query：What does compact presentation preserve, and how can clients request details?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Compact results retain next-action/safe-write information. All 17 tools accept include_details; get_memory remains a full single-record drill-down.
  - 必须保留：Compact results retain next-action/safe-write information. All 17 tools accept include_details; get_memory remains a full single-record drill-down.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0029-actionable-compact-mcp-results.md:9-20`; blob `76c2fed14c749f7b7c02f1be13309e5132e94c73`; SHA-256 `6e9a714ff3d6aa75320435d90bfaf09158f867e29cd9582bddcf65bc69ef1c96`; basis `direct_text`
> L9: MCP tools default to compact presentation retaining the information needed to choose
> L10: the next action and perform safe writes. All 17 tools accept include_details for the
> L11: extended previous metadata; get_memory remains a full single-record drill-down.
> L12: Search/recall score diagnostics also select detailed presentation. Service and Admin
> L13: records remain unchanged; protocol presentation contains no business logic.
> L14:&#32;
> L15: Recall includes source-note paths by default, with explicit include_sources=false
> L16: opt-out. A source-note path is distinct from managed-memory canonical_path. Descriptions
> L17: state exact field-to-argument transitions and preserve permissions and revision safety.
> L18: Unsupported parameter enum choices are hidden from discovery but old calls retain
> L19: explicit unsupported errors. History responses are paginated. The SDK text mirror and
> L20: structuredContent remain compatible representations of one logical result.
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:3-9`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `B_comparison_source_context`
> L3: - Status: Accepted
> L4: - Date: 2026-09-06
> L5: - Amends: ADR-0026 retrieval admission, ADR-0027 automatic quality gate
> L6: - Authority: explicit user approval of revised retrieval design
> L7:&#32;
> L8: ## Decision
> L9:&#32;
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S25-2

- 风险标记：no-answer-full-document-check, qualifier-negation-review
- 来源：S25, S24；split：holdout；severity：`medium`
- query：What exact response-size reduction percentage is promised?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：（空）
  - 必须保留：把该 ADR 未给出的精确值保留为未知。
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`no_answer`；no-answer：`true`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0029-actionable-compact-mcp-results.md:1-28`; blob `76c2fed14c749f7b7c02f1be13309e5132e94c73`; SHA-256 `6e9a714ff3d6aa75320435d90bfaf09158f867e29cd9582bddcf65bc69ef1c96`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0029: Actionable compact MCP tool results
> L7: ## Decision
> L22: ## Consequences
- `docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:1-42`; blob `895bc109af9d69ca4da8bb6ab17d785a66ed8240`; SHA-256 `61078371607c13b97d395cc95996ca3821a8e57982605d1a36f0c417b1fb3ba2`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0028: Model-guided chunks and diagnostic-only retrieval evaluation
> L8: ## Decision
> L26: ## Consequences
> L34: ## Amendment — 2026-09-06: rule-only chunking
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S26-1

- 风险标记：qualifier-negation-review
- 来源：S26；split：holdout；severity：`medium`
- query：Under the superseded ADR-0030 historical scheme, when could multiple source contributions support one formal memory?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Only when each current contribution independently supported the complete proposition; explicit assertions did not participate, and topic or transitive similarity could not establish support.
  - 必须保留：Only when each current contribution independently supported the complete proposition; explicit assertions did not participate, and topic or transitive similarity could not establish support.；This is ADR-0030's historical multi-source formal-memory rule, superseded for the current runtime; do not report it as current behavior.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0030-automatic-memory-equivalence.md:3-11`; blob `fe49abe24b82664092839bd813ed24caeebddcaa`; SHA-256 `70bd0b1c8df8ccc29810f2f2ed2f2257d63a78780401800b2be73c743cb760ef`; basis `status_and_supersession`
> L3: &gt; Superseded for the current memory runtime by &#91;ADR-0033&#93;(0033-source-preserving-memory-units.md). This document preserves historical rationale.
> L4:&#32;
> L5: - Status: Partially superseded by ADR-0031 (incremental organization, automatic old-state reset, no inclusion deletion/sentence rewriting/profile-change dissolution). Formal publication and source-support invariants remain accepted.
> L6: - Date: 2026-09-06
> L7: - Authority: explicit execution request for the automatic memory deduplication plan.
> L8: - Amends: ADR-0026 only in its one-source formal ownership restriction and the
> L9:   upgrade of already classified current v2.1 source sets. Ambiguous prerelease
> L10:   lifecycle rows still require the existing non-destructive classification.
> L11: - Implementation status: implemented; the active ExecPlan records engineering evidence and the separately pending real-provider/production gate.
- `docs/adr/0030-automatic-memory-equivalence.md:15-31`; blob `fe49abe24b82664092839bd813ed24caeebddcaa`; SHA-256 `70bd0b1c8df8ccc29810f2f2ed2f2257d63a78780401800b2be73c743cb760ef`; basis `direct_text`
  - 引用范围还有 5 行未摘录；须回看该范围。
> L15: Keep current source-owned sets as source contributions. A formal note-derived
> L16: memory may be supported by several current contributions only after each
> L17: independently supports its complete proposition. Explicit assertions do not
> L18: participate. Same-source full inclusion may remove redundancy; cross-source
> L19: inclusion, topic similarity and transitive similarity cannot establish support.
> L20: Case folding and Unicode compatibility normalization are lexical conveniences,
> L21: not proof that identifiers, formulas, quantities or propositions are equal.
> L22:&#32;
> L23: Formal content and exact support references must be canonical managed Markdown,
> L24: with rebuildable Vault-scoped projections. Every public memory reader, count,
> L25: vector source and mutation must use one formal view. Absorbed IDs become absent;
> L26: write operations must never redirect an old ID into a larger support group.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S26-2

- 风险标记：qualifier-negation-review
- 来源：S26；split：holdout；severity：`medium`
- query：After ADR-0031 partially superseded ADR-0030, which ADR-0030 invariants did its status say remained accepted?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：ADR-0030 says formal publication and source-support invariants remain accepted; ADR-0033 later supersedes ADR-0030 for the current memory runtime.
  - 必须保留：ADR-0030 says formal publication and source-support invariants remain accepted; ADR-0033 later supersedes ADR-0030 for the current memory runtime.；Do not reduce ADR-0030 to wholly invalid historical rationale: its status explicitly retains formal publication and source-support invariants, while its current runtime is superseded.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0030-automatic-memory-equivalence.md:3-11`; blob `fe49abe24b82664092839bd813ed24caeebddcaa`; SHA-256 `70bd0b1c8df8ccc29810f2f2ed2f2257d63a78780401800b2be73c743cb760ef`; basis `direct_text`
> L3: &gt; Superseded for the current memory runtime by &#91;ADR-0033&#93;(0033-source-preserving-memory-units.md). This document preserves historical rationale.
> L4:&#32;
> L5: - Status: Partially superseded by ADR-0031 (incremental organization, automatic old-state reset, no inclusion deletion/sentence rewriting/profile-change dissolution). Formal publication and source-support invariants remain accepted.
> L6: - Date: 2026-09-06
> L7: - Authority: explicit execution request for the automatic memory deduplication plan.
> L8: - Amends: ADR-0026 only in its one-source formal ownership restriction and the
> L9:   upgrade of already classified current v2.1 source sets. Ambiguous prerelease
> L10:   lifecycle rows still require the existing non-destructive classification.
> L11: - Implementation status: implemented; the active ExecPlan records engineering evidence and the separately pending real-provider/production gate.
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S27-1

- 风险标记：qualifier-negation-review
- 来源：S27, S29；split：holdout；severity：`medium`
- query：Under superseded ADR-0031's historical policy, what happened when a candidate was only related, partially overlapping, or uncertain?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：It stayed independent; that policy transferred only validated complete source support and kept the existing formal body stable.
  - 必须保留：It stayed independent; that policy transferred only validated complete source support and kept the existing formal body stable.；This describes the historical ADR-0031 merge rule, not the current runtime policy.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0031-incremental-memory-organization.md:3-9`; blob `d408693309d985ad1598ad73326c23d86630ee85`; SHA-256 `ea0e7449ebea657a7d8ecde54823c0c1474ef3bf2989f2adc60bc04cc1ba438b`; basis `status_and_retained_boundaries`
> L3: &gt; Superseded for the current memory runtime by &#91;ADR-0033&#93;(0033-source-preserving-memory-units.md). This document preserves historical rationale.
> L4:&#32;
> L5: - 状态：已接受并实现；本地工程验收见&#91;验收报告&#93;(../exec-plans/reports/incremental-memory-organization-20260907.md)。
> L6: - 日期：2026-09-07。
> L7: - 授权：用户要求完整实施增量方案，并在升级后清除旧自动合并任务和记录重新开始。
> L8: - 后续变更：&#91;ADR-0032&#93;(0032-lossless-memory-consolidation.md) 取代下文仅允许整条等价、保持正式正文不变的规则；保留本 ADR 的增量调度、来源安全及恢复边界。
> L9: - 替代：ADR-0030 的全库配对调度、同源包含删除、默认句子改写、模型配置变化全量拆组。保留正式记忆、精确来源支持、安全发布和删除语义。
- `docs/adr/0031-incremental-memory-organization.md:13-24`; blob `d408693309d985ad1598ad73326c23d86630ee85`; SHA-256 `ea0e7449ebea657a7d8ecde54823c0c1474ef3bf2989f2adc60bc04cc1ba438b`; basis `direct_text`
> L13: 来源提取保留完整事实，不因其他来源已经存在同义记忆而省略依据。提取结果的本地精确去重后，系统按变化贡献查找有界、相关的当前正式记忆。一个请求判断一条新事实与少量候选的完整等价关系。没有候选、部分包含、相关或不确定时，保留独立记忆。
> L14:&#32;
> L15: 归并仅转移已经验证的完整来源支持。已有正式正文保持稳定，不拼接事实，不借助传递相似性，不处理显式记忆。候选顺序、请求内编号、模型结果和发布版本均由应用校验。模型变化只影响未来判断，不自动拆散已发布事实。
> L16:&#32;
> L17: 采用 `memory.organize` 及按来源、贡献记录的持久待处理状态，不创建全库两两组合。来源事务标记增量工作；来源变更维护受影响支持；向量到达允许对应条目补查。稳定输入完成后保持空闲。任务中断恢复未完成工作；模型失败保留可读独立事实。
> L18:&#32;
> L19: 管理接口提供状态、立即整理、暂停、继续和重新检查。暂停只停止语义工作，来源失效和文件操作恢复仍然有效。进度按当前条目、实际调用、缓存命中和合并计数报告，不用候选组合数冒充记忆数。
> L20:&#32;
> L21: 初始化通过迁移 0025 保存拆组检查与来源接管游标，每阶段每轮最多检查 8 条。语义阶段每轮最多执行一次未缓存的模型判断；无候选或命中缓存的条目仍可每轮处理 8 条。单次逻辑请求独立使用配置的请求超时，涵盖并发门等待与重试；外层预算为 300 秒本地工作加一次请求预算。剩余时间不足以容纳请求及 30 秒发布余量时，保存检查点后正常续跑，不累计失败次数。该边界避免多次模型判断共享即将耗尽的批次截止时间。
> L22:&#32;
> L23: 向量维度以当前种子的有效向量为依据。模型配置中的预期维度仅用于校验返回值，未填写时不能跳过向量候选。迁移 0026 只重新检查受旧维度守卫影响的独立贡献，保留已验证分组、来源、缓存与暂停状态。
> L24:&#32;
- `docs/adr/0033-source-preserving-memory-units.md:3-9`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `B_comparison_source_context`
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-09。
> L5: - 授权：用户批准新记忆重构计划，允许生产切换时全部抛弃旧记忆数据，不转换旧记录。
> L6: - 替代：ADR-0031／0032 的正式正文合并；ADR-0026 中模型重写自动正文及旧契约兼容相关决定。Vault 隔离、来源当前资格、明确所有权及模型不可读历史等原则继续成立。
> L7: - 执行计划：&#91;新记忆系统&#93;(../exec-plans/active/memory-system-v3.md)。
> L8:&#32;
> L9: ## 问题
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S27-2

- 风险标记：cross-source-relation-review, qualifier-negation-review
- 来源：S27, S29；split：holdout；severity：`medium`
- query：Is ADR-0031’s whole-item/fixed-body policy current after ADR-0032/0033?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：ADR-0032 replaced ADR-0031's whole-item equivalence/fixed-body merge rule. ADR-0031 says its incremental scheduling, source-safety, and recovery boundaries were retained in that ADR-0032 revision. ADR-0033 later superseded formal-body merging; it explicitly retains Vault isolation, current-source eligibility, explicit ownership, and no model-readable history, but this task does not infer that ADR-0031's exact scheduling mechanism remains unchanged.
  - 必须保留：Separate ADR-0032's replacement of the fixed-body merge rule from the boundaries ADR-0031 says were retained in that revision. ADR-0033's current-runtime retained principles are isolation, current-source eligibility, explicit ownership, and no model-readable history; do not claim ADR-0031's same scheduling mechanism still runs unchanged.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - kind `supersedes`；candidate 端点顺序 `S29 → S27`（该顺序本身不是证据）；须从 ADR 声明核对方向、范围及任务用途。
- 冻结来源证据：
- `docs/adr/0031-incremental-memory-organization.md:3-9`; blob `d408693309d985ad1598ad73326c23d86630ee85`; SHA-256 `ea0e7449ebea657a7d8ecde54823c0c1474ef3bf2989f2adc60bc04cc1ba438b`; basis `direct_text`
> L3: &gt; Superseded for the current memory runtime by &#91;ADR-0033&#93;(0033-source-preserving-memory-units.md). This document preserves historical rationale.
> L4:&#32;
> L5: - 状态：已接受并实现；本地工程验收见&#91;验收报告&#93;(../exec-plans/reports/incremental-memory-organization-20260907.md)。
> L6: - 日期：2026-09-07。
> L7: - 授权：用户要求完整实施增量方案，并在升级后清除旧自动合并任务和记录重新开始。
> L8: - 后续变更：&#91;ADR-0032&#93;(0032-lossless-memory-consolidation.md) 取代下文仅允许整条等价、保持正式正文不变的规则；保留本 ADR 的增量调度、来源安全及恢复边界。
> L9: - 替代：ADR-0030 的全库配对调度、同源包含删除、默认句子改写、模型配置变化全量拆组。保留正式记忆、精确来源支持、安全发布和删除语义。
- `docs/adr/0033-source-preserving-memory-units.md:3-9`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `explicit_adr_relation_header`
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-09。
> L5: - 授权：用户批准新记忆重构计划，允许生产切换时全部抛弃旧记忆数据，不转换旧记录。
> L6: - 替代：ADR-0031／0032 的正式正文合并；ADR-0026 中模型重写自动正文及旧契约兼容相关决定。Vault 隔离、来源当前资格、明确所有权及模型不可读历史等原则继续成立。
> L7: - 执行计划：&#91;新记忆系统&#93;(../exec-plans/active/memory-system-v3.md)。
> L8:&#32;
> L9: ## 问题
- `docs/adr/0031-incremental-memory-organization.md:3-9`; blob `d408693309d985ad1598ad73326c23d86630ee85`; SHA-256 `ea0e7449ebea657a7d8ecde54823c0c1474ef3bf2989f2adc60bc04cc1ba438b`; basis `explicit_adr_relation_header`
> L3: &gt; Superseded for the current memory runtime by &#91;ADR-0033&#93;(0033-source-preserving-memory-units.md). This document preserves historical rationale.
> L4:&#32;
> L5: - 状态：已接受并实现；本地工程验收见&#91;验收报告&#93;(../exec-plans/reports/incremental-memory-organization-20260907.md)。
> L6: - 日期：2026-09-07。
> L7: - 授权：用户要求完整实施增量方案，并在升级后清除旧自动合并任务和记录重新开始。
> L8: - 后续变更：&#91;ADR-0032&#93;(0032-lossless-memory-consolidation.md) 取代下文仅允许整条等价、保持正式正文不变的规则；保留本 ADR 的增量调度、来源安全及恢复边界。
> L9: - 替代：ADR-0030 的全库配对调度、同源包含删除、默认句子改写、模型配置变化全量拆组。保留正式记忆、精确来源支持、安全发布和删除语义。
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S28-1

- 风险标记：qualifier-negation-review
- 来源：S28, S29；split：holdout；severity：`medium`
- query：Under superseded ADR-0032's historical lossless-merge proposal, what details had an overlap proposal to preserve?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Compatible overlap could merge only with per-fact support; shared facts appeared once, while source-specific steps, conditions, exceptions, limits, and negation stayed attached to their supporting source.
  - 必须保留：Compatible overlap could merge only with per-fact support; shared facts appeared once, while source-specific steps, conditions, exceptions, limits, and negation stayed attached to their supporting source.；This is ADR-0032's historical proposal; ADR-0033 superseded its formal-body merging for the current runtime.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0032-lossless-memory-consolidation.md:19-37`; blob `2960ecaef1e0af6136cf760dd180b5cab9c21440`; SHA-256 `1b7bf389c07dc1906f6b248a6fe1d27e78e334518ef874e711039898d5b1a42b`; basis `direct_text`
  - 引用范围还有 7 行未摘录；须回看该范围。
> L19: 整理单位改为同一具体对象、同一适用范围和同一问题下的一组事实。允许把等价、包含以及相容的部分重叠整合；仅主题相近、一般规则与具体实例、版本或时间不同、矛盾或范围不明仍保持独立。
> L20:&#32;
> L21: 正式记忆保留一个稳定 ID，正文由有序事实构成。每项事实记录支持它的当前来源贡献及精确版本，来源只支持自己实际覆盖的事实。共同事实表达一次；独有步骤、条件、例外、限制与否定必须保留。不得通过增加无关事实形成大段主题摘要。
> L22:&#32;
> L23: 例如，甲说明“模型和数据需要显式移到设备”，乙包含相同操作并补充“检查实际设备”。整合后保留这两项事实；前者可以由甲乙支持，后者只能由乙支持。模型无需把两条原文误判为完整等价才能减少重复。
> L24:&#32;
> L25: ## 生成与验证
> L26:&#32;
> L27: 1. 继续按变化贡献查找有界候选，向量门槛从 0.75 扩到 0.70，以覆盖已复现的 0.746789 等价译文。原 325 条的无向候选对从 31 对扩大到 91 对，仍只保留前 8 个候选；门槛不是语义正确性保证。
> L28: 2. 一次筛选请求标出具有实际重叠的目标，最多尝试其中 2 个。为选定目标读取完整当前贡献，生成未发布的逐项事实提案，每项引用请求内的贡献编号。来源成员最多 32 条，事实最多 32 项，单次输入最多 64 KiB。
> L29: 3. 应用验证完整返回集合、引用范围、体积上限、元数据兼容性及精确来源资格。时间范围、重要性与置信度必须匹配；标签和实体取并集，不补造值。
> L30: 4. 事实提案只接收当前贡献正文；笔记标题和路径仅作为对象／项目范围提示，不发送整段笔记背景，也不允许从范围提示扩写事实。标题来自当前 File ID 与内容哈希匹配的索引，过期标题不参与。
- `docs/adr/0033-source-preserving-memory-units.md:3-9`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `B_comparison_source_context`
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-09。
> L5: - 授权：用户批准新记忆重构计划，允许生产切换时全部抛弃旧记忆数据，不转换旧记录。
> L6: - 替代：ADR-0031／0032 的正式正文合并；ADR-0026 中模型重写自动正文及旧契约兼容相关决定。Vault 隔离、来源当前资格、明确所有权及模型不可读历史等原则继续成立。
> L7: - 执行计划：&#91;新记忆系统&#93;(../exec-plans/active/memory-system-v3.md)。
> L8:&#32;
> L9: ## 问题
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S28-2

- 风险标记：cross-source-relation-review, qualifier-negation-review
- 来源：S28, S29；split：holdout；severity：`medium`
- query：Under superseded ADR-0032's historical lossless-merge proposal, could a failed support/coverage check publish a merge?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：No. Failed checks blocked publication; ADR-0033 later superseded this merge approach for the current runtime.
  - 必须保留：No. Failed checks blocked publication; ADR-0033 later superseded this merge approach for the current runtime.；Keep the historical ADR-0032 rule and the supersession direction ADR-0033 → ADR-0032 explicit.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`historical_superseded`；no-answer：`false`
  - 预期来源关系：
    - kind `supersedes`；candidate 端点顺序 `S29 → S28`（该顺序本身不是证据）；须从 ADR 声明核对方向、范围及任务用途。
- 冻结来源证据：
- `docs/adr/0032-lossless-memory-consolidation.md:3-8`; blob `2960ecaef1e0af6136cf760dd180b5cab9c21440`; SHA-256 `1b7bf389c07dc1906f6b248a6fe1d27e78e334518ef874e711039898d5b1a42b`; basis `direct_text`
> L3: &gt; Superseded for the current memory runtime by &#91;ADR-0033&#93;(0033-source-preserving-memory-units.md). This document preserves historical rationale.
> L4:&#32;
> L5: - 状态：已接受；2026-09-09 真实服务质量验收不通过，自动无损发布存在待解决阻断。替代 ADR-0031 的整条等价与固定正文限制。
> L6: - 日期：2026-09-08。
> L7: - 授权：用户明确选择“允许无损整合重叠内容”，要求保留双方细节及逐项来源。
> L8: - 动机：用户要求优化真实整理效果；&#91;实测&#93;(../exec-plans/reports/organization-real-model-validation-20260908.md)显示整条完全等价规则无法消除概述与详细说明之间的重叠，且单次模型关系判断存在丢失细节的风险。
- `docs/adr/0033-source-preserving-memory-units.md:3-9`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `explicit_adr_relation_header`
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-09。
> L5: - 授权：用户批准新记忆重构计划，允许生产切换时全部抛弃旧记忆数据，不转换旧记录。
> L6: - 替代：ADR-0031／0032 的正式正文合并；ADR-0026 中模型重写自动正文及旧契约兼容相关决定。Vault 隔离、来源当前资格、明确所有权及模型不可读历史等原则继续成立。
> L7: - 执行计划：&#91;新记忆系统&#93;(../exec-plans/active/memory-system-v3.md)。
> L8:&#32;
> L9: ## 问题
- `docs/adr/0032-lossless-memory-consolidation.md:3-9`; blob `2960ecaef1e0af6136cf760dd180b5cab9c21440`; SHA-256 `1b7bf389c07dc1906f6b248a6fe1d27e78e334518ef874e711039898d5b1a42b`; basis `explicit_adr_relation_header`
> L3: &gt; Superseded for the current memory runtime by &#91;ADR-0033&#93;(0033-source-preserving-memory-units.md). This document preserves historical rationale.
> L4:&#32;
> L5: - 状态：已接受；2026-09-09 真实服务质量验收不通过，自动无损发布存在待解决阻断。替代 ADR-0031 的整条等价与固定正文限制。
> L6: - 日期：2026-09-08。
> L7: - 授权：用户明确选择“允许无损整合重叠内容”，要求保留双方细节及逐项来源。
> L8: - 动机：用户要求优化真实整理效果；&#91;实测&#93;(../exec-plans/reports/organization-real-model-validation-20260908.md)显示整条完全等价规则无法消除概述与详细说明之间的重叠，且单次模型关系判断存在丢失细节的风险。
> L9: - 执行计划：&#91;记忆整理质量优化&#93;(../exec-plans/active/organization-quality-improvement.md)。
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S29-1

- 风险标记：qualifier-negation-review
- 来源：S29；split：holdout；severity：`medium`
- query：Which information may become automatic memory, and how must time, version, scope, order, and exceptions be preserved?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Durable agreements/decisions/states/experience may be selected from complete source units. Tutorials/general knowledge remain searchable; premises, scope, order, exceptions, and verification steps stay intact. Preserve source-stated time, version, and scope; when a date is absent, do not infer current validity.
  - 必须保留：Durable agreements/decisions/states/experience may be selected from complete source units. Tutorials/general knowledge remain searchable; premises, scope, order, exceptions, and verification steps stay intact. Preserve source-stated time, version, and scope; when a date is absent, do not infer current validity.；ADR-0033 is accepted and in implementation; the design text is not evidence that production migration or rollout is complete.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0033-source-preserving-memory-units.md:1-6`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `status_and_implementation_state`
> L1: # ADR-0033：完整原文记忆单元与任务召回
> L2:&#32;
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-09。
> L5: - 授权：用户批准新记忆重构计划，允许生产切换时全部抛弃旧记忆数据，不转换旧记录。
> L6: - 替代：ADR-0031／0032 的正式正文合并；ADR-0026 中模型重写自动正文及旧契约兼容相关决定。Vault 隔离、来源当前资格、明确所有权及模型不可读历史等原则继续成立。
- `docs/adr/0033-source-preserving-memory-units.md:13-25`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `direct_text`
  - 引用范围还有 1 行未摘录；须回看该范围。
> L13: ## 决定
> L14:&#32;
> L15: 明确记忆直接保存授权正文。自动记忆聚焦约定、决策、状态及实践经验，模型从服务编号的完整原文单元中选择，服务提取正文并绑定当前来源。普通教程和通识资料保持知识检索用途。一次性 PPT／演讲／报告的排版、逐页文案、素材、命名和纯呈现步骤即使真实发生或写成“已决定”，仍留在原笔记按需检索；一次任务的重要安全前提、阻塞和已承诺下一步仍可选择，即使只执行一次。混合来源逐单元判断，不能按文件类型整体拒绝。参考架构事实没有当前项目明确采纳关系时不自动变成项目约定；保留原文实际给出的时间、版本和范围，未注明日期时不猜当前有效性，旧文档中的作者亲历经验仍按原有范围判断。选择的最小完整单元必须保留前提、适用范围、顺序、例外和验证步骤。原文摘录仍需完整上下文与质量验收，不宣称能够自动穷尽笔记知识。
> L16:&#32;
> L17: 完整单元保留标题、范围、前提、顺序、例外及验证步骤；模型分类、检索说明和主题概览为派生数据，不能替代正文或递归成为证据。单元来源哈希与修订控制发布及当前读取。明确记忆独立维护；自动项删除原子暂停来源，明确恢复后再生成。
> L18:&#32;
> L19: 自动提取先选择最小源单元，再对有父子或兄弟关系的结果执行有界完整性审查。审查输入只包含实际展示的源正文，严格限制为 32 个单元和 60 KiB 序列化 JSON；每个首轮 ID 只能归入一次审查、确定性省略或无关系直通。父替代必须由同一 scope 中实际可见的完整父正文和显式 replace_parent 动作授权，不能由字节包含关系推导。首轮和审查阶段使用不同的缓存阶段、schema、profile 与输入哈希，全部审查完成后才允许原子发布。
> L20:&#32;
> L21: 召回根据任务选择完整单元，去重只作用于明确相同正文／范围的展示，不能据相似性删改来源。模型概览单独标识、引用具体单元，失效时返回确定性导航。普通召回不调用生成模型，外部能力不可用时保留关键词路径。
> L22:&#32;
> L23: 自动单元直接返回原始笔记内容，因此需要 memory:read 和 vault:read；明确记忆继续按自身权限访问。生成内容和笔记都作为资料，不是更高优先级指令。
> L24:&#32;
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S29-2

- 风险标记：severity-critical, qualifier-negation-review
- 来源：S29；split：holdout；severity：`critical`
- query：Which permissions are needed to read returned source units?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：Both memory:read and vault:read; explicit memory keeps its own permission rule.
  - 必须保留：Both memory:read and vault:read; explicit memory keeps its own permission rule.；ADR-0033 is accepted and in implementation; the design text is not evidence that production migration or rollout is complete.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0033-source-preserving-memory-units.md:1-6`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `status_and_implementation_state`
> L1: # ADR-0033：完整原文记忆单元与任务召回
> L2:&#32;
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-09。
> L5: - 授权：用户批准新记忆重构计划，允许生产切换时全部抛弃旧记忆数据，不转换旧记录。
> L6: - 替代：ADR-0031／0032 的正式正文合并；ADR-0026 中模型重写自动正文及旧契约兼容相关决定。Vault 隔离、来源当前资格、明确所有权及模型不可读历史等原则继续成立。
- `docs/adr/0033-source-preserving-memory-units.md:21-25`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `direct_text`
> L21: 召回根据任务选择完整单元，去重只作用于明确相同正文／范围的展示，不能据相似性删改来源。模型概览单独标识、引用具体单元，失效时返回确定性导航。普通召回不调用生成模型，外部能力不可用时保留关键词路径。
> L22:&#32;
> L23: 自动单元直接返回原始笔记内容，因此需要 memory:read 和 vault:read；明确记忆继续按自身权限访问。生成内容和笔记都作为资料，不是更高优先级指令。
> L24:&#32;
> L25: 新 schema 和规范格式独立；不读取、转换或接管旧记忆。生产一次性离线初始化清除旧记忆业务数据及受管文件，保留普通 Vault 和服务操作配置，具备阶段恢复与完成幂等。普通启动不得反复清零。
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S30-1

- 风险标记：qualifier-negation-review
- 来源：S30, S29；split：holdout；severity：`medium`
- query：What does the superseded journal terminal state prove?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：A later independent create safely took over the same path/content; this does not claim the original completed or alter the later File ID/revision/history/outbox/canonical file. ADR-0034 is accepted and in implementation, not proof that rollout is complete.
  - 必须保留：Keep the narrow superseded-journal meaning and the explicit state 'accepted, in implementation'; this ADR alone does not prove production rollout is complete.
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`source_stated`；no-answer：`false`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0034-replayed-create-witness-recovery.md:1-5`; blob `3f8f834e7546027c1b2ec67efff6db73b551aa4e`; SHA-256 `2dc7ab998014c123a55d697ee3c9b5ea32895d5480531162bae740f0e75cfd5e`; basis `status_and_implementation_state`
> L1: # ADR-0034：有证据的重放 create journal 终结
> L2:&#32;
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-11。
> L5: - 相关执行计划：&#91;重放 create journal 恢复&#93;(../exec-plans/active/replayed-create-witness-recovery.md)。
- `docs/adr/0034-replayed-create-witness-recovery.md:13-19`; blob `3f8f834e7546027c1b2ec67efff6db73b551aa4e`; SHA-256 `2dc7ab998014c123a55d697ee3c9b5ea32895d5480531162bae740f0e75cfd5e`; basis `direct_text`
> L13: 增加 `superseded` 终态。它只表示原始 create 的物理结果已经被后续、同路径、同内容的独立 create 元数据事实安全接管；它不表示原始 journal 已经完成，也不改变后来的 File ID、revision、history、outbox 或 canonical 文件。
> L14:&#32;
> L15: State 在一个 `BEGIN IMMEDIATE` 事务内重新读取并校验全部证据，然后以 `state = 'file_committed'` 作为 CAS 条件更新原 journal，并写入一条 Vault 作用域 audit。校验必须同时满足：原 journal 属于当前 Vault、操作为 create、状态为 `file_committed`、payload 中 `require_absent` 为 true，路径和原 File ID 与调用参数完全一致；原 File ID 在当前条目和 revision 中都不存在；目标路径恰好由不同的活动 File ID 占用，活动 revision 和 create revision 的路径、revision、哈希一致；存在更晚的 `metadata … &#91;长行截断；须回看固定来源&#93;
> L16:&#32;
> L17: 恢复错误还携带 journal 的 operation ID 和 Vault 相对路径，供初始化任务安全持久化阶段诊断；原有不带上下文的 Core recovery API 保持兼容。
> L18:&#32;
> L19: 任何一个条件不满足都返回未证明，调用方保留 `needs_review`。普通恢复只处理仍为 `file_committed` 的记录；显式维护恢复可以对错误标记恰为“目标已存在”的 reviewed create 重新执行同一物理验证，成功后才允许进入 `superseded`。旧记忆一次性初始化是用户明确授权抛弃整个 predecessor memory namespace 的专用流程；它不调用本 ADR 的 replayed-create repair，而是在严格证明 journal 的所有 canonical 路径都位于该 legacy namespace 后，将旧 intent 标为 `discarded`，再按独立 manifest/hash 通过 Core 删除文件。其它 Vault 路 … &#91;长行截断；须回看固定来源&#93;
- `docs/adr/0033-source-preserving-memory-units.md:3-9`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `B_comparison_source_context`
> L3: - 状态：已接受，实施中。
> L4: - 日期：2026-09-09。
> L5: - 授权：用户批准新记忆重构计划，允许生产切换时全部抛弃旧记忆数据，不转换旧记录。
> L6: - 替代：ADR-0031／0032 的正式正文合并；ADR-0026 中模型重写自动正文及旧契约兼容相关决定。Vault 隔离、来源当前资格、明确所有权及模型不可读历史等原则继续成立。
> L7: - 执行计划：&#91;新记忆系统&#93;(../exec-plans/active/memory-system-v3.md)。
> L8:&#32;
> L9: ## 问题
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

### T-S30-2

- 风险标记：no-answer-full-document-check, severity-critical, qualifier-negation-review
- 来源：S30, S29；split：holdout；severity：`critical`
- query：What exact retry count or time window governs recovery of this journal?
- 候选 gold（只供离线复核；不得传给 Provider）：
  - 预期可用重点：（空）
  - 必须保留：把该 ADR 未给出的精确值保留为未知。
  - 禁止推断：不得补造来源未支持的数字、时间、条件、权限或来源关系。
  - 正确状态：`no_answer`；no-answer：`true`
  - 预期来源关系：
    - 无显式关系 gold；确认跨来源答案没有暗含关系主张。
- 冻结来源证据：
- `docs/adr/0034-replayed-create-witness-recovery.md:1-30`; blob `3f8f834e7546027c1b2ec67efff6db73b551aa4e`; SHA-256 `2dc7ab998014c123a55d697ee3c9b5ea32895d5480531162bae740f0e75cfd5e`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0034：有证据的重放 create journal 终结
> L7: ## 背景
> L11: ## 决定
> L21: ## 后果
> L25: ## 验收
- `docs/adr/0033-source-preserving-memory-units.md:1-39`; blob `b60e6f4ad6460bf6d2f6c9bf26b0fc1120657b53`; SHA-256 `c842aa464b888307fbcd67ee92731852906855bb55dc19b9c5d2f2bf4bd48f79`; basis `full_document_absence_candidate`
  - **无答案全文核查范围**：必须检查上述来源从第 1 行至文件末行；以下仅为导航，不能证明不存在答案。
> L1: # ADR-0033：完整原文记忆单元与任务召回
> L9: ## 问题
> L13: ## 决定
> L27: ### 在线受控初始化入口（2026-09-11 修订）
> L35: ## 后果与验收
- 已知争议/不确定点：独立复核者填写；未检查前不得写“无”。
- 独立裁决：`pending` / `agent_reviewed` / `insufficient_evidence`；结论与理由：________
- 复核者/模型版本：________；复核时间：________；`human_review=false`。
- 若建议改 gold：列出字段、固定来源行号、建议值；本包不直接修改 candidate。

## 独立复核汇总

- `candidate_sha256`：
- 30 条结论计数（agent_reviewed / insufficient_evidence / pending）：
- 需要修订/排除的 Task ID 与依据：
- source split / 跨题材泄漏疑点：
- `human_review=false`：true
- 复核者及模型/版本：
- 盲评映射在评分锁定后揭示：yes/no；映射证据摘要：
