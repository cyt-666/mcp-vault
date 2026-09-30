# ADR-0039：模型可见证据块 ID 绑定来源修订

- 状态：已接受（observation Provider 引用格式由 ADR-0040 修订）
- 日期：2026-09-28

## 背景

M6 Agent V1 Full R1 的 B/S17 observation 在本地结构校验时因时间证据数组的动态 enum 不匹配而首错终止。安全诊断仅证明返回值不属于该请求提供的 block ID 集合，无法区分模型复制错误或 MiMo `json_object` 未遵从 prompt schema；R1 不重试且不重用。

现有 block ID 使用序号和行文本 hash 前缀。相同来源块在不同来源或修订中可能得到同一 ID；若错误地将一份合法响应绑定到另一来源的 prepared handle，纯序号会更容易把它映射到目标来源同序号块。来源 fence 校验目标 extraction 的来源修订与原文，不识别 Provider 响应最初属于哪个请求。

## 决策

模型可见 block ID 是临时值，格式为 `b{revision-tag}-{base36-ordinal}`。revision-tag 的摘要输入包含固定域分隔符、Vault ID 和完整 SourceRevisionId；不从 UUIDv7 字符串前缀截取。常规 tag 从 SHA-256 的 12 个小写十六进制字符开始，至少保留现有 48 位前缀碰撞强度。

生成 tag 前，服务从当前 Vault 的 State 查询全部历史 SourceRevisionId，并逐字节扩展摘要前缀，直到该 tag 在该集合中唯一。查询严格限定 Vault。若完整 SHA-256 仍与另一修订相同，则使用该 SourceRevisionId 的完整规范 UUID 作为备用 tag；数据库的 `(vault_id, source_revision_id)` 唯一键确保此备用身份不会在同一 Vault 静默复用。若目标修订缺失或 State 身份无法解析，则拒绝准备。

准备时保存选定 namespace，并在首次 Provider 调用前及 observation proposal 解析前再次计算；来源修订集合发生足以改变 namespace 的变化时，当前 generation 失败终止。composition 不再解析 block ID，因此只保留来源/修订 fence，不重复扫描 namespace。输入 block、动态 JSON Schema enum 和本地 `LocalBlock` 解析使用同一完整 ID；未知、重复和跨来源/修订 ID 均严格拒绝，不自动纠错。序号以 checked `usize` 递增并用小写 base36 编码；溢出或非法零序号失败。

ID 不写入 State、EvidenceRef、卡片、MCP/Admin DTO 或 canonical Markdown。持久证据继续保存来源修订绑定的 byte spans 和内容哈希；旧 revision rebind 仍按原文 spans/hash 重建和验证，不读取或依赖历史 block ID。因此不需要数据库迁移或持久数据转换。

M6 observation prompt/schema 版本分别提升到 `semantic-cards-tracked-adr-m6-v8` 与 `semantic-cards-m6-json-v5`。所有旧 sealed run 继续绑定其原有 prompt/schema，不得复用来评价新协议。首错停止和零重试规则保持不变。

## 碰撞与并发语义

短 tag 的唯一性不是只依赖截断 hash 的概率：同一 Vault 的修订身份清单用于检测并扩展冲突，完整摘要冲突使用精确 revision UUID 备用身份。修订在 prepared generation 期间新增时，接收/业务验证前重算 namespace；检测到 namespace 变化就失败关闭。该实现每次准备及接受阶段读取该 Vault 的修订 ID 清单，时间/内存复杂度随该 Vault 历史修订数线性增长。若规模测量显示该扫描不可接受，后续设计须保留相同 fail-closed 唯一性，不得退回无检查的截断前缀。

## 验收

- 不同 source、相同首行及相同 revision 内容都不能共享模型可见 namespace。
- 同一来源重复行使用不同 ordinal，精确映射到各自 spans。
- 未知 ID、重复 ID、跨来源重放与修订漂移拒绝且不发布 partial。
- tag 前缀碰撞扩展，完整摘要碰撞回退完整 revision UUID；无法验证来源时准备失败。
- 有效 time evidence 可经动态 enum、业务证据验证并持久化成原有 spans。
- 历史 rebind 仅依赖 revision 内容及 spans/hash，数据库与 MCP/Admin DTO 无模型 block ID。
