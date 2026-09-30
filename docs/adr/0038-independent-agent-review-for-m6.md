# ADR-0038：M6 使用独立智能体进行语义评审

- 状态：已接受
- 日期：2026-09-28
- 关联：[语义记忆实施计划](../semantic-memory-implementation-plan.md) §12
- 实施：[语义记忆系统实施](../exec-plans/active/semantic-memory-implementation.md)

## 背景

M6 的既有计划要求人工逐项审阅真实语义输出。用户已明确授权以独立智能体复核替代该人工门槛。此变更只替换评审主体，不降低原有数值阈值、用例覆盖、证据要求或安全阻断项；评估报告必须准确标记 `human_review=false`，不得把 agent-reviewed 描述为人工审阅。

候选 gold 由数据集生产者编写。若生产者与评审者相同，或回答模型可读取 gold，评审就不能作为独立证据。holdout gold 一旦在 Provider 运行后变化，该 holdout 版本即受污染，不能继续作为未见评测。

## 决策

### 职责与独立性

- 数据集生产者（本执行计划中的 Luna）只负责从固定 Git 字节构造候选、引用证据和结构校验；不得自评 gold 或回答质量，也不得把未审候选标为 approved。
- 独立复核者（主代理 root）对 holdout gold 与证据作独立语义判断，并在 Provider 运行后对匿名化、随机编号的 A/B/C 输出盲评。生产者不得改写复核结论。
- pre-run gold 复核和 post-run 输出评分分别记录；后者在评分锁定前不得向评审者揭示 arm 映射。输出仅用随机 `blind_output_id`，arm mapping 单独保存在受限评测记录中。
- 所有记录均携带候选 SHA-256、固定 commit、source blob OID/SHA-256、任务 ID、精确行号和评审者身份/角色。生产者与复核者不得共享隐藏的提示、答案或 arm 映射。
- 模型仅可辅助标出歧义，不得作为唯一裁决者。证据不足、关键限定无法定位、来源关系方向不清或评审者利益冲突时，标记 `insufficient_evidence` 并 fail closed；不得推断为通过。

### Provider 与 gold 隔离

`query` 和必要来源内容可按冻结对照发给 Provider。`must_preserve`、`must_not_infer`、`expected_usable_information`、`expected_no_answer`、`expected_status`、`expected_source_relations`、severity、评审记录与候选答案不得进入任何 Provider 请求。gold 仅供离线评审和判分。

候选 hash 在独立 pre-run 复核后冻结。运行前后都校验同一候选 hash 和来源 fence。运行后修改 gold、查询、关系或来源划分即使措辞更准确，也会使原 holdout 结果不可用于验收；必须新建数据集版本并重新定义 holdout。

### 评分和状态

逐条记录事实性断言是否有同范围证据支持、必要限定是否完整、gold 重点是否在 pack 中可用、无答案是否表达缺口且未伪造当前依据、任务关键约束/状态/下一步/禁止推断是否满足。每项判断必须链接到输出位置和冻结来源证据；不给无证据的整体印象分。

状态必须区分 `pending`、`agent_reviewed`、`human_reviewed`、`insufficient_evidence` 与 `adjudicated`。Agent 评审的正式质量报告显式写 `human_review=false`，同时列出独立复核者、模型/版本、盲评映射何时揭示及裁决链。不得把 `agent_reviewed` 等同于 `human_reviewed`。存在未解决的 critical/high 错误或证据不足时，相关门槛不得判为通过。

### 保留既有发布门槛

本 ADR 不改变计划 §12.4 的任何门槛：陈述支持精度至少 95%，必要限定保留率至少 95%，已知重点覆盖至少 90%，冗余比较、无答案困难负例、任务结果、成本/延迟仍按既有定义和分母报告；C 的 task 结果点估计不得低于 A/B 中较好者。所有列明的权限、删除、来源失效、预算和恢复工程用例必须通过；任何泄露、遗忘复活、越权或关键错误都阻断验收。样本不足、未完成任务或失去证据均不允许降低分母或阈值。

## 后果

M6 可以由与候选生产者隔离的独立智能体完成语义复核，但结果必须标为 agent-reviewed 且 `human_review=false`。原有实现、Provider 隔离与安全不变量保持不变。该决定不批准当前候选 gold、不构成 M6 已通过，也不授权 Provider 请求、运行根、M7 或生产切换。
