# Source selection parent completeness review

## Status

- **状态**：已实现首轮最小候选、bounded review、显式 reducer 与现有 source-set 原子发布集成；工程门禁已通过，真实模型验收另行进行
- **负责人**：Luna 执行，Astra/root 负责设计审查
- **创建日期**：2026-09-11
- **目标**：修复源候选父单元独占首批、第一轮选择过宽以及最终 source union 直接以父覆盖子的问题，同时保持 v3 admission 提示词的语义边界。
- **验收边界**：实现与本地 fake Provider 工程验证已完成；不调用真实 Provider，不修改实验冻结目录、`comparison/run-01`、生产数据库、Provider binding 或运行服务。

## 目的与用户可见结果

自动记忆提取先按现有 v3 规则选择最小完整小节。若选择结果涉及父子或兄弟关系，再执行有界的完整性审查：审查模型只能根据实际发送的相关正文决定保留、增加必要兄弟、在证据完整时用父单元替代，或明确移除不完整选择。父单元不会因为在首轮独占一个 batch，或因为 `resolve_selection` 的字节包含关系，自动吞掉更精细的子单元。

未完成全部完整性审查前，不发布新的 source set。进程重启、任务续跑、来源变化和模型/profile 变化都能复用已验证的阶段结果或安全地从阶段边界继续；普通 recall 仍不调用生成模型。

## 依据

- `docs/product-requirements.md`：3.5 Long-term memory，要求 source-owned complete units、bounded batches、durable checkpoints、atomic complete source-set publication；3.6 Provider schema validation。
- `docs/architecture.md`：Vault Core/State 的边界、后台任务和协议层不得拥有记忆业务逻辑。
- `docs/memory-system.md`：当前 v3 source-preserving contract、最小完整候选、父子完整正文、profile/checkpoint/cache 和 complete-set publication。
- `docs/adr/0033-source-preserving-memory-units.md`：候选 ID、原文坐标、完整 context、模型只能选择已有 ID、不得生成或重写正文。
- `docs/exec-plans/active/memory-system-v3.md`：当前 v3 纵向实现、真实质量限制和 profile 语义。
- `AGENTS.md`：Vault-scoped 状态、协议边界、LLM 输出校验、崩溃恢复和禁止将派生索引作为知识唯一副本。

## 当前仓库状态

### 第一轮选择

- `crates/memory/src/units.rs::source_units` 生成带 heading、字节/行坐标、完整 body 和祖先 context 的原始 span；当前 parent subtree 不能直接继续作为第一轮 selection candidate。
- `crates/memory/src/v3/selection.rs::batches` 按候选顺序执行 32 units/60 KiB 输入限制，并保留不可发送候选的原因。
- 当前 `SYSTEM` 已恢复为冻结的 `memory-source-unit-selection-v3`；本计划不改它的 admission 语义。
- `selection::schema` 和 `units::resolve_selection` 只允许已提供的 unit ID、合法 kind/hint；现有最终 resolver 的父子折叠不能直接承担新的完整性审查。

### 应用流水线

- `MemoryService::extract_note_with_options` 在 source hash/profile 检查后读取 source、生成 minimal batches、读取/写入首轮 checkpoint，随后构造 ReviewPlan、读取/写入 review checkpoint，执行显式 reducer 后才进行完整 source-set 发布。
- 当前一次 `extract_note` 调用在遇到一个未缓存 batch 后停止本次调用；worker/任务下一次调用继续，因此必须保留阶段和缓存边界。
- `extraction_profile_hash` 含 prompt version、完整 SYSTEM、schema/batch contract、模型/provider 配置和 policy；恢复 v3 已使 v4 profile 不可复用。
- `state::units::work` 的 `memory_unit_selection_batches` 以 Vault、source File ID、source hash、input hash 存储首轮和 review 结果；progress 通过聚合 completed/total 与 skipped JSON 表达 selection+review 总阶段进度。

## 范围

包括：

1. 从原始 Markdown 生成“叶/最小小节 + 每个父级非空独有引言”的第一轮候选，不把完整 parent subtree 送入第一轮；结果阶段化保留，不让最终 union 在完整性审查前发布。
2. 父/兄弟关系的确定性 bounded review scope 构造和 60 KiB/32 unit 分片。
3. 第二轮严格结构化完整性审查、合法动作边界、父替换覆盖验证。
4. Vault/source/profile scoped 的 review checkpoint、progress、恢复、source hash/profile 失效和 atomic publication。
5. local fake Provider、父独占 batch、跨 batch sibling、超限 scope、模型输出非法、重启续跑和 source 变化测试。
6. Admin progress/diagnostic 所需的阶段状态和文档更新。

不包括：

- 修改 v3 admission SYSTEM 的判断标准或恢复 v4 prompt；
- 改变 Markdown parser、候选 ID、现有 32/60 KiB batching 规则或 recall ranking；
- 用第二轮把模型输出正文写入 canonical memory；
- 跨来源合并、语义去重、项目配置、实时 Provider 质量校准；
- 重新运行 `target/memory-selection-prompt-comparison-20260910` 或修改其 frozen/结果；
- 真实 Provider、生产 Vault、生产数据库或正式 binding 操作。

## 不变量与风险

### 必须保持的不变量

- 每个输出 unit ID 必须属于本次 scope 的生产候选；body、context、source identity 和 byte ranges 始终来自 Vault 原文。
- v3 admission 先于 review；review 不能把普通参考资料、纯交付规格或模型自造事实变成记忆。
- review 只能产生候选 ID 和受限动作，不产生 canonical body、路径、来源身份或新 metadata 事实。
- source set 只有在第一轮所有 batches 与所有需要的 review scopes 均成功、且 source hash/revision/profile 仍匹配时才发布。
- 任意部分结果可以 checkpoint，但不可作为 finished current set 或 recall 输入。
- 所有 review 状态、cache、job、audit、input hash 均 Vault-scoped；来源改变立即使旧阶段结果失效。

### 主要风险

- **父候选超限**：完整 parent subtree 不进入第一轮；无法完整发送父正文时禁止 `replace_parent`，review 只能保留或补充实际展示且可独立理解的最小单元，并记录 unresolved/too-large。不能把“覆盖了字节范围”当作跨 scope 的联合语义审查，也不能因此把所有叶子丢弃。
- **兄弟正文过多**：scope builder 从外向内尝试最大可完整装入的真实子树；超过 32 单元或最终序列化 60 KiB 才向下分区。每个 minimal 只归属一个 owner，避免祖先重复建立 scope。
- **scope ownership**：每个 minimal unit（无论首轮是否被选）必须由唯一 review scope 负责；同一 unit 不能在 scope A 被 omit 后又在 scope B 被 add。每个 scope 必须对其 first-round selected units 给出一次且仅一次 keep/omit/显式 parent replacement 决策，缺失、重复或冲突均拒绝。
- **父子跨批**：不能在单个 batch 内用 `resolve_selection` 提前折叠父子；第一轮 raw selection、review actions 和最终 source union 必须分层保存。
- **审查模型过度扩展**：结构化输出只包含 keep/omit ID、同 scope 的 additions 和显式 replace_parent；add/replace/omit 都必须通过确定性 ID、范围、覆盖和动作约束。
- **失败恢复**：selection 与 review 共用 Vault/source scoped checkpoint 表，以 stage/schema/profile/input hash 区分结果；聚合 progress 在每次阶段完成后更新，重启只复用已验证结果。
- **输出兼容**：模型可能返回合法 JSON 但动作语义非法；必须在 State checkpoint 前执行完整动作验证，失败即停并保留稳定诊断。

## 提议设计

### 1. 第一轮保持 v3 admission

第一轮仍使用恢复的 v3 `selection::SYSTEM`、现有 schema 约束和生产模型调用边界，但输入候选改为纯函数生成的最小集合：叶级完整小节，以及每个有子级的父 heading 下、位于第一个子 heading 之前的非空独有引言单元。完整 parent subtree 只作为后续 review 的候选描述，不进入第一轮 batch。每个最小候选仍携带必要 ancestor context，因此选择叶单元不会丢父级前提。

每个第一轮 batch 成功后保存 raw `SelectionOutput`，只做 batch-local ID/schema 校验；不调用最终 source union 作为发布前的唯一判断。第一轮没有选择时不触发 review，可按现有 empty evaluated-set 规则完成发布。

第一轮有选择时，应用层建立确定性关系图：按原始 byte ranges 找父/子，并从外向内尝试最大可完整装入的子树；超限再向下分区。关系图只用于 scope 构造和 reducer 校验，模型看到的正文仍必须来自真实候选。每个 minimal unit 只归属一个 owner scope，父独有引言不会被重复发送。

没有共同真实父标题的多个顶层候选使用仅用于 scope ownership 的 virtual-root。virtual-root 只能把实际展示的顶层兄弟放入同一 bounded review，不能生成 canonical parent ID、整篇虚拟正文或 `replace_parent` 目标；单个无标题且没有兄弟的完整段不触发 review。

### 2. Review scope 构造

只为可能影响完整性的非空选择建立 scope：

- 一个选中的 unit 有可见父/子关系；或
- 同一直接父下存在未选兄弟，且兄弟的正文可能是条件、例外、顺序或验证步骤；或
- 首轮选中了父候选，需要判断其是否应被更小完整子集替代。

scope 以最大可完整装入的父子子树为边界，不沿每一层祖先重复发送同一正文。输入包括：选中最小 unit 的完整 body/context、同一 owner 子树下实际相关兄弟的完整正文、父级非空独有引言，以及必要的 source unit ID。兄弟必须发送真实正文；只发送标题、摘要或模型生成的描述不合格。

scope 采用与 Provider 输入相同的 32 units/60 KiB 上限，按稳定 unit byte 顺序形成互不重复的 bounded scope。scope ID 由 owner、分片序号和 first-selected ID 稳定构成，不因任务重启改变。只有完整 parent subtree 本身能放入一个 scope、并且该 scope 实际包含父正文时，review 才能看到 parent candidate；否则应用将 scope 标记 unresolved 并禁止 `replace_parent`。多个局部 scope 的结果不能联合宣称完成了父级语义审查。

scope builder 对每个待发送 unit 的 `complete_text`、context 和 parent candidate 复用现有 `redact_generated_text`，任何 sensitive candidate/context 都从 review scope 排除并记录不可审查原因；被首轮过滤的内容、未展示的过大区域和 omitted candidate 不能通过 parent body 或其他 ancestor context 带回。scope 以最终 JSON 序列化后的完整 input 计算 60 KiB，而不是只计算裸 body 字节；超过限制就下拆或标记 unresolved，绝不截断。

### 3. Review 输出契约

第二轮使用独立的结构化 review schema，不改变第一轮 v3 admission。建议输出形状：

```json
{
  "reviews": [
    {
      "scope_id": "稳定 scope 标识",
      "keep_unit_ids": ["第一轮已选且仍完整的 ID"],
      "additions": [{"unit_id": "scope 中正文支持的必要兄弟 ID", "kind": null, "retrieval_hint": ""}],
      "replace_parent": null,
      "omit_unit_ids": []
    }
  ]
}
```

确定性验证规则：

- `keep_unit_ids` 和 `omit_unit_ids` 必须来自第一轮选择；`additions` 只能引用 scope 中实际发送且有完整正文的 minimal unit。
- 通常 keep+omit 必须穷尽首轮 ID；replace_parent 分支要求 keep、omit、additions 全为空。
- 每个 scope 必须覆盖它 owner 的全部 first-round selected units，且每个 such unit 只能出现一次；keep/omit/parent replacement 缺失、重复或互相冲突时整个 review 结果拒绝。
- `replace_parent` 只能指定一个实际发送、且完整正文在同一次完整 request 中可见的 review-only 父单元；所有被其替代的 minimal units 必须属于同一 scope。禁止跨多个局部 scope 拼出“已审查完整”，也不能因 `resolve_selection` 的默认包含关系隐式替代。
- `unresolved=true` 由应用根据 scope 的缺失范围和超限边界记录；模型不得用它掩盖未发送的正文。只要 scope 不完整，父替代被拒绝；模型只能 keep 已展示且可独立理解的最小单元、add 已展示且有正文的兄弟，或对依赖缺失范围的单元执行 omit。
- review 不得返回不在 scope 的 ID、正文或未授权事实；kind 与 retrieval_hint 只能作为受限检索元数据通过 schema 校验。
- 重复 ID、keep/omit/replace 冲突、父子覆盖不满足、缺失 scope 分片、超出上限或非法 schema 都是 `memory_selection_review_*` 失败，不进入 publication。

### 4. 父级超限和唯一引言

完整 parent subtree 无法进入 bounded review 时，不发送截断正文，也不让模型凭标题决定 replace。第一轮根本不会产生完整 parent selection；review 只能在已展示且通过 redaction 的最小单元范围内 keep/add/omit，缺失范围记录 `unresolved/too_large`，不能宣称跨分片完整。已展示子候选的 `complete_text` 带有必要 ancestor heading/introduction，因此父级独有引言不会因为选择子单元而丢失；这只是原文 context 的保留保证，不是对未展示兄弟的语义完整性证明。

### 5. Checkpoint、缓存和恢复

现有 `memory_unit_selection_batches` 的 result JSON 是任意 Value，仓储只检查大小、input hash 格式和 source 当前性，application 层在保存前完成强 schema/action 校验。review 复用该表，以 `stage`、review schema version、scope ID 和候选 hashes 进入 input hash，不新增 review 表。

第一轮 input hash 与 review input hash 分开：review hash 至少包含 `profile_hash`、第一轮 raw selection set hash、scope ID/index/total、scope candidate IDs/body hashes、review prompt/schema version 和 source hash。新流程版本必须区别旧 v3，例如 `memory-source-unit-selection-v3-minimal-review-v1`；SYSTEM 仍保留冻结 v3 admission 文本。模型/provider 改变会使全部阶段 stale，旧 v4 或旧 v3 结果不可因字段相同直接复用。

现有 progress 用聚合 completed/total 表示 selection 与 review scope 总工作量，并保存 skipped 诊断；只有全部阶段完成才报告 source set 完成。任何 partial 都只能显示处理中/失败，不能进入 recall。每次 `extract_note` 最多 dispatch 一个未缓存语义请求，成功 checkpoint 后返回，下一次任务继续下一个 batch/scope。

source hash/revision、Vault pause、expected set revision 和 profile 在每个 checkpoint 命中与最终发布前重新检查。来源变化会使所有旧 review scope 失效并回到新 source snapshot；重启只复用同 hash、同 profile、已验证的 scope，不重发成功请求。

### 6. 最终发布

完成所有第一轮 batches 与 review scopes 后，将 raw first-round selections、validated review decisions 和 first-round candidates 送入应用层 deterministic reducer：先应用 omit/add/explicit replace，再按 source-owned ID 注册表排序，不由字节包含关系隐式替代。构造 complete source set snapshot，沿现有 Vault Core/State 原子发布与 recovery 路径提交。

## 工作分解

1. **完成设计与持久化边界**：已确定最小候选、scope ID、schema/version、failure codes；复用现有 selection batch 任意 Value 和确定性 scope 重建，不新增 migration 或 review 表。
2. **候选关系与 scope builder**：在 memory crate 内实现纯函数，覆盖叶/父独有引言、父子、直接兄弟、重复 scope、scope 分片、60 KiB/32 unit、超限父和 context 保留；不访问数据库。
3. **review schema/validator/reducer**：实现严格反序列化、scope ID 校验、动作边界、coverage 检查、显式 parent replacement 和 partial 禁止发布；增加未知/重复/越界/非法 action 负例。
4. **State checkpoint/repository**：复用 Vault/source/profile scoped selection checkpoint，验证 source hash、跨 Vault 隔离、冲突和重启恢复。
5. **MemoryService vertical slice**：在 `extract_note_with_options` 中保持第一轮 v3 admission，按“一个未缓存调用/次 extract”推进 selection 或 review，完成后再走现有 source-set publication。
6. **local fake Provider tests**：父独占 batch、兄弟跨 batch、父引言、父超限、review add/replace/omit、模型 schema 错误、失败续跑、source 变化、profile 变化和 no-selection 不触发 review。
7. **Admin/文档**：展示 selection/review 阶段、partial/失败和父不可审查诊断；同步 `docs/memory-system.md`、接口说明、迁移/运维文档和本计划。

## 当前进度

- [x] 读取项目规范、ADR-0033、v3 memory plan、selection/service/units/state 流水线。
- [x] 确认现有 `selection_progress` 只有 batch 计数，无法安全表达 review 阶段。
- [x] 明确第一轮继续使用恢复的 v3 admission prompt，不改回 v4。
- [x] 实现纯函数 `minimal_selection_units` 与 local unit tests；首轮 service 使用 minimal，full units 仅用于 bounded review/rebuild registry。
- [x] 形成最终 parent/child ownership、scope redaction/serialized limit、超限父、显式 action、cache/checkpoint 边界设计，待 root/Astra 审查。
- [x] 确认 review schema、现有 selection checkpoint 复用和 ADR-0033 补充。
- [x] 实现候选关系/scope builder、validator/reducer、State checkpoint 和 MemoryService 集成。
- [x] 添加 local fake Provider 回归和 public-protocol/恢复验证。
- [x] 更新规范并运行相关工程门禁；Admin 沿用聚合 progress 展示。

## 决策记录

### D1：第一轮只允许最小候选

第一轮只发送叶级最小完整小节和父级非空独有引言；完整 parent subtree 只能作为 bounded review 的 review-only 候选。最终 reducer 不得因 byte containment 自动替代子候选；只有在一个完整 scope 实际发送了父正文、review 明确返回 `replace_parent` 时才允许父替代。父级独有引言通过独立最小引言单元和 child context 保留。

### D2：超限父不做标题推断或截断发送

父正文与兄弟正文无法共同满足 Provider 输入边界时，禁止依赖标题、模型摘要、截断正文或跨分片 byte coverage 补齐语义。此时不授权 `replace_parent`，只处理实际展示且可独立理解的最小单元；缺失范围记录 unresolved/too-large。这是保守边界，不把整父保留当作成功，也不把所有叶子自动丢弃。

### D3：阶段进度聚合但不提前完成

现有 progress 的 completed/total 聚合 selection batches 与 review scopes，skipped JSON 保存稳定诊断；任何阶段未完成都不能伪装成 published。

## 剩余验收边界

- 工程测试已覆盖 scope 分区、schema/action 校验、checkpoint 恢复、原子发布和 rebuild；剩余工作是冻结语料上的真实 Provider 语义质量与生产验收。
- 真实模型仍需人工检查 add/replace/omit 的语义边界、误保留和误省略率；fake Provider 不能证明语言理解质量。

## 验证计划

设计确认后执行：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

重点 local fake 用例：

- S19 形式的父候选独占 batch，子候选在后续 batch；确认 raw、review、final 三层不同。
- 父 introduction 只出现在 child context；选择 child 不丢前提。
- 直接兄弟正文真实进入 review，标题相同但正文不发送的候选不得被 add。
- 父/兄弟 scope 超出 60 KiB 时分片、恢复和禁止 premature replace。
- review schema 越界 ID、duplicate、keep/omit/replace 冲突、missing scope、父覆盖不足全部拒绝。
- 无第一轮选择不触发 review；一轮成功后进程中断，下一次只复用已 checkpoint 的 scope。
- source hash/revision/profile 变化使 selection/review cache 失效；Vault A 的 scope 不可被 Vault B 命中。
- review 未完成时旧 current set 保持可读，新 source set 不发布；全阶段完成后只原子发布一次。

## 回滚与恢复

本实现不新增 migration 或 review 表，复用 Vault/source scoped selection checkpoint。运行中断时保留已成功 selection/review checkpoint，失败 scope 从稳定 input hash 重试；profile/source revision 变化时安全忽略旧结果。任何阶段都不能直接修改 canonical Markdown 或正式 memory rows。

## 结果

已完成最小候选、fit 子树 scope、严格 review validator、显式 reducer、阶段 checkpoint 复用及原子发布集成。fake Provider 通过 MemoryService 覆盖 review add/replace/omit、nested 前提正文、冲突输出不缓存、失败恢复、profile/source 变化、暂停、Vault 隔离和 parent-intro cold rebuild；真实 Provider、部署和生产验收仍单独进行，工程测试不替代语义质量验收。
