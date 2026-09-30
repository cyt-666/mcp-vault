# 语义记忆数据与接口契约

本契约冻结 M0 的语义对象、来源资格和服务边界，供 M1—M7 实施。它遵循 [语义记忆实施计划](semantic-memory-implementation-plan.md) 和 [ADR-0035](adr/0035-semantic-memory-cards-and-task-packs.md)。逻辑职责不可合并；物理表名和 Rust 类型可在不改变语义的前提下调整。

## 当前实现范围

M1 只接收已启用自动记忆的 Vault 中、符合现有隐私和路径授权规则的 Markdown 笔记。当前来源身份复用稳定 File ID、Vault Core 修订和内容哈希。对话、工具记录、外部来源等 SourceKind 只作为未来扩展点，本阶段不读取、不存储，也不宣称已支持。显式记忆继续保留独立所有权和原始授权文本。

所有 Provider 输入受 Vault 当前授权/隐私配置控制。没有本轮明确授权时不得调用收费 Provider、发送真实资料或访问生产 Vault。工程测试可以用合成资料和 fake/replay 响应验证代码机制，但不能以此声明真实语义验收通过。

## Source 与 SourceRevision

`Source` 表示经授权进入记忆流程的来源身份。`SourceRevision` 表示不可变的来源版本。两者由服务和当前授权系统绑定，不由模型创建或改写。

| 对象 | 必需逻辑字段 | 规则 |
|---|---|---|
| `Source` | `vault_id`、`source_id`、`source_kind`、稳定定位符、作者/执行者、授权引用、创建时间、当前版本引用 | M1 `source_kind` 仅为 `markdown_note`；笔记定位使用稳定 File ID。授权引用指向当前权限/配置，不复制成永久 ACL 文本。 |
| `SourceRevision` | `vault_id`、`source_id`、版本标识、内容哈希、规范原文引用、系统记录时间、来源自述时间、元数据、沿袭引用、可用状态 | 系统记录时间与来源自述时间分开。未知作者、时间、版本或状态保持未知，不由导入时间和标题推断。原文仍在 Vault 文件系统，不在记忆表重复存为第二份规范正文。 |

更新、删除、路径移动和授权变化按稳定来源身份判定。只有 File ID 与内容版本均匹配时才沿用来源关系；同路径删除后重建不得继承旧身份。路径移动但 File ID 和内容不变时可更新导航信息。

## EvidenceRef 与 Span

`EvidenceRef` 将 Observation 的语义陈述绑定到特定 `SourceRevision` 的原文位置。

```text
EvidenceRef
  id, vault_id, source_revision_id,
  body_spans[], context_spans[], lineage_root_refs[], validation_status

Span
  start_byte, end_byte, hash, optional line_navigation
```

`start_byte` 是 UTF-8 原文的包含起点，`end_byte` 是不包含终点。服务在发布前校验边界、UTF-8 字符边界、片段哈希和来源版本。行号只供导航，不作为跨版本定位依据。

`body_spans` 支持具体陈述；`context_spans` 保留影响解释的标题、前提、范围、版本、否定、例外、角色或任务状态。沿袭引用用于识别多份转述是否来自同一根来源，不能把转述次数当成独立验证次数。模型只引用本次输入实际展示的临时证据。M6 v11/v7 observation wire 使用绑定 Vault 与完整 SourceRevisionId 的 namespace 和 1-based block indices；Eval adapter 在严格校验后映射为该 prepared generation 的临时 block ID。对 M6 v7 同一 observation 的 body/context 完全相同 local ID，adapter 在映射后按 body 优先从 context 移除重复角色；证据并集和原文 span/hash 不变，安全诊断只记录移除数量。该规则不适用于 legacy complete-proposal API，后者仍拒绝显式角色重叠。M6 v12/v8 对四个结构化阶段使用 MiMo strict function non-stream wire；composition/relation/answer 不改变其原本地验证和业务 fence。Relation wire 六个可选字符串用必填精确字符串表示，仅空字符串映射为字段缺省，再通过原 relation schema 验证。Memory service 再通过 Vault 内修订清单、span 和内容 hash 验证并绑定稳定 EvidenceRef。其他 legacy API 可继续使用本地临时 block ID。未展示部分须经有界扩展读取，不能接受模型编造的位置。

## ExtractionSet 与 Observation

`ExtractionSet` 是一个来源版本的完整提炼结果。系统保存规则/profile、输入哈希、作业状态和预期来源修订。状态至少区分：成功非空、成功空、部分、失败、取消。失败或取消不能发布成空集合；部分批次不能冒充完成。

`Observation` 是一个来源级的语义提炼记录，至少具有以下逻辑字段：

```text
id, vault_id, source_id, source_revision_id, extraction_set_id,
kind, statement, scope, assertion_status, source_time_scope?,
conditions[], exceptions[], ordered_steps[]?, result?, uncertainty?,
evidence_refs[], admission_reason, value_for_future_work,
extraction_profile_id, supersession_candidate_refs[]?
```

`statement` 可以是一段语义完整的表述，不强制三元组或单句。类型至少覆盖 `preference`、`constraint`、`decision`、`experience`、`procedure`、`state`。范围和类型互相独立：范围为 `user`、`project`、`task` 或 `unspecified`；内容类型为上述类型。`unspecified` 不等于全局适用。

`assertion_status` 至少区分 `source_asserted`、`proposed`、`adopted`、`committed`、`observed`、`rejected` 和 `unknown`。模型语气与置信度不得改变来源权威或该状态。任务是否完成、当前读取资格、证据版本及作业状态分别管理。字段不适用时省略，不为填满结构而编造。

身份、持久 ID、Vault、路径、权限、修订号、证据坐标和写入动作由服务绑定。模型提案须作为完整结果集验证，检查结构、展示过的证据、来源版本、授权和纠正/抑制版本，再原子发布。

旧两阶段 observation Provider 输入使用独立 DTO：顶层必须含 `observations` 数组，可选 `outcome` 仅允许 `success_empty` 或 `success_nonempty`，且提供时必须与数组是否为空一致。`outcome` 是从数组可确定的程序派生冗余状态；语义服务在完成 observation 内容、证据 ID 和来源 fence 严格校验后，将其规范化为 canonical `SuccessEmpty` 或 `SuccessNonempty`。卡片字段及未知属性均拒绝。此规则不修补 Provider 正文、证据 ID 或额外键，也不降低语义和来源校验。旧完整 proposal JSON 入口继续要求必填 `outcome`，保持原有兼容契约。

当前 M6 live extraction 按 [ADR-0044](adr/0044-bounded-a80-extraction-and-isolated-evaluation.md) 使用 A80 低复杂度 proposal：每次最多展示80个逻辑来源块，模型只给简短 `statement` 和来源本地整数 evidence indices；Vault/source revision namespace 必须精确匹配，索引严格为 1-based JSON integer、范围内且字段内不重复。advisory kind/scope/status/time 缺省或非法白名单值由确定性 adapter 显式规范为 `unknown`/`unspecified` 并计数，不丢弃 statement 或证据。来源身份由服务绑定；未知字段或无效核心/证据仍失败关闭。

同一 claim 的所有批次验证后才原子发布；validated batch 可在完全匹配的私有 root 中恢复复用，started/in-flight 请求计入单调预算且不得重放。M6 runner 继续其它独立来源、arm 和任务时，只隔离记录内容/item 错误；来源、权限、Provider 身份、预算或全局配置 fence 漂移仍终止。失败项继续占质量评估分母，任何 item failure 均使整次质量声明为 `not_evaluated`。Legacy complete-proposal API 及普通生产 Provider 路径不使用 A80 adapter。

## MemoryCard 与 CardRevision

`MemoryCard` 是围绕一个可使用的问题、决定、经验、流程或状态组织的稳定身份。`CardRevision` 保存面向未来工作的语义整理，不以原文标题或章节树划分卡片。

```text
MemoryCard
  id, vault_id, topic_key, kind, scope_ref,
  current_revision_id, created_at

CardRevision
  id, vault_id, card_id, revision_number,
  title, core_assertions[], required_qualifiers[],
  optional_details[], unresolved_items[], observation_refs[],
  evidence_supports[], source_dependencies[], assertion_status,
  temporal_scope, last_confirmed_at?, composition_profile_id, created_at
```

每个断言和必要限定都要分别绑定有效 Observation/EvidenceRef。来源依赖只标识本卡涉及来源，不能代替逐项支持。卡片结构应支持 AND/OR：AND 表示结论依赖一组来源/断言共同成立；OR 表示其中任一已验证来源可独立支持同一完整命题及限定。M1 可只实现单来源支持，但存储契约不得退化为卡片级总来源列表。

必要限定失去支持时，相关核心断言必须一并失去当前读取资格，不能变成范围更宽或更强的结论。标题、标签和检索提示只用于导航。简短视图必须从同一结构确定性渲染；预算不足时省略整张卡并返回获准的入口，不能截掉否定、前提、例外、顺序或验证条件。

发布的耐久 MemoryCard 通过 Vault Core 物化为受管 Markdown，以保持知识可移植。规范文件包含当前卡片修订、断言结构、必要限定，以及解释卡片所需的支持/证据引用。SQLite 中的搜索、向量和卡片查询投影可重建，不能成为唯一副本。受管 Markdown 不作为自动提炼的新来源，防止卡片和任务包递归成为证据。普通笔记始终不被记忆整理改写。

卡片发布横跨 SQLite 和 Vault Core，不假设两者天然共享一个事务。Core 写入前，State 持久化 Vault 作用域的 prepared snapshot，记录预期旧卡片/规范文件修订、拟发布修订、完整规范字节哈希、支持引用、来源依赖和策略版本。应用随后通过 Vault Core 和正常 journal/修订边界写入受管卡片，再按预期卡片修订 CAS 提交 State 当前投影，并标记 snapshot 已应用。重启恢复时，对照 snapshot、Core journal、当前规范修订和精确字节哈希。只有身份、修订和哈希都匹配，才能补完投影。若写入缺失或无法判定，卡片保持不可读并留下安全诊断，不发布混合投影，也不覆盖较新文件。已验证 snapshot 的重试必须幂等。卡片当前资格仍按来源和权限计算，不能由 snapshot 是否存在决定。

## 支持、关系与状态

每项可检验陈述和关键限定按项记录支持。来源增加或失效不得让不支持的内容继续被读取。关系类型至少区分 `equivalent`、`supplements`、`supersedes`、`conflicts`、`related` 和 `different_scope`。相似度只帮助找候选，不构成上述语义关系的证据。

下列状态维度不可合并为一个 `status`：

| 维度 | 示例 | 权威来源 |
|---|---|---|
| 来源表达状态 | `proposed`、`adopted`、`committed`、`observed`、`rejected`、`unknown` | 具体来源证据及用户授权 |
| 读取资格 | 当前可用、待重评、冲突、仅历史、已抑制 | 服务按来源资格、逐项支持和用户规则计算 |
| 作业状态 | 排队、执行中、检查点、成功、失败、取消 | 持久作业与完整结果集 |
| 任务状态 | 未开始、进行中、阻塞、完成等 | 有证据支持的任务 Observation；不能以过期天数推断完成 |

导入时间、模型自报置信度、相似度及新文件名都不能单独证明事实时间、正确性、采纳或替代关系。

## Correction 与 Suppression

`Correction` 和 `Suppression` 是独立、持久、Vault 作用域的规则，不依附于临时卡 ID。逻辑字段至少包含稳定语义目标、适用范围、授权主体、创建/修订版本及适用状态；Correction 另保存用户明确表述或指向其明确来源。服务在提交提炼/卡片结果前比较规则版本。与纠正矛盾的旧来源不得静默写回；新证据可形成待核实冲突。

Suppression 表达“不再将目标内容用于指定范围/不再由指定来源重建”等规则。若请求彻底遗忘，不在 Suppression 中重复保存完整敏感正文；保存执行所需的最小稳定目标、来源身份和范围策略。缓存、FTS、向量、候选、来源导航及任务包必须立即按规则失去资格，异步物理清理不得延迟权限效果。

切换或回滚读取器时继续应用最新 Correction、Suppression、来源删除和权限版本。旧读取器若不能满足这些规则，就不能切回其结果；可用安全来源检索或暂停相关读取。

## 隔离、授权与发布

- 所有主表和子表均含 `vault_id`；复合主键/唯一键和外键包含 `vault_id`。每条查询和缓存键均限定 Vault。
- 从认证端点及凭据构造 `VaultContext`。接口不接受任意 `vault_id`。
- M1 使用现有 Vault opt-in 与 Markdown 隐私、路径授权规则。发往 Provider 前仍执行当前出域策略；禁止真实资料未经授权外发。
- 来源版本、身份、授权或读取权限变化时，先同步改变卡片、证据、计数、导航、向量、缓存和任务包的读取资格，再异步执行重提炼/重建。
- 默认要求调用者具有卡片每项必要依赖来源的读取权限。隐藏引用不能成为泄露私密综合结论的方式。
- M1 完整结果集发布通过预期来源修订和卡片修订控制，并使用项目既有 Vault Core/State 可恢复写入边界。读取资格不依赖索引追赶速度。
- 普通任务召回不调用生成 Provider。所有日志和默认诊断只保存安全错误码、版本/哈希和必要计数，不保存原文、卡片正文、提示或秘密。

## M1 服务接口与 M5 公开兼容

M1 的应用服务至少提供以下能力；具体 Rust 类型按仓库现有分层确定：

| 服务能力 | 行为 |
|---|---|
| 提交单来源提炼 | 输入当前授权来源和预期来源修订；返回持久作业/完整集合状态。M1 仅限已授权 Markdown 笔记。 |
| 读取卡片 | 读取当前 CardRevision、必要限定、逐项支持和当前可读证据入口；每次读取重验资格。 |
| 列出卡片 | 在资格过滤之后分页；类型/范围筛选与计数不泄露无权对象。 |
| 读取证据 | 根据 EvidenceRef 返回精确原文及必要上下文；再次检查当前来源版本和调用者权限。 |
| 读取作业状态 | 返回阶段、完整/部分/失败状态、重试信息和安全诊断；不返回秘密 Provider 内容。 |

应用服务、State 仓储、MCP/Admin 适配器各自守住职责。协议层不能直接读文件、执行 SQL 或调用 Provider。证据读取通过 Vault Core/授权来源服务，不得借 `memory:read` 绕过普通来源权限。

M5 公开接口使用稳定的 `remember` 工具保存明确记忆，并提供
`build_memory_pack`、`get_memory_card`、`list_memory_cards`、
`get_memory_evidence`、`correct_memory`、语义 `forget_memory` 和
`get_processing_status`。完整 raw 记录另由 `get_raw_memory`、
`list_raw_memories`、`get_raw_memory_overview`、`update_raw_memory` 和
`forget_raw_memory` 提供。旧 `recall`、`get_memory`、`list_memories`、
`get_memory_overview`、`update_memory` 及旧 `forget_memory` 注册不可用，
不保留请求/响应兼容层。

`remember` 复用现有 v3 explicit ownership/canonical Markdown 写链路，原样保存并保持独立用户拥有；它要求非空幂等键，带来源时每条来源必须带稳定 File ID 与 revision，path 仅作导航，来源绑定不是 Semantic Evidence。MCP 无来源时只需 `memory:write`，带来源时还必须有 `vault:read`/`ReadVault` 以执行验证；权限不足在读文件前拒绝，避免探测文件存在性。它不是 SemanticCard、Evidence 或 MemoryPack 输入。显式记忆仍为 embedding-eligible：有效 `embedding_memory` binding 可沿既有 job 路径排队，没有 binding 时保存成功且不外发。Admin 继续保留其独立的 `remember-explicit` 控制面路由；MCP 与 Admin 的命名不表示两套存储。旧 v3 存储与非 MCP 入口保持不变。所有 raw 工具/资源必须明确 raw explicit ownership，语义工具不得返回完整 raw body。M0/M5 必须记录每个新能力到 MCP 工具、Admin API、UI 和 scope 的映射。

每个公开 MCP 输入的 JSON Schema 和 Rust 运行时 DTO/校验为单一契约：必填字段、类型、枚举、上限、未知字段和默认值应一致。CI 必须读取服务器实际公布的 `tools/list` schema，并以协议 JSON 请求覆盖接受与拒绝行为。手写文档不能代替该验证。客户端不能提交 Vault ID、持久卡 ID、来源路径、权限或数据库引用来取得服务端控制权。

## 数据库命名与阶段范围

当前仓库迁移最高为 `0042_semantic_rule_idempotency_vault_fk.sql`；`0033` 及更早迁移是历史 v3/legacy 基线。新语义表从 `0034` 起采用独立命名空间；不得改写历史迁移或复用 v3 `memory_unit_*` 表冒充新语义对象。迁移保持旧记忆和普通 Vault 内容不变。迁移编号或保留 v3 storage 不表示旧 MCP 工具名或 `vault://memory/*` URI 仍公开。

M1 只做单来源提炼和卡片读取。跨来源关系及综合在 M2；增量资格、纠正、抑制与恢复扩展在 M3；任务包在 M4；公开接口在 M5；真实模型/任务质量评测在 M6；隔离构建和明确授权切换在 M7。实际未运行的 Provider、语义或生产项目必须保持 pending。
