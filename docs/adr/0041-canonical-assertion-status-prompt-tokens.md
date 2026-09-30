# ADR-0041：observation prompt 明确 assertion status 规范 token

- 状态：已接受
- 日期：2026-09-29
- 修订：不改变 ADR-0040 的 indexed evidence schema；该 prompt 仅在 JSON-object wire 期间为 v10，v11 由 ADR-0042 规定。

## 背景

M6 R3 在 7 次 Provider 请求后，于 B/S18 的 observation 阶段终止。安全诊断为 `provider_schema_invalid / enum_mismatch`，路径是 `$.observations[6].assertion_status`。R1/R2 的错误是 evidence ID enum mismatch，v9/v6 indexed namespace/index 修复后错误路径已转移到静态 assertion status enum。schema diagnostic 未保留响应值或结构化响应摘录，因此无法确定模型输出了哪个非法 token。代码检查未发现动态 schema 与请求 schema 漂移：`assertion_status` 的 schema enum 与 Memory 的七值解析契约一致；MiMo `json_object` 由同一请求 schema 进行本地严格验证。

v9 的 JSON Schema 已包含完整 enum，但手写 prompt 没有逐项解释七个合法值，只提到 accepted ADR 可标记为 adopted。assertion status 影响来源表达的权威边界、memory card 的持久字段、等价合并判断和 MemoryPack 输出，不能从 kind、scope、时间或 evidence index 推导。

## 决策

Observation prompt 升为 `semantic-cards-tracked-adr-m6-v10`，schema 保持 `semantic-cards-m6-json-v6`。手写 prompt 逐字列出 `source_asserted`、`proposed`、`adopted`、`committed`、`observed`、`rejected`、`unknown`，说明各值含义；明确不得照抄来源状态同义词、不得因语气推断状态，无法确定时选择 `unknown`。

动态 schema 和本地 validator 继续只接受以上七个 enum 值。Adapter 不自动翻译同义词、不把非法值归为 `unknown`，未知状态仍按 `provider_schema_invalid` 终止当前 extraction 并执行既有清理；不重试、不降低 schema 严格度。其余 v9/v6 namespace/index 行为、持久数据契约、旧 run seal 均不变。

## 限制

此次修改是提示清晰度改进，不证明模型一定遵守 schema。R3 错误 token 的具体内容未知；不能据此断言这次失败由特定同义词造成。若 R4 仍有 enum mismatch，应继续只凭安全字段诊断并保留失败现场。

## 验收

- 模板测试断言 prompt 七 token 与 schema enum 完全相同。
- 本地 HTTP fake 验证非法 assertion status 得到 `enum_mismatch` 和精确 schema path，且不会被改写或送入 Memory acceptance。
- R4 必须用全新隔离 root；不得复用 R1/R2/R3。
