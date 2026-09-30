# ADR-0035：语义记忆卡与任务记忆包

- 状态：已接受，实施中。
- 日期：2026-09-14。
- 设计基线：[语义记忆实施计划](../semantic-memory-implementation-plan.md)。
- 数据与接口契约：[语义记忆契约](../semantic-memory-contract.md)。
- 执行计划：[语义记忆系统实施](../exec-plans/active/semantic-memory-implementation.md)。
- 替代：ADR-0033 中“完整原文单元作为自动记忆正文、禁止跨来源整理”的冲突部分，以及其一次性清理 predecessor memory 的旧切换授权。

## 背景

ADR-0033 为避免生成摘要丢失事实，规定模型只选择原文单元，服务把原文正文写成自动记忆，并将跨来源整理排除在当前运行时之外。新的产品基线要求 Agent 得到已整理的工作背景：系统先从有授权的来源提炼 Observation，再形成有逐项证据支持的 MemoryCard，最后按当前任务组合 MemoryPack。原文承担证据和核对入口的职责；卡片承担默认记忆表达。

现有 v3 运行时、迁移、MCP/Admin 契约及规范文档仍描述 ADR-0033。该 ADR 因此冻结新的设计方向和分阶段边界，但本身不表示新运行时已实现，也不授权切换、清理或 Provider 请求。

## 决定

### 记忆表达

采用以下逻辑链路：

```text
已授权来源与当前版本
    → 原文证据与必要上下文
    → 来源级 Observation
    → MemoryCard 与逐项支持
    → 按任务构建的 MemoryPack
```

模型可以提炼语义、判断关系并在后续阶段受控综合。每项可检验的卡片陈述及必要限定都要追溯到具体 Observation 和 EvidenceRef。服务生成和验证身份、权限、版本、范围边界、证据坐标、支持关系与发布动作。卡片的简短视图从同一结构确定性渲染；关键条件不能因预算或展示模式而消失。

原文是证据，不是默认记忆表达。系统不得把完整原文章节直接当成语义提炼的替代品，也不得把旧卡片、提示、概览或任务包递归当作新证据。更新卡片必须回到当前有效来源级 Observation 和原文，不能只对旧摘要继续摘要。

### 阶段边界

M1 只建立已授权 Markdown 来源到 Observation、单来源 MemoryCard、证据读取的闭环。来源纳入沿用现有 Vault opt-in 和 Markdown 隐私/路径授权政策。M1 的已实现来源类型只有 Markdown 笔记；对话、工具记录及其他 SourceKind 是后续扩展，不阻塞闭环，也不得在文档或产品中声称已经支持。

M1 先实现保守的当前来源资格和失效。来源内容、身份或读取权限变化时，同步使受影响卡片失去当前读取资格，再异步重提炼或重建。卡片发布采用完整结果集、预期修订和可恢复写入。

M2 在 M1 验收后加入跨来源等价、补充、冲突、不同范围和有证据的综合。相似度只产生候选，不能证明事实等价、采纳、新旧替代或独立支持。不同范围保持分开；逐项支持区分共同支持（AND）和任一充分支持（OR）。不建设全局事实图或自由协作的记忆 Agent 网络。

M3 建立持久纠正、抑制、删除和重建规则。规则绑定稳定语义目标与范围，不能只绑定临时卡片或提炼 ID。重启、重新提炼、模型/profile 变化、读取器回滚和命名空间切换均须继续应用最新纠正、禁止再生成和来源权限。

M4 以确定性检索和组包为先，生成式组包保持可选且受限。M5 向 MCP、Admin API 和 UI 暴露新入口。经 2026-09-18 用户授权，MCP 采用一次性 clean break：旧工具名与 `vault://memory/*` URI 在当前开发面移除，不设迁移窗口，也不保留旧请求/响应兼容层。保留 v3 storage/Admin 适配器不等于保留旧 MCP 契约。发布的 JSON Schema 与运行时校验必须一致。

M6 的真实模型、真实任务和费用验收必须独立报告。M7 才进行隔离影子构建和经授权的读取切换。没有真实 Provider 运行的质量项保持 pending；生产切换、数据清理及旧记忆物理删除均不由本 ADR 授权。

### 存储与迁移

新实现使用独立语义记忆命名空间和前向迁移，不改写已发布迁移，也不把旧生成摘要作为新证据。来源笔记继续由 Vault 文件系统保存；发布的耐久记忆卡通过 Vault Core 物化为 Markdown，SQL 保存 Vault 作用域的版本、支持关系、授权资格和作业投影。生成式索引和任务包可重建，不成为唯一知识副本。

现有 v3 读取和数据在影子阶段保持原状。迁移不得默认清空、格式转换、覆盖普通笔记或改写来源。任何默认读取切换或旧数据清理都需要单独的生产授权和备份/回滚方案。明确由用户提交或更正的内容不能被当作普通生成摘要丢弃；其所有权和授权来源须在后续迁移决策中单独识别。

新实现继续遵守 Vault 隔离：所有表、外键、缓存、任务、支持关系和查询都带 `vault_id`，运行时从 `VaultContext` 推导 Vault，不接受调用方任意指定 `vault_id`。

## 保留的不变量

- Vault 文件系统仍是普通笔记和附件的规范来源；记忆管理不能改写普通笔记。
- 所有记忆数据和读写经过 `VaultContext`、Vault Core 与仓储边界，不跨 Vault 查询。
- 来源权限撤销先影响读取资格，再清理索引并异步重建。
- 原文版本、证据位置和必要解释上下文必须可追溯；未知时间、范围和采纳状态保持未知。
- 每个断言及关键范围、否定、例外、步骤顺序和验证要求逐项受支持；必要限定失去支持时，核心断言一并退出当前读取。
- 记忆是资料，不是更高优先级指令。来源和模型输出均是不可信输入。
- 普通任务召回不要求在线生成模型；Provider 失败不得阻断文件操作、显式记忆或词法读取。
- 日志不保存原文、记忆正文、提示、凭据或 Provider 完整响应。

## 后果

系统将以语义提炼而非原文摘录作为卡片的默认表达，并保留来源证据供验证。M1 可独立交付单来源闭环，跨来源去重与综合在之后实施。系统允许保留独立来源贡献、重复候选、冲突和不确定性；不以记忆条目数量下降作为质量指标。

新语义契约落地前，仓库中的 v3 代码和相关规范仍是当前运行行为的描述。执行计划记录这些冲突及同步顺序；本 ADR 不将文档决策冒充为工程、真实语义或生产验收完成。

## 2026-09-18 公开号面修订（用户明确授权）

本修订将开发期 MCP 对外命名收口为一次性 clean break，不保留旧请求/响应
兼容层，也不把旧 URI 静默映射到新语义。语义工具固定为
`build_memory_pack`、`get_memory_card`、`list_memory_cards`、
`get_memory_evidence`、`correct_memory`、`forget_memory` 和
`get_processing_status`。其中 `forget_memory` 只表示语义规则/抑制动作。

用户拥有的完整原文记忆继续由 `remember` 写入，并通过独立 raw 命名空间
`get_raw_memory`、`list_raw_memories`、`get_raw_memory_overview`,
`update_raw_memory` 和 `forget_raw_memory` 读取或修改。Raw 工具和资源必须
明确标记 explicit/raw ownership；语义工具不得返回完整 raw body，也不得
把 raw record 当作卡片、证据或 MemoryPack。

MCP 资源固定为 `vault://raw-memory/context`、
`vault://raw-memory/{memory_id}` 与可选的
`vault://memory-card/{card_id}`。旧 `vault://memory/*` URI 不可用且不能
作为别名。Admin 的 `/semantic/remember-explicit` 仍是控制面适配器；本次
修订不删除 Admin、v3 存储、迁移或数据，也不授权 Provider/网络/生产切换。

## 2026-09-18 运行时默认上下文收口

语义卡/MemoryPack 是默认上下文的唯一自动记忆读取路径。启动恢复、周期
维护和文件事件不得重新 admission `memory.*` 的旧 v3 automatic
extraction/reconcile/overview；旧任务若已存在，只能在 Worker admission 阶段
安全取消。文件事件只进入 `semantic.*` source path。语义卡或任务包因来源、权限、
修订、支持关系或规则 fence 不可读时，调用方必须停用该 card/pack 读取并降级到
已授权的 `search_notes`/`read_note`，不得回到旧 v3 reader。

旧 v3 storage、raw explicit canonical Markdown、普通笔记、history、SQLite
任务/审计记录均保留；Admin 只将 explicit raw 作为普通记忆管理对象，旧自动单元
最多作为明确标注的 storage/legacy 诊断。此次收口不执行 cleanup、不删除旧数据，
也不触发 Provider 或生产操作。
