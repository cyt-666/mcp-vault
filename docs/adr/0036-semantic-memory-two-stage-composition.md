# ADR-0036：语义记忆采用两阶段提取与卡片组合

- 状态：已接受
- 日期：2026-09-22
- 更新：2026-09-23

## 决策

M1 单来源提取在 Provider 边界分为 observation 与 composition 两次调用。第一次调用只生成经过业务校验的 observations；observation 输入 DTO 要求 observations 数组，outcome 可选且只能与数组空/非空状态一致。outcome 是可由程序从数组派生的冗余状态：语义服务在严格验证 observations 与来源 fence 后生成 canonical `SuccessEmpty` 或 `SuccessNonempty`。此规则不放宽文本、证据 ID、source/revision/hash/授权/generation/rules fence 校验。第二次调用只负责在范围内选择语义子分组和标题，card metadata 由服务从 observations 推导，最终复用原子 publication validator。

空结果直接完成 success_empty。任一阶段失败、取消、预算耗尽或 fence 漂移都终止 running extraction，禁止发布 partial。MiMo `json_object` 只保证 JSON 语法，远端仍可能遗漏字段或增加属性；本决定不保证 Provider 遵从提示或 schema。旧完整 proposal API 保留用于旧协议和历史数据，其完整 proposal JSON 仍要求原有 outcome，不与新 prepared handle 混用。

## 原因

模型同时填写 observation 与 card metadata 会把不同 kind/status 的 observations 错误合并。两阶段协议保留模型的语义分组职责，同时由程序控制跨 kind、scope、assertion status 和时间范围的边界。

## 约束

新协议标识为 `m1-two-stage-v2`，prompt/schema 版本递增；生产预算按选定来源、任务关系和阶段调用上界计算，当前 M6 上界为 151，预算必须覆盖该上界。
