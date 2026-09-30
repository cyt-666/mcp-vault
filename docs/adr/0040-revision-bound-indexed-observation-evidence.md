# ADR-0040：来源修订绑定的 observation 整数证据索引

- 状态：已接受
- 日期：2026-09-29
- 修订：ADR-0039 保留其 Vault/source-revision namespace 与碰撞规则；本 ADR 修订模型可见 evidence 引用格式。

## 背景

M6 R1 和 R2 都在第 3 次 Provider 请求、B observation 阶段以 `provider_schema_invalid / enum_mismatch` 失败。R1 安全路径为 `source_time_scope.evidence_block_ids[3]`；R2 在短化 block ID 后仍于 `body_block_ids[3]` 失败。输入 block catalog 与动态 enum 同源，adapter 对 MiMo 使用 `response_format=json_object` 并在本地按完整 JSON Schema 验证。仅凭保留的安全诊断无法区分模型复制错误、生成不在 enum 内的值或其他模型输出偏差；原始输出没有被读取。失败清理与 unknown ID fail-closed 正确工作，不能用模糊匹配或重试绕过。

## 决策

M6 observation Provider 输入不再暴露每个 block 的 local ID。输入包含单个 `evidence_namespace` 和按 prepared block 顺序从 1 开始的 `evidence_index`，以及文本、行号和块类型。`evidence_namespace` 是 ADR-0039 生成的当前 Vault + 完整 SourceRevisionId 绑定 namespace；它不包含 Vault ID、来源路径、SourceRevisionId UUID 或任务 gold。

observation 响应必须携带一个必填 `evidence_namespace`。resolved JSON Schema 将它限制为本次 prepared generation 的唯一值；`body_block_indices`、`context_block_indices` 和 `source_time_scope.evidence_block_indices` 的动态 enum 限定为 `1..=prepared block count`。三个字段只接受 JSON integer，拒绝字符串、浮点、布尔、零、越界值及各自列表内重复索引。每个列表单独检查；同一有效索引可以同时作为 body 与时间证据，既有 memory 业务规则继续裁决 body/context 重叠及时间限定。

Provider 返回后，Eval adapter 要求 namespace 与保存的 `SemanticPreparedGeneration` 精确相同，拒绝缺失 namespace、跨 source/跨 revision 响应及未知字段。它按该 prepared generation 的原始 block 顺序把索引严格映射回原 local IDs，并移除临时 namespace/index 字段；随后调用现有 `accept_observation_result`。Memory service 仍重验 Vault/source/revision generation fence、namespace inventory、byte spans 和内容 hash，再验证时间证据并进入 composition。映射不写入 State，也不改变持久 evidence schema。

Legacy 完整 proposal API 和旧 `prepare_legacy_for_arm` 保持原 contract。Composition、relation、answer 阶段不解析 observation block 索引，也不增加 namespace 扫描。

Observation prompt/schema 升为 `semantic-cards-tracked-adr-m6-v9` / `semantic-cards-m6-json-v6`。所有已 seal 的旧 run 固定其原 prompt/schema，R1/R2 不复用。Provider retries 保持 0；schema 错误继续终止当前 extraction，不自动修正或重发。

## 安全性质与限制

- 只使用 prepared generation 的 namespace 和 block 序列进行映射；source A 的 namespace/index 响应不能静默绑定到 source B，即使首行文本及整数索引相同。
- namespace 变更会被精确拒绝；block index 只在当前 prepared block catalog 内解释。
- 48-bit 最小 namespace tag、Vault 范围内的前缀碰撞扩展、完整 SHA-256 碰撞 fallback，以及历史 revision inventory fence 仍由 ADR-0039 管理。
- MiMo JSON object mode 仍不保证远端严格执行 JSON Schema；应用侧 schema validator 和 index mapper 都必须 fail closed。
- namespace 仍可能被模型输出错误，因此重放/错误值仍会导致本次 extraction 失败；本 ADR 降低重复复制 ID 的负担，不声称消除模型不遵从问题。

## 验收

- resolved enum、Provider input catalog 和 local mapping 使用同一 1-based block 顺序。
- 缺失/错误 namespace、跨源重放（包括相同首行）、source revision drift、浮点/字符串/布尔/零/越界/重复索引及额外字段均拒绝，并终态化 prepared extraction。
- 有效 body/context/time 索引映射回正确原 local IDs，再由既有 span/hash/source fence 验证；时间证据可与 body 共享索引。
- 历史 rebind 仍依据旧/新 revision 的内容、byte spans 和 hash 重建，不依赖模型可见索引或旧 local ID。
- legacy complete proposal 与 composition 流程不变。

## 2026-09-29 修订：M6 v7 body/context 同块归一化

D10 首次验证了 strict non-stream function response、v7 schema 和 source-revision index mapping，随后在 Memory 接受阶段因同一 observation 的 body/context 指向完全相同 block 被拒绝。当前 provider prompt 已要求两种角色不重叠；问题是跨数组互斥不能由该 JSON Schema表达。

仅对 M6 v7 observation wire：在 namespace、完整 schema、每字段整数/范围/重复校验均通过，索引映射回当前 prepared generation 的 local IDs 后，对同一 observation 执行 `context_block_ids -= body_block_ids`。body 优先；只移除完全相同 local ID 的重复角色，不按文本、hash、行号或语义相似去重。body/context 证据并集不变，原文、byte spans、hash、source revision fence 和时间证据语义不变；时间证据仍可与 body 共享索引。后续仍走原 Memory Vault/source/revision/span/hash 校验和 composition。

该归一化不接收无效或未知 ID，不更正非法 namespace/index，也不放宽每个索引列表内重复值拒绝。安全诊断只记录移除数量，不记录 ID、行号或来源正文。legacy complete-proposal API 仍拒绝显式 body/context 重叠；既有 legacy fail-closed 行为不变。Wire prompt/schema 仍为 v11/v7，版本化安全 diagnostic report 增加归一化计数并升级到 v2。

本修订允许同一已验证证据块兼任直接支持与解释上下文时，以 body 角色保留唯一 span，避免整个 observation 因跨数组冗余而失败。它不允许来源证据缺失或削弱独立 context 块。
