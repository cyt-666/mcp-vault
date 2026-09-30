# ADR-0043：M6 全部结构化阶段使用 MiMo strict function output

- 状态：已接受
- 日期：2026-09-29
- 修订：仅改变隔离 M6 evaluation 的 MiMo observation、composition、relation、answer Provider wire；普通 MiMo、production extraction 与其它 Provider 默认路径不变。

## 背景

D11 的 observation 已成功通过 v7 schema、namespace/index 映射和 body-first 证据角色归一化，随后 relation 工作仍使用普通 streaming `json_object` 并以泛化 `provider_response_invalid` 终止。之前 strict function 只覆盖 observation，令同一评测纵切在后续结构化阶段回到不具结构约束的协议。此前 R1–D11 均未产生 task score，不能据此声明 M6 语义质量通过。

## 决策

M6 模板版本升级到 `semantic-cards-tracked-adr-m6-v12` / `semantic-cards-m6-json-v8`。仅当 Provider 为 MiMo 且 prompt/schema ID 和阶段精确匹配 observation、composition、relation 或 answer 时，Eval 使用单一 strict function、`tool_choice=auto`、`thinking.type=disabled`、`stream=false`。不发送 `response_format`。其它 Provider、普通 MiMo 请求和 production extraction 保持既有 wire 路径。

完整响应必须包含唯一 choice/index 0、唯一且名称匹配的 function tool、`finish_reason=tool_calls`、无非空 content，以及类型/ID/arguments 正确且不超限。No-tool、错名、多工具、混合正文、非法 JSON、截断、超限和不受支持 schema 全部 fail closed，不读文本 JSON fallback，不做 retry 或自动修复。Provider strict-schema 验证后，Eval 仍执行原阶段 schema 与业务验证；observation namespace/index、Vault/source/revision/span/hash 与 task fences保持不变。

Relation wire 对每个 action 将 `card_ref`、`title`、`content`、`item_kind`、`support_operator`、`reason` 六个字段声明为必填字符串。仅这些字段允许精确空字符串作为“不适用”标记；`item_kind` 和 `support_operator` 的 wire enum 因该标记移除。验证顺序为：strict wire schema → Eval 仅对这六个字段执行 `""` 到字段缺省的逐项映射 → 原有 relation schema → 现有 relation/M2 业务 fence。缺字段、非字符串、未知字段和其它不合法值拒绝；不做通用空值转换。其它三个阶段不做该归一化。

实现复用现有一次性非流式 JSON transport 与限制，并保留 typed safe protocol issue；严格 function 参数仍在内存中处理。该变更不改 candidate/gold、预算或质量阈值。

## 验收与边界

- 本地 HTTP fake 覆盖四阶段请求 wire、四阶段合法响应及 no-tool、错名、多工具、混合 content、非法 arguments、schema 错值/缺字段/未知字段、响应超限等失败路径。
- Relation fake 覆盖六个空字符串哨兵和非空合法值；映射后仍通过原本地 schema。普通 MiMo JSON-object 与其它 Provider 的请求兼容测试保持通过。
- 离线门禁通过后，只运行全新 D12 B/S17 单来源 diagnostic，最多两个 Provider 请求、零任务评分。D12 失败则停止 D13/full R5；成功只解锁 full R5 准备，不代表 M6 通过。只有完整 holdout 评分满足 ADR-0038/原阈值才能作 M6 质量结论。
