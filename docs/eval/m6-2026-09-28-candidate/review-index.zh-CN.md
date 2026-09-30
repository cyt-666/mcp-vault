# M6 新候选语料索引

> 状态：候选草稿，等待独立智能体证据复核。不是原 live 数据/gold，不是人工已审结果，也不代表 M6 通过。任何 agent-reviewed 结论都必须明确标记 `human_review=false`。

- 候选版本：`mcp-vault-tracked-adr-m6-candidate-20260928-v4`。
- `candidate.json` SHA-256：`bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2`。所有反馈和最终确认必须绑定此 hash；若文件改变，先重算 hash 并重新确认版本。
- `holdout-independent-review.zh-CN.md` SHA-256：`8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a`；review-record template/validator 绑定两项 hash。
- `holdout-independent-review.zh-CN.md` SHA-256：`8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a`；记录模板和 validator 同时绑定 candidate/package 双 hash。
- 固定 Git commit：`e046ceb59393db9a4977ba55899eba301ad28005`。
- 来源：30 份已提交 ADR；Git blob OID、SHA-256、行数见 `candidate.json`。
- 划分：15 development / 15 holdout 来源；60 条任务（30/30）。
- B 来源：8 个；每条 holdout 任务恰含一个 B 来源。
- Holdout 当前包含 7 条 no-answer、10 条 high/critical、6 条跨来源关系任务；任务和 source split 数量未变。
- 当前 live 只运行 holdout 30；development 30 不进入本轮 live task set，也不作为通过证据。
- Holdout 的逐题 query/gold、固定来源短摘录、blob/SHA、行号、关系元数据和全文无答案检查范围，见 [holdout 独立复核包](holdout-independent-review.zh-CN.md)。该包由脚本从固定 Git 字节确定性生成；包仍待独立复核，不表示已审。
- 所有 gold、来源关系、无答案、严重度和 split 均待复核；candidate 内容保持冻结，任何改动都须新 hash/版本。
- v2 改写五条 no-answer 任务；v3 按固定 ADR 字节收窄历史/当前状态和关系边界，调整 `T-S23-1` 的唯一 B 对照来源为 S24，并补充 ADR-0033/0034 实施状态限定。所有任务仍 pending independent-agent-review，未批准。
- v4 按独立复核意见补上 `T-S16-1` 分类 supersession 直接证据，收窄 `T-S17-2` 的时间状态，标注 `T-S23-2` 为 `historical_amended`，统一 no-answer status，并限定 `T-S27-2` 可外推范围；gold 仍 pending。
- 新 no-answer 项都引用整份 ADR 行范围，仍待独立智能体全文核查，不得把局部摘录当作缺失证明。
- 证据仅列路径与行号，不复制来源正文。
- 可复现的确定性结构检查：`python3 docs/eval/m6-2026-09-28-candidate/validate_candidate.py`。

## 独立复核与状态语义

本轮由与候选生产者隔离的独立复核智能体检查 holdout 30，并在运行后以匿名输出 ID 盲评。Development 30 保留作后续迭代语料，不进入当前正式 live holdout 分数。独立复核、人工复核和数据生产是不同职责；agent-reviewed 不能称为人工已审。

独立复核完成后状态记为 `agent_reviewed`，报告必须写 `human_review=false`。证据不足、未解决的关键争议或 candidate hash 漂移均为 fail-closed；不能将 pending 改成 approved。结构校验通过不等于语义 gold 通过。

复核记录格式：

```text
candidate_sha256=bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2
T-Sxx-n | agent_reviewed/insufficient_evidence | 字段=... | 依据=ADR 路径:行号/blob/SHA | 意见=...
human_review=false
```

需要修订时只记录 Task ID、字段、建议文本和固定证据位置；不得直接改当前冻结 candidate。holdout 在 live 运行后任何 gold/query/关系/source split 修订都会污染原评测，须创建新版本。

## 来源清单

| Split | Source ID | ADR | 主题组 | B |
|---|---|---|---|---:|
| development | `S01` | `0001` | canonical-vs-derived | 否 |
| development | `S02` | `0002` | vault-isolation | 否 |
| development | `S03` | `0006` | provider-pluggability | 否 |
| development | `S04` | `0003` | webdav-client-compatibility | 否 |
| development | `S05` | `0008` | vault-bound-credentials | 否 |
| development | `S06` | `0009` | overwrite-history | 否 |
| development | `S07` | `0010` | provider-safety | 否 |
| development | `S08` | `0011` | managed-memory-core | 否 |
| development | `S09` | `0012` | provider-presets | 否 |
| development | `S10` | `0004` | control-plane-network-boundary | 否 |
| development | `S11` | `0005` | modular-monolith-boundary | 否 |
| development | `S12` | `0019` | forwarded-header-security | 否 |
| development | `S13` | `0018` | oauth-security | 否 |
| development | `S14` | `0020` | multi-vault-lifecycle | 否 |
| development | `S15` | `0021` | no-replace-move | 否 |
| holdout | `S16` | `0015` | memory-admission-history | 否 |
| holdout | `S17` | `0016` | two-phase-history | 是 |
| holdout | `S18` | `0022` | source-health-history | 是 |
| holdout | `S19` | `0023` | language-retrieval | 是 |
| holdout | `S20` | `0024` | embedding-limits | 是 |
| holdout | `S21` | `0025` | chunk-ranking | 否 |
| holdout | `S22` | `0026` | current-ownership-and-forget | 是 |
| holdout | `S23` | `0027` | retrieval-calibration | 否 |
| holdout | `S24` | `0028` | diagnostic-and-chunking | 是 |
| holdout | `S25` | `0029` | compact-mcp-results | 否 |
| holdout | `S26` | `0030` | formal-memory-support | 是 |
| holdout | `S27` | `0031` | incremental-organization-history | 否 |
| holdout | `S28` | `0032` | lossless-merge-history | 否 |
| holdout | `S29` | `0033` | source-preserving-units | 是 |
| holdout | `S30` | `0034` | recovery-witness | 否 |

## 逐条任务审阅

| Task ID | Split | 来源 | 审阅重点 | 证据位置 | 无答案 | Severity |
|---|---|---|---|---|---:|---|
| `T-S01-1` | development | S01 | canonical-vs-derived: 决定与主要结论 | docs/adr/0001-markdown-content-is-canonical.md:12-21 | 否 | high |
| `T-S01-2` | development | S01 | canonical-vs-derived: 边界、限定或无答案 | docs/adr/0001-markdown-content-is-canonical.md:1-41 | 是 | medium |
| `T-S02-1` | development | S02 | vault-isolation: 决定与主要结论 | docs/adr/0002-vault-is-the-isolation-boundary.md:12-16 | 否 | high |
| `T-S02-2` | development | S02 | vault-isolation: 边界、限定或无答案 | docs/adr/0002-vault-is-the-isolation-boundary.md:1-35 | 是 | medium |
| `T-S03-1` | development | S03 | provider-pluggability: 决定与主要结论 | docs/adr/0006-llm-and-embeddings-are-pluggable-enrichment.md:14-27 | 否 | high |
| `T-S03-2` | development | S03 | provider-pluggability: 边界、限定或无答案 | docs/adr/0006-llm-and-embeddings-are-pluggable-enrichment.md:1-48 | 是 | medium |
| `T-S04-1` | development | S04 | webdav-client-compatibility: 决定与限定 | docs/adr/0003-use-standard-webdav-and-existing-obsidian-clients.md:12-16 | 否 | medium |
| `T-S04-2` | development | S04 | webdav-client-compatibility: 决定与限定 | docs/adr/0003-use-standard-webdav-and-existing-obsidian-clients.md:14-16 | 否 | medium |
| `T-S05-1` | development | S05 | vault-bound-credentials: 决定与主要结论 | docs/adr/0008-mcp-credentials-bind-to-one-vault.md:12-18 | 否 | high |
| `T-S05-2` | development | S05 | vault-bound-credentials: 边界、限定或无答案 | docs/adr/0008-mcp-credentials-bind-to-one-vault.md:1-39 | 是 | critical |
| `T-S06-1` | development | S06 | overwrite-history: 决定与主要结论 | docs/adr/0009-webdav-overwrite-tombstone-archival.md:18-27 | 否 | high |
| `T-S06-2` | development | S06 | overwrite-history: 边界、限定或无答案 | docs/adr/0009-webdav-overwrite-tombstone-archival.md:1-44 | 是 | medium |
| `T-S07-1` | development | S07 | provider-safety: 决定与主要结论 | docs/adr/0010-provider-adapters-and-vector-fallback.md:19-40 | 否 | high |
| `T-S07-2` | development | S07 | provider-safety: 边界、限定或无答案 | docs/adr/0010-provider-adapters-and-vector-fallback.md:1-66 | 是 | medium |
| `T-S08-1` | development | S08 | managed-memory-core: 决定与主要结论 | docs/adr/0011-managed-memory-canonical-files-use-vault-core.md:22-40 | 否 | high |
| `T-S08-2` | development | S08 | managed-memory-core: 边界、限定或无答案 | docs/adr/0011-managed-memory-canonical-files-use-vault-core.md:22-40 | 否 | medium |
| `T-S09-1` | development | S09 | provider-presets: 决定与主要结论 | docs/adr/0012-compose-provider-presets-behind-the-shared-transport.md:21-40 | 否 | high |
| `T-S09-2` | development | S09 | provider-presets: 边界、限定或无答案 | docs/adr/0012-compose-provider-presets-behind-the-shared-transport.md:21-40 | 否 | medium |
| `T-S10-1` | development | S10 | control-plane-network-boundary: 决定与限定 | docs/adr/0004-admin-control-plane-is-lan-only.md:13-15 | 否 | high |
| `T-S10-2` | development | S10 | control-plane-network-boundary: 安全或缺失信息候选 | docs/adr/0004-admin-control-plane-is-lan-only.md:1-78 | 是 | medium |
| `T-S11-1` | development | S11 | modular-monolith-boundary: 决定与限定 | docs/adr/0005-use-a-modular-monolith.md:12-16 | 否 | medium |
| `T-S11-2` | development | S11 | modular-monolith-boundary: 决定与限定 | docs/adr/0005-use-a-modular-monolith.md:14-16 | 否 | medium |
| `T-S12-1` | development | S12 | forwarded-header-security: 决定与限定 | docs/adr/0019-remove-webdav-proxy-peer-allowlist.md:19-24 | 否 | high |
| `T-S12-2` | development | S12 | forwarded-header-security: 决定与限定 | docs/adr/0019-remove-webdav-proxy-peer-allowlist.md:32-39 | 否 | medium |
| `T-S13-1` | development | S13 | oauth-security: 决定与主要结论 | docs/adr/0018-built-in-oauth-authorization-server.md:22-41 | 否 | high |
| `T-S13-2` | development | S13 | oauth-security: 边界、限定或无答案 | docs/adr/0018-built-in-oauth-authorization-server.md:1-115 | 是 | medium |
| `T-S14-1` | development | S14 | multi-vault-lifecycle: 决定与主要结论 | docs/adr/0020-managed-multi-vault-lifecycle.md:22-41 | 否 | high |
| `T-S14-2` | development | S14 | multi-vault-lifecycle: 边界、限定或无答案 | docs/adr/0020-managed-multi-vault-lifecycle.md:1-77 | 是 | medium |
| `T-S15-1` | development | S15 | no-replace-move: 决定与主要结论 | docs/adr/0021-serialize-legacy-noreplace-move-fallback.md:22-41 | 否 | high |
| `T-S15-2` | development | S15 | no-replace-move: 边界、限定或无答案 | docs/adr/0021-serialize-legacy-noreplace-move-fallback.md:1-67 | 是 | critical |
| `T-S16-1` | holdout | S16,S17 | memory-admission-history: ADR-0015 历史分类与标记限定 | docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:29-48；docs/adr/0016-use-codex-style-two-phase-memory.md:3-9 | 否 | high |
| `T-S16-2` | holdout | S16,S17 | memory-admission-history: marker-free admission 与 direct promotion supersession 范围 | docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:5-11；docs/adr/0016-use-codex-style-two-phase-memory.md:3-9；docs/adr/0015-vault-level-automatic-memory-requires-no-note-markers.md:3-9 | 否 | medium |
| `T-S17-1` | holdout | S17 | two-phase-history: 历史两阶段及 ADR-0026 取代范围 | docs/adr/0016-use-codex-style-two-phase-memory.md:3-13；docs/adr/0016-use-codex-style-two-phase-memory.md:39-58 | 否 | high |
| `T-S17-2` | holdout | S17 | two-phase-history: 边界、限定或无答案 | docs/adr/0016-use-codex-style-two-phase-memory.md:3-13 | 否 | medium |
| `T-S18-1` | holdout | S18 | source-health-history: superseded ADR-0022 历史 stale 行为 | docs/adr/0022-continuous-exact-memory-source-health.md:1-7；docs/adr/0022-continuous-exact-memory-source-health.md:24-41 | 否 | high |
| `T-S18-2` | holdout | S18 | 困难无答案：reconcile retry/backoff 参数及失败触发条件未规定 | docs/adr/0022-continuous-exact-memory-source-health.md:1-103 (全文) | 是 | high |
| `T-S19-1` | holdout | S19 | language-retrieval: 决定与主要结论 | docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md:26-45 | 否 | high |
| `T-S19-2` | holdout | S19 | 困难无答案：alias enrichment 上线效果证据 | docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md:1-99 (全文) | 是 | medium |
| `T-S20-1` | holdout | S20 | embedding-limits: 决定与主要结论 | docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:24-43 | 否 | high |
| `T-S20-2` | holdout | S20 | 困难无答案：UTF-8 envelope 上线后故障/召回实测 | docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:1-83 (全文) | 是 | medium |
| `T-S21-1` | holdout | S21,S20 | chunk-ranking: 决定与主要结论 | docs/adr/0025-aggregate-note-vector-chunks-before-ranking.md:24-43；docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:3-9 | 否 | medium |
| `T-S21-2` | holdout | S21,S20 | 排名聚合 refinement 与 ADR-0024 UTF-8 输入上限的区别 | docs/adr/0025-aggregate-note-vector-chunks-before-ranking.md:5；docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md:24-28；docs/adr/0025-aggregate-note-vector-chunks-before-ranking.md:24-35 | 否 | medium |
| `T-S22-1` | holdout | S22 | current-ownership-and-forget: 决定与主要结论 | docs/adr/0026-current-source-owned-memory-sets.md:38-57 | 否 | medium |
| `T-S22-2` | holdout | S22 | 困难无答案：forget 后备份保留/恢复保证 | docs/adr/0026-current-source-owned-memory-sets.md:1-202 (全文) | 是 | critical |
| `T-S23-1` | holdout | S23,S24 | ADR-0028 amendment：bundled gate 改为 diagnostic-only | docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:5；docs/adr/0027-bounded-automatic-retrieval-calibration.md:9-16；docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:10-15 | 否 | medium |
| `T-S23-2` | holdout | S23,S22 | ADR-0027 historical_amended gate（不是整体失效） | docs/adr/0027-bounded-automatic-retrieval-calibration.md:5；docs/adr/0026-current-source-owned-memory-sets.md:114-124；docs/adr/0027-bounded-automatic-retrieval-calibration.md:9-16 | 否 | medium |
| `T-S24-1` | holdout | S24 | current rule-only chunking 与 diagnostic-only evaluation | docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:10-15；docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:34-42 | 否 | medium |
| `T-S24-2` | holdout | S24 | 困难无答案：similarity 的真实 answerability probability | docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:1-42 (全文) | 是 | high |
| `T-S25-1` | holdout | S25,S24 | compact-mcp-results: S24 是 B-only 干扰来源，不支持 compact 结论 | docs/adr/0029-actionable-compact-mcp-results.md:9-20；docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:3-9 | 否 | medium |
| `T-S25-2` | holdout | S25,S24 | compact-mcp-results: S24 是 B-only 干扰来源，不支持百分比 | docs/adr/0029-actionable-compact-mcp-results.md:1-28；docs/adr/0028-model-guided-chunks-and-diagnostic-evaluation.md:1-42 | 是 | medium |
| `T-S26-1` | holdout | S26 | formal-memory-support: superseded ADR-0030 历史多源规则 | docs/adr/0030-automatic-memory-equivalence.md:3-11；docs/adr/0030-automatic-memory-equivalence.md:15-31 | 否 | medium |
| `T-S26-2` | holdout | S26 | formal-memory-support: ADR-0030 明示保留的 accepted invariants | docs/adr/0030-automatic-memory-equivalence.md:3-11 | 否 | medium |
| `T-S27-1` | holdout | S27,S29 | incremental-organization-history: superseded ADR-0031 历史合并规则 | docs/adr/0031-incremental-memory-organization.md:13-24；docs/adr/0033-source-preserving-memory-units.md:3-9 | 否 | medium |
| `T-S27-2` | holdout | S27,S29 | incremental-organization-history: fixed-body merge replaced; scheduling/source-safety/recovery retained | docs/adr/0031-incremental-memory-organization.md:3-9；docs/adr/0033-source-preserving-memory-units.md:3-9；docs/adr/0031-incremental-memory-organization.md:3-9 | 否 | medium |
| `T-S28-1` | holdout | S28,S29 | lossless-merge-history: superseded ADR-0032 historical proposal | docs/adr/0032-lossless-memory-consolidation.md:19-37；docs/adr/0033-source-preserving-memory-units.md:3-9 | 否 | medium |
| `T-S28-2` | holdout | S28,S29 | lossless-merge-history: superseded ADR-0032 historical publication gate | docs/adr/0032-lossless-memory-consolidation.md:3-8；docs/adr/0033-source-preserving-memory-units.md:3-9；docs/adr/0032-lossless-memory-consolidation.md:3-9 | 否 | medium |
| `T-S29-1` | holdout | S29 | source-preserving-units: accepted/in implementation; not production rollout proof | docs/adr/0033-source-preserving-memory-units.md:1-6；docs/adr/0033-source-preserving-memory-units.md:13-25 | 否 | medium |
| `T-S29-2` | holdout | S29 | source-preserving-units: accepted/in implementation; not production rollout proof | docs/adr/0033-source-preserving-memory-units.md:1-6；docs/adr/0033-source-preserving-memory-units.md:21-25 | 否 | critical |
| `T-S30-1` | holdout | S30,S29 | recovery-witness: accepted/in implementation; witness semantics are not rollout proof | docs/adr/0034-replayed-create-witness-recovery.md:1-5；docs/adr/0034-replayed-create-witness-recovery.md:13-19；docs/adr/0033-source-preserving-memory-units.md:3-9 | 否 | medium |
| `T-S30-2` | holdout | S30,S29 | recovery-witness: 边界、限定或无答案 | docs/adr/0034-replayed-create-witness-recovery.md:1-30；docs/adr/0033-source-preserving-memory-units.md:1-39 | 是 | critical |

## 复核重点

- 查询是否自然、可答，且只依赖所列来源。
- 可用重点、必要限定、禁止推断和状态是否被对应固定字节直接支持。
- 无答案候选是否确实无法从所列 ADR 全文得答。
- 跨来源关系任务必须明确询问来源间关系，并引用一端的关系声明、另一端的基准规则及答案用途；不明确时删去关系 gold 或改为待裁决。
- source split 是否存在同一决定链泄漏；如有，重划并重算冻结字段。
- severity 是否反映困难度/失败影响；数量满足 draft 门槛不等于标签正确。
- 固定 HEAD、纳入/排除来源规则、跨 split 主题泄漏；新草案不得称为旧 gold 恢复。
- Post-run A/B/C 的盲评标签锁定后才能揭示映射；全部指标沿用实施计划原阈值，不得因 agent-reviewed 降低。

## 准备边界

目录仅含静态候选与评审索引。没有 Provider 配置、数据库、密钥、run_root、seal 或 claim，不能直接运行 prepare CLI。独立智能体复核并锁定新 hash 后，仍需生成新的 CLI DraftConfig 并单独准备私有运行根。
