# ADR-0044：M6 有界 A80 提炼与隔离恢复

- 状态：已接受
- 日期：2026-09-29
- 范围：只改变 M6 live evaluation 的 observation 提炼、checkpoint/resume 和 item-failure 行为；MCP、生产 extraction、其它 Provider 与已锁定 gold 不变。

## 背景

R1–D13 显示 MiMo `json_object`/strict-function 长结构输出无法稳定满足完整 Observation schema。逐字段更改提示和继续单源 wire 试验不能解决这一产品边界。模型应只负责提出简短、带来源索引的候选陈述，稳定身份、证据、卡片形状和事务边界由服务确定性处理。

## 决策

M6 A80 observation 请求每片最多包含 80 个确定性逻辑 source blocks。Provider 输入仅含逻辑 block 文本、行号、类型和批次序号；JSON 核心响应为 `claims: [{statement, evidence_indices}]`，可选 advisory 字段仍受白名单/归一化规则约束。模型不接收 Vault/source ID、content hash、offset，也不需要回填 namespace。服务端 prepared handle 将响应绑定到当前 Vault、source revision、catalog/input hash、batch index、prompt/schema 和 Provider fingerprint；只有该 handle 对应的当前调用结果才可提交，normalized proposal 中的稳定证据身份由服务端映射注入。整数索引必须是有效 JSON integer，属于本批允许范围且字段内无重复；未知字段、无效 statement/index、未知/越界/重复索引均拒绝，不进行猜测修复。不得声称省略 namespace 的 JSON 自身能证明来源身份。

可选语义 advisory 不能控制来源身份或证据。advisory 缺失或不属于白名单时，保留 statement 与已验证证据，并确定性记为 `unknown`/`unspecified`，只累计安全计数。source time evidence 仍须通过原有当前 block、角色、时间范围、span 和 hash fences。

2026-10-11兼容补充：A80仅把精确历史提示 `stated` 识别为 `source_asserted`，其语义仅为“来源这样陈述”，不授予当前有效、采纳或验证状态，不推定时间范围。prompt v16显式列出规范值，要求每条相关历史陈述保留其限定；其他非法提示继续回退，legacy完整提案入口不接受该别名。保存的响应可离线检验此确定性映射，但不能据此声称历史限定或新提示的模型质量通过。

每个 batch 在 State 中绑定 Vault、source revision、catalog/input/prompt/schema/provider fingerprints。调用前先原子 reserve attempt；崩溃留下的 `dispatching` 变为 `uncertain` 并按已消耗预算计数，绝不默认重放。明确的结构/核心响应错误可在 source fence 复核后消耗该 source-arm 唯一 regen token；token 与第二次 batch reservation 在 State 同一事务中提交，跨全部 batches 共享并跨重启保留。仅严格验证通过的 batch 能持久为 validated proposal；同一 claim 中所有片验证且最终 source fence 通过后，才经既有 source-set 原子发布路径提交。重启可复用匹配的 validated batches；source、provider、template、预算或manifest漂移 fail closed。

一次成功发布生成一条 Observation 和一张本地确定性 MemoryCard。Card/answer pack 的证据摘录从 EvidenceRef/span/hash 回读规范原文，不能用模型回述代替。Relation 是可选增强；relation item 的失败不应用状态但不阻断卡片或答案。单题 answer/pack/retrieval 错误记录到权限受限的 safe `item-failures.jsonl` 并保留评分分母，继续其它独立臂和任务；来源、权限、Provider identity、预算或全局配置 fence 漂移仍终止运行。任何 item failure 都使最终质量状态保持 `not_evaluated`，不得报告通过。

M6 live runner 使用唯一私有 lease 防止同 root 并发。恢复仅接受相同 sealed manifest/config/provider fingerprints。每次 A80 call intent 先原子写私有 `attempts.jsonl` 为 `reserved`，再 reserve State batch；成功后状态改成 `started`。`reserved`、`started` 与崩溃后确认的 `uncertain` 都单调消耗共享 160 请求预算，所以该预算计数是保守reservation数，不表示每条都已到达HTTP Provider。已有 terminal answer/card artifact 可跳过；State中已validated且proposal hash匹配的batch也可跳过；未validated batch若 ledger已有对应 batch attempt则视为 outcome uncertain 或已消耗的未完成意图，绝不重发。A80 batch 状态与卡片 publication 不允许部分 source set 对 recall 可见。计算式中的 relation/answer regeneration 余量只有在相应 retry 路径实现后才算可用；当前保留为未使用的容量余量，不会为了花完预算增加请求。

A80 observation、relation 与 answer 三阶段对 Provider auth/secret、配置或身份漂移、权限/端点/能力拒绝、State 错误及请求预算耗尽统一 fail-fast，停止整个 run；这些全局错误不得转写成可继续的 source/task item failure。普通单项传输或局部响应格式错误仍依对应阶段的隔离规则处理。

## 兼容与迁移

A80 为新的 versioned Eval protocol，不替换 legacy complete-proposal API；旧 proposal 继续严格验证重叠、未知字段及 EvidenceRef 约束。数据库迁移 `0043_semantic_observation_batches.sql` 仅增加 Vault-scoped batch/checkpoint 状态，不改写旧卡片或源文件。未知 kind 使用显式兼容存储标记并在读模型呈现为 `unknown`，不借用已有 kind。

## 验证

- 固定逻辑分片函数同时用于 prepare-time budget upper-bound 和运行时实际 catalog，并在开始运行前逐source核对 manifest block counts/batch counts。
- A80 wire 版本为 `semantic-cards-tracked-adr-m6-v14` / `semantic-cards-m6-json-v10`；v13/v9 和 legacy complete-proposal wire 保持原契约。
- 本地 fake 必须覆盖 schema/source-local index fail-closed、revision drift、validated batch resume、crash uncertain、不重复计费、单源原子发布、span/hash excerpts、relation failure隔离、answer failure分母及后续 task 继续。
- 完整离线 workspace gate 通过后，先由主代理复核代码和容量测量；此 ADR 本身不授权新的 Provider root 或真实运行。

## 取舍

A80 牺牲模型自由组织复合卡片的能力，以可审计的低复杂度提案和确定性证据绑定换取更可靠的 source-local 提炼。没有有效核心陈述或证据时仍失败，不以“unknown”掩盖核心缺失。任务 item failure 可继续收集独立结果，但最终整体质量验收仍 fail closed。
