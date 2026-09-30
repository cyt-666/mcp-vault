# M6 独立智能体复核与盲评流程

本文件落实 [ADR-0038](../../adr/0038-independent-agent-review-for-m6.md)。候选 producer 和 independent reviewer 是不同职责：producer 生成候选与证据包；root reviewer 独立判断 gold 与模型结果。producer 不自评、不改写评审结论。当前 candidate 与 pre-run 包仍为 pending；本文件不表示 gold approved、Provider 已运行或 M6 通过。

## Pre-run gold 审查

复核输入只用固定 candidate SHA-256 和固定 HEAD Git blob 生成的 [holdout 30 证据包](holdout-independent-review.zh-CN.md)。每个任务须逐项确认 query、必要可用重点、必要限定、禁止推断、正确状态、来源关系类型/方向、严重度和证据行号。所有引文必须由文件中的 Git blob OID/SHA-256 重取并逐字节核实。关系题至少核对两端与声明行；无答案题必须检查来源全文所有行，局部摘录只帮助导航，不能证明缺失。

`gold_reviewed` 冻结记录要求 30 个任务逐题完成，外层决定及八个字段决定均为终态；字段决定只能是 `supported` 或 `not_applicable`，且每项 rationale 非空。`no_answer_scope` 对无答案题必须为 `supported` 并逐来源附上 `1..EOF` 证据；对可回答题必须为 `not_applicable`。评审 evidence 必须属于该 task 的固定 `source_ids`，与固定 commit 的 Git blob OID、SHA-256、UTF-8 行数和行范围完全匹配。冻结时 reviewer 必须与 producer 身份不同，`human_review=false`，提供带时区的 `gold_locked_at`；answer/artifact scores 和 adjudications 均须为空，`quality_claim` 保持 `not_evaluated`。

结论逐任务记录为 `agent_reviewed`、`revise_candidate`、`insufficient_evidence` 或 `pending`。只有 30 项全部 `agent_reviewed` 且候选/package hash 固定后才允许 freeze。任何修订都产生新的 candidate version/hash；已经运行过的 holdout 不能通过事后改 gold 恢复为未见数据。审核记录固定 `human_review=false`；这表示没有人工复核，禁止将其称为人工已审。

## Provider 与后运行盲评隔离

gold 中只有 query 与来源材料属于实验输入；所有 `expected_*`、`must_preserve`、`must_not_infer`、关系和 severity 都留在本地 review side。answers 收集完后，由离线工具为每个 task/arm 生成至少 128-bit 随机 `blind_output_id`，去掉 answer/pack/config 中的 A/B/C 标识和按臂文件顺序。盲评者只取得任务、冻结 gold、pack/answer 内容及匿名引用，不取得 arm mapping、Provider 配置标签、消费次序或目录名。mapping 在独立受限文件中保存，评审逐题记录并锁定后才能揭示；揭示者记录时间与映射文件哈希。若无法证明匿名化/映射隔离，盲评门禁未满足。

Post-run 数据形状见 [JSON Schema](post-run-agent-review.schema.json) 和 [空白模板](post-run-agent-review.template.json)。`artifact_scores` 覆盖匿名 observations、cards、relations、packs；`blind_scores` 恰好覆盖 holdout 的 30×3 个 task answers。每条 fact claim 记 output locator、supported/unsupported/contradicted/unverifiable 与冻结证据；限定项逐项记 preserved/partial/lost；coverage 逐项记 gold item 是否在对应 pack 可用及是否用于 answer；no-answer 记是否承认缺口和无依据当前断言数量；任务结果分别记录关键约束、状态、下一步和禁止推断。记录引用输出位置，不复制完整模型响应。裁决记录独立保存评审意见、分歧、证据及最终裁决，不删除初审分歧。盲评数据不包含 arm label；匿名输出与原始 artifact 的映射由受限流程单独持有。

每题的固定 gold item ID 用 `usable:0..n-1` 对应 `expected_usable_information` 顺序，用 `preserve:0..n-1` 对应 `must_preserve` 顺序。不得增删或重复 ID；validator 按固定候选 hash 检查评分分母。

## 指标尺度和原门槛

分数按固定 gold 与实际分母计算，不允许审后删题、删断言或把 unresolved 计为成功：

- 支持精度 = `supported` factual claims / 全部 factual claims；partial/unsupported/contradicted/unverifiable 均不进分子。整体值沿用至少 95% 门槛，并按 semantic artifact、pack、answer 分层报告，防止汇总隐藏局部退化。
- 必要限定保留率 = `preserved` 必要限定项 / 全部冻结必要限定项；`partial`、`lost` 和不可判定不进分子。
- 已知重点覆盖 = pack 中可用的 gold item / 全部冻结 `expected_usable_information` items；另报告答案实际采用数，不得用无关全选顶替相关性。
- 无答案困难负例必须全部正确承认缺口，且 `unsupported_current_claim_count=0`；已知冒充依据错误是阻断项。
- 任务结果沿用 §12.4：固定 `critical_constraints/state/next_step/forbidden_inference` rubric，每维 `met=1`、`not_met=0`，task score 为四维等权平均；`insufficient_evidence` 使该项 unresolved，不得删出分母。A/B/C 用同一 rubric、同一 holdout 任务及 30 项分母，逐类报告失败；C 点估计不得低于 A/B 中较高者。不能用总均分掩盖关键约束错误。
- 支持精度仍至少 95%，必要限定保留率仍至少 95%，已知重点覆盖仍至少 90%；关键操作条件用例全部通过。
- 权限、删除、来源失效、预算、幂等与恢复用例全部通过；任何越权/私密内容泄露、删除/抑制复活、关键来源失效漏读、超预算或部分发布都阻断。请求、延迟、实际 usage/cost 缺失如实标 unknown，禁止填零。

分母为零时标为 not-evaluable，不能声称 100%；样本或记录不足、审查证据不全、评分 unresolved 均保持 pending/insufficient-evidence，不得 pass。Agent reviewed 和 human reviewed 是互斥 provenance 字段；后续可选人工复核另开记录，不能覆盖 agent 结果。

## 最小校验

```text
python3 docs/eval/m6-2026-09-28-candidate/generate_holdout_review.py --check
python3 docs/eval/m6-2026-09-28-candidate/validate_agent_review_record.py
```

校验器只验证 hash、来源坐标、任务覆盖和 blind record 结构，不判断 query 或 gold 的语义正确性，不代表 M6 达标。
