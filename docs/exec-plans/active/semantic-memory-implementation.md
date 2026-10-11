# 语义记忆卡与任务记忆包

- 状态：M0、M1、M2、M3、M4-A、M4-B、M4-C、M5、M6 本地 harness 及 M7-A1/A2/A3 本地确定性工程闭环已完成；M4-C 的 supersedes/different_scope 独立 fixture 已通过，真实任务质量评测仍 pending。M3-D1 deterministic full-ExtractionSet rebind 已完成并保持 partial、source-stated 等场景 fail-closed。M5 的唯一 MCP 保存工具为 `remember`，采用严格 explicit-memory contract；Admin 保留独立 `remember-explicit` 控制面适配。2026-09-18 起 semantic-only 默认上下文不再 admission 或调度旧 v3 automatic extraction/reconcile/overview；旧数据、普通笔记、history 与 v3 存储仍保留，不做 cleanup。2026-09-21 本地门禁复核中，fmt、workspace Clippy、Eval/MCP/Memory、State/Server 和 Admin 前端检查通过；workspace 测试被 2 项计划外 Provider capability 集成测试阻断，详见文末复核记录。用户已明确授权 M6 真实 Provider/任务验收且不设费用上限；2026-09-22 已使用真实已提交 ADR 来源完成非 synthetic manifest 和隔离 current-schema A/B/C roots 准备，但首次真实运行在 observation 阶段以 `provider_dns_failed` 停止，未形成语义质量结果，M6 仍未通过。2026-09-28 用户授权 ADR-0038：以与 candidate producer 隔离的独立智能体语义复核替代人工必审，所有原门槛保持不变，报告必须标 `human_review=false`；新的 30 条 holdout 已独立复核并冻结（human_review=false，quality_claim=not_evaluated）；2026-09-28 M6 Agent V1 Full R1 已执行并在第 3 次 Provider 请求因 observation `provider_schema_invalid/enum_mismatch` 首错停止，未形成质量结论且禁止复用；随后全量 R2 使用 v8/v5 重现 B observation `enum_mismatch` 并保留已 claim 失败现场。2026-09-29 接受 ADR-0040，observation wire先升级v9/v6 indexed evidence；R3的7请求失败后，ADR-0041把status词表写入v10提示，R3仍保留。R4使用MiMo JSON-object运行3请求后在B/S17 observation以type_mismatch终止，0 tasks complete；R1–R4全保留且不复用。ADR-0042现规定隔离M6 observation使用单一MiMo strict function schema与有界SSE args聚合，JSON schema仍在本地严格检查；prompt/schema为v11/v7。下一步完成本地fakes与workspace门禁，再用fresh root做单来源B诊断；若通过才开另一个fresh full M6。M6质量仍未评估。M7真实shadow/client/生产切换与回滚须在M6通过后另行授权；旧数据清理另需授权。
- Owner/Agent：Codex / Luna 执行；主代理负责统筹与验收。
- Created：2026-09-14。
- Updated：2026-09-29 Asia/Shanghai。

> 最新状态（2026-09-30）：历史 D13 能力结论记录见后文；用户随后授权解决模型对复杂 JSON 的依赖，当前以 ADR-0044 A80 方案为准。A80 已落 migration 0043、source-wide 单次 regen token、validated batch resume 和原子单 claim→card 发布。A80 wire 为 `semantic-cards-tracked-adr-m6-v14` / `semantic-cards-m6-json-v10`，响应不要求模型回填 source namespace；服务端只用 prepared handle 绑定当前 source revision/batch/input/catalog/prompt/schema/provider，normalized evidence ID由服务映射注入。State 层proposal hash校验、source fence、span/hash evidence及single-source atomic publish仍生效。Eval使用 `reserved`→`started` ledger write-ahead；恢复时未验证 batch若已出现在ledger就不重发，validated batch按State hash校验后复用。A80当前定向门禁覆盖单source多batch跨服务重启、单source一次regen、未知kind完整读链、无namespace两batch Provider fake和runner保守请求计费。source/core结构失败仅允许每source-arm一次regen；来源/hash/auth/Provider identity drift与in-flight uncertain仍fail closed。当前预算上界公式按catalog计算primary `31 batch + 90 answer + 6 relation = 127`，source-arm最多23次regen，另6 relation +4 answer为保守预留，总额160；关系/answer retry当前没有执行路径，不得报告为已实现，10次只作为未使用上界余量。生产 `semantic.extract` 仍是worker拒绝占位；旧 `semantic-card-live-diagnostic` 是observation+composition两阶段，不能跑A80。全workspace gates尚待一次正式提权离线测试；无MiMo请求、无新App Support root写入，R1–D13现场保留，gold与质量阈值不变。

## Purpose and user-visible result

本计划落实 [语义记忆实施计划](../../semantic-memory-implementation-plan.md)。用户或 Agent 留下经过授权的资料后，系统提炼可用于后续工作的重点，保留原文证据及范围、否定、例外、顺序和状态，再按当前任务交付最相关的工作背景。默认读取语义卡，不直接返回原文章节，也不递归摘要旧卡。

最终结果应支持来源版本追踪、逐项证据、跨来源受控整理、任务记忆包、无答案表达、持久纠正/禁止再生成、及时的权限失效、可恢复发布和可比较验收。完成前必须分别通过本地工程验证、真实模型/任务验收和另行授权的生产切换验收。

## Governing requirements

- [语义记忆实施计划 §1—§9](../../semantic-memory-implementation-plan.md)：产品决策、数据模型、模型契约、状态、任务包和安全边界。
- [语义记忆实施计划 §10—§16](../../semantic-memory-implementation-plan.md)：M0—M7 顺序、测试语料、A/B/C 对照、迁移与完成定义。
- [ADR-0035](../../adr/0035-semantic-memory-cards-and-task-packs.md) 与 [语义记忆契约](../../semantic-memory-contract.md)：本工作的架构和数据/接口基线。
- [ADR-0038](../../adr/0038-independent-agent-review-for-m6.md)：独立 agent-reviewed 替代人工必审的 M6 评审流程；不降低阈值，必须标记 `human_review=false`。
- [ADR-0044](../../adr/0044-bounded-a80-extraction-and-isolated-evaluation.md)：MiMo JSON-object 条件下 A80 分片、确定性 EvidenceRef/Card、失败隔离和 same-root 单调恢复。
- [产品需求 §3.5、§3.6、§4.2—§4.5、§6](../../product-requirements.md)：现有记忆、Provider、数据完整性、可移植性、隐私及完成要求。§3.5 当前存在 v3 冲突，须随新规范同步。
- [架构 §3、§4、§7、§9、§11、§12、§15、§17](../../architecture.md)：规范/派生数据、Vault Core、仓储、恢复、任务、当前 v3 冲突、Provider 边界和依赖方向。
- [数据模型 §8、§9、§10、§13、§19](../../data-model.md)：稳定文件身份、journal/outbox、任务、记忆迁移和前向迁移规则。§13 当前规范为 v3，须更新或标为过渡期旧契约。
- [接口规范 §4—§6、§8—§10、§12](../../interfaces.md)：MCP schema、scope、工具、错误及 Admin 契约。MCP 当前采用用户授权的 clean break；仅 v3 storage/Admin 边界保留，旧 MCP 名称和 URI 不设迁移窗口。
- [安全规范 §7、§9—§12、§14—§17、§20](../../security.md)：Vault 绑定、路径、写入、提示注入、出域隐私、跨 Vault 隔离和验证。
- [开发与测试 §7—§8、§11—§14](../../development-and-testing.md)：迁移、协议、Provider、召回、崩溃恢复与安全测试。
- [ADR-0001](../../adr/0001-markdown-content-is-canonical.md)、[ADR-0002](../../adr/0002-vault-is-the-isolation-boundary.md)、[ADR-0006](../../adr/0006-llm-and-embeddings-are-pluggable-enrichment.md)、[ADR-0011](../../adr/0011-managed-memory-canonical-files-use-vault-core.md)：可移植内容、Vault 边界、Provider 与 Vault Core 写入。
- [ADR-0033](../../adr/0033-source-preserving-memory-units.md)：历史 v3 设计及与本基线的冲突由 ADR-0035 记录，不直接改写历史理由。

## Current repository state

当前记忆实现同时包含既有 `crates/memory/src/v3/` 和 M1/M2/M3 语义命名空间；语义 State/Memory 实现位于 `crates/state/src/semantic_*.rs` 与 `crates/memory/src/semantic/`。迁移 `0043_semantic_observation_batches.sql` 增加 Vault-scoped A80 batch/checkpoint 状态。旧公开 MCP 工具和参数位于 `crates/mcp/src/lib.rs`，Admin 路由和 DTO 位于 `crates/admin-api/src/lib.rs`。Provider 适配在 `crates/providers/`，源文件操作必须走 Vault Core。

现有 [memory-system.md](../../memory-system.md)、[interfaces.md](../../interfaces.md)、[product-requirements.md](../../product-requirements.md)、[architecture.md](../../architecture.md) 和 [data-model.md](../../data-model.md) 仍包含 v3 原文单元、工具输出及旧初始化/清理流程。本次 M0 新增 ADR 与数据/接口契约，后续须按边界同步规范文件。ADR-0033 的“原文单元即自动记忆正文”及旧清理授权不再作为新计划基线。

M0 检查时，工作区已有 Admin 批量 forget 相关的未提交改动，涉及 `crates/admin-api/src/lib.rs`、`crates/memory/src/v3/model.rs`、`crates/memory/src/v3/service.rs`、`crates/memory/tests/memory_v3.rs`、`docs/interfaces.md`、`docs/memory-system.md` 和 `frontend/admin/`。这些改动不属于本计划实现范围，后续修改同名文件时必须保留。除此之外现有未跟踪文件也归工作区原所有者管理。

## Scope

### Included

- M0：代码/迁移/公开接口映射，ADR 与语义数据契约，合成评估 manifest，隔离 dry-run 入口。
- M1：单来源 Observation、语义卡、证据读取、当前资格、完整结果集、权限和可恢复发布。
- M2：候选、等价/补充/冲突/不同范围、逐项 AND/OR 支持、增量跨来源整理。
- M3：来源版本及上下文更新、任务状态、Correction/Suppression、删除、重启与回滚恢复。
- M4：任务召回和确定性 MemoryPack，A/B/C 对照 runner、预算与降级诊断。
- M5：版本化 MCP/Admin/UI 入口、schema/运行时一致性及端到端权限测试。
- M6：经授权的真实 Provider 与真实任务语义评测、独立智能体复核、用量/成本报告；结果准确标记 `human_review=false`。
- M7：授权来源隔离构建、影子评测、测试客户端切换、生产切换和受限回滚准备。

### Not included

- 训练模型、全局事实图、跨服务拆分或多 Agent 自由协作网络。
- 未经授权的收费 Provider 调用、真实资料外发或生产 Vault 操作。
- 自动清空/转换旧记忆、改写普通笔记或用旧生成摘要作为新证据。
- 旧 v3 automatic extraction/reconcile/overview 的重新 admission、周期调度或文件事件入队；旧 v3 数据和 raw explicit storage 仍保留，供显式管理或 storage/legacy 诊断。
- 以 fake Provider 或历史响应重放冒充真实语义质量验收。
- 将所有来源整理成一段文本、追求零重复或按数据库对象减少判定质量。

## Invariants and risks

- 所有持久对象、外键、查询、任务、缓存、索引、支持关系和审计按 `vault_id` 隔离；接口从授权得到 `VaultContext`。
- M1 输入仅是现有 Vault opt-in 和隐私/路径规则准许的 Markdown 笔记；对话/工具 SourceKind 是未来扩展，当前不支持。
- 原文是规范证据，语义卡是默认记忆表达。卡片每个事实和必要限定有逐项支持；限定失效时不得留下更宽断言。
- 来源身份/版本/权限变化立即改变读取资格，索引和卡片异步重建；候选数、导航、摘要与缓存采用同一资格边界。
- 成功空集合与失败/部分集合可区分；批次全部验证后原子发布。取消、源版本变化或纠正策略变化时不能发布旧结果。
- 来源角色和 `proposed/adopted/observed` 区分清楚；模型不得创建持久 ID、范围权限或事实时间。
- Correction/Suppression 使用稳定语义目标和范围，重启、重新提炼、切换和回滚后继续生效。
- 公开 JSON Schema 与实际运行时校验一致；不存在把旧工具名响应悄然改变为新语义的版本漂移。
- 新迁移从 `0034` 起并使用独立 namespace。旧记忆、普通笔记、配置及凭据不在本计划内自动清理。
- 风险：原始 Markdown 证据完整性与 EvidenceRef span 校验；跨 Vault 关系查询；来源撤权与缓存竞态；读权限改变先后顺序；卡片 Markdown 与 SQLite 投影一致性；新接口与现有客户端兼容；语义质量只能由真实模型/任务评测判断。

## Proposed design

### Components and dependency direction

M0 先映射当前 v3 分层。M1 新建独立的语义记忆应用/存储命名空间，不把生成式 Observation/Card 语义塞入要求原文正文的 v3 `MemoryUnit`。建议落点：

- `crates/memory/src/semantic/`：域对象、来源提炼、验证、卡片发布、读取资格与渲染。
- `crates/state/src/semantic_memory.rs`（必要时拆分子模块）：Vault-scoped SQL 仓储、版本/支持关系/作业与投影。
- `migrations/0034_semantic_memory.sql`：新表、复合键/外键和索引；不修改历史迁移。
- `crates/mcp/src/lib.rs` 与 `crates/admin-api/src/lib.rs`：只负责认证后的 DTO/参数适配和调用应用服务。
- `crates/providers/`：复用现有 Provider 能力；默认合成测试路径不外发真实资料。
- `crates/server/`：受限的持久后台工作和恢复编排。
- `crates/memory/tests/fixtures/semantic-memory/` 与集成测试：冻结合成来源、任务、gold 标注及失败记录。

### Data and transaction flow

来源身份复用 Vault Core 文件身份与当前授权规则。提炼输出先绑定不可变 SourceRevision 与服务分配的 EvidenceRef，再形成完整 ExtractionSet。单来源卡片的每一断言和必要限定都引用 Observation/EvidenceRef。发布前重验来源修订、权限、预期卡片修订和 Correction/Suppression 策略版本。规范卡片通过 Vault Core 写成受管 Markdown；State 先保存含预期修订与规范字节哈希的 prepared snapshot，再经 Core journal 提交文件，最后按 CAS 提交 State 当前投影。重启只在 snapshot、journal、修订与精确字节哈希一致时补完投影；不能证明时卡片保持不可读且不覆盖较新内容。索引晚到不能让无效旧卡恢复可读。

M2 跨来源整理只在有界、获准候选内生成关系/卡片提案，服务做身份、支持、修订和范围校验。保守失效先于异步重建。M3 将用户纠正和禁止规则应用于全部后续写入。M4 从当前有权卡片中确定性筛选、去重和组包。M7 使用独立 namespace 与影子评测，切换不覆盖并行期间的新写入。

### Public interfaces and schema changes

M1 先提供服务级提炼状态、卡片读取/列表与证据读取。M5 发布新 MCP/API/UI 能力；新工具的输入 schema、DTO、运行时拒绝规则和接口文档必须一同测试。经用户授权，MCP 旧 v3 工具和 `vault://memory/*` URI 当前已移除，不保留请求/响应迁移窗口；v3 storage/Admin 继续保留。所有端点由认证上下文导出 Vault，不接受任意 `vault_id`。

### Failure, retry, and recovery

Job checkpoint 绑定来源版本、输入哈希、profile/schema、权限策略、Correction/Suppression 版本和预期对象修订。网络/Provider 错误不能变为成功空输出；重试仅复用匹配且已验证的批次。来源在进行中变化时丢弃/重做受影响结果，不能绑定新版本。并发冲突重新读取并重算，不以最后写入覆盖。Vault Core/journal 恢复规范 Markdown 与 State 投影；回滚只允许回到仍能应用最新来源权限和 Suppression 的读取器，否则降级到来源检索或暂停读取。

## Work breakdown

1. **M0：落地映射与冻结契约。** 文件：本计划、`docs/adr/0035-semantic-memory-cards-and-task-packs.md`、`docs/semantic-memory-contract.md`、文档索引及评估 manifest。验证：核对现有代码/迁移/接口、检查无 Provider dry-run manifest、确认脏工作区未被覆盖。当前状态：已完成；目标与当前 v3 的过渡冲突由独立 ADR、语义 contract、manifest 和执行记录承载，未改写已有 dirty 的 `docs/interfaces.md`、`docs/memory-system.md`、Admin/UI/v3 文件。
2. **M1：单来源提炼闭环。** 文件：`migrations/0034_semantic_memory.sql`、新增 memory/State semantic 模块、`crates/mcp/src/lib.rs` 的服务适配、`crates/memory/tests/` 合成 fixtures。验证：EvidenceRef、来源角色、条件/否定/例外/顺序、成功空与失败、完整发布、权限、跨 Vault、失效先行、重启/重试及真实 JSON 入参。当前状态：本地工程通过；真实 Provider/任务语义质量不在本阶段结论内。
3. **M2：受控跨来源整理。** 文件：新关系/支持表及 memory organize 服务、候选查询与审计投影、关系 fixtures。验证：重复只折叠展示、相容补充逐项支持、冲突和范围隔离、强弱证据、AND/OR 失效、A≈B/B≈C 不传递合并。
4. **M3：增量/更正/遗忘。** 文件：来源更新事件、资格仓储/缓存失效、Correction/Suppression 存储和 API、恢复测试。验证：修改、移动、删除、撤权；纠正后重提炼；禁止再生成后重启/回滚；删除例外不会加强断言；取消任务不发布。
5. **M4：任务记忆包与对照入口。** 文件：检索与组包服务、无答案/预算用例、manifest/runner。验证：范围/版本过滤、核心完整预算、权限诊断、降级、A/B/C 相同来源与预算的工程重放。
6. **M5：人和 Agent 入口。** 文件：新 MCP schema/handler、Admin API/UI、接口文档、工具 schema 对照和协议端到端测试。验证：MCP `tools/list` schema 与 handler 相同接受/拒绝行为，跨工具/资源权限一致，旧 MCP 名称/URI 明确不可用且不提供迁移窗口；Admin/v3 storage 保持独立验证。
7. **M6：真实质量验收。** 文件：合成冻结集及保留集报告、真实运行结果和独立 agent review 记录。验证：本计划 §12 的支持精度、限定保留、重点覆盖、冗余、无答案、任务结果、用量及费用。未经授权不运行，未复核时保持 pending；报告不得将 agent review 标为 human review。
8. **M7：并行构建与切换准备。** 文件：dry-run/build/verify 脚本、影子状态、切换/回滚手册、运维检查表。验证：构建可重复、并行期事件追赶、来源与纠正连续性、测试客户端切换。生产切换和旧数据清理需要单独授权，不在此计划默认执行。

## Progress

- [x] `2026-09-14 17:30 Asia/Shanghai`：读取仓库指导、`PLANS.md`、当前 ADR、v3 记忆/接口/架构/迁移与安全规范；确认 v3 与新基线冲突及工作区已有批量 forget 改动。
- [x] `2026-09-14 17:30 Asia/Shanghai`：创建 ADR-0035 与 M0 语义数据/接口契约，建立独立执行记录；未运行 Provider，未改代码/迁移。
- [x] `2026-09-15 11:40 Asia/Shanghai`：M0 合成 manifest、无 Provider dry-run 与隔离入口已由 `semantic_memory_manifest` 集成测试验证；manifest 仅含合成来源、固定任务/预算/模型占位和本地路径，未知 live Provider 配置会被拒绝。`git diff --check` 通过。
- [x] `2026-09-15 10:20 Asia/Shanghai`：完成 M1 单来源 State/Memory foundation：0034 独立 semantic namespace、来源修订与 UTF-8 EvidenceRef、完整 ExtractionSet 状态、Observation、单来源 Card/CardRevision/逐项支持、prepared snapshot 和 M3 规则表；所有新对象按 `vault_id` 复合约束。
- [x] `2026-09-15 10:25 Asia/Shanghai`：完成 M1 本地 service 闭环实现：合成 Markdown block 输入、deny-unknown-fields proposal、伪造 block/空集合/完整集合校验、条件/例外/顺序/状态保留、受管 Markdown、Core 写入见证、State CAS projection、来源/授权版本先行失效和恢复入口；未调用 Provider。
- [~] `2026-09-17`：M4-A 确定性 MemoryPack 内部闭环进行中：新增 MemoryPackService、Vault-scoped State 当前 M1/M2 candidate/relation loaders、词法 admission/排序、严格范围/路径/版本未知降级、语义去重/冲突输出、完整限定保留、原子预算和候选前后资格重验。当前仅内部服务，不接 MCP/Admin/UI、Provider、向量、网络或生产；历史模式在当前结构无法证明时明确 gap。
- [x] `2026-09-15 11:40 Asia/Shanghai`：M1 定向测试通过；修复 SQLite 可空 `current_revision_id` 使用 scalar 解码为 `Some("")` 导致的新卡 CAS 冲突，并为 card-item 查询补充 `observation_kind` 列别名。完整结果集替换、成功空清卡、证据读取和 State Vault 隔离均有本地测试覆盖；未调用 Provider。
- [x] `2026-09-15`：补齐 M1 高优先级回归：State 直接调用拒绝跨来源修订的 Observation/CardItem/Evidence 支持并验证事务无半投影；generation/授权失效后的 written snapshot 不能由 apply 或恢复复活；同 File ID 与相同 hash 的移动只更新导航、不改变来源修订/generation，后续新内容修订仍可完成提炼。测试均使用临时 SQLite/Vault 和固定本地 proposal，未调用 Provider。
- [x] `2026-09-15`：真实并发复核发现同一来源/版本的不同幂等提炼在旧 generation 不变时可交叉提交：旧 running extraction 能在新 `success_empty` 或完整发布后复活/覆盖结果。新增前向迁移 `0036_semantic_extraction_commit_fences.sql`；新 extraction 在 State 事务内推进并捕获 Vault-scoped 提交序列，终态、prepare、apply、恢复和 Core 写入见证均校验该序列；旧作业只返回冲突，不会因新作业失败而复活。新增 A/B extraction 回归，覆盖旧 empty/prepare/apply 拒绝、新作业完整发布和持久 snapshot 栅栏。未调用 Provider。
- [x] `2026-09-15`：M2 第一段 State/schema foundation 完成。新增前向迁移 `0037_semantic_memory_organization.sql`、独立 `SemanticOrganizationRepository`/StateStore accessor、Vault-scoped job/idempotency、关系候选与决策、关系证据、独立 composed card/revision/items、AND/OR support groups/members、当前来源依赖、prepared snapshot/State projection witness 和 alias 表；未改 M1/v3/Admin/MCP/UI，未调用 Provider。
- [x] `2026-09-15`：M2 第二段组织 service 完成：确定性同 Vault M1 Observation 候选、严格 deny-unknown-fields proposal、scope/status/完整命题/实际候选 ID 校验、独立 composed Markdown 的 Core snapshot/CAS 发布；M2 关系、限定保留、non-transitive merge、AND/OR 失效语义均有本地回归，未调用 Provider。
- [x] `2026-09-15`：M2 第一轮语义 P1 修复：候选允许受限跨 scope，`keep_separate_scope` 持久化 relation decision；新增 0038 action audit，使 relation-only/no_change 可成功终态并支持幂等状态返回；supersedes 允许同 scope 不同 statement，但要求显式 reason/evidence，当前卡只保留新侧、历史关系不丢；OrganizationObservation/确定性 composed rendering 保留 conditions/exceptions/ordered_steps/result/uncertainty/time/value；create 强制所有合并候选直接连接同一 anchor，拒绝 A≈B、B≈C 的传递合并。新增 unit/State/Memory 回归，未调用 Provider。
- [x] `2026-09-15`：M2 生命周期/恢复完成：0039 前向迁移、精确 idempotency source fence 绑定、显式 `running/prepared/written/applied/failed/blocked/cancelled` 作业终态、Core witness 校验、pending snapshot/job 恢复入口和失败阻断均落地。修复 crash-before-prepare、候选/audit 失败清理、written witness 后 source fence/CAS 冲突无限重试；`organization_recovery_without_snapshot` 与 `semantic_recovery_apply_failed` 均持久化安全终态，批量恢复继续处理其他 job。M2 本地工程验收通过，未调用 Provider。
- [x] `2026-09-15`：M2 完整路径回归通过：真正 Core write+witness+apply 的 AND support 单侧来源失效后不可读，OR 单侧失效仍可读；关系候选/决策、限定保留、non-transitive merge、source-set/idempotency、Vault 隔离和 recovery/CAS 均有本地测试。0037—0039 均为前向迁移，未改写旧迁移或旧 v3 数据。
- [x] `2026-09-15`：M0 标记完成；独立 ADR/contract/manifest 处理新目标与当前 v3 的过渡冲突，未改写已有 dirty interface/memory/v3/Admin/UI 文档。
- [x] `2026-09-15`：M1 标记为本地工程通过。workspace gate 的 fmt、Clippy、全 workspace tests 和 `git diff --check` 均退出码 0；全 workspace 测试 376 通过、0 失败，M1 专项为 State semantic repository 5、Memory semantic 6、manifest 3。两次真实本地失败均已保留并修复：nullable `current_revision_id`/`observation_kind` 查询缺陷，以及新增 0036 后 8 处迁移版本 35 断言未更新为 36。下一步为 M2 受控跨来源整理。
- [x] `2026-09-15`：M2 收口后的最终 workspace gate 通过：`cargo fmt --all --check`、workspace Clippy、`cargo test --workspace --all-features`（389 tests 通过、0 失败）和 `git diff --check` 均退出码 0；中间真实失败（0037—0039 迁移版本断言、Xcode license wrapper、memory fixture 权限、M2 witness/CAS 与 candidate 生命周期）均已记录并修复。未调用 Provider、未操作生产 Vault。
- [ ] M3—M7：下一步进入 M3 增量/更正/遗忘；M6 真实 Provider/任务语义验收与 M7 隔离影子构建、切换仍 pending。
- [~] `2026-09-15`：M3-A 基础实现进行中：新增前向迁移 `0040_semantic_memory_lifecycle_rules.sql`，建立 Vault-scoped `semantic_targets`、单调 `semantic_rules_runtime`、规则幂等表和 `semantic_task_states`；扩展 M1/M2 extraction/snapshot/card revision/job/composed revision 的 target/rules fence。新增 `SemanticRulesRepository` accessor，支持稳定 target 派生、Correction/Suppression apply/revoke（expected revision/idempotency）、当前证据约束的 task state，以及 suppression 对 M1/M2 当前读取资格的即时过滤。未实现 worker、来源事件、公开 API、M4/M5、Provider 或生产操作。
- [~] `2026-09-15`：M3-B 规则语义 P1 进行中：新增前向迁移 `0041_semantic_memory_target_backfill.sql`，为既有 M1/M2 revision 建立非空 legacy target alias；新 target fingerprint 改为版本化 Observation 语义（statement/kind/scope/conditions/exceptions/steps/result/uncertainty），标题变化和 ID 重建不改变 target。Correction/`suppress_regeneration`/source-bound Suppression 在 M1/M2 prepare/read 资格前校验，rules fence 覆盖 success-empty、terminal/apply，task state 改为 not_started/in_progress/blocked/completed/unknown 并排除 completed target。新增 suppression 即时隐藏 M1 card/evidence 与 M2 composed card、AND/OR、cross-Vault、expected revision/idempotency 和 target 重用回归；未进入 worker/M4/M5/Provider/生产。
- [~] `2026-09-15`：M3-B 阻断修复：来源范围 Suppression 现在按 source/revision/observation 资格过滤 M1 Observation 候选、M1 卡片/证据和 M2 support member；M2 AND/OR 逐组判定，OR 保留仍有效的替代支持，不能仅以 target 规则替代来源资格。M1/M2 generation fence 改为实际依赖集合，并支持结构化 `replace`/`statement` 的逐行精确匹配；冲突保持安全 Conflict/终态审计路径，未将 Correction 永久阻断。revoke 要求 authorized actor，绑定校验 source/revision/observation/card/composed-card 复合归属；新增 0042 idempotency Vault FK。未调用 Provider。
- [x] `2026-09-16`：M3-B/B4 阻断修复完成（历史阶段记录）：Correction remove/replace 统一 NFKC、Markdown 列表/引用前缀与空白规范化，并拒绝 old/new 规范化后共存；source/revision 精确 Suppression 不再因共享 target 误伤其它来源，card-specific 规则按服务端稳定核心结构身份阻断 target 变化后的重生成；M2 composed card 依赖只保留实际 support，完整 job freshness fence 留在 job source 表；Core recovery 前统一执行 rules preflight。新增三来源 discovery→suppression→prepare、OR 有效证据保留、纠正行规范化、card_ref 改变后的稳定 ID/修订、真实 Core journal stale-rules preflight 回归。该阶段 State 74 tests、Memory semantic/organization 定向结果均为历史记录；本轮不调用 Provider/生产。
- [x] `2026-09-16`：M3-B6 修复完成：M2 composed card 保留 Observation kind，稳定卡片身份纳入 kind；composed semantic target fingerprint 纳入 item content/kind/ordinal、support operator/ordinal 及既有来源/证据语义，OR/AND 和新增/移除 supported information 不再复用同一 target，但相同核心身份仍连续复用卡片修订。`get_evidence` 增加 M2 composed support 的当前卡片/来源/修订/规则资格路径，并阻断被 M1 card-specific/source suppression 隐藏的支持边；新增 procedure kind、target split、added-item target split 和 M2 evidence 回归。未新增迁移（最高仍为 0042）；未调用 Provider/网络/生产。
- [x] `2026-09-16`：M3-B8 完成：M1 recovery 在任何 Core recovery 前统一校验 source revision/hash/authorization/generation/extraction commit sequence，stale source 会安全 terminalize 且保留 `RenameCommitted` journal；M1 source-revision/card-specific suppression 与 M2 candidate binding 精确区分，M2 composed-card suppression 按稳定 card ID 跨 target split 持续生效；M1 card kind 校验及 fingerprint、M2 span role/ordinal/hash fingerprint、completed task 当前 extraction evidence fence 补齐。新增 M1 stale-journal、card-only candidate、kind mismatch、旧 extraction task evidence 回归。未新增迁移，未调用 Provider/网络/生产。
- [x] `2026-09-16`：M3-B9 修复 M2 等价判定遗漏：`result`、`uncertainty`、`source_time_scope`、`value_for_future_work` 任一限定不同均不再判为 `equivalent` 或折叠进 composed card；新增四类限定逐项 adversarial 回归，确认 `create_composed_card` 拒绝丢失右侧限定。未调用 Provider/网络/生产。

`2026-09-18`：完成 semantic-only 默认上下文运行时收口。Worker admission 会取消
已存在的 `memory.*` 自动任务；启动、周期维护、Provider/embedding 变更不再排入
旧 overview，文件事件不再排入 `memory.source_reconcile`，仅保留
`semantic.source_reconcile`/`semantic.extract` 路径。Admin 普通 `/memories` 列表和
单项读取限定为 explicit raw；旧 extraction/resume/generation/overview/init 写入
入口返回停用诊断。旧 v3 数据、普通笔记、history 与 SQLite 记录未清理。新增 server
回归覆盖旧任务取消和文件事件不入队；Provider、网络、生产与 `./data` 均未访问。

## Decisions

- 新记忆表达以 Observation、MemoryCard 和 MemoryPack 为主，原文只作为证据；该决定由 ADR-0035 固定。
- M1 来源限于当前 Vault opt-in 和授权规则下的 Markdown 笔记；对话和工具记录是未来扩展，不阻塞闭环，也不声称已实现。
- 新语义数据采用独立 namespace 与迁移 0034 起的前向 schema；不复用现有 v3 完整原文单元正文模型。
- M1 单来源，M2 才开始跨来源整理；M4 先确定性组包；M5 才发布公开入口。
- 当前 MCP 公开接口采用用户授权的 clean break；旧工具名与 `vault://memory/*` URI 不保留迁移窗口，公开 schema 与运行时 DTO/校验采用一致性测试约束。v3 storage/Admin 仅作为历史/控制面边界保留。
- 不调用收费 Provider、不发送真实资料、不访问生产 Vault、不清空旧记忆、不改写普通笔记。真实语义和生产验收 pending。
- M0 文件路径选择复用 `semantic-memory-implementation-plan.md` 的小写半角命名；首个语义 migration 为 `0034_semantic_memory_cards.sql`，当前迁移最高为 0042。

## Surprises and discoveries

- ADR-0033 与当前 v3 规范将原文片段规定为正式自动记忆正文，并排除跨来源整理，与本计划核心表达方向冲突。ADR-0035 仅取代冲突决定，不改写 ADR-0033 的历史理由。
- `docs/product-requirements.md` §3.5、`docs/architecture.md` §11、`docs/data-model.md` §13、`docs/memory-system.md`、`docs/interfaces.md` 仍称 v3 为规范契约，并记录已授权旧记忆清理；需要按新契约同步，不能让同一版本继续存在两套规范定义。
- 仓库迁移当前到 0042；0033 及更早是历史 v3/legacy 基线，新语义 schema 从 0034 起。
- 工作区有未提交批量 forget 修改，语义计划和该功能共享 `memory-system.md`、`interfaces.md`、v3 memory service 等区域；本计划要求后续精确合并，不能覆盖原改动。
- 无真实 Provider 请求，也没有真实语义质量结果。本计划不把工程替身输出当成质量证据。
- State 回归首次揭露 `upsert_source_revision` 内容变化分支错误返回 `content_changed=false`；修复为 true 后，新的来源修订能保留 `source_changed` 失效原因。当前工作区已有其他改动，未触碰 v3/Admin/UI/interfaces/memory-system。

## Validation

M0 文档验证：

```bash
git diff --check
```

M1 开始后的必需工程检查（按实际变更增补）：

```bash
cargo fmt --all --check
cargo test -p mcp-vault-state --all-features
cargo test -p mcp-vault-memory --all-features
cargo test -p mcp-vault-mcp --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

本轮 M1 实际命令与结果：

```text
git diff --check                                      FAIL before diff inspection: local macOS git wrapper refused because the Xcode license is not accepted (exit 69)
cargo check -p mcp-vault-memory --all-features        PASS
cargo check -p mcp-vault-state --all-features         PASS
cargo check -p mcp-vault-memory --tests --all-features PASS
cargo check -p mcp-vault-state --tests --all-features  PASS
cargo clippy -p mcp-vault-memory -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo test -p mcp-vault-state --all-features --lib    PASS (27 tests)
cargo test -p mcp-vault-memory --all-features --lib   PASS (23 tests)
cargo test -p mcp-vault-memory --test semantic_memory --all-features FAIL at link stage: local macOS xcrun/cc refused because the Xcode license is not accepted (exit 69)
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features FAIL at link stage: local macOS xcrun/cc refused because the Xcode license is not accepted (exit 69)
```

`2026-09-15` 定向复核（本地合成数据、无 Provider）实际结果：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (3 tests)
cargo test -p mcp-vault-state --all-features --lib             PASS (27 tests)
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features PASS (2 tests)
cargo test -p mcp-vault-memory --test semantic_memory_manifest --all-features PASS (3 tests)
cargo clippy -p mcp-vault-memory -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
git diff --check                                           PASS
```

本次 M1 高优先级回归实际命令与结果：

```text
cargo fmt --all                                           PASS
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features PASS (4 tests, including observation/evidence revision fences and rollback checks)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 tests)
git diff --check -- <三项改动文件>                           PASS
cargo clippy -p mcp-vault-state --test semantic_memory_repository --all-features -- -D warnings FAIL (现有 semantic_memory.rs:934 type_complexity，非本轮新增测试；exit 101)
cargo clippy -p mcp-vault-memory --test semantic_memory --all-features -- -D warnings FAIL before test target analysis (same existing semantic_memory.rs:934 type_complexity; exit 101)
```

早期完整 crate suite 曾因本地 Xcode license 和定向工程缺陷中止；后续修复并重跑 workspace gate，最终结果以本节最后的 workspace 记录为准。

本次复核保留的真实失败记录：首次语义集成测试为 3 个中 1 通过、2 个失败（`State(Conflict)`）；定位为 SQLite nullable scalar 解码使新卡的 NULL CAS 期望变成空字符串。修复为显式 `(Option<String>,)` 行解码后，下一轮仅剩 `Database(ColumnNotFound("observation_kind"))`，补充 SQL 列别名后上述 3 个测试全部通过。失败均为本地工程缺陷，不是 Provider 或语义质量验收。

`2026-09-15` 并发栅栏复核首次运行 `cargo test -p mcp-vault-state --all-features` 时，8 个历史迁移测试仍断言迁移版本 `35`，而新增 `0036_semantic_extraction_commit_fences.sql` 后当前版本为 `36`；其余 State 单元测试为 19/27 通过。按真实失败修复，仅将 `crates/state/src/pool.rs` 的 8 处当前版本断言更新为 `36`，无运行时逻辑变更；随后 State/Memory 全特性测试及 Clippy 均通过，不将该断言失败归因于 Provider。

断言修复后的实际结果：`cargo fmt --all --check` 退出码 0；`cargo test -p mcp-vault-state --all-features` 退出码 0（State 单元 27、auth 3、background 12、repositories 18、semantic repository 5，doc tests 0）；`cargo test -p mcp-vault-memory --all-features` 退出码 0（Memory 单元 23、Admin initialization 9、memory initialization 1、v3 11、semantic 6、manifest 3，doc tests 0）；`cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings` 退出码 0；`git diff --check` 退出码 0。全部为本地合成/隔离测试，未调用 Provider、网络或生产 Vault。

`2026-09-15` lint 修复与定向复核：`semantic_memory.rs:934` 的 `clippy::type_complexity` 已通过私有 `PublicationSourceSnapshot` 类型别名最小化修复，查询字段和运行时行为未改变。实际结果：

```text
cargo fmt --all --check                                      PASS (exit 0)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS (exit 0)
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS (exit 0)
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features PASS (4 passed, exit 0)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 passed, exit 0)
git diff --check                                           PASS (exit 0)
```

`2026-09-15` extraction commit fence 修复定向结果：

```text
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features PASS (5 passed, exit 0)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 passed, exit 0)
```

本轮新增迁移为 `0036`，没有改写 `0034`/`0035`；新增回归使用临时 SQLite/Vault 和合成数据，未调用 Provider、网络或生产 Vault。

`2026-09-15` M2 State/schema foundation 实际结果：

```text
cargo fmt --all --check                                      PASS
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (3 passed)
cargo test -p mcp-vault-state --all-features                  PASS (68 passed, 0 failed; doc-tests 0)
git diff --check                                             PASS
```

首轮 State 全量测试曾因新增迁移后旧的 migration-version 断言仍为 `36` 失败；仅将受影响的测试期望更新为 `37` 后重跑通过。未调用 Provider、网络或生产 Vault。

`2026-09-15` M2 organization service 实际结果：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (2 passed)
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
```

本段为本地合成/隔离工程验证；M2 关系完整矩阵、M6 真实 Provider 语义质量和 M7 隔离影子切换仍 pending。

`2026-09-15` M2 P1 修复实际结果：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-memory --lib --all-features           PASS (28 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (2 passed)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 passed)
cargo test -p mcp-vault-state --all-features                  PASS (68 passed, 0 failed; doc-tests 0)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (3 passed)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                             PASS
```

M2 仍是本地工程阶段；真实 Provider/M6、完整关系质量评测和 M7 隔离影子切换 pending。

本轮新增前向迁移为 `0038_semantic_organization_action_audit.sql`，未改写 `0037`；Memory unit tests 新增 5 个 P1 语义回归（different scope candidate、完整限定渲染、supersedes、no_change、非传递合并）。

`2026-09-15` M2 生命周期/恢复 P1 修复进行中：新增前向迁移 `0039_semantic_organization_lifecycle_witness.sql`，为组织作业固定 `running/prepared/written/applied/failed/blocked/cancelled` 状态，并为 prepared snapshot 保存 expected/actual 文件身份、修订、hash 与 source-fence witness。幂等重放绑定排序后的完整 source fence 集合、input/policy/profile，并重新验证当前来源资格；候选、action audit、prepare、Core witness、apply 失败均留下 failed/blocked 终态。新增 pending job/snapshot 查询、精确 Core NotFound 创建与恢复入口；恢复仅在 source fence、Core file identity/revision/bytes hash 和 State CAS 一致时发布。新增/补充 stale replay、wrong revision/hash witness、source-set mismatch 回归；未调用 Provider、网络或生产 Vault。

本轮实际结果：

```text
cargo fmt --all                                           PASS (exit 0)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (3 passed, exit 0)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (2 passed, exit 0)
cargo test -p mcp-vault-state --all-features                PASS (68 passed, 0 failed; doc-tests 0)
cargo test -p mcp-vault-memory --all-features               PASS (60 passed, 0 failed; doc-tests 0)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS (exit 0)
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS (exit 0)
git diff --check                                          PASS (exit 0)
```

本轮曾出现一次仅闭包换行导致的 `cargo fmt --all --check` 失败，随后 `cargo fmt --all` 修复；无 Provider/网络/生产操作。M2 仍仅为本地工程阶段，完整关系质量矩阵、M6 真实语义验收和 M7 影子切换 pending。

`2026-09-15` M2 第三轮实际结果：

```text
cargo fmt --all                                           PASS (exit 0)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (4 passed, exit 0)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (3 passed, exit 0)
cargo test -p mcp-vault-state --all-features                PASS (69 passed, 0 failed; doc-tests 0)
cargo test -p mcp-vault-memory --all-features               PASS (61 passed, 0 failed; doc-tests 0)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS (exit 0)
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS (exit 0)
git diff --check                                          PASS (exit 0)
```

新增回归覆盖：无 snapshot 的 running job 恢复转 blocked、批量恢复继续处理其他 job；prepare 失败后 candidate 无 pending、audit 为 blocked；失败 job 的 Vault/job 关联清理保持隔离；失败后同源新 job 使用 job-scoped candidate ID 可安全重试。M2 仍为本地工程阶段，完整关系质量矩阵、M6 真实语义验收和 M7 影子切换 pending。

`2026-09-15` M2 最终恢复 P1 实际结果：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (4 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (4 passed)
cargo test -p mcp-vault-state --all-features                  PASS (69 passed, 0 failed; doc-tests 0)
cargo test -p mcp-vault-memory --all-features                 PASS (62 passed, 0 failed; doc-tests 0)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                             PASS
```

本轮回归实际覆盖 written snapshot source fence 失效、recovery 阻断、pending 清空、卡不可读和 batch continuation。未调用 Provider、网络或生产 Vault。

`2026-09-15` M2 完整路径回归实际结果：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (4 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (4 passed)
cargo test -p mcp-vault-state --all-features                  PASS (69 passed, 0 failed; doc-tests 0)
cargo test -p mcp-vault-memory --all-features                 PASS (62 passed, 0 failed; doc-tests 0)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                             PASS
```

Recovery apply gate 仅用于确定性本地回归，不改变普通调用路径；无 Provider、网络或生产 Vault。

`2026-09-15` M3-A 当前实际验证：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-state --test semantic_rules --all-features PASS (2 passed)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (4 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (4 passed)
cargo test -p mcp-vault-state --all-features                  PASS (72 passed, 0 failed; doc-tests 0)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
cargo fmt --all --check                                      PASS
git diff --check                                             PASS
```

本段曾先后发现并修复规则模块的 UUID/类型/摘要编译错误，以及 M1 prepared snapshot 扩展后的 SQLite `23 values for 24 columns` 占位符错误；修复后上述 State/M2 回归通过，并新增 rules_revision 变化阻止旧 composed snapshot/job apply 的回归。此前一次 `cargo test -p mcp-vault-memory --all-features` 曾因 `memory_v3` fixture 在 `crates/memory/tests/memory_v3.rs:172` 执行 `TcpListener::bind("127.0.0.1:0")` 受沙箱本地监听限制而返回 `Operation not permitted`，导致 9 个失败；该结果属于历史环境记录，与本轮 MCP clean-break 逻辑无关，不代表当前状态。M3-A 仍未进入来源事件或 worker；M6/M7、Provider 与生产验收保持 pending。

`2026-09-15` M3-B 当前实际验证：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-state --test semantic_rules --all-features PASS (2 passed)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (4 passed)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (4 passed)
cargo test -p mcp-vault-state --all-features                  PASS (71 passed, 0 failed; doc-tests 0)
cargo test -p mcp-vault-memory --all-features                 PASS (62 passed, 0 failed; doc-tests 0)
cargo clippy -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                             PASS
```

M3-B 中间真实错误包括 migration 0041 版本断言更新、规则 API action/actor 参数迁移、source-scoped suppression 初次错误地使同 target 其他来源一起失效，以及 stable target 测试 fingerprint 调整；均已修复。M3 仍进行中，尚未实现来源事件/worker、公开 correct/forget API、M4/M5；M6/M7、Provider 与生产验收保持 pending。

补充历史失败记录：早先一次 `cargo test -p mcp-vault-memory --all-features` 曾在 `crates/memory/tests/memory_v3.rs:172` 的 `TcpListener::bind("127.0.0.1:0")` 处受当前沙箱本地监听限制，返回 `Operation not permitted` 并有 9 个失败（与 M2 service/schema 及本轮 MCP clean-break 逻辑无关）；该记录保留为历史环境证据，不能作为当前失败结论。修复后当前环境全量重跑通过 64 项；State 全量通过 74 项（本轮新增规则单元测试后，Rust 单元测试总数为 29，集成测试合计 45）。

本次修复未调用 Provider、网络或生产 Vault；真实语义质量验收仍为 pending。

`2026-09-16` M3-B4 修复后验证：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-state --test semantic_rules --all-features PASS (3 passed)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (4 passed)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (6 passed)
cargo test -p mcp-vault-state --all-features                  PASS (74 passed, 0 failed; doc-tests 0)
cargo test -p mcp-vault-memory --all-features                 PASS (64 passed, 0 failed; doc-tests 0)
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                             PASS
```

新增恢复回归使用 Vault Core 的真实 `RenameCommitted` 故障注入生成未完成 journal；规则版本过期时，M1/M2 recovery 在调用 `core.recover` 前终止，journal 保持未 reconciliation，State 不发布卡片。该回归未调用 Provider、网络或生产 Vault。

`2026-09-16` M3-B6 实际验证（仅本地定向工程检查）：

```text
cargo fmt --all --check                                      PASS
cargo test -p mcp-vault-state --test semantic_rules --all-features PASS (3 passed)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (4 passed)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (7 passed)
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                             PASS
```

`2026-09-16` M3-B7 实施并定向复核：success-empty 后 M2 support qualification/current observations/evidence 绑定当前 source extraction commit sequence，AND 失去一侧不可读、OR 保留独立有效支持；M1/M2 read/qualification 谓词区分 composed-card-specific 规则，M2 输出过滤实际被 M1 card-specific 规则隐藏的 support member；observation 规则同时校验 card/composed-card 绑定及当前 item/support membership；M2 正常写入与 recovery 在每次 Core write/recover 前执行统一 rules/source prewrite fence；M1 target fingerprint 纳入 item kind/content/ordinal 和全部 evidence span identity。新增 success-empty AND/OR、composed-card scope isolation、observation binding、stale source prewrite 和 direct State target-split 回归。未新增迁移，未调用 Provider/网络/生产，未进入 M3-C/M4/M5。

本轮定向命令：

```text
cargo test -p mcp-vault-state --test semantic_rules --all-features PASS (3 passed)
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features PASS (7 passed)
cargo test -p mcp-vault-state --test semantic_organization_repository --all-features PASS (5 passed)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (6 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (11 passed)
cargo fmt --all --check PASS
cargo check -p mcp-vault-state -p mcp-vault-memory --all-features PASS
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check PASS
```

`2026-09-16` M3-B8 实际验证：State semantic_rules 3、semantic_memory_repository 8、semantic_organization_repository 5；Memory semantic_memory 7、semantic_organization 11，全部通过。新增 M1 `RenameCommitted` stale-source recovery 回归确认 incomplete journal 保持未恢复且无卡片发布；新增旧 extraction 同 source/revision 的 completed task evidence 拒绝回归。`cargo fmt --all --check`、定向 State/Memory Clippy 和 `git diff --check` 均通过。

`2026-09-16` M3-B9 实际验证：`cargo fmt --all --check` 通过；`cargo test -p mcp-vault-memory --lib semantic::organize::tests --all-features` 7 passed；`cargo test -p mcp-vault-memory --test semantic_organization --all-features` 11 passed；`cargo test -p mcp-vault-memory --test semantic_memory --all-features` 7 passed；`cargo clippy -p mcp-vault-memory --all-targets --all-features -- -D warnings` 通过；`git diff --check` 通过。新增回归覆盖 result/uncertainty/source_time_scope/value_for_future_work 差异均不创建 composed equivalent card。未调用 Provider/网络/生产。

`2026-09-16` M3-C1 实施并验证：`FileRepository::commit_mutation` 通过 State 内部事务 seam 同步处理 semantic source qualification；delete/restore/实际 content hash 变化递增 source generation、设置 pending rebuild 并撤回 M1 card/evidence，M2 AND/OR 继续由现有 source qualification 谓词即时计算；同 FileID 同 hash Move 只更新 semantic source navigation，不失效或递增 generation；新 FileID 不继承旧 semantic source。`SettingsRepository::set_vault` 对 `memory.units.policy` 任意 revision 变化在同一事务失效当前 Vault semantic sources/cards/evidence。新增 Core/State/M1/M2/policy/Vault-scope 回归，未接 server worker、M4/M5、Provider、网络或生产。

本轮命令与结果：

```text
cargo fmt --all                                            PASS
cargo check -p mcp-vault-state -p mcp-vault-memory --all-features PASS
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features PASS (9 passed)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (9 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (11 passed)
cargo test -p mcp-vault-state --all-features                      PASS (79 passed, 0 failed)
cargo test -p mcp-vault-memory --all-features                     PASS (74 passed, 0 failed)
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                             PASS
```

`2026-09-16` M3-C1.1 审查修复并验证：semantic schema guard 现在依据 `_sqlx_migrations` 区分未迁移/0034 之前数据库与已记录语义迁移的数据库；已迁移但缺少 `semantic_sources`、`semantic_memory_cards` 或 `semantic_evidence_refs` 时返回 `IntegrityFailure`，不再静默跳过失效。新增 partial-schema/迁移版本回归；补充 restore 提交后的卡片/evidence 即时失效，以及 file metadata 事务在 outbox hook 失败时回滚语义失效的回归。SQLite `file_entries.id` 是全局主键，因此跨 Vault 不能合法复用同一 FileID；既有 Vault-scoped 查询和跨 Vault 回归保持。未新增迁移，未调用 Provider/网络/生产。

本轮定向命令与结果：

```text
cargo fmt --all                                            PASS
cargo test -p mcp-vault-state --test semantic_memory_repository --all-features PASS (10 passed)
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (11 passed)
cargo test -p mcp-vault-memory --test semantic_organization --all-features PASS (11 passed)
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
cargo fmt --all --check                                    PASS
git diff --check                                           PASS
```

`2026-09-16` M3-C1.2 修正 restore 回归：测试现在按 A 发布 → replace 为 B → 重新 prepare/submit 发布 B 卡片与 evidence 并确认其当前可读 → restore 到 revision 1 → 在未消费 outbox 前确认 B 卡片/evidence 立即不可读，同时确认 source `eligible=0`、`pending_rebuild=1`。不再由 replace 之前的失效状态充当 restore 证据。未改运行时架构，未新增迁移，未调用 Provider/网络/生产。

本轮定向命令与结果：

```text
cargo fmt --all                                            PASS
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (11 passed)
cargo clippy -p mcp-vault-memory --test semantic_memory --all-features -- -D warnings PASS
cargo fmt --all --check                                    PASS
git diff --check                                           PASS
```

`2026-09-16` M3-C2 实施并验证：新增 `SemanticMemoryService::reconcile_source_event` 内部应用服务，仅从当前 Vault `FileRecord` 和 Vault Core 读取活动 Markdown；删除/缺失/非 Markdown/受管路径保持 source 失效，policy disabled/invalid 返回安全配置 disposition 且不读文件。活动来源复核 File ID、current revision、记录 hash 与实际字节 hash 后调用 State source rebind；同 FileID 同 hash 仅返回 `navigation_only`，内容变化、restore 或新 source 返回 `needs_rebuild`，不调用 Provider。新增重定位、内容变化、删除、restore、重复/晚到事件及 policy disabled/invalid 临时 Vault 回归；补充 Core 读取后再次读取当前 FileRecord，避免并发晚到事件写回旧 path/revision。未接 server worker、未新增迁移、未调用 Provider/网络/生产。

`2026-09-16` M3-C2.1 审查修复并验证：State 新增 `upsert_source_revision_fenced`，在同一 State 事务内按 Vault + File ID 读取并校验当前 path、typed revision 和带/不带 `sha256:` 前缀的 content hash；不匹配统一返回 `Conflict`，不更新 semantic source。`prepare_source` 与 `reconcile_source_event` 均改走 fenced rebind；保留无文件行的历史 direct-State fixture 使用旧显式方法。新增 same-hash Move 后旧 `FileRecord` fence 拒绝、source navigation/card 保持新路径，随后当前事件仍 `navigation_only` 的确定性回归。未接 C3 worker、未新增迁移、未调用 Provider/网络/生产。

本轮定向命令与结果：

```text
cargo fmt --all                                            PASS
cargo check -p mcp-vault-memory --all-features             PASS
cargo test -p mcp-vault-memory --test semantic_memory --all-features PASS (12 passed)
cargo clippy -p mcp-vault-memory --test semantic_memory --all-features -- -D warnings PASS
cargo fmt --all --check                                    PASS
git diff --check                                           PASS
```

M3-C2.1 定向验证：`cargo test -p mcp-vault-memory --test semantic_memory --all-features` PASS（13 tests）；`cargo test -p mcp-vault-state --test semantic_memory_repository --all-features` PASS（10 tests）；`cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings` PASS；`cargo fmt --all --check` PASS；`git diff --check` PASS。

`2026-09-16` M3-C3a 实施并验证：保留旧 v3 `index.rebuild` 与
`memory.source_reconcile` 入队/handler 行为，文件 outbox 事件另外按
Vault + event ID 去重入队 `semantic.source_reconcile`。语义 worker 只通过
`SemanticMemoryService::reconcile_source_event`、State 和 Vault Core 复核
当前 FileRecord；`NeedsRebuild` 才入队 `semantic.extract`，导航、删除、
managed、disabled/invalid policy 不入队提炼。`semantic.extract` 当前为安全
终止占位：不读 source、不调用 Provider、不伪造结果，永久失败码为
`semantic_provider_not_authorized`，不会形成重试风暴。progress 仅写安全的
disposition、稳定 ID、revision/generation 与 follow-up，不写 path/body。
新增真实临时 Vault/server worker 回归覆盖 v3+semantic 双链、event-id
去重、当前源复核、extract 安全失败和 progress 脱敏；未改 v3 表/handler，
未调用 Provider、网络或生产 Vault。

本轮定向命令与结果：

```text
cargo fmt --all                                      PASS
cargo check -p mcp-vault-server --all-features      PASS
cargo test -p mcp-vault-server --all-features       PASS (49 tests)
cargo clippy -p mcp-vault-server --all-targets --all-features -- -D warnings PASS
cargo test -p mcp-vault-server --all-features workers::tests::semantic_ PASS (2 tests)
cargo test -p mcp-vault-state --all-features                 PASS (80 tests)
cargo test -p mcp-vault-memory --all-features                PASS (78 tests)
```

`2026-09-16` M3-C3a.1 admission 修复并验证：semantic.source_reconcile 现在只接受事件所属 Vault 中当前存在的 regular FileRecord，并使用该权威记录的路径判断 managed/reserved；无效 File ID、缺失 FileRecord 和目录记录均不再产生 semantic job。保留删除 tombstone 的 FileRecord 以便 source invalidation；v3 index/source-reconcile admission 未改变。新增真实临时 Vault/server 回归覆盖目录事件、payload 路径与权威 FileRecord 冲突、删除 tombstone、无效/缺失 FileRecord，以及相同 event ID 的跨 Vault 去重隔离。未调用 Provider、网络或生产 Vault。

本轮定向命令与结果：

```text
cargo fmt --all                                      PASS
cargo test -p mcp-vault-server --all-features workers::tests::semantic_ -- --nocapture PASS (4 tests)
cargo test -p mcp-vault-server --all-features       PASS (51 tests)
cargo clippy -p mcp-vault-server --all-targets --all-features -- -D warnings PASS
git diff --check                                     PASS
```

`2026-09-16` M3-C3b 实施并验证：启动恢复现在先为每个 Vault 执行 M1 publication 与 M2 organization 的 source/rules preflight，再进入 generic Core maintenance recovery；stale/blocked 语义作业将该 Vault 安全标记为 Error，保留未能证明安全的 Core journal，不影响其它 Vault。generic Core recovery 成功后，语义 publication/organization 通过 no-Core-recovery recovery API 重新校验 source/rules fence，再完成 canonical witness 与 State projection apply；无 pending work 时保持 no-op。v3 initialization/recovery 流程未改变，未接 Provider。

`2026-09-16` M3-C3b.1 审查修复并验证：State 的 M1/M2 pending recovery 查询现在继续纳入与未完成 `operation_journal` 目标路径相交的 blocked snapshot/job；因此 stale source/rules preflight 后的 blocked 语义 journal 在第二次启动仍构成 Vault 级 barrier，generic Core 不会在后续启动 finalize `RenameCommitted`。新增统一跨类型 semantic preflight，所有会触发 generic Core recovery 的 M1/M2 service API（包括提案自动恢复）均先检查同 Vault 两类语义 barrier；server 启动复用该 coordinator。补充第二次启动回归，以及 M1 recovery 遇 stale M2 journal、M2 recovery 遇 stale M1 journal 的交叉回归。v3 initialization pending 仍在 semantic barrier 之后检查，不能绕过 stale semantic journal。未调用 Provider、网络或生产 Vault。

C3b.1 定向验证：`cargo fmt --all --check`、`git diff --check`、State/Memory/Server 相关 semantic integration tests、`cargo check -p mcp-vault-state -p mcp-vault-memory -p mcp-vault-server --all-features` 与 `cargo clippy -p mcp-vault-state -p mcp-vault-memory -p mcp-vault-server --all-targets --all-features -- -D warnings` 均通过；相关 State/Memory/Server 测试分别通过 15、24、52 项（其中交叉与二次启动回归均通过）。

新增真实 server startup 回归：`RenameCommitted` pending semantic journal 在 stale source 时不会被 generic Core finalise，managed card 不发布且 source/card 不可读；同一启动批次中的另一 Vault 仍可完成安全语义恢复；原有 initial scan、disabled Vault 和 worker recovery 测试保持通过。未调用 Provider、网络或生产 Vault。

本轮定向命令与结果：

```text
cargo fmt --all --check                                      PASS
cargo clippy -p mcp-vault-memory -p mcp-vault-server --all-targets --all-features -- -D warnings PASS
cargo test -p mcp-vault-memory --all-features --test semantic_memory --test semantic_organization PASS (24 tests)
cargo test -p mcp-vault-state --all-features --test semantic_memory_repository --test semantic_organization_repository PASS (15 tests)
cargo test -p mcp-vault-server --all-features                 PASS (52 tests)
git diff --check                                             PASS
```

`2026-09-16` M3-C3b.2 实施并验证：M2 正常 `organize_json` 在
`ensure_composed_file` 期间发生 Core 错误时现在把 job 终止为 blocked，保留
blocked snapshot 与未完成 journal 的 pending barrier；写前纯校验失败仍保持
普通 failed。新增真实 `RenameCommitted` 失败 → source stale → recovery 回归，
确认第二次 recovery 不 finalize journal 且不发布 composed card。

跨类型 semantic preflight 下沉到 StateStore 应用边界，Backup 不依赖 Memory
crate，也不直接访问 SQL。restore post-check 与 maintenance reopen 的每个
generic Core recovery 入口均先执行同一 Vault-scoped preflight；stale semantic
journal 会安全失败并保持 Offline/不可 ready。新增临时 Vault 的 backup
maintenance recovery 回归。

server stale-barrier startup 回归改为 file-backed SQLite，第一次恢复后关闭
StateStore，重新 migrate/connect 后执行第二次 startup recovery；blocked journal
仍保持未 reconciliation，另一 Vault 仍安全恢复。未调用 Provider、网络或生产
Vault，未新增迁移。

C3b.2 定向验证：

```text
cargo fmt --all --check                                      PASS
cargo clippy -p mcp-vault-state -p mcp-vault-memory -p mcp-vault-backup -p mcp-vault-server --all-targets --all-features -- -D warnings PASS
cargo test -p mcp-vault-backup --all-features                 PASS (3 tests)
cargo test -p mcp-vault-memory --all-features --test semantic_organization normal_organization_core_failure_remains_a_barrier_on_recovery PASS
cargo test -p mcp-vault-server --all-features backup_maintenance_recovery_preflights_stale_semantic_journal PASS
cargo test -p mcp-vault-server --all-features startup_semantic_preflight_blocks_stale_core_recovery_but_recovers_other_vaults PASS
git diff --check                                             PASS
```

Provider 接线、真实 Provider/任务语义验收、M4/M5 及生产仍保持 pending。

`2026-09-16` M3-C3b.3 审查修复：restore 在进入 Offline 后创建 pre-restore
safety backup；若该 backup 的 `mark_running` admission 失败，服务继续保持
Offline，并立即将 readiness 置为 false，不能出现 Offline 但 ready=true，也不
能把真实 restore 当作成功。新增仅测试态 failure injection，通过完整 restore
入口确认原 Vault 文件未被替换、restore 返回错误、两个 backup catalog rows
均为 failed、maintenance lease 已释放但 mode 仍为 Offline。未调用 Provider、
网络或生产 Vault。

C3b.3 定向验证：

```text
cargo test -p mcp-vault-backup --all-features restore_safety_backup_admission_failure_keeps_offline_not_ready PASS
cargo test -p mcp-vault-backup --all-features PASS
cargo fmt --all --check PASS
cargo clippy -p mcp-vault-backup --all-targets --all-features -- -D warnings PASS
git diff --check PASS
```

`2026-09-16` M3-D1 deterministic local rebind 收口：source content revision/hash
变化后，服务使用当前 FileRecord path 配合 Vault Core immutable revision 读取旧
bytes，并校验旧 File ID、revision/hash、source generation、extraction commit、
authorization 与 rules fence。每个历史 body/context span 必须在新 UTF-8 内容中
唯一、非重叠、边界完整匹配，并且精确落在当前 LocalBlock；任一 observation
无法证明时，该 card 不进入新 proposal。完整 card 子集经正常
`submit_proposal_json -> prepare_publication -> Core journal -> apply` 路径生成
新 source revision/card revision；只要任一旧 card、observation 或 evidence
无法完整证明，整个 ExtractionSet 都不发布、不生成 `success_empty`，现有
cards/evidence 保持不可读并返回 `NeedsRebuild`。不引入持久 partial 状态，避免
把已验证子集伪称为完整结果集。

M2 不直接改绑 composed card；其旧 AND 支持在 M1 revision/evidence 变化后不再
满足资格，独立 OR 支持仍可由另一当前 source member 保持可读，等待正常后续
organization。restore、晚到 current FileRecord fence、rules/auth revision
变化、重复 span、关键 context 变化、LocalBlock role/code-fence 漂移、
`source_stated` time scope、跨 Vault/File ID 均 fail-closed；仅 `unknown` time
scope 可在其余约束完整时保留。Provider
仍未接线，真实 Provider/任务质量、生产 Vault、M4/M5/M6/M7 保持 pending。

D1 中间故障及修复记录：

1. 首次 unrelated-insert 回归返回 `NeedsRebuild`，因为 C1 invalidation 已将
   current source generation 从历史 revision 的 0 增至 1；修正为比较 immutable
   historical revision generation，而不是失效后的 current source generation。
2. 首次 proposal 重发被正常校验拒绝为
   `semantic_unknown_time_has_value`；历史 `unknown` time scope 不得携带重绑的
   context evidence；这是已被后续审查替换的中间尝试。最终实现对
   `source_stated` time scope 直接 fail-closed，不进行 deterministic rebind。
3. 第二次 proposal 重发被拒绝为
   `semantic_observation_index_invalid`；card 的多个 material item 可指向同一
   observation，重建 card proposal 时现按 card item 顺序去重 observation index。
4. 审查发现 partial 子集发布会破坏完整 ExtractionSet 不变量；已改为任一 card
   不完整即整源 fail-closed，未调用 normal publish，也不创建 partial 状态。
5. 审查发现历史 loaders 原先缺少完整 extraction/source-revision scope 约束；
   现通过 Vault/source/revision/profile joins 与显式 ID/FileID 校验，并拒绝
   source-stated time scope，避免把不精确保存的全部 context 冒充 time evidence。

D1 新增/验证测试：完整 ExtractionSet 无 partial 发布、重复/关键 context
fail-closed、rules/auth fence、LocalBlock role/code-fence 漂移、source-stated
time scope、file-backed historical observation scope corruption，以及 M2 local
rebind 后 AND 不可读、独立 OR 保持可读回归；现有 restore/晚到事件、无卡片
`pending_rebuild`、跨 Vault/File ID、State rules persistence 与 recovery 回归
继续作为边界证据。corruption 回归通过真实 SQLite update 将已发布历史
observation 归属改到同 Vault 的另一合法 source revision，再经真实
`reconcile_source_event` 验证 loader/service 拒绝采纳、无 publish；另有 card
revision profile corruption 与 State `prepare_publication` profile mismatch
atomicity 回归。最终实现对 `source_stated` time scope 明确采取 fail-closed，
不把中间尝试保留为可重绑行为。

D1 最终 gate（2026-09-16）：

```text
cargo test -p mcp-vault-memory --all-features --test semantic_memory --test semantic_organization PASS (22 + 14; includes observation/profile corruption regressions)
cargo test -p mcp-vault-state --all-features --test semantic_memory_repository --test semantic_organization_repository PASS (12 + 6; includes prepare profile atomicity regression)
cargo test -p mcp-vault-server --all-features workers::tests::semantic_ -- --nocapture PASS (4)
cargo fmt --all --check PASS
cargo clippy -p mcp-vault-state -p mcp-vault-memory -p mcp-vault-server --all-targets --all-features -- -D warnings PASS
git diff --check PASS
```

Clippy 首轮仅发现本轮 `try_local_rebind` 参数数量超过默认阈值，增加局部
`#[allow(clippy::too_many_arguments)]` 后重跑通过；未扩大接口或改变行为。

`2026-09-16` M3-D2/D3 本地持久状态收口：State 新增 Vault-scoped
`cancel_extraction` 与 `cancel_job`。只有仍处于 `running` 的 M1/M2 语义作业
可进入持久 `cancelled`；已经 `prepared`/`written` 的作业返回 Conflict，保留
原有 snapshot、Core journal 与 recovery barrier，不把取消伪装成
`success_empty`。新增 file-backed SQLite close/reconnect/migrate 回归，覆盖
running→cancelled、跨 Vault 隔离、prepared/written cancellation 拒绝，以及
reconnect 后 pending/recovery 不复活取消结果。

新增 M1 Correction/Suppression file-backed 回归：规则写入后关闭 StateStore，
重新连接并 migrate，再执行旧 proposal；当前读资格仍被抑制，旧结果不会通过
恢复路径复活。新增 M2 organize_json file-backed 回归：重连后 composed card
仍不可读，旧组织结果经 recovery 检查不会恢复。固定 proposal 在某些 target
重算分支可能返回安全的重新组织结果；测试只断言持久抑制后的读资格和 recovery
安全，不把该分支冒充成稳定的写前拒绝证明。

当前 `semantic.extract` 仍是未获 Provider 授权的安全终止 placeholder，不创建
真实 ExtractionSet；organization 也没有后台 worker。本轮没有伪造
worker↔semantic job 绑定，没有把通用 `JobOutcome::Cancelled` 当作语义取消证据。
Provider worker 接线属于后续明确授权工作，保持 pending。

D2/D3 定向验证：

```text
cargo test -p mcp-vault-state --all-features --test semantic_memory_repository --test semantic_organization_repository PASS (11 + 6 tests)
cargo test -p mcp-vault-memory --all-features --test semantic_memory --test semantic_organization PASS (14 + 13 tests)
cargo fmt --all --check PASS
cargo clippy -p mcp-vault-state -p mcp-vault-memory -p mcp-vault-server --all-targets --all-features -- -D warnings PASS
git diff --check PASS
```

本轮未新增迁移，最高版本仍为 `0042_semantic_rule_idempotency_vault_fk.sql`。M3-B8 未运行 workspace 全量测试、Admin/UI、MCP/WebDAV conformance、真实 Provider、网络或生产 Vault；这些保持 pending。此前 State 75 / Memory 64 的全特性结果属于 B5 前状态，不能替代本轮定向 gate。

M1 本地工程闭环和 workspace gate 已通过；这不等于真实语义质量验收。真实 Provider、真实资料、生产 Vault、M6 任务验收和 M7 切换仍为 pending。

早期 `git diff --check` 与新增集成测试曾受本机 Xcode license wrapper（exit 69）影响；后续 workspace fmt、Clippy、测试和 `git diff --check` 已在当前环境重跑通过。

若 M5 改动 Admin UI，再运行 `pnpm --dir frontend/admin lint`、`pnpm --dir frontend/admin test` 和 `pnpm --dir frontend/admin build`。最终依 `AGENTS.md` 执行相关 WebDAV/MCP conformance、迁移、多 Vault、恢复及协议端到端检查。M0 本次仅要求 `git diff --check`，不声称任何代码测试、Provider 测试或语义测试通过。

预期结果：文档补丁无 whitespace 错误；后续阶段的测试应按阶段保留成功、失败、退出码和结果文件。不运行的检查注明原因和 pending 状态。

`2026-09-16` M3 最小收口：新增真实 file-backed SQLite + Vault Core
`RenameCommitted` journal 回归。Core 写入故障产生 prepared/written 语义快照和
未完成 journal 后，`cancel_extraction` 返回 Conflict；source fence 变 stale，
关闭并重新连接 StateStore，再通过 semantic preflight + generic recovery 入口，
journal 仍保持安全 barrier、未发布卡片。该回归未伪造 Core journal、未调用
Provider；M2 cancellation 仍由既有 State/Organization cancellation tests 覆盖，
本轮未扩展新的 M2 Core 故障组合。

M3 最终定向命令与结果：

```text
cargo test -p mcp-vault-memory --test semantic_memory --all-features prepared_core_journal_rejects_cancel_and_survives_reconnect_recovery PASS (1)
cargo test -p mcp-vault-memory --all-features --test semantic_memory --test semantic_organization PASS (23 + 14)
cargo test -p mcp-vault-state --all-features --test semantic_memory_repository --test semantic_organization_repository PASS (12 + 6)
cargo test -p mcp-vault-server --all-features workers::tests::semantic_ -- --nocapture PASS (4)
cargo fmt --all --check PASS
cargo clippy -p mcp-vault-state -p mcp-vault-memory -p mcp-vault-server --all-targets --all-features -- -D warnings PASS
git diff --check PASS
```

未运行 workspace 全量测试、Admin/UI、MCP/WebDAV conformance、真实 Provider、
网络、生产 Vault、M4/M5、M6 真实任务验收与 M7 生产切换；均保持 pending。

`2026-09-17` M4-A 首个内部闭环进行中：新增 `MemoryPackService` 与纯结构化
`MemoryPack` 类型，仅接收 `VaultContext` 和任务/过滤/预算请求，不接受
`vault_id`。State 增加 pack candidate loader，复用 M1 当前卡片与 M2 当前
composed-card 的既有资格谓词，并增加当前 accepted relation loader；Memory 不
复制资格 SQL。组包只输出语义卡核心断言、条件、例外、步骤、未解决项、状态、
证据/来源引用、冲突和安全 gap，不读取普通笔记正文、不调用 Provider、不写
持久状态。当前模式支持确定性 lexical admission/subject 排序、scope/path/kind
过滤、current suppression/source-stale 过滤、M1/M2 结构去重、冲突显式输出、
候选前后资格重验和 JSON bytes/ceil(bytes/4) token/entry 原子预算；history、
未知 version/as_of 在当前持久结构无法证明时明确 gap，不近似匹配。M2 AND/OR
资格仍由 State 既有 loader 负责，未直接改绑 M2。

M4-A 本地回归与命令：

```text
cargo test -p mcp-vault-memory --test memory_pack --all-features PASS (9 integration + 4 pack unit tests)
cargo test -p mcp-vault-memory --all-features PASS (unit 32; integration 9+1+9+11+23+3+14)
cargo test -p mcp-vault-state --all-features --test semantic_memory_repository --test semantic_organization_repository PASS (12 + 6)
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
cargo fmt --all --check PASS
git diff --check PASS
```

首次 M4-A 定向测试仅因测试文件从 `mcp_vault_domain` 错误导入
`VaultStatus` 失败（编译错误，非契约/实现阻塞）；改为从 `mcp_vault_state` 导入
后 7 项 pack 回归通过。M4-A 仍未接 M5 MCP/Admin/UI、向量/Provider、网络、
生产或历史持久模式；A/B/C 仅完成本地工程 comparison runner，真实任务质量和 M6 保持 pending。

M4-A P1 第三轮收口补充：final recheck 后会从保留 entries 重建 related sources
并重新加载当前 relation checks；policy 不可用时清空 entries、sources、relations，
只保留安全 gap/diagnostic，最终再次执行 budget loop。新增 accepted conflict Pack
fixture，输出方向 observation/source check 且不错误 dedupe；supersedes 和
different_scope 仍沿用保守 source-level barrier，独立 Pack fixture 仍 pending。
M4-A P1 收口补充：pack 入口现在要求 Vault Core，M1 使用现有 semantic
`list_cards(context, core, ...)` 的 policy 与 canonical managed FileID/revision/hash
验证；M2 composed cards 通过等价 managed canonical verifier。M2 support member
返回自身 source path/revision/evidence，M2-only path/path-prefix 不借全局 M1
map。非空 subject terms 是确定性范围过滤；path-prefix 采用 VaultPath segment
边界；candidate cap 返回 `candidate_truncated`；conflicts/different_scope/
supersedes 形成 dedup barrier；最终 JSON wrapper、entries、related sources、
conflicts/gaps、diagnostics、optional/core 均经最终 bytes 与 ceil(bytes/4)
budget 闭环。新增 policy-disabled、managed-card corruption、M2-only path、
subject/预算/候选二次资格与 prefix 边界回归；无 Provider/网络/生产。

`2026-09-17` M4-C 收口实现（仅本地只读 projection）：final recheck 现在重新加载
当前 M1/M2 candidates 和 accepted relations，按保留 candidate keys 重建 entries，
再以最新 relation snapshot 执行 relation-aware dedup、冲突 checks、related source
重建与最终 budget loop；失效候选、policy 不可用不会残留旧 source/relation metadata。
relation barrier 包含 relation id、left/right endpoint、source 和 observation，
不再以 source-only key 把同一关系两端折叠。A/B/C runner 增加不可变
`MemoryPackInput`、Vault-bound input hash、真实 IndexService lexical baseline、
comparison runner now requires explicit `comparison_source_paths`; it freezes only
Vault-scoped Markdown FileRecords before A/B/C (file id/path/current revision/content
hash plus semantic source/revision mapping), rejects missing/duplicate/managed/non-Markdown
paths, and never infers the manifest from lexical hits. A 对 scope/kind/version/as_of/history/
subject 等普通索引无法表达的过滤返回明确 `ordinary_*_unavailable` degraded gap，
exact path 使用实际 lexical path 再过滤，B/C 仍走 MemoryPack canonical Core/state
资格路径；三臂统一 serialized envelope、manifest、diagnostics、relations 与
bytes/ceil(bytes/4)/entries budget。该 runner 仍是工程对照，不声称真实语义质量、
覆盖精度或成本验收。

M4-C 定向结果：

```text
cargo test -p mcp-vault-memory --test memory_pack --all-features PASS (15)
cargo test -p mcp-vault-state --all-features --test semantic_memory_repository --test semantic_organization_repository PASS (12 + 6)
cargo clippy -p mcp-vault-memory -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo fmt --all --check PASS
git diff --check PASS
```

新增真实回归覆盖：`build_with_test_hook` 在 initial dedup 后通过真实
`SemanticOrganizationService::organize_json` 创建 accepted conflict，final recheck
重新展开两张 entry 并断言 left/right source+observation 方向；file-backed Core
revision 变化而 Index projection 保持旧 hit 时，A/B/C 三臂均返回
`source_manifest_changed_before_response`、零正常 candidate/returned count。
comparison_source_paths 缺失返回 `comparison_source_manifest_required`；同 FileID
move 后 Index 仍保留旧 path 的 case 也 fail-closed。A/B/C 的 serialized envelope
统一计入 manifest、entries、gaps、diagnostics、relations 及最终 byte/token/entry
budget；B/C manifest fence 遍历每个保留 entry 的完整 source references，不依赖可被
budget 裁剪的 related_sources；A 另报告 raw candidate、eligible、relevant、returned
计数。显式 manifest 只含一个 source、另一个候选在极小 budget 下被裁剪的回归仍
保留 `source_manifest_changed_before_response`，不会以空 related_sources 绕过校验。
comparison 先快照 Vault-scoped source manifest（file id/path/current revision/content
hash 及 semantic source/revision mapping），再执行 A/B/C；普通 Indexer 无法表达的
scope/kind/version/as_of/history/subject 明确 degraded。新增 Pack 级 accepted
supersedes 与 different_scope fixture，均断言 relation direction、source/observation
checks 与不错误 dedupe。final relation snapshot 与 response serialization 之间仍
保留窄 TOCTOU P2 限制；本轮不引入过度全局锁设计。M4-C 不接 Provider、向量、网络、
M5/M6/M7/生产。
首轮 comparison 实现误将 Index page limit 设为 200，触发真实
`index query page is invalid`；已按现有 Index 协议上限改为 100 并保留真实
candidate/returned 统计，未改索引协议或绕过分页边界。

M4 后 workspace gate（2026-09-17）仅作验证记录：

```text
cargo fmt --all --check                              exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings  exit 0
git diff --check                                     exit 0
```

`cargo test --workspace --all-features` 观察到所有已运行的 unit/integration/doc
test suites 均通过且无失败输出（包括 MemoryPack 15、semantic memory 23、semantic
organization 14、State semantic repositories 12 + 6 及其余 workspace suites）；
最后 doc-test 输出后宿主 session 关闭，未返回独立退出码。因此本记录不将该命令
写为 exit 0，也不声称无条件的 workspace 全量通过；M4 定向命令的实际 PASS 结果
仍以上述 M4-C 定向结果为准。

`2026-09-17` M5-A1 本地闭环完成：新增协议无关 `SemanticPublicFacade` 与 schemars DTO，
State 增加 Vault-scoped typed card/composed-card/evidence target resolver；facade 的
pack/card/list/evidence/status 读取要求 `ReadMemory + ReadVault`，规则命令要求
`ManageMemory`、trusted actor、expected rules/parent revision、显式 source binding
和幂等键，不接受请求体 actor，不删除来源。Evidence facade 返回真实 span 坐标和
文本；current source/validated evidence/published parent qualification 在 State
resolver 侧执行。当前未接 MCP/Admin/UI、Provider、迁移或生产。

M5-A1 本轮验证：

```text
cargo check -p mcp-vault-state -p mcp-vault-memory --all-features PASS
cargo test -p mcp-vault-memory --test semantic_public --all-features PASS (3)
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
cargo fmt --all --check PASS
git diff --check PASS
```

历史 parent corruption、multi-parent evidence 的显式 parent fixture、State-level
resolver 专项测试和更完整 composed source-binding 回归仍 pending；不得以当前
facade 3 项测试替代 M5 MCP/Admin/UI 契约测试。

`2026-09-17` M5-A1 独立 file-backed 覆盖收口：在
`crates/memory/tests/semantic_public.rs` 新增真实文件 SQLite close/reconnect/migrate
fixture，覆盖：(1) M1 card revision 和 M2 composed revision 变为历史后，EvidenceRef
不再解析为 current target，facade 不应用规则；(2) 一个 current Evidence 同时挂在
current M1 card 与 current composed card 时，无 parent 返回 Conflict，显式正确
card/composed parent 成功，历史/跨 Vault parent 拒绝；(3) current composed card 的
两个 support source 只允许真实 support source 的 source-scoped rule，错误 source
拒绝；(4) expected parent/rules revision、trusted actor、幂等键和重连后的 rules
revision 持久性。未修改 MCP/Admin/UI、Provider、迁移或生产文件。

本轮发现并做了一个最小 resolver 修复：`resolve_evidence_target` 原先在显式 parent
筛选后丢弃 card/composed-card ID，`SemanticTargetResolution.parent_revision` 因此为
0，合法 expected-parent fence 被错误拒绝。现在保留真实 parent kind/ID，统一由现有
`finish_resolution` 读取 current parent revision；Vault/current/published/source
资格查询未放宽。初次测试搭建还真实触发了同一 Observation 复用被组织服务拒绝以及
未被 card 完整承载的 observation，均改为合法 fixture，不改变生产语义。

M5-A1 最终命令与结果：

```text
cargo test -p mcp-vault-memory --all-features --test semantic_public \
  file_backed_composed_target_keeps_source_scoped_rules_to_real_supports -- --nocapture  PASS (1)
cargo test -p mcp-vault-memory --all-features --test semantic_public \
  file_backed_evidence_from_historical_composed_revision_is_not_resolvable -- --nocapture PASS (1)
cargo test -p mcp-vault-memory --all-features --test semantic_public -- --nocapture     PASS (7)
cargo test -p mcp-vault-state --all-features --test semantic_rules \
  --test semantic_memory_repository --test semantic_organization_repository             PASS (3 + 12 + 6)
cargo test -p mcp-vault-memory --all-features --test semantic_memory \
  --test semantic_organization                                                          PASS (23 + 14)
cargo fmt --all --check                                                                  PASS
cargo clippy -p mcp-vault-state -p mcp-vault-memory --all-targets --all-features -- -D warnings PASS
git diff --check                                                                          PASS
```

M5-A1 仍未接公开 MCP/Admin/UI 工具、协议 schema/HTTP conformance、Provider、网络、
生产或真实语义质量验收；这些继续 pending。

`2026-09-17` M5-A2 HTTP contract 收口：7 个版本化 semantic MCP tools 均通过
configured router 的真实 `tools/list`/`tools/call`；新增覆盖 full、ReadMemory-only、
ReadMemory+VaultRead、ManageMemory-only 四种 principal scope，并断言 advertised
tools 与实际 call permission 一致。file-backed/temporary Vault fixture 覆盖真实
principal actor 派生、Correction/Forget 的 expected parent/rules revision conflict、
同 idempotency retry、普通 source note 保留、ReadOnly maintenance rejection、
limit=0/201、unknown/include_details schema/runtime rejection、pack no-answer，以及
同一个 StateStore/SQLite 中注册的 Alpha+Bravo 两个 Vault：各自真实发布 semantic
card/evidence 后，双向 foreign ID 调用均为统一 `not_found`。Evidence span
coordinates 由既有 HTTP round-trip 覆盖；evidence parent ambiguity 与 composed
stale AND/OR target mutation 由已通过的 Memory facade/State semantic fixtures
覆盖（新增 OR 全部 support stale 与 AND 任一 support stale 两个 facade 回归）。早先
独立 StateStore 的 foreign-ID fixture 不作为隔离证据，已替换为同库双 Vault fixture。

两个 shared limit DTO 的 tools/list schema 现在显式声明 `minimum=1`、`maximum=200`，
并由真实 HTTP call 对 0/201 做同样拒绝；composed mutation resolver 在 StateStore
入口复用当前 published/source/AND-OR support qualification，全部 OR support
失效后不会继续解析 target 或应用 rule。

首轮 no-answer 测试使用的随机词经 State lexical normalization 后为空，真实返回了
unfiltered current context；测试改用有效 lexical terms `violet submarine` 后得到
`no_answer`，未修改 pack runtime。unknown field 的真实协议结果是 RMCP parameter
deserialization error（不是 ToolEnvelope），测试按实际 wire contract 断言。

```text
cargo test -p mcp-vault-mcp --all-features PASS (33 + 0 doc-tests)
cargo test -p mcp-vault-mcp --all-features semantic_v1_http_ -- --nocapture PASS (5)
cargo test -p mcp-vault-mcp --all-features semantic_v1_tools_round_trip_through_real_http_harness PASS (1)
cargo test -p mcp-vault-memory --all-features --test semantic_public --test semantic_memory --test semantic_organization PASS (9 + 23 + 14)
cargo test -p mcp-vault-state --all-features --test semantic_rules --test semantic_memory_repository --test semantic_organization_repository PASS (3 + 12 + 6)
cargo fmt --all --check PASS
cargo clippy -p mcp-vault-mcp -p mcp-vault-memory -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
git diff --check PASS
```

changed MCP/Memory/State fmt、clippy、diff 最终 gate 已在本轮补跑并记录如下。Admin/UI、
Provider、网络生产和公开真实客户端验收保持 pending。

`2026-09-17` M5-B1 本地闭环完成：新增独立 `crates/admin-api/src/semantic.rs`，仅在
`admin-api/src/lib.rs` 增加模块声明、semantic route merge，以及 AccessDenied 的稳定
memory error 映射；未重写或覆盖已有 Admin/bulk-delete dirty hunk。新增
`/semantic/status`、`/semantic/cards`、`/semantic/card`、`/semantic/evidence`、
`/semantic/pack`、`/semantic/correct`、`/semantic/forget`，全部经
`current_vault`、per-Vault Core 和 `SemanticPublicFacade`，body 不接收 `vault_id` 或
actor；mutation 经既有 Admin session/CSRF/origin/maintenance middleware，并追加不含
rule payload/evidence 正文的 admin audit metadata。Admin principal actor 由服务端转换
为 trusted `SemanticActor`，forget/correction 不删除 source。

真实 Admin HTTP fixture 当前覆盖：selected route slug、cards/status/pack、同一
StateStore/SQLite 双 Vault 的 foreign card `404`、unknown-field、CSRF-negative、
correction/forget mutation、source 文件仍存在、read-only maintenance 拒绝；evidence
span、audit-body redaction 的更完整 Admin contract 仍 pending。首次 fixture 发现 proposal 缺少完整 observation 字段而被
真实 `semantic_proposal_schema` 拒绝，随后仅补齐 fixture 所需条件/例外/步骤字段，未
放宽生产校验。

`2026-09-17` M5-B1 HTTP contract 收口：双 Vault fixture 现在在同一个临时
StateStore/SQLite 中注册 `default` 与 `work`，分别发布真实 M1 card/evidence；新增
Admin HTTP 回归覆盖 current card/composed-card read、evidence 无 parent ambiguity、
显式 card/composed parent span 坐标、双向 foreign evidence ID 404、unscoped legacy
`/semantic/*` 只落到 persisted legacy default、scoped slug mismatch 404、correction/
forget 幂等 replay、stale parent/rules revision Conflict、普通 source 文件保留、
Admin principal actor、audit actor/target/rules metadata 及 payload/evidence-body
不泄漏、status/cards limit 0/201、unknown query rejection 与 pack no-answer。

发现并最小修复 Admin handler 缺口：`/semantic/cards` 原先将 limit 0/201 直接传入
State，错误映射成 500；现在在 adapter 边界按 1..=200 返回 `422 validation_failed`。
未放宽 State/facade 资格、未改变 mutation/audit 语义。

M5-B1 最终命令与结果：

```text
cargo test -p mcp-vault-admin-api --all-features PASS (30 unit + 1 integration + 0 doc-tests)
cargo test -p mcp-vault-admin-api --all-features semantic_admin_http -- --nocapture PASS (2)
cargo clippy -p mcp-vault-admin-api --all-targets --all-features -- -D warnings PASS
cargo fmt --all --check PASS
git diff --check PASS
```

M5-B1 本轮未修改 frontend、MCP、Provider、网络或生产；Admin semantic API 仍是
control-plane adapter，真实浏览器 UI、公开客户端和生产验收继续 pending。

`2026-09-17` M5-B2 Admin UI 本地闭环：新增
`frontend/admin/src/semantic-memory.tsx`、`semantic-memory.css`、
`semantic-memory.test.tsx`，并在干净的 `view-model.ts`/`App.tsx`/`api.ts` 中加入
“语义记忆”导航、selected-vault loader 与 `/semantic` scope；`pages.tsx` 只增加
semantic page import/switch，未编辑既有 MemoryPage bulk-delete 区、`App.test.tsx`
或 `app.css` dirty hunk。页面展示 M1/M2 kind/scope/status/revision、核心断言及可用
限定字段、pack no-answer/gaps/diagnostics/budget、真实 evidence body/context span
坐标；Correct/Forget 只发送 target、当前 parent/rules revision、幂等键和 typed
mutation，不发送 actor/vault_id；错误、maintenance、conflict 通过既有 API client
提示，未展示 semantic explicit remember。

首次无 `CI=true` 的 pnpm build 触发 Corepack registry/network 安装失败；随后使用现有
依赖以 `CI=true` 重跑，未改 lockfile 或依赖目录。最终前端结果：lint、45 tests（含
新增 semantic 2 tests 与既有 43 tests）、build 全部通过。

`2026-09-17` M5 shared DTO card-kind 收口：`SemanticCardRequest.card_kind` 现在是
共享 `SemanticCardKind` enum，仅允许 `card`/`composed_card`，MCP 与 Admin 均按该 enum
匹配；MCP tools/list 暴露同源 enum schema，MCP bogus kind 返回
`structuredContent.error.code=invalid_argument`，Admin bogus query 返回
`422 validation_failed`。M1 默认 card 与 M2 composed-card 读取保持通过。RMCP 入口使用
仅用于保留 shared schema 的窄验证 shim，将反序列化失败转为稳定 semantic error，不保留
任意字符串 fallback。

`2026-09-17` M5 final local gate（严格按顺序执行）：

```text
cargo fmt --all --check                                                        exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings          exit 0
cargo test --workspace --all-features                                           中断于 tests/memory_pack.rs；此前已观察到的 suites 无失败输出，但宿主未返回 exit code，不能记为通过
CI=true pnpm --dir frontend/admin lint                                         exit 0
CI=true pnpm --dir frontend/admin test                                         exit 0；4 files / 45 tests passed，新增 semantic UI 测试有 React act 环境 warning
CI=true pnpm --dir frontend/admin build                                        exit 0
git diff --check                                                               exit 0
```

因此 M5 的本地确定性 feature 结论依据为已完成的定向 protocol/MCP、Admin HTTP/API、
shared schema、权限/跨 Vault、以及 Admin UI lint/test/build gates；workspace Cargo
test 本轮没有无条件全量通过证据；在该历史 gate 时点 `semantic_remember_explicit`
仍 pending，旧 v3 `remember` 未修改。后续 M5 explicit-v1 适配回归见下文。M6 真实 Provider/任务语义与成本验收、M7 影子构建/生产切换/回滚
和旧数据清理、Provider/网络/生产操作均继续 pending。

`2026-09-17` M6-A 本地 harness 实现完成（不等于真实评估准备或质量通过）：新增独立 `crates/eval`，只依赖 domain 与
serde/sha2，未接 Provider、网络、普通 notes、v3、Admin、MCP 或生产。`EvaluationManifest`
严格校验 synthetic-only source/file/path/revision/hash、authorization/profile/rules/
generation/extraction commit 与 task split/query；提供现有 M0
`semantic-memory-manifest-v1` 的 checked converter，保留 12 sources/24 tasks、logical
IDs、gold usable/status/relations/severity/ref 字段；`EvaluationRunConfig` 固定 A/B/C
source/task/budget/index/prompt/schema/answer-model/retrieval-model identifiers，并校验三臂输入一致。所有
DTO deny unknown fields，路径/hash/secret-bearing keys fail-closed。

`SourceSnapshotVerifier` adapter 对实际授权 snapshot 的 logical/file/path/revision/source
revision/content hash、authorization revision、profile、rules revision、source generation
和 extraction commit sequence 做 fail-closed 校验；`ArmRecord` 只有三个完整 fingerprint 全部匹配
manifest/config 时 comparison 才一致，缺臂或空 records 为 false/NotRun。`run_mock_or_replay`
只生成独立 redacted artifact：manifest/run-config hashes、工程/
语义/任务/成本四层状态、A/B/C consistency、manual-review pending、空 usage/cost 与
安全错误码；mock/replay 不能声称语义或任务通过。Live 在任何外部请求前因
`allow_live`、明确授权、正成本预算、专用 artifact root 或 source allowlist 缺失而拒绝；
allowlist 必须精确覆盖每个 manifest logical/file ID，且 synthetic placeholder 不能 live-ready；local replay Provider error 在首个错误处停止，
不把后续 fake success 冒充质量通过。artifact 只允许写绝对 `.json` 临时路径，拒绝
`vault`/`production` 路径。

M6-A 实际验证：`cargo test -p mcp-vault-eval --all-features` 7 tests + doc-tests 通过；
覆盖 M0 fixture conversion/gold refs、authoritative source hash success/mismatch、unknown
field、manifest path/hash drift、live refusal/allowlist mismatch、A/B/C budget/index/prompt/
schema/answer-model/retrieval-model mismatch/complete arm fingerprints、每个 source fence
mismatch、artifact path/redaction 和 first-error stop。`cargo clippy -p mcp-vault-eval --all-targets
--all-features -- -D warnings`、`cargo fmt --all --check`、`git diff --check` 均通过。
真实 Provider、真实语义/任务质量、用量与成本仍 pending，不能以本地 harness 代替。

`2026-09-18` M6 live runner 工程切片完成（仍不运行 Provider）：`crates/eval` 新增
`run_live_evaluation` 及 `ProviderAppBoundary`/`SemanticMemoryAppBoundary`。Runner 在首个外部
请求前 fail-closed 校验明确 live 授权、非 synthetic manifest、精确 logical/file source allowlist、
独立且不重叠的 source/state/history/artifact roots、独立 Vault identity/current-schema 标识、
固定 A/B/C 的 provider/model/prompt/schema/index 配置，以及正数 request/task/cost/budget。来源
fence 先由 `SourceSnapshotVerifier` 完整校验；Provider 仅经应用边界调用，语义提案仅经
`SemanticMemoryService` 适配边界提交。每次成功阶段写入脱敏的 observations/relations/cards/packs/
answers/review/usage/report，加上 manifest/run-config 与 checkpoint；Provider 首错持久化红acted
checkpoint 和完整 artifact 集后停止，不重试、不把 fake/replay 当质量通过。report 为 A/B/C 分臂
分别记录重点覆盖、支持精度、条件保留、重复、无答案、任务结果的 `pending_manual_review`，并
分别统计 usage/cost；不会自动宣称语义质量通过。新增 `crates/eval/tests/live_runner.rs` 的
local fake-provider 契约测试覆盖预检拒绝、隔离路径、artifact 完整性/脱敏、A/B/C usage 统计及首错停。
本轮未调用 Provider、网络、真实 Vault、`./data` 或普通笔记。根据 `2026-09-18` 产品决策，
M6/M7 的正常读取目标按 semantic-only 评测设计，不依赖旧 automatic recall fallback 或双读指针；
raw explicit 工具/存储和旧数据保留，旧 v3 代码本轮不删除。

`2026-09-18` M6 CLI/production-boundary 接线完成（仅本地验证，未执行 live）：新增
`crates/eval/src/bin/semantic-card-live-eval.rs`，只有精确的
`--run-authorized-real-semantic-evaluation CONFIG_JSON` 旗标才会打开指定的 current-schema
isolated run root/state DB；无旗标在读取 DB、master key 或构造 Provider 前拒绝。`ProviderServiceAppBoundary`
把冻结 stage template 映射到现有 `ProviderService::generate_structured`，凭据继续由既有
加密 secret store 解析；`SemanticMemoryServiceAppBoundary` 只委托
`SemanticMemoryService::prepare_source`/`submit_proposal_json`，canonical 文件仍由
`VaultCore` 负责。`VaultCoreSourceVerifier::load` 在首个 Provider 请求前从当前 Core/State
加载并校验 File ID、路径、修订、hash、授权修订、generation、extraction commit、规则修订和
semantic profile。CLI 要求显式 master-key path、隔离 Vault slug/model/template 配置，拒绝
context/root/identity 不匹配；不读取 `./data` secrets，不提供旧 v3 fallback。新增 CLI 默认拒绝、
fake runner、隔离 artifact 和 boundary contract 本地测试。真实运行仍需用户另行提供：独立
current-schema DB/Vault、匹配 manifest source fence、Provider/model binding、加密 master-key
reference、冻结 stage prompt/schema/template、allowlist 与预算；本轮这些输入均未使用，真实
Provider/网络/费用仍 pending。

`2026-09-18` 独立审查 P1 修复（仍仅本地）：LiveEvaluationConfig 现在把 `run_root`、
`unbounded_cost_authorized` 纳入冻结配置，source root、每个 manifest source file、state/history/
artifact roots 必须位于受控 run root 且不含 `data`/`vault`/`production`；source escape 有回归测试。
请求预算使用原子 reservation，Provider 返回的已知 cost 累计超过 cap 时写 checkpoint 并停止；未知
cost 不能绕过 request budget，unbounded currency 必须显式记录授权。artifact redaction 对键名做
大小写、分隔符和常见变体归一化，覆盖 access/refresh token、apiKey、source_text、note_body、
request_headers 等，并对不可信 Provider output 递归清洗。CLI 在调用前核对 internal ModelId、
external model、template 与 A/B/C run-config 指纹。SemanticMemoryAppBoundary 增加真实
`SemanticPublicFacade::build_pack` 应用服务路径；relation/answer 只消费真实 pack，packs artifact
保存 pack 摘要、source references、qualifiers/conflicts/gaps。artifact 合同补齐 `review.jsonl` 和
`report.md`；报告按 A/B/C 和 task 输出分别保留重点覆盖、支持精度、条件保留、重复、无答案、
任务结果的待人工复核状态及 usage/cost，不自动判定质量通过。M6 synthetic validator 以明确
development/holdout/no-answer/relation 计数校验，而非仅 any。新增 source-root、model mismatch、
cost stop、redaction variant 和 CLI default refusal 本地回归；仍未调用 Provider/网络/production。

`2026-09-17` M7-A1 本地 shadow dry-run 基础完成：在 `crates/eval` 增加 typed
`ShadowBuildConfig`/`ShadowBuildPlan`、独立 source/shadow/state/history/artifact root
校验、精确 manifest logical/file allowlist、synthetic temporary source-only copy、
逐文件 hash/unchanged 验证，以及 outbox cursor/rules revision drift 的
`needs_replay`/`needs_rebuild` flags。shadow Vault identity/state paths 独立规划，
不复用 source DB/jobs/rules/cards，不启动 Provider、不发布 source、不切换 pointer。
新增 overlap/path traversal、manifest drift、source unchanged、allowlist、cursor/rules
drift 回归；runbook 为
`docs/runbooks/semantic-shadow-and-rollback.md`，明确生产授权、paired backup、测试
client、rollback pointer 和 no-cleanup 边界。

随后修复 symlink P1：source root、manifest source file、shadow/state/history/artifact
父目录逐组件使用 `symlink_metadata` 检查，source 必须 regular file；现有路径 canonicalize
后再做真实 non-overlap，输出目录创建后再次校验。macOS `/var`/`/tmp` 系统别名仅作为
平台系统路径例外，用户/fixture symlink 仍 fail-closed。新增 source-root/file、output
parent、真实 canonical overlap 回归；source unchanged 断言也重新检查 regular/non-symlink。

补充 nested destination-parent P1：每次 `create_dir_all(destination.parent())` 前后都逐
组件校验已有目录，拒绝 `shadow_root/notes -> outside` 等 symlink，不跟随写入；新增回归
确认 outside 目录不会出现 copied file。该检查同样不改变真实 shadow/Provider/生产边界。

M7-A1 仍只是本地 synthetic dry-run preparation；真实 shadow、Provider/network、测试
client cutover、production pointer、rollback rehearsal 和 cleanup 均 pending。

`2026-09-17` M7-A2 本地隔离执行 fixture 通过：`crates/eval/tests/shadow_execution.rs`
先在 source Vault 通过 `VaultCore` 创建真实 source revision、读取实际
authorization/profile/rules/generation/commit fence，并以该记录生成 synthetic-only
manifest；`prepare_shadow_dry_run` 将 allowlist 文件复制到独立 shadow root 后，shadow
使用显式 `shadow_state`/`shadow_history`、独立 `VaultContext` 和真实 `VaultCore`
reconcile。确定性结构化 proposal 经 `SemanticMemoryService::submit_proposal_json`
发布，`list_cards` 与 managed canonical file read 均通过 Core-backed verification。
测试断言 source/shadow Vault、FileRecord、managed card 路径和 history root 不交叉；
shadow card 在 `SemanticPublicFacade` 的 `SuppressRead` 后仍保留文件但不可读，关闭并以
同一 shadow VaultId 重开后规则仍生效。source 普通文件、source semantic card/file
revision、source history count 在 shadow build/reopen 前后保持不变；manifest drift
安全失败。该测试只证明本地结构/隔离/规则连续性，不证明真实语义质量、Provider、网络、
生产 shadow 或切换；这些及 M7 rollback/cleanup 仍 pending。

`2026-09-17` M7-A3 本地 test-client simulation 通过：同一集成 fixture 增加仅测试用
`ShadowReadClient`，默认 target 固定为 source，显式切换后才读取 shadow；读取调用
`SemanticPublicFacade::list_cards`，并使用 `ReadMemory + ReadVault` access，因此仍走
Core-backed 当前资格和 Vault 边界。此前审查发现仅比较调用者手传 fence 不足，现改为每次
读取/切换从临时 fixture 实际验证 source FileRecord 的 FileID/path/revision/content hash、
语义 source revision、manifest hash、shadow 当前 rules revision，以及明确命名的
source-file aggregate outbox checkpoint（不是全局生产 outbox cursor）。真实 source 文件
修改会同时改变 revision/hash/checkpoint 并使旧 client fail-closed；真实 shadow
SuppressRead 改变 rules revision，旧 client 拒绝，仍处于 source target 的旧 client
直接切换也会被拒绝且 target 不变；刷新到已验证 revision 后返回空结果。
同一 shadow VaultId 重启后仍保持该行为，source target 不受 shadow rule 影响。该 client
只是本地模拟，不是生产全局 pointer；真实 client、shadow/cutover、rollback、
Provider/network 与生产仍 pending。

`2026-09-17` M5 explicit-v1 follow-up 本地回归通过：共享
`SemanticRememberExplicitRequest`/DTO 与 `SemanticExplicitFacade` 复用既有 v3
explicit ownership/canonical Markdown 写链路，结果明确返回 raw explicit identity，
要求非空幂等键；带 sources 时每条必须提供稳定 File ID/revision，path 只作导航，
来源绑定不冒充 Evidence，不进入 SemanticCard/Evidence/MemoryPack。MCP 新工具
`semantic_remember_explicit_v1` 要求 `WriteMemory`/writable，Admin 新增
`/vaults/{slug}/semantic/remember-explicit`，要求 session、Origin/CSRF 和维护写入门；
两者均从认证 principal 派生 actor，不接受 `vault_id`/actor。`cargo test -p
mcp-vault-memory --test semantic_explicit --all-features` 通过 2 tests：无
`embedding_memory` binding 时 exact body、幂等冲突、source revision conflict、删除重建
path 的旧 File ID/revision fail-closed、重启 canonical read、cross-Vault、
card/evidence/pack 排除及无 embedding job；配置临时 binding 时沿既有 embedding job
路径排队，未调用网络。Admin semantic HTTP 通过 explicit ownership/no binding、
missing/empty key、CSRF/Origin/maintenance 和 audit redaction 回归；MCP semantic
HTTP tools/list/call 通过新工具、scope filtering、WriteMemory-only/no-source、
ReadVault source guard、missing/empty key、maintenance 和 schema/runtime 路径。专用
Admin UI 已在现有独立语义页增加“用户授权原文记忆”面板，只发送原文和客户端生成的
非空幂等键，显示 explicit ownership/ID/indexing eligibility/binding state，不进入
cards/pack；explicit list/edit/source binding 继续 pending，旧 v3 `remember` 未修改。
前端新增失败重试复用同一 payload/key、pending 防重复点击两项回归；首次测试错误地把
成功清空内容后的按钮断言为 enabled，已改为断言请求完成/结果出现。前端全量为 48 tests
通过，保留既有 React `act(...)` 环境 warning。
首次 MCP source-guard 回归曾把 semantic source revision ID 字符串误放入 wire 的
file revision `u64` 字段，真实 HTTP 反序列化按契约拒绝；修正 fixture 使用当前 FileRecord
revision 后通过，未放宽 schema 或 source 权限边界。
随后 MCP tools/list 真实断言 explicit 输入 required 为 `content`+
`idempotency_key`，嵌套 source required 为 `path`+`file_id`+`revision` 且两层均
`additionalProperties:false`；full `MemoryWrite+VaultRead` source-bearing 调用使用
真实 FileRecord ID/revision 成功，WriteMemory-only source-bearing 调用在读取 source
前拒绝。Admin 审计断言包含 `admin.semantic.explicit_remembered` 与 principal actor，
不含正文或 source path。

`2026-09-18` MCP public-surface clean break按用户明确授权收口：MCP 注册只发布
`build_memory_pack`、`get_memory_card`、`list_memory_cards`、
`get_memory_evidence`、`correct_memory`、语义 `forget_memory`、
`get_processing_status`，以及 `remember`、`get_raw_memory`、
`list_raw_memories`、`get_raw_memory_overview`、`update_raw_memory`、
`forget_raw_memory`。旧 recall/get/list/overview/update/forget 注册和旧
`vault://memory/*` 资源不再可用；raw 工具改用 protocol-neutral
`SemanticExplicitFacade` 的 explicit-only DTO seam，语义工具不返回完整 raw body。
新增 `vault://memory-card/{card_id}` 资源模板，task pack 保持工具-only。Admin
`/semantic/remember-explicit`、v3 数据/存储/迁移保持不变。本次未进行 Provider、网络、
生产或数据清理；MCP 定向 public JSON-RPC/tool-list/resource 验收及全 workspace gate
由最终集成代理复核，不能提前宣称整体通过。

`2026-09-18` P2 public-surface follow-up：新增真实 MCP HTTP 回归覆盖 full
`vault:read` 与 memory-only caller 的 raw overview 过滤、source/path/topic
侧信道、来源脱敏和 `vault://raw-memory/*` URI；补充 raw `remember` 顶层
ownership/representation、overview schema 范围和 forget idempotency 描述校验。
MCP crate 35 tests、Clippy、fmt 与 diff check 均通过；未调用 Provider、网络或生产。

## Rollback and recovery

本轮 M0 文档可通过后续普通版本控制审查回退；不改历史迁移、普通 Vault 文件、v3 数据或 Provider 配置。新迁移仅向前新增，正式使用前需配对备份数据库和 Vault 内容。若 M1—M6 验收失败，旧读取路径保持默认；来源和显式记忆仍按现有授权路径可读。

M7 切换前记录新写入、事件追赶、source/hash、权限、Correction/Suppression 和影子修订。优先切换读取指针，不以旧备份覆盖新增内容。回退读取器必须继续应用最新权限和禁止规则；否则停用卡片读取，回到获授权来源检索。生产切换、数据清理和恢复操作必须另行获得明确授权。

## Outcomes

M0、M1、M2、M3、M4、M5 本地确定性工程闭环已通过；M3 包含 C3b recovery
preflight、D1 deterministic full-ExtractionSet rebind、profile/source/history
fences、真实 Core journal cancellation rejection 和持久规则/取消回归。D1 的
partial、source-stated time scope、历史 scope/profile corruption 等不确定场景均按
fail-closed 处理。M5 已完成 shared facade/DTO、版本化 MCP、Admin API、Admin UI 的
本地 schema/runtime、权限、跨 Vault、mutation、maintenance、evidence span 和
frontend lint/test/build 验证；MCP `remember` 现在复用 v3 explicit ownership/canonical
Markdown 链路，MCP/Admin 及 embedding binding/no-binding 本地回归通过，Admin UI 独立原文
面板已接入，explicit list/edit/source binding 仍 pending。Provider/M6 真实任务语义/成本验收、M7 隔离影子构建与生产切换、
生产 Vault 和旧数据清理仍 pending；本地工程结果不替代真实语义验收。

`2026-09-18` workspace gate 发现 MCP metadata 回归：新增
`semantic_remember_explicit_v1` 后实际工具数为 26，而模型-facing metadata 测试仍断言
25；该工具描述也缺少 `On success, \`data...` 结果字段指导，并暴露了不应出现在模型契约中的
Provider/实现细节。修复为 26，补齐精确成功字段、输入前置条件和后续动作，移除实现术语；同时将
实际已注册但接口清单遗漏的 `get_memory_overview` 列为第 26 项。未修改 legacy/v3、Provider、
生产或外部数据。

`2026-09-18` MCP surface break 按用户授权收口：删除重复的
`semantic_remember_explicit_v1` 工具及其 permission/order/tools-list 测试引用，不保留旧
`remember` 的宽松请求/响应兼容层；唯一 MCP `remember` 现在直接采用
`SemanticRememberExplicitRequest` 与 `SemanticExplicitFacade`，要求非空幂等键，来源必须含
`path`/`file_id`/`revision` 且先通过 `ReadVault`，始终要求 `WriteMemory`/writable，actor 与
Vault 从认证上下文派生，返回 `data.explicit`，不进入 card/evidence/pack。Admin 的
`remember-explicit` 路由、v3 存储和数据保持不变。MCP crate 33 tests、targeted metadata、
6 个 semantic_v1 HTTP tests、fmt/clippy/diff 均通过；先前 workspace test 因设计 pivot 被中断，
本次不宣称 workspace test 通过。

`2026-09-18` post-clean-break final gate：`cargo test --workspace --all-features` 自然退出
exit 0；随后 `cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`cargo fmt --all --check` 和 `git diff --check` 均自然退出 exit 0。Frontend explicit UI 的既有
事实为 CI lint exit 0、48 tests 通过、build exit 0。本轮未调用 Provider、网络或生产环境；M6
真实 Provider/质量/成本验收，以及 M7 真实 shadow/client/生产切换与 cleanup 仍 pending。

`2026-09-18` M6 synthetic corpus slice：在既有 `semantic-memory-manifest-v1` fixture
contract 上新增独立 `crates/eval/tests/fixtures/semantic-memory-m6/`，冻结 30 个
合成 Markdown 来源和 60 个任务。来源具有固定 logical/file ID、File revision、
SourceRevision、授权/profile/rules/generation/commit fence 与 SHA-256；每个任务保存
source fence、查询引用、expected usable support、必须保留的范围/条件、禁止推断、关系、
状态、严重级别和显式 `expected_no_answer`。P01—P06 为 development（36 tasks），
P07—P10 为 holdout（24 tasks），source-disjoint 且覆盖范围、否定、例外、顺序、状态、
困难负例、无答案、重复和受控跨来源整理。新增 `convert_m6_synthetic_fixture_manifest`/
`validate_m6_synthetic_manifest` 复用既有 generic manifest verifier，并以固定最小数量、
task fence、source hash、split leakage 回归阻止数据漂移；`verify_synthetic_fixture_files`
只校验仓库内合成文件，不访问 `./data`、普通笔记、网络或 Provider。M6 runner 的真实
报告仍按 A/B/C 分别记录重点覆盖、支持精度、条件保留、重复、无答案、任务结果和 usage，
但 synthetic/mock/replay 不能被当作真实 Provider 成功或语义质量通过。

本轮验证：`cargo test -p mcp-vault-eval --all-features`、`cargo clippy -p
mcp-vault-eval --all-targets --all-features -- -D warnings` 通过；所有结果均为本地
合成 fixture 工程验证，M6 真实 Provider/任务质量验收仍 pending。

`2026-09-20` M6 live-runner P1/P2 核心修复（仅本地 fake/隔离测试）：删除未使用的
`retrieval_model_id` 配置字段；A arm 通过 `SemanticMemoryAppBoundary::ordinary_retrieve`
走 Vault Core 普通 note retrieval，B 只允许每个任务一个 source card，C 仅在受控选定
source 集上构建跨来源 pack/relation，runner 不再按 manifest 全集提供输入。Live task
必须有完整非空 source fence，VaultCore verifier 在 prepare/provider/submit/card/pack/
answer 关键边界重新读取 Core/State；漂移记录 fail-closed checkpoint。卡片与 pack 改为
允许字段投影并保留 assertions/qualifiers/evidence refs/hash，不保留 note 正文；pack
在 relation/answer 前通过原子 JSONL 持久化，后续结果引用 pack hash。run config 之外新增
完整 live-config/hash，checkpoint 与 JSONL 写入使用临时文件原子 rename；显式 unbounded
且 Provider 未返回费用时 usage 报告为 `unknown`，请求预算仍为硬上限，计数器使用有界
累加。M6 fixture 增加 q_refs/relation source-set、symlink-safe 文件校验。

本轮验证：`cargo fmt --all --check`、`cargo clippy -p mcp-vault-eval --all-targets
--all-features -- -D warnings`、`cargo test -p mcp-vault-eval --all-features` 通过；未调用
Provider、网络、生产 Vault、真实笔记或 secret。真实 Provider/任务语义验收仍 pending。

`2026-09-20` M6 live-runner latest-review contract repair（本地代码与 fixture
测试，未调用 Provider/网络/生产 Vault）：A arm 改为通过 IndexService 的冻结
`index-frozen-v1` lexical application boundary，返回有界 indexed snippet/metadata，
不再经 Vault Core 读取整篇正文；M1 card 读取新增按 source/revision 的无全局 page
上限查询，避免固定 200 条漏掉已发布目标。Live preflight 现在要求 observation、
relation、answer 三个 provider template 恰好各一份，并把 internal model id、实际
template、roots、allowlist、预算与 task 配置纳入 live-config/hash artifact；master
key 的现有路径组件拒绝 symlink 且 canonical parent 必须位于 run_root。A/B/C 要求
同一精确 task set，task/q_refs/source/relation IDs 去重，C 必须覆盖关系所需来源，
fixture validator 同步加强这些 refs/ground truth fence 检查。Checkpoint 初始及阶段
增量状态为 Running，成功阶段原子更新 observations/cards/packs/answers/usage JSONL
及 checkpoint，失败边界写入 Failed；只有最终成功才写 Completed。finite currency cap
继续在首个请求前拒绝，unbounded 下未知 Provider 成本保留 `unknown`。本轮验证：
`cargo fmt --all --check`、`cargo clippy -p mcp-vault-eval --all-targets
--all-features -- -D warnings`、`cargo test -p mcp-vault-eval --all-features` 通过；
完整 workspace gate 和真实 Provider/任务质量验收仍 pending。

`2026-09-20` M6 material-contract repair（本地 fake/dual-Vault 边界）：
`MemoryPackRequest` 新增可选 `source_scope`，由 memory service 在候选卡片和
relation projection 内同时校验 source ID、canonical path、source revision；未授权
M1/M2 卡片被排除，普通 MCP pack 在未提供 scope 时保持原行为。评测 adapter 的 B/C
pack 请求现在携带完整三维 scope，runner 通过 arm-aware boundary 传递隔离语义，A
普通检索后、首个 Provider 请求前再次重验任务全部 source fence。C relation 与 answer
复用一次已持久化 pack/hash；B 只在 answer 阶段构建一次 pack，避免重复重建。live
manifest 强制至少存在 holdout，选定 task set 必须全为 holdout，query/relation refs
必须留在 task source 边界，重复 expected relation 记录拒绝，A 覆盖每个 task 的完整
required source 集；模板写入 live-config 时按 observation/relation/answer 固定顺序
安全投影，不落 system/schema 原文。新增 memory 层 source-scope 单元回归；eval
现有 live_runner/m6/shadow 测试通过。未调用 Provider、网络、生产 Vault、真实笔记或
secret；memory_v3 的 9 个失败仍是沙箱禁止测试 fixture TCP listener 的既有环境限制。

`2026-09-20` M6 arm-isolation review P0/P1 follow-up：审查发现此前 runner 的 JSON
pack projection 使用了与 `MemoryPack` 不匹配的字段名，结果虽有 hash 却会把实际 pack
投影成空对象；现在先反序列化为生产 `MemoryPack` typed model，强制检查七个顶层字段，
保留 current/relevant/conflict/gap/source sections 及断言、qualifier、evidence refs，
并由规范化 JSON 内容生成 hash；无法解析的 pack fail-closed，raw note body 不在模型中，
不会被 typed projection 序列化。B/C pack adapter 从各自隔离 State 按 canonical path
找到本地 FileRecord，再读取实际 `SemanticSourceId`/`SourceRevisionId` 组成 source scope；
提交后刷新同一映射，card projection 与 pack 均按该 arm 的本地 SourceRevision 查询。
`SemanticArmRoots` 现在是 live config 的必填部分，B/C 分别绑定不同 Vault ID/slug、
source root、SQLite root 和 history root；live preflight 检查它们与 A/彼此互不重叠，
CLI 在开任何 StateStore/migrate 前校验 master-key path，随后只打开预先配置的独立 arm
数据库，不复制源文件。应用 adapter 也校验三套 DB/source/history roots 不重叠；A 仅使用
baseline IndexService 普通检索，不进入语义 prepare/submit/card 路径。关系在进入
relation barrier/dedupe 前先要求两端 source ID 均属于 scope；模板字段使用移除标点后的
标准 key 归一化检测敏感字段，canonical JSON hash 对对象键显式排序。M6 fixture 的
`q_refs` 是 query IDs（如 `Q-p01-support`），不是 source IDs；保持 q_refs 唯一并绑定
其 task，relation source refs 则继续必须是 task 内 source IDs，避免错误地把两类标识混用。

本次本地验证：`cargo test -p mcp-vault-eval --all-features` 通过（16 unit、2 app-boundary、
13 live-runner、4 synthetic-fixture、1 shadow tests）；`cargo test -p mcp-vault-memory
--lib --all-features` 36 passed；memory `memory_pack`/`semantic_public` 25 passed；State
`semantic_memory_repository`/`semantic_organization_repository`/`semantic_rules` 21 passed。
无 Provider、网络、生产、`./data`、真实笔记或 secret 访问。完整 memory_v3 TCP-listener
集成目标未纳入本轮定向命令；真实 Provider/语义质量验收仍 pending。`cargo fmt --all
--check`、`cargo clippy -p mcp-vault-memory -p mcp-vault-state -p mcp-vault-eval
--all-targets --all-features -- -D warnings`、`git diff --check` 均自然退出 0。

`2026-09-20` M6 final-review P1 A/B/C 修复完成（仅本地）：ordinary lexical FTS 与
current projection 查询增加 `notes.revision=file_entries.current_revision` 及内容 hash
一致谓词；A adapter 逐个核对 manifest source 的当前索引投影、过滤不匹配的 hit，并记录
source coverage、候选数、stale-hit 数和 degradation reasons。coverage 不完整、索引错误、
旧 hit、degradation 或非 no-answer 任务无 eligible 候选会在 A answer Provider 请求前
fail-closed，脱敏计数写入 review artifact。B/C preflight 将 observation source_ids 限制在
所选 holdout task source union 内，且只有所有所选任务仍满足 B 单来源/C 关系覆盖条件时才
允许子集；observation 执行按此已验证集合遍历。live-config artifact 写出 B/C 实际 allowlist
IDs 与 hash。新增 stale projection、零 eligible 候选、开发集及未选 holdout source 预检回归；
额外开发/任务外来源拒绝发生在 Provider 调用前。首次 Indexer 定向测试暴露 full rebuild
后旧 topic-membership 断言过期，改为验证 move 后从 `docs/**` membership 移除，再运行全套
Indexer 测试通过。验证：`cargo test -p mcp-vault-eval --all-features`（44 tests）、
`cargo test -p mcp-vault-indexer --all-features`（14）、`cargo test -p mcp-vault-state
--all-features`（83）、`cargo test -p mcp-vault-memory --lib --all-features`（36）、
四 crate 合并 `cargo clippy ... --all-targets --all-features -- -D warnings`、
`cargo fmt --all --check`、`git diff --check` 均通过。未调用外部 Provider/网络/生产，未访问
`./data`、真实笔记或 secret；M6 真实 Provider 与语义质量验收仍 pending。

### M6 live runner latest final review follow-up — completed 2026-09-20

按 latest final review 继续收紧 M6 live runner 的 runtime 边界：baseline/B/C SQLite
leaf 在打开及迁移前后均须做非 symlink regular-file 与 canonical-in-run-root 校验；
`external_request_budget` 接入 shared Provider transport budget 并计入 retries；冻结
Provider/model/endpoint/settings/mode/capability 运行指纹及模板，逐次调用前后 fail
closed；Provider 成功后立即持久化用量/费用；强制 split 来源互斥和完整 holdout task
set；arm copy 在 pack/relation/answer 边界重新验证；补齐配置工件字段及 card 双 hash
语义。只使用 fake/local fixtures，不调用外部 Provider/网络或生产服务，不读取真实笔记。

另补 `MemoryPack` 来源清单对当前 Canonical File 内容哈希与 SourceRevision 哈希的比较，
避免 stale SourceRevision 在 Indexer 拒绝旧 projection 后只表现为 no-answer 而丢失 fence
诊断。路径移动 fixture 先消费 navigation-only source event，再验证同 File ID、同内容版本
的卡片更新为新路径；文件内容变化 fixture 仍要求零返回并报告来源清单变化。

验证：`cargo test -p mcp-vault-eval --all-features` 全通过（50 tests）；
`cargo test -p mcp-vault-providers` 全通过（27）；
`cargo test -p mcp-vault-memory --all-features` 全通过（126）；
`cargo test -p mcp-vault-state -p mcp-vault-server --all-features` 全通过（137）；
`cargo clippy -p mcp-vault-eval -p mcp-vault-providers -p mcp-vault-state -p mcp-vault-memory -p mcp-vault-server --all-targets --all-features -- -D warnings`、
`cargo fmt --all --check`、`git diff --check` 通过。eval/provider transport 重试验证只使用
loopback fake server；未调用外部 Provider、外部网络或生产服务，未访问 `./data`、真实笔记
或 secret。M6 真实 Provider 与语义质量验收仍 pending。

### M6 live runner final-review P1/P2 repairs — completed 2026-09-20

本次复审补齐三项边界：

- baseline/B/C 的 state.sqlite3 leaf 在 live root preflight、CLI 打开/迁移前后均检查非
  symlink、regular file、位于 run root 内；Unix 另要求 nlink == 1，拒绝跨目录共享 inode。
  无可靠 nlink API 的平台在 leaf 检查入口立即 fail closed，即使 leaf 尚不存在也不会打开
  SQLite 或创建 state root。新增三臂 hardlink 回归和 cfg(not(unix)) 缺失 leaf 回归。
  当前 macOS 执行 Unix 硬链接测试；cfg(not(unix)) 的缺失 leaf 与 public-runner 回归需在
  不支持 Unix nlink 的目标平台执行。
- live-config.json 只保留 Provider template 的 stage/model/prompt/schema/profile ID 与
  输出长度、temperature、timeout 等安全设置，并写入完整 template 的 canonical
  fingerprint；system prompt 和 schema 正文不再落盘。opaque live_config_hash 仍对整个
  配置计算，template fingerprint 的回归确认 prompt/schema 语义变化均改变摘要。
- public runner 对任何 preflight 错误都先校验 source/state/history/artifact roots 位于
  隔离 run root 且互不重叠，再决定是否写失败 checkpoint。新增 direct API 回归确保
  artifact 与 state root 重叠时不创建 state 目录或 checkpoint。

验证结果：

~~~text
cargo test -p mcp-vault-eval -p mcp-vault-providers -p mcp-vault-state --all-features PASS
  eval: 53 tests; providers: 27 tests; state: 83 tests
cargo clippy -p mcp-vault-eval -p mcp-vault-providers -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
cargo fmt --all --check PASS
git diff --check -- crates/eval/src/lib.rs crates/eval/tests/live_runner.rs docs/exec-plans/active/semantic-memory-implementation.md PASS
~~~

所有 eval/provider 行为均由本地 fake、临时目录和本地测试服务验证；未调用真实 Provider、
外部网络或生产服务，未访问 ./data、真实笔记或 secret。M6 真实 Provider 与语义质量
验收仍 pending。

### M6 live runner final-review Unix permissions P1 — completed 2026-09-20

统一 M6 full live runner 与 Provider capability probe 的私有运行目录及敏感文件策略：

- Unix 下 run/source/state/history/artifact roots 与 master-key parent 均必须是非 symlink、
  mode `0700` 的目录。已有目录 mode 不安全即拒绝；新目录通过显式 mode 和 permissions
  设置创建。M6 CLI 在打开 SQLite 前准备并复验这些目录；所有 live preflight 对已有 root
  检查权限及路径组件。非 Unix M6 live preflight 在 State/key/Provider 边界前 fail closed。
- 隔离 master key 必须是单链接、非 symlink、mode `0600` 普通文件。M6 所有 live JSON、
  JSONL、checkpoint 和 Markdown report，以及 probe checkpoint 共用 create-new 临时文件、
  mode `0600`、写入和文件 sync、rename 与父目录 sync。现有目标若权限/类型/link 不安全则
  拒绝覆盖，不依赖 umask 或沿用既有宽权限。
- 新增 Unix root mode/symlink/key/file mode 与 live artifact mode 测试；添加
  `cfg(not(unix))` 的 M6 preflight fail-closed 测试。本轮 macOS 环境未执行该条件编译测试。
- 安全文档明确所有真实评测 artifact 即使已脱敏仍按敏感资料保护；未修改全局 auth
  master-key loader 或普通非 live fixture 写入策略。

验证结果：

~~~text
cargo fmt --all                                            PASS
cargo test -p mcp-vault-eval --all-features                PASS (56 tests)
cargo test -p mcp-vault-providers -p mcp-vault-auth -p mcp-vault-state --all-features PASS (142 tests)
cargo clippy -p mcp-vault-eval -p mcp-vault-providers -p mcp-vault-auth -p mcp-vault-state --all-targets --all-features -- -D warnings PASS
git diff --check                                           PASS
~~~

`cfg(not(unix))` 的 fail-closed 测试已加入，但本机仅安装 `aarch64-apple-darwin` target，
因此本轮未在非 Unix target 上编译或执行该条件编译测试。

以上均为本地临时目录、合成数据与 fake/loopback 测试，不调用真实 Provider、外部网络或
生产服务，不访问 `./data`、真实笔记或真实 secret。M6 真实 Provider 与语义质量验收仍
pending。

### 2026-09-21 当前工作区门禁复核

- `cargo fmt --all --check`：通过。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`：通过。
- `cargo test --workspace --all-features`：未通过，退出码 101。已运行并通过 Admin API
  34 项、Auth 29 项、Backup 4 项、Core 24 项、Domain 28 项、Eval 70 项、Indexer 14 项、
  MCP 35 项和 Memory 127 项；Provider 单元/审计 17 项通过。Provider 集成测试 11 项中 9
  项通过、2 项失败：`provider_disabled_mode_fails_before_remote_request` 预期
  `PrivacyDenied`，实际先得到 `ModelCapabilityMismatch(embeddings)`；
  `provider_service_uses_encrypted_secrets_and_vault_model_bindings` 的 fixture 未声明
  embedding capability，解包时得到同一 capability mismatch。失败调用栈在
  `crates/providers/tests/providers.rs:602`、`:1635`。本复核没有修改这些共享 Provider
  文件；同日独立的 `admin-provider-binding-save-issue` 工作计划包含按 role 强制 capability
  的改动，需由用户决定其保留方式后再处理整合失败，不把 workspace test 记为通过。
- `cargo test -p mcp-vault-state -p mcp-vault-server --all-features`：通过。Server 55 项；
  State 85 项（unit 31、auth 3、background 12、repositories 18、semantic memory 12、
  semantic organization 6、semantic rules 3）。Eval、Memory、MCP 的所有测试目标已在
  workspace 命令中通过。
- `CI=true pnpm --dir frontend/admin lint`：通过。
- `CI=true pnpm --dir frontend/admin test`：通过，4 个文件、40 项通过、10 项跳过；
  Semantic Memory 测试仍有 React `act(...)` 环境 warning。
- `CI=true pnpm --dir frontend/admin build`：通过（TypeScript 与 Vite build）。
- `git diff --check`：通过。
- M5 explicit-memory 界面状态复核：Admin `memory` 页面请求 `/memories`，服务以
  `MemoryReadAccess::ExplicitOnly` 返回；`RawExplicitMemoryPage` 已提供显式记录的列表、创建、
  修订保护编辑和删除。MCP 的 raw explicit surface 已提供 remember/list/get/update/forget，
  remember 可提交带 File ID/revision 的可选来源绑定。旧进度记录中的“explicit list/edit/source
  binding pending”不再准确描述当前能力；Admin 表单未提供来源选择器，但 §8 未将该选择器列为
  M5 退出条件。
- 本轮均为本地静态检查、合成/隔离测试和本地 Provider fakes，没有发起真实 Provider 或
  网络请求，没有访问生产 Vault、`./data`、普通笔记或 secret。上述工程门禁不替代 M6
  真实任务语义/成本验收，也不替代 M7 真实 shadow/client/cutover/rollback 验收。

### 2026-09-21 Admin embedding 配置更新

- 操作者通过 Admin 更新后，以 SQLite read-only 查询复核当前默认 Vault 的有效角色绑定。
  `embedding_memory` 已指向启用的 Provider/model，model capability 中 `embeddings=true`；
  capability 未声明固定维度（`dimension=null`）。`memory_extraction` 仍是独立绑定。
- 查询只读取 Vault slug、role、Provider/model 名称与类型、启用状态及 capability；未读取
  endpoint、secret、凭据、Vault 文件或记忆正文。该结果只证明本地声明与绑定已保存，不证明
  Provider endpoint 可用或实际向量维度；本轮没有发起 Provider 请求。
- Admin 进程目前已退出、8081 不再监听。worker 保持关闭。若后续需要真实能力/维度验证，须
  明确授权 Provider 调用、指定维度策略（固定已知值或受控发现）、隔离运行配置及预算；M6
  真实语义质量门禁和 M7 生产切换授权仍未完成。

### 2026-09-21 M6 真实验收授权

- 用户明确授权运行真实 Provider/任务验收，并说明费用不设上限。Live config 应将
  `unbounded_cost_authorized=true`；计划仍要求有限的外部请求次数预算，并在第一个 Provider
  错误处停止，不能将无费用上限解释为无限重试。
- 当前仓库只有标记为 synthetic 的 M6 30-source/60-task corpus；live validator 明确拒绝
  `synthetic_only` 或 `synthetic_placeholder` 来源。仓库内没有可运行的非 synthetic live
  manifest/config，因此本轮未调用 Provider，也没有从 `./data` 选择或读取普通笔记。
- 启动 M6 仍需一个获准的精确来源 allowlist、相符的非 synthetic revision/auth/profile/rules
  fence，以及预先隔离的 current-schema baseline/B/C Vault、State、history、master-key 与
  artifact roots。收到用户对具体来源集合的许可后，执行器可生成对应隔离配置和报告。
- 当前有效 `memory_extraction` model capability 标记为 `structured_output=false`。这表示
  登记信息没有声明支持结构化 JSON 输出；角色绑定写入会要求该 capability。现有绑定仍能被
  读取，具体 M6 schema 响应仍须由模型调用和完整 schema 校验验证；之前的单次简单 JSON
  capability 响应不替代该检查。

### 2026-09-21 M6 relation-path acceptance audit

- 只读追踪真实 runner 后发现：C arm 当前先构建并持久化 M1 task pack，再向 Provider
  请求 relation 诊断；该 JSON 只写入 `relations.jsonl`，没有经
  `SemanticOrganizationService::organize_json` 验证/发布，之后回答仍使用 relation 前的
  pack。因此它没有实测 M2 受控整理，不能作为计划第 12.3 节 C（提炼＋受控整理）的真实
  质量对照。
- 在做任何真实 Provider 请求前，先把 C relation stage 接到隔离 C Vault 的候选发现、受限
  relation proposal 校验和 Core 发布，再于发布后构建 C pack。模型只得到本 task scope 中
  的临时 candidate aliases/观察内容；State ID 映射留在 adapter。无候选时记录诊断并跳过
  Provider 调用。B 仍只读取单来源 M1，A 仍使用已冻结的 lexical `index-frozen-v1`。
- 该修正属于 M6；不更改 Provider binding、生产读路径或 M7 切换。到目前为止仍未发起真实
  Provider 请求。

### 2026-09-22 M6 首次真实运行

用户恢复 M6 后，先完成离线准备，再按“不设费用上限、有限请求预算、首次 Provider 错误即停”
的授权运行真实评测。来源 allowlist 使用仓库 `HEAD=e046ceb59393db9a4977ba55899eba301ad28005`
中已提交的 30 份 ADR Markdown（开发集 15、holdout 15），任务集共 60 条（开发集 30、
holdout 30），困难任务 38 条。三套隔离 State/Vault/history 根均为当前迁移版本 42，
Provider 配置从默认 Vault 的 `memory_extraction` 绑定复制为隔离配置，模型为
`xiaomi_mimo / mimo-v2.5`，每个 comparison 的外部请求预算为 160，重试为 0。

离线准备首次暴露两处真实工程缺陷；旧 r2 准备结果保留作失败现场，不作为后续真实运行输入：

- 新来源登记后按语义存储契约处于 `eligible=0,pending_rebuild=1`，但 live verifier 在首次
  Provider 调用前要求完整 current source fence。旧准备器曾提交本地 `success_empty` 伪造
  资格初始化；现已移除。Verifier 现在允许且只允许当前授权有效、revision/source fence
  一致、`generation=0`、`extraction sequence=0`、`eligible=0,pending_rebuild=1` 且
  `invalid_reason=source_changed` 的 fresh source 进入首次提炼；已发布 source 仍要求
  `eligible=1,pending_rebuild=0`。policy 关闭或 revision、来源 generation、内容／revision
  变化均拒绝。该修复只在 eval adapter/准备器内，未改变生产 State/Memory 语义。
- 旧准备器先调用 `load_or_create_master_key` 再收紧权限，存在创建窗口。现改为准备器内
  `create_new` 配合 Unix `0600` 首次写入、同步后再加载；不再依赖事后 chmod，也未扩大
  auth 全局改动。

离线验证与准备命令：

```text
cargo fmt --all                                             PASS
cargo check --locked -p mcp-vault-eval --all-targets --all-features PASS
prepare-semantic-card-live-eval --prepare-authorized-real-semantic-evaluation .../drafts.json PASS
```

旧 r2 准备目录为 `/private/tmp/mcp-vault-m6-real-20260922-r2`，仅保留失败现场，禁止复用。
后续应从新的 0700 run root 重新准备。旧配置摘要为：manifest hash
`e573adf6f34ecbb86af5b09467a47562f520cc33d61dc71de54f399d8bd0ab`，配置中明确
`allow_live=true`、`explicit_live_authorization=true`、`unbounded_cost_authorized=true`，
运行配置和 key 文件均为 0600，运行根为 0700。准备阶段报告的真实 Provider 请求数为 0。

真实 runner 命令已使用显式 live flag 启动，首次 Provider 边界在 observation 阶段失败：

```text
status=failed
first_error_code=provider_dns_failed
provider_requests=0
completed_tasks=0
sequence=1
stage=observation
```

runner 没有重试，也没有写入 observation、card、pack、relation 或 answer 记录；usage 文件
显示 input/output tokens 与 cost 均为 0，`review.jsonl` 保持 `status=pending`。失败现场及
脱敏报告保存在 `/private/tmp/mcp-vault-m6-real-20260922-r2/artifacts/`，其中
`checkpoint.json`、`usage.json`、`review.jsonl` 和 `report.md` 可用于后续诊断。

本次结果只能证明隔离配置、来源 fence 和首个 Provider 错误停止路径已实际执行；不能证明
模型输出质量、A/B/C 对照、M2 关系质量、成本或 M6 通过。按用户既定规则，本次 Provider
错误后暂停，不重试、不消耗额外额度。需要在网络/DNS 问题解决并获得继续指令后，从新的空
artifact 根重新执行；本次失败现场保留，不得覆盖为成功。

### 2026-09-22 M6 r3 真实运行（第二次）

在完成 fresh source verifier、准备器去除本地 `success_empty`、0600 首次 key 创建和
Clippy 修复后，使用新的 0700 根 `/private/tmp/mcp-vault-m6-real-20260922-r3`，通过
沙箱外网络执行了已授权 runner。运行配置保持请求预算 160、`max_retries=0`；r2 失败现场
未复用或覆盖。

本次在首个真实 Provider 请求处停止：

```text
status=failed
first_error_code=provider_response_timeout
provider_requests=1
completed_tasks=0
sequence=1
stage=observation
```

失败发生在 comparison B 的 observation 请求。`checkpoint.json` 的 `last_boundary=final`，
`usage.json` 记录 request_count=1、token/cost unknown，`review.jsonl` 保持 pending；没有
写入 observation、card、relation、pack 或 answer，报告的 `quality_claim=not_evaluated`。
按首错即停规则未重试；r3 artifacts 保留在 `/private/tmp/mcp-vault-m6-real-20260922-r3/artifacts/`。
本次真实运行只能证明新准备器和首个 Provider 超时的停止/脱敏产物路径，不能证明模型质量、
A/B/C 对照、M2 关系质量或 M6 通过。后续如需继续，必须使用新的空 artifact 根并重新确认
运行授权。

### 2026-09-22 M6 r4 单请求超时诊断

经审查后，保持同一 `xiaomi_mimo / mimo-v2.5`、来源内容、prompt/schema 和
`max_output_tokens=2048`，仅在新的 0700 隔离根
`/private/tmp/mcp-vault-m6-real-20260922-r4` 将 Provider `timeout_ms` 和全部 stage
template timeout 提高到 120 秒；`connect_timeout_ms=5000`、`max_retries=0` 保持不变，
request budget 限为 1。r2/r3 失败现场均未复用。

离线准备确认三套 State/Vault 每套 30 个 fresh source、extraction record=0，且真实
Provider requests=0。随后在沙箱外执行单次诊断 runner。首个 B arm observation 请求在
约 120 秒后仍返回：

```text
status=failed
first_error_code=provider_response_timeout
provider_requests=1
completed_tasks=0
sequence=1
stage=observation
```

runner 的 request budget 在 Provider 调用前 reserve，预算 1 保证没有第二个付费请求；
本次未进入 response schema 解析、Memory 提交或任务回答。`usage.json` 记录 request_count=1、
token/cost unknown，`review.jsonl` 保持 pending，r4 artifacts 保留在
`/private/tmp/mcp-vault-m6-real-20260922-r4/artifacts/`。这是单请求 Provider 传输诊断，
不是完整 M6 质量验收；质量结论仍为 `not_evaluated`。

### 2026-09-22 M6 r5 当前模型单请求诊断

在 Admin 将默认 Vault `memory_extraction` 绑定到已启用的 `mimo-v2.6-flash`（revision 2）
后，创建新的 0700 隔离根 `/private/tmp/mcp-vault-m6-real-20260922-r5`。来源、30 份 ADR、
60 条任务、prompt/schema、`max_output_tokens=2048` 与 r4 保持一致；配置为
`timeout_ms=120000`、`connect_timeout_ms=5000`、`max_retries=0`、request budget=1。
三套 State/Vault 每套 30 个 fresh source、extraction record=0；r2/r3/r4 现场未复用。

沙箱外 runner 首个 B arm observation 请求成功越过 Provider 传输并返回模型输出，随后在
M1 observation 提交校验处停止：

```text
status=failed
first_error_code=semantic_evidence_role_overlap
provider_requests=1
completed_tasks=0
sequence=1
stage=submit_observation
```

`usage.json` 记录 input_tokens=3413、output_tokens=4155、cost unknown；没有发布
observation/card/pack/answer，`review.jsonl` 保持 pending。r5 artifacts 保留在
`/private/tmp/mcp-vault-m6-real-20260922-r5/artifacts/`。这证明当前模型已完成一次真实
Provider 生成并到达语义提交边界，但提交校验失败；不是完整 M6 质量验收，质量结论仍为
`not_evaluated`。按 budget=1 未重试或发送第二次请求。

随后对 r5 的 `semantic_evidence_role_overlap` 做了本地 validator 诊断和修复。原始
模型输出的 observation 0（source `S16`）将 `block-000003-4737d94c7bf6` 作为
`source_time_scope.evidence_block_ids`，同时该 block 已在 body；模型没有显式填写
`context_block_ids`。Validator 原先无条件把时间 evidence 加入 context，再按 body/context
角色重叠拒绝，导致合法的同一原文 block 同时支撑正文和来源时间时失败。

当时的规则仍拒绝模型显式 body/context 重叠，并继续解析/校验所有 time evidence IDs；仅在
时间 evidence 已属于 body 时不重复加入 context，独立时间 context 仍保留并去重。该拒绝规则后由
ADR-0040 2026-09-29 的 M6 v7 wire 修订收窄；legacy complete-proposal API 仍按本段 fail closed。新增回归
覆盖：同一 body block 作为 source-stated time evidence 可完整发布且 evidence context 不重复；
显式 context overlap 仍 fail-closed；原有独立时间 context/rebind 回归保持通过。该修复只
影响本地语义 validator，未重跑 Provider，r5 真实失败仍保留为历史诊断，不能改称 M6 通过。

随后使用 r5 保存的完整 observation JSON 与冻结 S16 ADR，在临时独立内存 State/Vault 中做
了一次离线 M1 submit 回放。回放重新经过 `prepare_source` 和 `submit_proposal_json`，未复用
r5 extraction、未修改模型输出、未调用 Provider；临时测试文件已删除。time evidence 修复
已越过原来的 `semantic_evidence_role_overlap`，但回放在下一项既有语义校验处停止：

```text
offline replay result: failed
error: semantic_card_kind_scope_or_status_mismatch
provider_requests: 0
```

因此 r5 输出在当前 validator 下尚未完整发布；没有声称离线回放或 M6 通过。该结果说明
原始真实失败的第一层已被本地修复，但完整 proposal 还包含下一项 card kind/scope/status
不一致，需要另行分析；本轮不自动改写模型输出。
r5 结构回放未通过，不能改称 M6 通过。

2026-09-22：针对 r5 暴露的 card proposal 契约缺口，评测准备器 observation prompt/schema
补充明确要求：每个 card 引用的 observations 必须与 card 的 kind、scope、assertion_status
完全一致，每个 observation 恰好被一个 card 引用且索引有效。该修正是通用接口契约修复，
不是对 S16 输出或 golden 的定向修改。S16 已在 r3/r4/r5 诊断中实际暴露，后续全量运行的
质量结论需保留这一来源已接触限制。

### 2026-09-22 M6 r6 全量运行（首个语义错误停止）

在 A gate 与 card proposal 通用契约修复后，使用全新 0700 根
`/private/tmp/mcp-vault-m6-real-20260922-r6`，prompt 版本为
`semantic-cards-tracked-adr-m6-v2`，模型为 `mimo-v2.6-flash`，预算 160、timeout 120 秒、
connect timeout 5 秒、retries 0。offline preflight 确认 A/B/C 为 30/8/15 sources、各 30
tasks，三套 State 每套 30 fresh sources 且 extraction=0，holdout 近似 lexical zero overlap=0。

真实 runner 在首个语义错误处停止：B arm 的首个 observation 已成功提交并继续到 C arm；
C arm 的首个 observation 在提交时触发：

```text
status=failed
first_error_code=semantic_evidence_role_overlap
provider_requests=2
completed_tasks=0
sequence=2
stage=submit_observation
```

`usage.json` 记录 B request_count=1/input=3499/output=6498，C request_count=1/input=3499/
output=5001，合计 input=6998、output=11499、cost unknown。未进入完整任务回答、质量
threshold 或人工 review；r6 artifacts 保留在 `/private/tmp/mcp-vault-m6-real-20260922-r6/artifacts/`。
本次证明新 prompt 能让至少一个真实 observation 通过，但下一份模型输出仍触发 evidence
role overlap；M6 仍未通过，未继续第三个 Provider 请求。

本轮另修正 M6 artifact projection：production `current_card_projection_for_arm` 原先在
`list_cards_for_source_revision` 后 `.find(...)` 第一张卡，导致 B 的实际 7 张 card 只写入
一张 `cards.jsonl` artifact；State 中的完整 cards、关系和 pack 读取路径未被删除。现在
返回 `{source_id, source_revision_id, card_count, cards:[...]}`，安全 projection 允许并递归
保留完整卡片集合，hash 覆盖整个集合；合法 `success_empty` 在已成功提交且 source fence
有效时返回 `card_count=0,cards=[]`，未提交/失效仍拒绝。新增多卡、时间范围/证据字段和空
集合 projection 回归，未改变生产 State schema。

另修正 A lexical gate：健康索引下 `eligible_count=0` 不再被归类为 infrastructure fatal，
而是输入 `quality_events=["lexical_no_match"]` 并继续 answer，review 保留 coverage 与
zero-hit 事实；coverage/hash/revision/stale/index degradation 仍 fail-closed。A 仍是
lexical-only baseline，holdout 的 zero-overlap 仅是基于 HEAD ADR 文本的 token substring
近似预检，不是 IndexService 实测覆盖证明。

## M1 两阶段组合协议（2026-09-22）

已实现 `begin_generation`、`accept_observation_result`、`submit_composition` 及 eval adapter 接线。prepared handle 在首 Provider 请求前建立并携带 source fence；composition 只接收标题和 observation 引用，card metadata 由服务推导。新 live 配置使用 `m1-two-stage-v2`、observation/composition/relation/answer 四个模板，旧三模板配置走独立 legacy prepare-only 路径。

本轮新增 `crates/eval/tests/two_stage_live_boundary.rs` 的 production boundary 集成回归，共 3 项通过：normal 两阶段成功路径、`success_empty` 单阶段路径，以及 phase2 loopback HTTP 错误后调用 semantic abort/cancel 路径。三项均使用真实 `ProviderService`/`ProviderServiceAppBoundary`、共享模板 builder、隔离临时 State/Vault 和本地 Axum OpenAI-compatible endpoint，不访问 `./data`、真实资料或付费 Provider。normal 案例实际断言两次 HTTP、单个 `success_nonempty` extraction、两张 card 及由 observations 派生的 metadata；empty 案例断言一次 HTTP、无 composition 和空 card projection；phase2 错误案例断言恰好两次 HTTP、无 card 且 extraction 不留在 pending/running。

r9 首次生产边界回归在首阶段因 `semantic_observation_phase_contains_cards` 失败，根因是共享 observation template/schema 仍要求并传递旧的 cards 结构。现已修复为两阶段契约，并补充共享 template builder 与对实际 HTTP request schema 的断言；该失败及修复不代表真实 Provider 质量验收结果。当前本地复核结果为：Eval 83 tests 通过，Memory semantic 35 tests、Memory lib 38 tests 通过；`cargo fmt --all --check`、两阶段测试 target Clippy（`-D warnings`）通过。S16 已暴露的模型输出限制仍保留，A 仍是 lexical-only baseline，不能将这些工程边界回归解释为 M6 语义质量通过。

r10 已完成新的隔离准备，运行根为 `/private/tmp/...r10`。其单 comparison 外部请求上界为 144（23 observation + 23 composition + 90 answer + 8 relation），request budget 配置为 160，Provider retries 为 0。真实执行结果当前仍 pending；不得将已准备或上界计算宣称为真实运行通过，须保留首次 Provider 错误即停和质量未评估约束。

## Provider 流式结构化生成（2026-09-22）

实现中：新增有界 `SseDecoder`，覆盖跨 chunk UTF-8、CRLF、多个 data、注释心跳、`[DONE]`、大小上限和终端 EOF。下一步接入 OpenAI-compatible adapter 的显式 streaming mode 与分层 deadline；最终 JSON/schema 校验和 semantic 原子发布保持不变。

### 2026-09-22 M6 r10 真实运行结果

r10 实际启动并完成到首个 Provider 超时，不能标记 pending。请求数为 9，completed_tasks=0；第 9 请求为 observation 阶段 `provider_response_timeout`。该错误发生在成功 headers 后读取完整 response body 的总 120s deadline 内，未继续重试。此前成功 materialization 的四个 arm/source 与 card 数为：B/S16=7、C/S16=6、C/S17=5、B/S18=6，共 24 cards；无任务完成。第 9 个 source 没有 artifact 记录，不对其 source ID 作确定结论。r10 artifacts 保留，质量仍为未评估。

r11 已重新准备为线上候选，使用新四阶段 streaming 配置、Flash、144 computed upper bound/160 budget、retries=0、stream total 600s、first-event/idle 120s、connect 5s；当前 requests=0，尚未启动。

### 2026-09-22 r11 evidence-catalog diagnosis and schema fix

r11 首次 C/S16 observation 在 3 requests 后于 accept 阶段失败，错误为 `semantic_forged_evidence_id`；usage 为 input 11543、output 12423、cost unknown，B/S16 已成功发布 8 cards，0 tasks。六个 bad IDs 的 hash 后缀实际都存在于本 source 的 allowed catalog，但数字前缀被重编（合法序号为 42、43、44、56、56、58），完整 opaque ID 校验因此正确拒绝。

已在 eval app boundary 的 observation Provider 请求前，将 prepared source 的完整 typed block catalog clone 到每次 request schema 的 body/context/time evidence ID items.enum；原模板不变、每 source 独立解析，最终 memory validator 不变。prompt/schema 版本随 dynamic enum contract 递增；r11 原始 artifact 保留。

### 2026-09-22 r13 真实运行与安全 schema 诊断

r13 使用 `m1-two-stage-v2`、prompt v5/schema v3、stream total 600s、first-event/idle 120s、connect 5s、retries=0，computed request upper bound 144、budget 160。真实运行在 15 个 Provider 请求后首错停止，0 tasks；此前 7 个 source collections 成功 materialize，实际 cards=46。首错为 observation 阶段 `provider_schema_invalid`，模型最终结构化输出不符合 schema；原 r13 artifact 未保存 schema issue/path，因此无法判断具体约束类别，未把 source/arm 依据执行顺序猜测写入事实。

为保留这一安全诊断边界，eval Provider error 现在传递现有 Provider 层的白名单 `schema_diagnostic()`（issue 与可信 schema path），runner 在失败时额外写入 `schema-diagnostics.jsonl`，并关联 sequence/stage/arm/source/task/code。不会保存模型响应、prompt、source body、凭据或任意错误字符串；旧 code/checkpoint 格式继续兼容。离线 eval 全套 46+4+28+4+1 项通过，eval clippy 与 fmt check 通过。r13 artifact 保持只读，未重试。

每次 Provider 调用前另写入安全的 `attempts.jsonl` started 记录，确保普通 timeout/transport 错误也保留调用上下文；该记录不表示 HTTP 已成功，仅表示已进入 Provider boundary。

### 2026-09-22 r13 后续边界与诊断入口事实

全 Rust 流式 Provider 套件（9 个 generation preset/adapter 场景）已完成一次完整门禁：workspace tests、workspace Clippy 和 fmt 均通过。该工程门禁不构成真实 Provider 语义质量通过。

r13 的事实保持为：在第 15 个 observation 请求处收到 `provider_schema_invalid`，此前 7 个 source groups 已 materialize 46 张 cards，0 个 task 完成；原 r13 State 中发现 C/S20 extraction 曾保持 `running`。原 r13 artifact 与 State 作为失败证据保留，只读、不复用、不重跑，不据此推断 C/S20 已通过。

正式 runner 首个 observation 失败后的 abort/cleanup 已修复。新增真实 State/Core/Provider/Semantic boundary 回归确认：本地 loopback 返回 schema-invalid output 后，真实 `ProviderServiceAppBoundary` 只发 1 次 HTTP，调用真实 semantic abort，pending extraction 清空且没有 cards；该回归与 runner 自动 abort 测试组合证明 cleanup 路径，不宣称完整 CLI 运行通过。安全 `schema-diagnostics.jsonl`、`attempts.jsonl`、issue/path 和 arm/source/task 上下文也已补齐；不保存模型正文、prompt、source body 或 secret。

新增独立 single-source diagnostic 入口，必须使用显式 flag 和新准备的 sealed config：

```text
semantic-card-live-diagnostic \
  --run-authorized-real-semantic-diagnostic <new-sealed-config.json> \
  --arm B|C --source-id <manifest-source-id>
```

入口仍先执行完整 M6 manifest/run-config/holdout/frozen preflight；source 必须属于原始 arm allowlist 和 holdout source union。诊断最多 2 次 Provider 请求，`success_empty` 只发 1 次，retries=0，首错 abort，结果固定为 `quality_claim=not_evaluated`、`m6_acceptance=not_run`。fresh preparation seal 和一次性 claim 会拒绝旧 State、旧 artifact、路径漂移或 seal 复用；不得拿旧 r13 配置做诊断，必须重新 prepare 并取得新 seal。

当前本地验证计数为 Eval 97 tests（lib 50、app boundary 4、runner 29、fixture 4、shadow 1、diagnostic 3、two-stage boundary 6），Eval Clippy 与 fmt 通过。C/S20 仅作为即将准备/执行的诊断对象，尚未形成真实运行结果，不能称为通过。

### 2026-09-23 首阶段 prepared extraction abort 补正

新增的 loopback SSE 非法 JSON 回归暴露了此前 abort 证据未覆盖的状态：首个 observation Provider 请求失败时，`prepared_generations` 中仍持有 handle，但 `abort_semantic_for_arm` 只移除它，没有将其 extraction ID 交给 State 取消；因此 `semantic_extraction_sets` 保持 `running`。`pending_extractions` 与 card projection 为空并不足以证明 extraction 已终止。

已在 `crates/eval/src/adapters.rs` 的应用边界 abort 中，对移出的 `SemanticPreparedGeneration` 调用 `SemanticMemoryService::cancel_extraction(context, extraction_id)`；原 observation phase 分支仍用同一服务取消。State 继续执行 Vault-scoped 条件更新，错误码继续向调用方传播；协议层没有新增 SQL。真实 `ProviderServiceAppBoundary` loopback SSE 回归断言一次 HTTP、结构化 JSON 解析诊断脱敏、最终 extraction 为 `cancelled`、pending 为空且 cards 为空。phase2 abort 与成功/空结果路径也继续覆盖。

本轮验证（全部使用本地 loopback/fake，不调用真实或付费 Provider）：

```text
cargo test -p mcp-vault-eval --test two_stage_live_boundary       PASS (7)
cargo test -p mcp-vault-eval --all-features                      PASS (50 unit + 49 integration)
cargo test -p mcp-vault-providers --all-features                  PASS (33 unit + 20 integration)
cargo fmt --all --check                                           PASS
cargo clippy -p mcp-vault-eval -p mcp-vault-providers --all-targets --all-features -- -D warnings  PASS
```

M6 真实语义质量验收仍未运行/未通过，本修复只证明首阶段失败的本地清理边界；既有 r13/d1 数据保持不动。

### 2026-09-23 R14 正式真实评估结果

R14 使用全新正式目录 `/private/tmp/mcp-vault-m6-real-20260923-r14` 与
`live-config.json`。运行前只读核验确认 preparation seal 存在且 evaluation hash 与
配置一致、一次性 claim 不存在、artifacts 为空；baseline/B/C 三个独立 State 各有 30
sources，cards、extraction sets、organization jobs、task states 均为 0。冻结 manifest
包含 30 sources、60 tasks、30 holdout tasks；A/B/C sources 为 30/8/15、各 30 tasks；模型
`mimo-v2.6-flash`、protocol `m1-two-stage-v2`、prompt v5、schema v3，四个 Provider stage
timeout 均为 600s，connect/stream first/idle/total timeout 为 5/120/120/600s，retries=0，
每个 comparison request budget=160。既有 9 月 22 日目录均保留未改。

离线构建命令 `cargo build --offline --locked -p mcp-vault-eval --bin
semantic-card-live-eval` 成功（exit 0）。随后只启动一次：

```text
target/debug/semantic-card-live-eval --run-authorized-real-semantic-evaluation /private/tmp/mcp-vault-m6-real-20260923-r14/live-config.json
```

同一进程运行约 19 分钟后退出，进程 exit code=0；runner 结果 `status=failed`，不能把
CLI exit code 当成验收通过。总 Provider requests=31，completed_tasks=0；第 31 次调用为
C arm / S25 / observation，首错 `provider_schema_invalid`，安全 schema diagnostic 为
`issue=unexpected_property`、`path=$.observations[0]`。没有重试（冻结配置 retries=0），
没有继续运行或复用 claim。输出未读取或复制任何原始模型响应、来源正文、prompt 或 secret。

失败 checkpoint/usage/attempts/schema diagnostic 与 runner artifacts 保留于
`/private/tmp/mcp-vault-m6-real-20260923-r14/artifacts/`。安全 usage 汇总为 B 12 requests、
input 66,942 / output 27,997 tokens；C 19 requests，token/cost unknown；整体 cost unknown。
artifact 行数为 attempts 31、observations 30、cards 15、schema diagnostics 1，answers、
packs、relations 均为 0。隔离 State 复核：baseline 30 sources、0 cards、0 extraction sets；
B 30 sources、32 cards、6 个 `success_nonempty` extraction sets；C 30 sources、69 cards、
9 个 `success_nonempty` 与 1 个 `cancelled` extraction sets，取消码
`semantic_extraction_cancelled`；三库 organization jobs 均为 0。A 未产生评估数据。卡片与
Observation artifacts 包含生成内容，继续按私有评估资料保留，不在本计划中展开。

质量指标和人工质量复核均未完成，`quality_claim` 保持 `not_evaluated`，M6 未通过。保留 R14
失败证据，禁止在 R14 上重跑；下一步由主代理独立审阅安全诊断、runner/State abort 语义及
评估产物，再决定是否需要修改评估器并另行授权一个全新 run root。既有 r13、d1、d2、d3、d4
及 R14 目录均不得覆盖或复用。先前 D4 C/S20 两请求成功只证明受限 source diagnostic 可运行，
`quality_claim=not_evaluated`；D1 旧 JSON 错误未复现且原因未知。M7、部署、服务重启仍未授权。

### 2026-09-23 R14 observation 契约提示约束修复

R14 首错已核对为 C/S25 第 31 次 observation 请求：本地严格 schema 校验以
`unexpected_property`、`$.observations[0]` 拒绝；原始 Provider 响应未保存，因此多余键的名字
未知。Eval 在校验前不会向响应添加键。该次 R14 State 为 `cancelled`、0 个 organization
tasks；既有现场和失败 artifacts 保持原样。

本地契约追踪确认 Xiaomi MiMo OpenAI-compatible 请求只使用 `response_format=json_object`，
完整动态 schema 由 Provider adapter 附入 system prompt。此次只加强
`crates/eval/src/templates.rs` 的 observation system 约束，明确顶层仅有 `outcome`、
`observations`；逐一列出 observation 必填字段 `kind`、`statement`、`scope`、
`assertion_status`、`admission_reason`、`value_for_future_work`、`body_block_ids` 和可选字段
`source_time_scope`、`conditions`、`exceptions`、`ordered_steps`、`result`、`uncertainty`、
`context_block_ids`，并禁止额外解释、来源摘录、调试或自行创造字段。prompt ID 升至
`semantic-cards-tracked-adr-m6-v6`，CLI 准备器及相关 runner/source diagnostic 测试期望同步。
schema 字节定义、`schema_id=semantic-cards-m6-json-v3`、动态证据 ID enum、本地
`additionalProperties=false` 严格 validator 和不做输出清理/修补均保持不变。

新增的本地 Eval→ProviderService loopback 回归检查 MiMo wire 的 `json_object`、system 中的
动态 evidence ID enum 与字段约束；合法 observation 输出被本地 validator 接受，带额外字段的
输出仍以 `unexpected_property`、`$.observations[0]` 拒绝。这只验证本地生成契约与 validator，
不表示 MiMo 远端会严格遵守 schema，也不证明人工语义质量。M6 质量仍未通过，
`quality_claim=not_evaluated`；没有真实 Provider 调用，没有准备或启动 R15，没有修改 R14 或旧
现场，也没有部署、重启、提交或推送。

本轮门禁结果：

```text
cargo test -p mcp-vault-eval --all-features                                      PASS (50 unit + 50 integration)
cargo test -p mcp-vault-providers --all-features                                  PASS (33 unit + 20 integration)
cargo fmt --all --check                                                          PASS
cargo clippy -p mcp-vault-eval -p mcp-vault-providers --all-targets --all-features -- -D warnings PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings             PASS
cargo test --workspace --all-features                                            FAIL (first failure: webdav concurrent_unconditional_puts_to_one_path_are_serialized; expected HTTP 201, got 500 at crates/webdav/src/lib.rs:1167)
```

workspace test 在 WebDAV crate 首错退出；本轮未重跑或扩展修复该非目标用例。此前完成的
crate 用例均通过，不能据此将 workspace test 门禁记为通过。

### 2026-09-23 WebDAV workspace 门禁复核

保留上方首次 `cargo test --workspace --all-features` 的失败记录。随后按复核约定只运行一次
精确目标测试 `cargo test --locked -p mcp-vault-webdav concurrent_unconditional_puts_to_one_path_are_serialized --all-features -- --nocapture`，结果 PASS（1 passed）；接着只重跑一次
`cargo test --locked --workspace --all-features`，结果 PASS，包含 WebDAV crate 9 项测试及全部
doc-tests。该次复核没有复现 HTTP 500。并行资源影响仅是可能解释，现有证据不足以确认根因；
没有修改 WebDAV 业务逻辑。Eval/Providers、fmt 与 Clippy 未重跑；没有真实 Provider 请求，
没有准备/启动 R15，也没有修改 R14 或旧现场、部署、重启、提交或推送。

### 2026-09-23 D5 单来源诊断与 R15 离线准备

D5 是全新 C/S25 单来源诊断：在 prompt v6、schema v3 与原严格 validator 下，恰好完成
2 次真实 MiMo 请求，产生 11 observations、5 cards，`first_error=null`。诊断固定报告
`quality_claim=not_evaluated`、`m6_acceptance=not_run`；这只证明受限单来源调用完成，不能证明
语义质量，也不是 M6 通过证据。R14 仍是正式全量现场：C/S25 observation 第 31 次请求以
`unexpected_property` 首错停机、0 个任务完成；R14 artifacts/State 保留为失败证据，不复用、不
覆盖、不重跑。Eval/Providers、fmt、workspace Clippy 与复核后的完整 workspace tests 均已绿，
但工程门禁不改变上述真实质量结论。

以冻结输入
`/private/tmp/mcp-vault-diagnostic-c-s25-20260923-d5-drafts.json` 生成全新私有草稿
`/private/tmp/mcp-vault-m6-real-20260923-r15-drafts.json`。独立 `json.load` 后逐字段比较确认，
仅 `run_root` 改为 `/private/tmp/mcp-vault-m6-real-20260923-r15`；30 sources、60 tasks、30
holdout tasks、gold/query/reference 内容、source allowlist、MiMo Flash、timeout 600、request
budget 160、retries 0 均未改变。草稿 mode 为 0600；全新规范 root 预先以 mode 0700 创建。仅执行
一次离线准备命令：

```text
cargo run --locked -p mcp-vault-eval --bin prepare-semantic-card-live-eval -- \
  --prepare-authorized-real-semantic-evaluation \
  /private/tmp/mcp-vault-m6-real-20260923-r15-drafts.json
```

准备成功，`real_provider_requests_started=0`，模型为 `mimo-v2.6-flash`。sealed config 使用
prompt `semantic-cards-tracked-adr-m6-v6`、schema `semantic-cards-m6-json-v3`、protocol
`m1-two-stage-v2`；observation/composition/relation/answer 四 stage timeout 均为 600 秒，
Provider connect/stream first/idle/total 为 5/120/120/600 秒，retries=0，concurrency=1。
A/B/C source 数为 30/8/15，各臂各含 30 tasks；每臂 request budget=160。seal 存在且
一次性 claim 不存在；artifacts 不存在或为空。baseline、B、C 三份 State 各有 30 个
semantic sources/source revisions，extraction sets、cards、organization jobs 与 task states
均为 0。准备输出与上述只读复核均未执行正式 runner 或诊断、未发送 Provider 请求。R15 当前
仅为新鲜 prepared 状态；不得据此宣称 M6 质量通过。R14 旧现场不复用；没有修改旧数据、其他
代码、部署或服务状态，也没有提交或推送。后续正式运行仍须遵守每轮首错即停、零重试与有限预算。

### 2026-09-23 M6 R15 正式真实评估（首个 Provider 错误停止）

使用全新隔离根 `/private/tmp/mcp-vault-m6-real-20260923-r15`。启动前只读确认准备 seal 存在、`.live-prepared-claim` 不存在、artifacts 为空；baseline/B/C 各有 30 个冻结 source，extraction/card/task 均为 0。运行配置冻结 60 tasks（30 development、30 holdout），A/B/C source 数为 30/8/15、每臂 30 tasks，任务预算 30。Provider 为 MiMo `mimo-v2.6-flash`，prompt `semantic-cards-tracked-adr-m6-v6`，schema `semantic-cards-m6-json-v3`，检索 profile `index-frozen-v1`，四个 stage timeout 各 600 秒；transport timeout 600 秒、stream first/idle 120 秒、total 600 秒、connect 5 秒、retries 0，每臂 request budget 160。准备阶段 Provider 请求为 0。

离线构建 `cargo build --locked -p mcp-vault-eval --bin semantic-card-live-eval --offline` 成功。随后只启动一次授权 runner：

```text
target/debug/semantic-card-live-eval --run-authorized-real-semantic-evaluation /private/tmp/mcp-vault-m6-real-20260923-r15/live-config.json
```

shell 进程退出码为 0，但 runner 报告 `status=failed`，不能按 CLI exit 0 判为成功。运行共发出 7 次 Provider 请求，完成任务 0/30；attempts 共 7 条：B 3 次、C 4 次。第 7 次（B observation）首个 Provider 错误为安全码 `provider_schema_invalid`，checkpoint 为 `sequence=7`、`stage=observation`、`last_boundary=final`。usage token 状态为 unknown（B 的 token 未知，C 已报告 input 21344/output 8111），费用状态 unknown。没有重试，也没有第二轮请求。

失败后只读核对显示 baseline State 的 extraction/observation/card/task/job 记录均为 0；B、C 各有 2 个 extraction sets，13/20 observations 和 7/13 cards；三臂 `semantic_task_states` 与 organization jobs 均为 0，未形成任务状态或待处理评测 job。`review.jsonl` 仅有 pending 人工复核标记。报告的 `quality_claim=not_evaluated`、engineering status failed，所有质量指标仍为 `pending_manual_review`。7 张 B 卡与 13 张 C 卡只代表已落入隔离评测 State 的中间产物，不代表质量通过。

本轮验证到真实 Provider 调用、部分提炼/卡片写入和首个 schema 错误停止路径；由于任务完成数为 0，未得到 A/B/C 任务结果或可判定的正式质量指标。D5 的 C/S25 两请求成功仍不构成质量证据。R15 的 claim、checkpoint、usage、attempts、review、report 和隔离 State/Vault 现场全部保留，不得复用或重跑。M6 仍待人工审查错误诊断、部分产物与评估设计，并在新的单独授权和全新隔离根下完成可审阅的评测；不得据此启动 M7、部署、服务重启、清理旧数据或宣称质量阈值通过。


### 2026-09-23 R15 outcome 冗余状态契约调整

R15 第 7 次真实请求（B/S18 observation）以 `provider_schema_invalid` 停止，安全诊断为
`issue=required_property_missing`、`path=$.outcome`；失败 extraction 已为 `cancelled`，
organization tasks 为 0。R14 先前在 C/S25 observation 因 `unexpected_property`、
`path=$.observations[0]` 被拒绝。两次失败现场和 artifacts 均只读保留，不读取或修补模型正文。

调用链审计确认 `SemanticMemoryService::accept_observation_result` 原先反序列化完整且必需
`outcome` 的 `SemanticExtractionProposal`；Eval 后续已经按受校验后的 `observations.len()` 判定
空/非空，正式 `semantic.extract` worker 仍为占位，Eval 模板是独立的 Provider 契约。调整仅作用于
两阶段 observation 输入：新增要求 `observations`、允许省略 `outcome`、拒绝 cards 和未知属性的严格
DTO；服务校验数组后派生 canonical `SuccessEmpty` / `SuccessNonempty`。若显式 outcome 与数组矛盾仍拒绝。
完整 proposal JSON 提交接口保持要求原 outcome。observation schema 顶层只将 required 改为
`observations`，保留 outcome enum、`additionalProperties=false`、内层 schema 和动态证据 ID enum；
system 提示以 observations 为主，说明可省略 outcome 及一致性条件，不允许 schema 外字段。

schema 版本升为 `semantic-cards-m6-json-v4`，prompt 版本升为
`semantic-cards-tracked-adr-m6-v7`，prepare CLI 与相关配置测试同步。MiMo 请求仍使用
`response_format=json_object`，完整动态 schema 仍放入 system；这只改善程序派生冗余状态的契约，
不放宽 evidence/source fence/语义校验，也不保证 MiMo 远端遵从字段要求。离线回归已覆盖无
outcome 的非空组合路径、空路径、矛盾 outcome、cards、未知键、伪造证据、legacy 必填 outcome 与
本地 MiMo wire。未准备或启动 R16，未执行真实 Provider 请求。


实现与本地验收已完成：新增 DTO 和语义服务派生逻辑；schema required、提示及 v4/v7 精确版本同步；prepare CLI 和 Eval 测试期望同步。相关 Memory/Eval/Providers 全部测试通过，`cargo fmt --all --check`、相关包全目标 Clippy 与完整 workspace 全目标 Clippy 通过；`cargo test --offline --locked --workspace --all-features` 全部通过，含 WebDAV 并发写入用例。没有修改数据库 schema、公开 HTTP/MCP 接口或前端。没有真实 Provider 调用，没有准备或启动 R16，没有修改 R15、D5 或旧现场，没有提交、推送、部署或重启。

### 2026-09-28 恢复审查与离线验收

复核暂停前的 observation JSON 调整及 prepared extraction 清理：两阶段 DTO 要求 `observations`、允许省略 `outcome`，对显式 `null` 使用自定义反序列化拒绝；显式 outcome 与数组不一致会拒绝。unknown 字段、cards、伪造/重复 evidence ID、格式错误 JSON、source revision/policy/rules fence 失败均不能发布，并将运行中的 extraction 置为 failed/cancelled。旧完整 proposal JSON 入口仍使用必需 `outcome`。Eval 的 Provider、JSON 解析和阶段失败 abort 会清理对应 prepared extraction。现有 regression 覆盖这些边界，本轮未发现需要窄修的代码缺陷。

本地门禁结果：

```text
cargo fmt --all --check                                      PASS (exit 0)
cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings  PASS (exit 0)
cargo test --offline --locked --workspace --all-features      PASS (exit 0; 所有 crate、integration 与 doc tests)
git diff --check                                              PASS (exit 0)
```

2026-09-28 对 `/private/tmp/mcp-vault-diagnostic-b-s18-20260923-d6` 仅做目录、权限和文件名/数量核对：目录树权限为 `0700`，文件数为 0，seal、claim、artifacts 和 source/model binding 均不可读取或验证；未读取任何 note 正文、模型原始输出或密钥。D6 当前不具备可验证的一次性执行准备状态，不能复用。

评估 D7 离线重建时，确认仓库含 `e046ceb59393db9a4977ba55899eba301ad28005`，以及 `crates/eval/tests/fixtures/semantic-memory-m6/` 的 30-source/60-task fixture；该 fixture 自述为 synthetic/mock-only，不能证明等同于原 live 的 30-source、60-task、30-holdout 及 B/S18 冻结内容。仓库中未找到 D6 的 sealed config、原 live manifest/source-task hashes 或 claim 状态，也未验证当前 MiMo Flash provider/model binding。因此无法证明重建等价，未创建 D7、未访问旧 R14/R15/D5 现场或密钥、未发送 Provider 请求。后续需从可信保存的原始 manifest/config 与 provider binding 元数据重新核验后，才可考虑全新隔离准备。

### 2026-09-28 新 M6 候选语料离线起草

旧 live manifest、任务 gold 和 draft 均不可恢复，本候选是新数据集，不能标为原 gold、人工已审或 M6 通过。候选固定 `HEAD=e046ceb59393db9a4977ba55899eba301ad28005` 的 30 份 ADR：15 development、15 holdout；60 条任务各 30 条；B 选择 8 个 holdout 来源；包含 12 个无答案候选、23 个 high/critical 标签及跨来源关系候选。纳入/排除的 ADR、每个 source ID/path/Git blob OID/SHA-256、行数、TaskDraft 字段和逐项路径/行号证据见 `docs/eval/m6-2026-09-28-candidate/candidate.json`；中文逐项审阅入口为 `docs/eval/m6-2026-09-28-candidate/review-index.zh-CN.md`。

确定性结构检查命令：

```text
python3 docs/eval/m6-2026-09-28-candidate/validate_candidate.py
```

结果：`PASS`；30 source、15/15 split、60 task、30/30 split、8 B source、23 high/critical、12 no-answer candidates；Git blob 与 SHA-256 fence、source/task split、ID、B 恰一来源、source coverage、证据行范围均通过；无 Provider run material。`git diff --check` 通过。检查只证明固定字节、引用和结构约束，不证明查询自然度、gold 正确性、no-answer 成立或严重度合理。

后续窄审阅修订了 `T-S21-2` 与 `T-S23-2`：两题现在直接询问 ADR 关系及其规则后果；关系声明分别定位到 ADR-0025 第 5 行和 ADR-0027 第 5 行，并补充相对端规则/本题决策证据。candidate validator 与 `git diff --check` 再次通过；关系语义仍待人工复核。

所有任务与语义标签均为 `pending-human-review`。人工必须逐题审阅可用重点、必要限定、禁止推断、状态、来源关系的方向/类型、困难负例、权限/删除含义及 severity；无答案候选须通读对应 ADR 全文。另需人工确认 15/15 topic split 是否仍有 ADR 决策链泄漏；不适合的样本应在正式 freeze 前调整并重新计算 manifest/source/task fences。此候选文件不含 Provider 配置、密钥、run_root、seal 或 claim，未生成 CLI-ready DraftConfig，未准备真实运行根，也未发送 Provider 请求。

### 2026-09-28 M6 answer query 输入修复（离线）

只读追踪用户批准任务草案后的执行路径时发现：A 的普通检索适配器通常在结果中回显 query，但 runner 没有强制这个 answer 输入契约；B/C 虽按 `task.query` 构建 query-limited pack，却只向 answer Provider 发送 `pack` 和 `pack_hash`，漏掉显式 task query。Runner 现在在 A answer retrieval object 中规范写入当前 task query，并在 B/C answer object 顶层写入相同 query；未修改 pack、`pack_hash`、来源 fence、阶段顺序或预算，也没有把 `must_preserve`、`must_not_infer`、`expected_*`、severity 等 gold 字段传给 Provider。

边界回归断言 A/B/C answer 收到相同 query，B/C pack 与关系阶段已持久化的 pack/哈希一致且 pack 本体不添加 query，gold sentinel 和 gold 字段名不在请求中；零普通检索结果且任务标注为 no-answer 时，A/B/C 仍各收到 query 并继续既有回答路径。该回归只验证请求边界，不把 fake Provider 输出当作质量证据。

实施计划 §12.2 要求 fixture 为每个任务包含 gold 字段，并要求开发/保留集按来源分离；§12.5 禁止用保留集迭代提示；§12.4 的语义质量门槛明确针对保留集，当前 live config/runner 也只运行完整 holdout task set。因此计划没有明确要求 30 条 development task 必须逐条人工语义评分，才能报告 holdout M6 结果；但若要冻结并正式采用完整 60-task manifest，开发任务的证据支持和字段结构仍需成立，候选的“60 条逐项人审”是当前数据集验收约束，不能冒充计划原文规定。

验证结果：两个定向 answer 边界回归通过；`cargo fmt --all --check`、
`cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings`、
`cargo test --offline --locked --workspace --all-features` 和 `git diff --check` 均通过。
零候选/expected no-answer 测试验证 A/B/C 仍收到相同 query；未观察到请求数、执行顺序或预算定义变化。
未调用真实 Provider、未创建运行根、未改候选 gold、未提交或推送。

### 2026-09-28 M6 独立智能体复核标准与 holdout 证据包

用户明确授权以独立智能体复核替代人工逐项语义审阅。新增已接受 ADR-0038，并更新实施计划 §12.1、§12.4、§12.6、M6/T12 与本计划 Scope/阶段说明。候选 producer 与 root reviewer 职责分开；Provider 输入不含任何 `expected_*`/gold 字段；post-run answer 先以无 arm 标签的随机 ID 评分，评分锁定后才揭示 A/B/C 映射。报告区分 `agent_reviewed`、`human_reviewed`、`insufficient_evidence`，固定 `human_review=false`；原有 95% 支持精度、95% 限定保留率、90% 重点覆盖、C task point estimate 与 A/B 较优值比较、无答案困难负例以及权限/删除/失效/预算/恢复阻断项均未降低。无证据、review unresolved 或 hash 漂移 fail closed。

未修改 `candidate.json`。其固定 SHA-256 仍为 `50bb80ddebe8a206f9466435b682582ba027b2b09fed8f7bd874fa3e73e95221`。新增 `generate_holdout_review.py`，只从固定 `HEAD=e046ceb59393db9a4977ba55899eba301ad28005` 与 candidate 构建 holdout 30 证据包；运行时逐个核对 15 个 Git blob OID、内容 SHA-256、行数、引用行范围和 candidate hash。无答案任务不把片段作为缺失证明，而要求 reviewer 核对每个任务来源的全文行范围。输出 [holdout-independent-review.zh-CN.md](../../eval/m6-2026-09-28-candidate/holdout-independent-review.zh-CN.md) 的 SHA-256 为 `5ed55fd20f8046d9af4206bc65b6d05388b3d805d8ade016e740ba4ed91a1382`，含 30 项 query/gold、带行号短摘录、关系端点和 review decision 栏；仍待 independent review，不是已审结果。

新增 post-run JSON Schema、空白记录模板和结构 validator，记录 pre-run 30-task `gold_reviewed` 冻结与 post-run 90 个匿名答案评分/裁决，绑定候选与证据包 hash，强制 `human_review=false`，并要求评分锁定时间早于 arm mapping reveal。该 validator 不计算语义质量阈值，也不证明评审正确。当前环境未安装 JSON Schema 校验库；新增 Schema 已通过 JSON 语法解析，模板经项目 validator 检查。

离线确定性检查均通过：`generate_holdout_review.py --check`、`validate_agent_review_record.py`（template-only）、原 `validate_candidate.py`、Schema/template JSON 解析和新增文件 whitespace 检查。未改 Rust；前一节已通过的 fmt、workspace Clippy 与完整 workspace tests 仍是当前 Rust 代码的门禁结果，本段不重跑 Cargo。没有 Provider 请求、run_root、secret、候选批准或 M6 pass。

数据分布风险交独立 reviewer 复核：当前 holdout 30 中只有 2 个 no-answer task、8 个 high/critical task、5 个跨来源关系 task；candidate 总集 12 个 no-answer 中 10 个落在 development。计划未规定 holdout no-answer 的最低数量，因此这不构成已证明的硬阈值违反，但只用 2 个 holdout 负例支撑无答案质量结论偏弱。建议先审语义与 split，再考虑整体调换 source component：将 S18、S19、S26（3 个单 source 组件）换为 S01、S02、S03，并将 S16/S17 组件换为 S05/S06；相应把 3 个 B 标记从旧单源映射到新来源，再把 S16/S17 的 1 个 B 映射到 S05 或 S06。若所有来源/任务结构校验仍成立，holdout 会变为 15 sources/30 tasks、B 仍 8、每题恰 1 个 B，no-answer 约 7、high/critical 约 9。此为待审方案，不是修改或批准；重划前须确认题材泄漏与 source/evidence/gold 的新有效性并重算所有 hash。

### 2026-09-28 Holdout no-answer 候选 v2（定向修订，待独立审阅）

按主代理批准的只读分析方案，仅重写五个 holdout 非关系任务：`T-S18-2`、`T-S19-2`、`T-S20-2`、`T-S22-2`、`T-S24-2`。它们继续引用原单一 source（S18/S19/S20/S22/S24），原 split 与 B 映射不变，且每个 source 仍由另一条任务覆盖；五项 `expected_source_relations` 仍为空。新题分别核查来源 reconciliation retry/backoff、alias enrichment 的实际召回效果、embedding byte envelope 上线效果、forget 后备份恢复保证、以及 similarity 的实测 answerability 概率。前四类改写不得把 accepted design/expected consequence 当成实测或可恢复承诺；S24 仅问 ADR-0028 未报告的经验性 answerability 率，不把原文明确否定的“similarity 等于 probability”标成无答案。

`candidate.json` 升为 `mcp-vault-tracked-adr-m6-candidate-20260928-v2`。只改五条 query/gold/evidence/severity/review_focus 和 candidate review-status metadata；未改源内容、30/30 source split、60/30 task split 或 B-source 选择。所有 candidate/task review state 仍为 pending independent-agent-review，未批准 gold。新 SHA-256 为 `3756e135e574f5d2203ac90a496fca8e03d7ba554b5bdbbe7ba29b2e96122f9f`。全文无答案 evidence 均加入对应 ADR 的 1..EOF 范围及关键相邻摘录。

重建 holdout packet 后，新 package SHA-256 为 `02c3915e637fa004f852cb0a79ed5f08015808b55ea34e37605cc4b2cbd05c90`。确定性统计：holdout 30、no-answer 7、high/critical 10、relation tasks 5、source coverage 15/15；全体候选 no-answer 17、high/critical 25。候选 review index、generator candidate hash、post-run review-record validator/template hash 均同步更新。没有创建运行根或密钥，没有 Provider 请求。


### 2026-09-28 Holdout candidate v3 定向语义边界修订（待独立复核）

按主代理独立复核的固定 HEAD 证据，将候选版本升级为 `mcp-vault-tracked-adr-m6-candidate-20260928-v3`。只调整已点名任务及关联证据/索引：T-S16-1/2、T-S17-1/2、T-S18-1/2、T-S21-2、T-S23-1/2、T-S24-1、T-S25-1/2、T-S26-1/2、T-S27-1/2、T-S28-1/2，以及 T-S29-1/2、T-S30-1 的实施状态限定；其他 task 内容不改。所有语义 gold 仍 pending independent-agent-review，没有批准或通过结论。

关键边界：历史规则题显式限定 superseded ADR 的时间/范围；T-S17-2 只使用 ADR-0016 明示的 ADR-0026 supersession，不把未列入 source_ids 的 ADR-0033内容当作答案；T-S18-2 的无答案范围只涉及 retry/backoff 参数及其失败触发条件，并承认 ADR-0022 已说明文件事件会入队；T-S21-2 区分 ADR-0025 排名聚合与 ADR-0024 UTF-8 输入上限；T-S23-1 改用 S24 作为唯一 B 来源，明确 ADR-0028 将 ADR-0027 bundled activation gate 改为 diagnostic-only；T-S23-2 只陈述 ADR-0027 自身的历史 gate；T-S24-1 以 ADR-0028 amendment 为准，model grouping 已撤销、rule-only chunking；T-S25-1/2 将 S24 标为 B-only 干扰来源，不支持 compact 响应字段/百分比；T-S26/27/28 历史任务区分被替换的合并行为与来源中明示保留的约束；ADR-0033/0034 的 accepted/in-implementation 状态不被解释成生产切换完成。

候选仍为 30 sources（15/15）、60 tasks（30/30），B 来源仍 8，holdout 每题恰有一个 B 来源、15 个 holdout source 全覆盖；holdout no-answer 7、high/critical 10，跨来源关系任务由 5 增至 6（T-S23-1 新增 amends 关系，端点由 ADR-0028 第 5 行直接声明）。生成器核对固定 commit 的 source blob OID/SHA/行数和引用行范围；无答案题保留全文核查要求。candidate SHA-256：`a0f9feddbd9f82b9a5c203a33fdb0008e168d1f765710052eecef4acb3d2e61d`；holdout packet SHA-256：`f7d028b747d7568d00b13b5be9d148515aba48860b6d98bc5bec78ba07c03a34`，已同步 review index、模板及 validator pins。

确定性验证结果：`validate_candidate.py` PASS；`generate_holdout_review.py --check` PASS；`validate_agent_review_record.py` PASS（`template_only`、`human_review=false`）；Eval 目录 JSON 解析、Markdown 相对链接/whitespace 和 `git diff --check` 均 PASS。未重跑 Cargo 门禁（本轮仅修改离线候选与文档），未创建 run_root/secret，未运行 Provider，未修改旧 R14/R15/D5 现场。

### 2026-09-28 Holdout candidate v4 最终窄修（待独立复核）

根据主代理对 v3 全部 holdout 的复核，只改五项：T-S16-1 增加 ADR-0015 第 10–11 行作为分类被 ADR-0026 supersede 的直接证据；T-S17-2 删除把较早 ADR-0017 注中的 “two-phase architecture remains accepted” 当当前状态的说法，改由 ADR-0016 第 3/8 行说明当前 memory 架构已被 ADR-0026 supersede，并将第 11–13 行定位为针对 prerelease 段落的旧注；T-S23-2 的状态规范为 `historical_amended`，表示历史 gate 被修订而非 ADR-0027 整体无效；T-S25-2 与 T-S30-2 的 `expected_status` 从 `unknown` 规范为 `no_answer`；T-S27-2 限定 ADR-0031 的调度/来源安全/恢复边界只是在 ADR-0032 修订时保留，不外推到 ADR-0033 当前 runtime，后者按所列文本仅保留隔离、当前来源资格、显式所有权等原则。候选仍是 pending independent-agent-review，未批准 gold。

版本为 `mcp-vault-tracked-adr-m6-candidate-20260928-v4`。候选 SHA-256：`bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2`；holdout packet SHA-256：`8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a`，已同步 index、packet generator、template 与 validator pins。7 条 holdout no-answer 的 status 均为 `no_answer`；其余 split、B、来源覆盖和任务分布不变。

检查通过：`validate_candidate.py`；`generate_holdout_review.py --check`；`validate_agent_review_record.py`（template-only、`human_review=false`）；Eval JSON parse、Markdown link/whitespace 和 `git diff --check`。本次只做离线候选/评审文件修改，无 Provider 请求、无运行根/密钥、无旧现场操作，也未运行 Cargo。

### 2026-09-28 Pre-run gold-review record fail-closed 修复

修复独立评审记录 `gold_reviewed` 冻结边界。Python validator 现在要求 30 题外层决定均为 `agent_reviewed`，每题八个字段决定均为 `supported`/`not_applicable` 且 rationale 非空；`no_answer_scope` 必须与冻结 candidate 的 `expected_no_answer` 一致，无答案 task 每个 source 必须有 1..EOF 审查证据。Evidence 限定在该 task 的 source IDs，并对所有 holdout source 重取固定 commit Git blob，核对 blob OID、SHA-256、UTF-8 行数及引用范围。冻结还要求 reviewer 与 producer 身份不同、`human_review=false`、带时区的 `gold_locked_at`、`quality_claim=not_evaluated`，以及空的 artifact/blind score 和 adjudication 数组。JSON Schema同步约束冻结状态的 30 条终态 review、8 字段终态值和空后运行数组；跨文件 no-answer/source 关系由 Python fail-closed 检查。`independent-review-protocol.zh-CN.md` 补充相同约束。

新增 `test_validate_agent_review_record.py`，使用临时内存构造的 synthetic-only review fixture 做 10 项正/负自测，不生成或保存真实 review record。验证覆盖正例、字段 revise、空 rationale、无答案 scope 错误、可回答题误标 no-answer、缺全文证据、task 外 source、错误哈希/行界、同一 reviewer、human_review=true、无效锁定时间及非空后运行数组。首次运行暴露 validator 原有 ROOT 计算偏移一级，导致 Git blob 校验在仓库父目录失败；已修正为脚本仓库根路径。

检查结果：10 项 unittest PASS；record validator template-only PASS；candidate validator PASS；packet generator `--check` PASS；Schema JSON parse/shape PASS；修改文件 whitespace 与 `git diff --check` PASS。未改 candidate/gold/template hash，未填写 review record，未触发 Provider，未创建 run_root/secret。仅修改评审 schema、validator、protocol、测试和本执行计划；无需重跑 Cargo 门禁。

### 2026-09-28 D7 B/S18 preflight 阻断快照（权限修复前；后续结果见下一节）

只读复核 v4 candidate、holdout packet 与 agent-reviewed pre-run record pins：candidate `bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2`、packet `8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a`、正式记录 `76011645ef83faee7a65948574acfd930b9ec2b7109c0b4548ee3f3d89bd7bb2` 均匹配。内存中按 `prepare-semantic-card-live-eval` 的 SourceDraft/TaskDraft 字段投影并核对 30 个固定 ADR、60 tasks/30 holdout、15/15 source split、B=8（含 S18）、每个 holdout task 恰有一个 B source；未把 candidate-only review/evidence 字段传入 Draft。完整预算草案保持 160 requests 与 600s timeout；没有写入 drafts 文件。

当前 shell 无 MCP_VAULT_* 覆盖，按 repository default-config 路径只读查询到唯一 `default / memory_extraction` binding：provider `xiaomi_mimo`、external model `mimo-v2.6-flash`，binding/provider/model revision 分别为 2/2/3，均 enabled。该结果来自 `/Users/chengyuntian/Mygit/mcp-vault/data/state/mcp-vault.sqlite3`，不读取 settings JSON、secret ID 或 key 内容。当前 state DB 与 master key 均为 owner `chengyuntian`、mode 0644；其祖先 `/Users/chengyuntian`、`Mygit`、repo、`data`、`secrets` 均 0755，ACL 核对未见读/执行拒绝（home 仅有 deny-delete ACL）。因此其他本机账户可以遍历路径并读取 master key；State 目录中的数据库也可读。`MasterKeyRing::load_file` 只检查普通文件/编码，不拒绝宽权限 key。该 secret-at-rest 隔离缺口是 fail-closed 阻断，不能仅因候选 run_root 计划设为 0700 就继续。

目标 `/Users/chengyuntian/Library/Application Support/mcp-vault/m6-runs/2026-09-28-d7-b-s18-agentreview` 及 mcp-vault 父目录当前不存在；尚未申请外部目录写权限、创建 root、生成 seal/claim/drafts，或执行 prepare CLI。未读取/修改旧 D6/R14/R15/D5 或任何密钥内容，未触发 Provider。继续前需由主代理决定原 Admin state/master-key 的权限修复方案；不得由本轮擅自 chmod、复制或迁移原 key。

### 2026-09-28 D7 B/S18 离线隔离准备完成（Provider 未启动）

上述 preflight 是权限修复前的历史快照。随后复核 source Admin State 与 master-key：`data/state`、`data/secrets` 均为 owner UID 501 / mode `0700`，State DB 与 master-key 均为 owner UID 501 / mode `0600`；shell 未设置 `MCP_VAULT_*` 覆盖。只读查询得到唯一 `default / memory_extraction` binding：`xiaomi_mimo` / `mimo-v2.6-flash`，binding/provider/model revision 为 `2/2/3` 且均 enabled；未读取 settings JSON、secret ID 或 key 内容。

私有 drafts `/private/tmp/mcp-vault-m6-d7-b-s18-agentreview-drafts.json` 为 owner UID 501 / mode `0600`，文件 SHA-256 `8043eb4a42978219a2f9df510a004d350538ff13f29b1c098340ec23fb5f3e9d`。其 source/task 字段投影与冻结候选完全相符，且未携带候选专用的 `evidence_refs` / `candidate_review_state`；30 sources（15/15）、60 tasks（30/30）、30 holdout、B=8（含 S18）、每条 holdout task 恰好一个 B source、request budget 160、timeout 600s 均通过安全元数据核对。

冻结 pin 在准备前后均匹配：candidate `bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2`；holdout review package `8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a`；gold-reviewed pre-run record `76011645ef83faee7a65948574acfd930b9ec2b7109c0b4548ee3f3d89bd7bb2`。record 的 `candidate_sha256` 与 `review_package_sha256` 分别匹配前两项，状态为 `gold_reviewed`，含 30 条 review，`human_review=false`，`quality_claim=not_evaluated`，后运行评分/裁决数组为空。

离线构建 `cargo build --offline --locked -p mcp-vault-eval --bin prepare-semantic-card-live-eval` 成功。随后只调用一次官方 `prepare-semantic-card-live-eval --prepare-authorized-real-semantic-evaluation`，exit 0。run root 为 `/Users/chengyuntian/Library/Application Support/mcp-vault/m6-runs/2026-09-28-d7-b-s18-agentreview`。CLI 返回 `prepared=true`、30 sources、60 tasks、30 holdout tasks、25 difficult tasks、provider `xiaomi_mimo` / `mimo-v2.6-flash`、160 request budget、`retries=0`、`real_provider_requests_started=0`；manifest SHA-256 为 `ab44e32e37de09e3df29678adfac2fd8fb14d1f11b1d37fc7f52b07bb4ce310c`。冻结 prompt 为 v7、schema 为 v4；manifest 保存在 `live-config.json` 的 evaluation 配置中。

准备后只读核验确认 manifest 的 30 source hashes 与候选一致、60 task 字段与候选 projection 一致，提交为 `e046ceb59393db9a4977ba55899eba301ad28005`；A/B/C source 分别为 30/8/15，每臂 30 holdout tasks。baseline、B、C State 各有 30 `file_entries`、30 semantic sources 与 30 source revisions；各自 extraction sets、observations、cards、organization jobs、task states 与通用 jobs 均为 0。三份 State 的当前 source hash 与 revision hash 全部匹配候选。

`.live-prepared-seal.json` 存在且 mode `0600`，seal 路径与 config 匹配；`.live-prepared-claim` 不存在，`artifacts` 为空。首次写入后，storage 新建的嵌套目录及 SQLite sidecars 继承了较宽的 umask mode；只在本次新 run root 内将所有目录/文件收紧为 `0700/0600`，内容字节未改变。最终树共 111 directories / 193 files，全部 owner UID 501、无 symlinks，所有目录/文件权限分别为 `0700/0600`。核验只输出安全元数据、计数与 hashes；没有打印 note/task 正文、secret/DB 内容或 raw model output。没有启动 Provider、diagnostic 或完整 runner，也未访问旧 D6/R14/R15/D5 roots，未提交、推送或部署。

### 2026-09-28 D7 B/S18 单来源真实诊断完成

执行前完成权限与密封核验：run root 为 `0700`；`live-config.json`、`.live-prepared-seal.json` 和隔离 master key 均为 `0600`。seal 的 evaluation hash 匹配；claim 不存在，artifacts 为空，B 臂 allowlist 包含 S18。

规范化 manifest SHA-256 为 `ab44e32e37de09e3df29678adfac2fd8fb14d1f11b1d37fc7f52b07bb4ce310c`。冻结集含 30 个来源、60 个任务和 30 个 holdout 任务，使用 prompt v7/schema v4。冻结的隔离 Provider snapshot 与 State 一致，配置为 `xiaomi_mimo` / `mimo-v2.6-flash`、`max_retries=0`。只读复核确认当前 Admin `memory_extraction` binding 仍指向同一 provider/model。诊断传输硬上限为 2 次请求。

离线构建 `cargo build --offline --locked -p mcp-vault-eval --bin semantic-card-live-diagnostic` 成功。随后只执行一次官方 B/S18 单来源诊断，exit 0；未运行完整 M6 runner、A/C 或其他来源。安全报告 `artifacts/diagnostic-report.json` 状态为 `diagnostic_success_nonempty`，sequence=2、Provider requests=2、`first_error=null`。报告保留 `quality_claim=not_evaluated` 和 `m6_acceptance=not_run`。

B State 中有 14 条 observation、5 张 card（38 条 card item），`semantic_task_states=0`，task_count=0。唯一 extraction set 已进入终态 `success_nonempty`。claim 已创建；artifacts 仅含安全诊断报告。没有读取或输出 note body、task query、raw model text、secret、header 或 key bytes。未重试或复用 run root，也未做代码修改、提交、推送、部署、M7 或 R16。


### 2026-09-28 M6 Agent V1 Full R1 离线准备完成（Provider 未启动）

本轮只完成新的全量 M6 隔离准备。冻结输入与来源为：candidate `candidate.json` SHA-256 `bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2`；holdout review packet SHA-256 `8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a`；锁定的 agent-reviewed pre-run gold record SHA-256 `76011645ef83faee7a65948574acfd930b9ec2b7109c0b4548ee3f3d89bd7bb2`。review record 覆盖 30 条 holdout，`human_review=false`、`quality_claim=not_evaluated`，没有后运行分数或裁决。candidate 与 sources 均固定到 `HEAD=e046ceb59393db9a4977ba55899eba301ad28005`。

离线 candidate 与 record validators 均 PASS。DraftConfig 的 source/task/B 投影与 candidate 相符：30 个 ADR sources（15/15 split）、60 tasks（30/30 split）、30 holdout tasks、B=8，含 25 个 high/critical tasks；每个 holdout task 恰有一个 B source。当前 `default / memory_extraction` binding 唯一且 enabled，为 `xiaomi_mimo` / `mimo-v2.6-flash`，binding/provider/model revisions 为 2/2/3。新草稿 `/private/tmp/mcp-vault-m6-agent-v1-full-r1-drafts.json` mode `0600`，SHA-256 `cca5c2cdb1b1393f8acc0b1794392fdde75836e6e4a4589d8e747c96053bfa1a`；相对允许读取的 D7 草稿仅更改 `run_root`，request budget 保持 160、timeout 保持 600 秒。

`cargo build --offline --locked -p mcp-vault-eval --bin prepare-semantic-card-live-eval` PASS。官方 `prepare-semantic-card-live-eval --prepare-authorized-real-semantic-evaluation` 仅调用一次并 exit 0；其安全摘要为 30 sources、60 tasks、30 holdout tasks、25 difficult tasks、`xiaomi_mimo` / `mimo-v2.6-flash`、budget 160、retries 0、`real_provider_requests_started=0`。新 run root：`/Users/chengyuntian/Library/Application Support/mcp-vault/m6-runs/2026-09-28-m6-agent-v1-full-r1`。规范化 manifest SHA-256 为 `ec54854a4b113e2c469343155b671c3c75750c557d058fa5d048cc5f5f911295`；prompt v7、schema v4，timeout 600 秒。A/B/C sources 为 30/8/15，各含 30 holdout tasks。

准备后只读核验确认 manifest source/task 投影、三个 State 内 30 个来源及 30 个 source revisions 的哈希均匹配 candidate；三份 State 均为当前 schema，且 extraction sets、observations、cards/card revisions、semantic task states、organization jobs 和通用 jobs 全为 0。Provider snapshot 为 MiMo Flash、retries 0。Seal、config 与隔离 master key 存在且 mode `0600`；claim 不存在，artifacts 为空。只在本次新 root 内收紧权限后，整棵树为 owner-only：111 directories `0700`、193 files `0600`，无 symlink；mode 更新保持文件 inode/size/mtime，未修改内容字节。没有输出 note/task 正文、raw model output、secret、headers 或 key bytes。

本轮没有启动 Provider、diagnostic 或 full runner；没有触碰/复用 D7 run root，没有实现代码改动、提交、推送、部署、M7 或旧数据清理。M6 语义质量仍未评估；R1 当前为全量运行已准备状态，不代表真实运行或验收通过。

### 2026-09-28 M6 Agent V1 Full R1 一次真实运行结果（首错停止）

执行前只读 preflight PASS：prepared seal 与完整 live config/path list 一致，claim 不存在、`artifacts/` 为空；manifest、candidate、gold record hashes 与冻结值一致，仓库 HEAD 与 source commit 一致，A/B/C 来源为 30/8/15 且各含相同 30 个 holdout tasks。run tree 所有目录/文件权限为 `0700/0600`、无 symlink。MiMo runtime snapshot fingerprint、隔离 State 的 Provider/model revisions、endpoint、privacy mode 与 zero-retry 设置匹配；计算请求上限为 160，`max_retries=0`。

官方 `semantic-card-live-eval --run-authorized-real-semantic-evaluation` 只调用一次，命令 exit 0，但安全 checkpoint/report 的终态均为 `failed`：

```text
cargo run --offline --locked -p mcp-vault-eval --bin semantic-card-live-eval -- --run-authorized-real-semantic-evaluation '/Users/chengyuntian/Library/Application Support/mcp-vault/m6-runs/2026-09-28-m6-agent-v1-full-r1/live-config.json'
```

第 3 次请求（arm B）触发首错：`provider_schema_invalid`，stage=`observation`，issue=`enum_mismatch`，path=`$.observations[12].source_time_scope.evidence_block_ids[3]`。attempt 顺序为 C observation、C composition、B observation；总 Provider requests 为 3/160，无重试，首错后未再发请求，也未启动第二个 runner、diagnostic、D7/R2 或 Provider 切换。

安全计数：A/B/C 各请求 30 个任务，已完成任务为 0，answer/pack/relation records 均为 0。Provider attempts 按 arm/stage 为 B observation=1、C observation=1、C composition=1；observation records 为 A/B/C=0/0/2，card records 为 0/0/1。使用量总计因运行提前失败而 unknown：请求 3，输入/输出 token totals 未能完整确定，cost status=`unknown`；B 的 token usage unknown，C 记录 input=12742、output=4329。Evaluator 未记录 latency，故 latency unavailable。`quality_claim=not_evaluated`；full-run report 没有 `m6_acceptance` 字段，当前结果不通过 M6 acceptance，不能据此声称语义质量达标。

只读取并核对了 checkpoint、report header、usage totals、attempt metadata 及 schema diagnostic 的安全 issue/path；没有读取或输出来源正文、task query、卡片/回答正文、raw provider output、credentials、headers 或 key bytes。失败 artifacts 与 checkpoint 保留在本次私有 R1 root；本轮在首错后停止，不进行任何进一步 Provider 请求。没有代码修改、提交、推送、部署或 M7 操作。

### 2026-09-28 R1 block evidence ID 本地修复

R1 的安全首错定位于 observation 动态 evidence enum；输入 block catalog 与 enum 均由同一 request input 派生，没有本地构造漂移证据。具体模型可见 ID 不在安全输出中，根因不能断言为模型复制错误或 Provider 未遵从 JSON prompt。按 ADR-0039，将临时 ID 改为 `b{revision-tag}-{base36-ordinal}`：tag 对 Vault ID 与完整 SourceRevisionId 做域分隔 SHA-256，默认取 12 位 hex（48 bit）；与该 Vault 全部历史 revision tags 有前缀冲突时每次扩展 2 位；完整哈希碰撞回退完整 SourceRevisionId UUID。State 查询严格按 Vault 隔离。准备时保存 namespace，并在首次 Provider 调用前和 observation proposal 解析前重算；发现修订清单变化导致 tag 改变则失败终止。composition 不解析 block ID，仅保留原来源/修订 fence，避免无必要清单扫描。序号 checked increment，无回绕；全部 body/context/time enum 和本地解析复用同一 ID。未放宽 unknown/duplicate ID 校验。

`make_blocks` 现要求 Vault 和 SourceRevisionId namespace；历史 rebind 为旧/新 revision 分别重新计算 ID，但持久 byte spans/hash 和 local rebind 的证据判定没有改变。State 只新增只读的 revision-ID 列举方法，没有 schema/数据迁移；MCP/Admin/卡片/evidence DTO 无 block ID 持久输出。版本升至 prompt `semantic-cards-tracked-adr-m6-v8` / schema `semantic-cards-m6-json-v5`，旧 R1 seal 仍固定 v7/v4，不作重用。短标签唯一性检测扫描同 Vault 全部历史 source revisions，复杂度随历史 revision 数线性增长；若后续测量显示成本不可接受，应以具有等价唯一性保证的方案替换，不退回未检查的短前缀。

新增/更新回归包括：不同与相同首行的跨来源响应重放拒绝、同来源重复行序号区分、未知与重复 evidence ID 终止、prefix/hash 碰撞扩展与完整碰撞 fallback、模拟新增修订造成 namespace 变化时拒绝已准备 ID、序号溢出拒绝、prepared observation 合法 source-time evidence、revision drift 终止及历史 spans rebind。namespace inventory 在首次 Provider 调用前与 observation block-ID 解析前重验，composition 不扫描 revision 清单。

最终本地门禁结果：

```text
cargo fmt --all --check                                                    PASS (exit 0)
cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings PASS (exit 0)
cargo test --offline --locked --workspace --all-features                   PASS (exit 0; 所有 crate/unit/integration/doc-test suites 均通过)
git diff --check                                                           PASS (exit 0)
```

首次完整 Clippy 曾报新增碰撞单测使用 `clone()` 构造单元素 slice；已改用 `std::slice::from_ref` 并重跑全 workspace Clippy 通过。最终测试在新增 prepared time evidence regression 后再次完整运行，Memory semantic integration 为 40/40，通过；无真实 Provider 请求。candidate/locked gold 未修改，R1 不复用；没有准备 R2、提交、推送或部署。`quality_claim=not_evaluated`，真实模型效果仍须未来另行授权验证。当前 namespace 冲突保护扫描 Vault 全部 source revisions，复杂度按历史 revision 数线性增长，是后续规模评估项。


### 2026-09-29 R2 重复 observation enum 错误及 v9/v6 本地修复（R3 待门禁）

用户恢复继续 M6 后，先保留并只读核对 R1/R2；两者都已有 one-shot claim，不复用。R1 checkpoint 终态 `failed`，requests=3、completed_tasks=0、last_boundary=`final`；安全诊断为 B observation `enum_mismatch`，位于 `source_time_scope.evidence_block_ids[3]`。R2 使用 ADR-0039 短 ID 与 v8/v5 后同样在第 3 请求 B observation 失败，requests=3、completed_tasks=0、last_boundary=`final`；安全诊断路径为 `$.observations[2].body_block_ids[3]`。未读取任何原始 Provider 输出、答案、卡片或来源/task正文。

根因边界：Eval adapter 从同一 prepared input block catalog 注入动态 enum，MiMo `json_object` 模式把该 schema 作为系统提示并由本地 Provider validator 严格验证；无证据显示 catalog/enum 发生本地漂移。安全诊断只能证明响应中的第 4 个 block 引用不在当前 enum，不能区分模型抄写失误、编造或其他输出偏差。该 fail-closed 行为与 prepared extraction 失败终态保留。

依据 ADR-0040，M6 observation Provider 契约改成单个 revision-bound `evidence_namespace` 和 source-local 1-based integer indices。Eval `prepare_for_arm` 只向模型投影 namespace、index、文本、行号和块类型，不暴露 per-block local ID、Vault ID、SourceRevisionId 或 source path。Resolved schema 把 namespace enum 限为本次 prepared 值，body/context/time index enum 限为当前 catalog 的 `1..=N`。Provider 返回后，adapter 精确要求 namespace 相等、JSON integer 范围有效、每个索引列表内无重复、字段无未知项；按保存的同一 prepared block 列表映射回原 local IDs，再调用既有 `accept_observation_result` 执行 generation/Vault/source/revision/span/hash fence。索引不落库，持久 byte span/hash、历史 rebind、legacy 完整 proposal API 和 composition 流程不变。prompt/schema 为 `semantic-cards-tracked-adr-m6-v9` / `semantic-cards-m6-json-v6`；R1/R2 sealed runs 仍绑定旧版本且不可重用。

新增 fake/本地回归覆盖：namespace 必填、准确 namespace enum、1-based 当前索引 enum；模型输入不含本地 block ID/source 身份字段；相同首行的跨 source 响应重放、同源 revision drift 拒绝；零、越界、浮点、字符串、布尔、重复索引和旧 ID 字段拒绝；有效 body/context/time 索引映射到原 ID，time evidence 与 body 可合法共享索引；既有 Memory 历史 rebind/span-hash 测试继续覆盖持久边界。Composition/provider relation 输入未更改。

目前已通过：`cargo test --offline --locked -p mcp-vault-eval --lib`（52/52）；`cargo test --offline --locked -p mcp-vault-eval --test two_stage_live_boundary`（9/9）；`cargo test --offline --locked -p mcp-vault-eval --tests`（Eval lib/bin/所有 integration targets）。本节追加时尚未完成 `cargo fmt --all --check`、全 workspace Clippy/test、`git diff --check`；这些门禁通过前不创建/启动 R3。R3 的新 DraftConfig 需保持 candidate、holdout packet、agent-reviewed gold hashes 不变、MiMo Flash、budget=160、retries=0；创建新隔离 root 和真实 runner 仍需根侧正式提权。本轮尚未准备 R3、未发 R2 后的新 Provider 请求、未改 gold、未提交/推送/M7。


#### v9/v6 本地门禁完成状态

实现完成后重新执行全部 workspace 门禁，结果均为 PASS：

```text
cargo fmt --all --check                                                    PASS (exit 0)
cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings PASS (exit 0)
cargo test --offline --locked --workspace --all-features                   PASS (exit 0; 所有 crate、integration、doc-test suites 均完成)
git diff --check                                                           PASS (exit 0)
```

Eval 专项共 53 个 lib tests，`two_stage_live_boundary` 9 个生产 adapter/本地 HTTP fake tests，Eval 其余 integration suites 也全部通过。额外只读 whitespace 检查确认本轮修改新增的行无尾随空格；`docs/semantic-memory-implementation-plan.md` 原有第 3–5 行两空格是既存 Markdown hard-break，本轮保留。candidate、holdout packet、locked pre-run gold 的 SHA pins 未更改。R1/R2 仍为已 claim 失败现场；R2 后没有新 Provider 请求。R3 的新 draft/preflight 与根侧正式 prepare/run 尚待后续步骤。


### 2026-09-29 R3 assertion_status enum mismatch 与 v10 prompt 修复（R4 待准备）

R3 的确定性 preflight 在运行前通过：candidate `bdc5f4d0…ff2c2`、holdout packet `8750f576…1119a`、agent-reviewed gold `76011645…7bb2`、canonical manifest `293ba9d4…e469601` 均匹配；30/60、15/15 sources、30/30 tasks、B=8且含S18、MiMo Flash、v9/v6、retries=0、budget=160、seal存在、claim先前缺失、artifacts为空、三份 State 只有冻结 sources/revisions 且 memory/extraction/task/job表为空。

R3 终态为 `failed`，7 requests、0 completed tasks、last boundary=`final`；第7请求 B/S18 observation 报 `provider_schema_invalid / enum_mismatch`，安全路径 `$.observations[6].assertion_status`。schema diagnostic 不含结构化响应摘录或错误值；不推断具体无效 token。静态核查确认 resolver只动态改 namespace/index enum，assertion_status schema保持 v6 七值，MiMo请求schema与本地验证使用同一对象，无本地 enum漂移证据。

按 ADR-0041，手写 prompt 改为 v10，明确七个规范 token及其含义、禁止照抄来源状态同义词、状态不明时选 `unknown`。schema保持 v6，Memory语义及严格 enum不变；不会自动把非法值归类或重试。新增模板一致性测试，以及真实Provider adapter boundary fake，验证非法 status 仍以 `enum_mismatch` 在精确字段路径 fail closed。

R3根保持原样且保留artifact/checkpoint。R4 draft/prepare须固定 candidate/packet/gold pins，使用新鲜 run root，v10/v6、MiMo Flash、160 request budget、zero retries；本节变更的 `cargo fmt --all --check`、`cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings` 与 `git diff --check` 均通过。一次在沙箱内重跑 full tests 因既有假Provider测试绑定 `127.0.0.1:0` 返回 `PermissionDenied / Operation not permitted` 而中断；按正式提权重跑后，受控状态文件记录 `cargo test --offline --locked --workspace --all-features` exit_code=0，根侧确认 62 suites / 646 tests pass、0 failed。日志与状态文件权限均为0600；R4尚未准备或启动。无原始模型正文、notes、密钥访问记录。

R4 离线准备由根侧正式提权执行并PASS，Provider requests=0；manifest SHA=`6a3fc27061eadc05e0ca0b627635ce82af14d6e55d1bde64af7bf0ab1862081c`。preflight `fe7d367eff4095008b1e8607e97bcb55b6a6076aa771e498b466c984df4eb045` 本地PASS：fixed candidate/packet/gold、30/60与split、B IDs/cardinality、MiMo Flash、v10/v6、retries0/budget160、seal present、claim absent、artifacts empty、三个 isolated State空且无symlink/权限漂移均通过。runner wrapper `96cb2d8522e777346ae799f206d949d37ec67e42bca7836cc2d4f0ccd565c808` 固定preflight及runner binary SHA，单次运行且只输出safe result字段；尚未启动R4 Provider。R3现场保留且不复用。


### 2026-09-29 R4 observation item type mismatch 与M6 strict-function wire（本地实现中）

R4 preflight/manifest pins在运行前通过并绑定 v10/v6、MiMo Flash、candidate/packet/gold SHA不变、预算160、zero retries。正式R4运行终态 `failed`，3 requests、0 completed tasks、last boundary=`final`；第3请求B/S17 observation安全诊断为 `provider_schema_invalid / type_mismatch`，路径 `$.observations[6]`。没有structured response diagnostic；未读取原始输出。R1–R4累计失败分布：R1/R2为evidence index enum mismatch，R3为assertion_status enum mismatch，R4为observations元素type mismatch，表现为字段轮流错，未见本地schema drift。R3/R4 artifacts与checkpoint保留。

依据MiMo JSON-object合同限制，ADR-0042选择 M6-only observation strict function wire，不影响生产memory_extraction及其它stage默认路径。v7 schema所有Observation字段 required；optional语义逐项定义为空数组/空字符串/time unknown marker，adapter只做这些明确转换，不做通用缺省推断。Eval在指定v11 observation request设置 strict_function_call；Provider单工具strict schema使用auto选择，不假设forced tool_choice；SSE聚合name/argument fragments并校验function名、ID/index/choice/finish；no-tool、错名、多工具、混合content、截断、重放/超限、schema invalid皆fail-closed。严格Provider schema子集检查只放宽上游minItems/minimum提示；本地完整v7 JSON schema validator及Memory Vault/source/revision/span/hash fences保持完整。

本地已开始实现：Providers strict-wire schema-subset tests、SSE聚合边界tests、Eval v7明确空值转换与MiMo strict-tool HTTP fake初次定向通过；全部 workspace门禁尚未完成。R5诊断前须先完成fmt/Clippy/tests/diff-check、更新文档和fresh root预检；先做单来源B/S17或S18，不直接跑full R5。无当前R5/R6 root，无v11 Provider请求。

#### v11/v7 strict function wire 本地门禁完成

定向验证通过：Providers strict-function tests 5/5，Eval v7明确空值映射 1/1，`two_stage_live_boundary` 11/11。Fake HTTP覆盖MiMo单工具 strict/auto请求、SSE碎片聚合、普通 MiMo `json_object` 路径、其他Provider既有 structured-output 路径及 no-tool/错名/多工具/混合内容/截断/schema-invalid失败关闭。Strict 模式不发送 `response_format`；非strict分支仍按原Provider preset发送既有 `json_object`/`json_schema` body。

最终离线门禁：`cargo fmt --all --check`、`cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings`、`cargo test --offline --locked --workspace --all-features`、`git diff --check` 均 PASS（exit 0）。第一次 workspace test 在沙箱中因现有fake-provider测试绑定 `127.0.0.1:0` 返回 `PermissionDenied` 而退出101；按正式 escalation 重跑完整套件，绑定测试通过且无测试失败。没有真实Provider请求。candidate SHA `bdc5f4d0ce3c06a8531b97a1ec5d781eb59095df7195c1e92b962a55fc5ff2c2`、holdout packet SHA `8750f57608e393fc393faea73f21f36fd595b6b7d0150be522f68553e2c1119a`、agent-reviewed record SHA `76011645ef83faee7a65948574acfd930b9ec2b7109c0b4548ee3f3d89bd7bb2` 均未变化。下一步只准备全新单来源 B/S17 或 B/S18 strict诊断；根侧正式提权前不写 App Support、不发Provider。Full M6/R5 尚未运行，本地门禁不代表语义质量通过。


### 2026-09-29 D8 no-tool协议失败、MiMo thinking设置修复（D9待门禁）

D8为fresh隔离B/S17诊断，manifest SHA `d64a6321a98aad12f0f3ae411a0f697b18d25a164eb096333a7543497e326f3a`。诊断终态 `diagnostic_failed`，runner process exit 0、wrapper exit 2，Provider requests=1、task scoring=0、quality claim=`not_evaluated`、M6 acceptance=`not_run`；first error只保存 `provider_response_invalid` / `provider_observation`，没有schema/parser分类。D8 claim/artifacts保留且不可复用；未读取raw response、tool arguments、reasoning或secret，没有第二请求。

只读sealed config显示preset与thinking mode均为 `auto`；固定MiMo adapter将 `auto` 解析为 `thinking.type=enabled`。MiMo官方FAQ说明thinking启用时tool调用可能进入reasoning内容并不稳定/不完整。D8安全记录没有SSE形状，故这是假设而非已证实根因。`provider_response_invalid` 是 strict stream的一般InvalidResponse类别，不能区分no tool、finish reason、tool delta、choice/index或SSE event shape；它排除已分类的本地schema mismatch、StructuredJson parse failure及HTTP-status rejection。

经ADR-0042修订，仅strict M6 observation最终请求强制`thinking.type=disabled`；普通MiMo及其他Provider仍采用原thinking设置，strict其它body参数、M6 schema/Memory fences不变。严格SSE协议失败新增受限`StrictFunctionCallIssue` enum并从Provider边界安全传入诊断报告，最多只记录no-tool/wrong-finish/missing-id/invalid-choice/mixed-content等类别，不记录tool name、arguments、message/reasoning、raw payload。D8及R1–R4原始根均保留不触碰。

已完成本地定向：Providers strict-function分类测试6/6；Eval `two_stage_live_boundary` 12/12，其中strict MiMo HTTP/SSE fake断言thinking disabled和no-tool分类、普通MiMo fake仍为JSON-object且thinking enabled；source diagnostic 3/3，覆盖safe protocol enum传递、失败终态及零重试。尚未完成该增量的全workspace Clippy/tests/fmt/diff-check。完成完整离线门禁后再准备fresh D9 B/S17诊断，最多2请求、0 task scoring；根侧正式提权运行。若D9在正确thinking配置下仍无tool或strict schema被拒，不直接开full R5，先报告可判定的能力边界。


### 2026-09-29 D9 invalid_tool_delta 与strict M6非流式响应修复（D10待门禁）

D9在thinking disabled配置下仍终止：wrapper exit=2、诊断CLI exit=0、`diagnostic_failed`、1 Provider request、0 task scoring、first_error `provider_response_invalid / invalid_tool_delta`，manifest=`f83ac572ea26c2d6301f016e4d1358a6b60be2e7c2226a7f5e08b9bd0177923b`。D9 claim与私有diagnostic report保留；未读tool arguments、message或secret。当前SSE enum将多个不同shape错误折叠到`invalid_tool_delta`，因此不能定位D9触发的具体字段。

静态对照MiMo Chat API：官方公开完整非流式`choices.message.tool_calls`形状，但流式页面未保证每个partial delta帧都重复包含function子对象或明确字段分片次序。当前strict SSE adapter对每个工具delta要求`function` object，这有可能比实际分片契约更严格。相比继续试错细分SSE形状，按ADR-0042仅strict M6 observation使用`stream=false`，解析完整响应；普通MiMo和其他Provider仍使用原stream路径。完整响应仍要求一个choice/index0、一个function tool、`finish_reason=tool_calls`、空message content、正确type/name/id和非空arguments；arguments必须通过现有完整v7本地JSON Schema及Memory evidence/Vault/source/revision/span/hash fences。新transport `request_json_once`复用endpoint/auth/SSRF/content type/status/concurrency、request budget、请求/响应大小限制、connect/overall timeout与usage处理，但绕过retry loop，强制一次dispatch。

此外，修复SSE tool delta type的fail-open：字段存在时必须是`function`字符串；数字/null不再因为`as_str()`失败而被当成“未提供”。SSE严格协议错误仍只暴露受限enum；非流式解析也使用相同enum，但非法JSON arguments仍产出不含文本的parser诊断，超限和schema错继续独立分类。

首轮本地定向通过：Providers strict-function classifications 6/6；`request_json_once` max_retries=5遇429仍只dispatch一次 1/1；Eval完整非流式strict成功HTTP fake 1/1；no-tool/multiple/wrong-name/invalid-arguments/mixed-content/oversize/wrong-finish/missing-ID响应矩阵1/1。完整workspace fmt/Clippy/test/diff-check尚待对本轮非流式实现执行；未发D9之后的Provider请求。门禁完成后只准备fresh D10 B/S17 diagnostic，最多2请求、0 task scoring；若正确nonstream契约仍失败，根据safe协议分类作能力结论，不再继续无期限试探或开full R5。


### 2026-09-29 D10 evidence role overlap 与 M6 v7 body-first 归一化

D10 使用单次 MiMo strict nonstream observation 请求；Provider完整tool response解析、v7 JSON schema与 namespace/index 映射均通过。随后 Memory `accept_observation` 因 `semantic_evidence_role_overlap` 终止。safe report 仅记 provider_requests=1、0 task、stage/code、B/S17；B State 有1条 failed extraction set，observations/cards/task states/jobs 为0。未读取模型 arguments、正文或secret。Memory检查的精确含义是同一observation的显式body/context引用重合；time evidence 与body共享已由既有规则支持。adapter索引映射没有推断、重排或改变索引。

依用户确认的窄决策，ADR-0040 与 semantic contract 改为：M6 v7 仅在 namespace/schema/每字段整数范围/重复校验通过且映射回 prepared local IDs 后，对同一 observation 执行 `context -= body`。body优先，只移除完全相同local ID的重复角色；原文和证据并集、span/hash/source revision/time fence不变。legacy complete-proposal API继续拒绝显式重叠。安全diagnostic v2仅记录移除数量，不保存ID、行号或正文。

代码位于 Eval v7 restore之后、`accept_observation_result`之前；非法/未知索引仍由此前严格mapper拒绝。新增unit回归证明集合并集不变、独立context保留、时间证据仍可复用body，以及未映射ID拒绝；新增MiMo本地HTTP fake+Memory集成回归覆盖单次strict observation、归一化计数、composition提交与card evidence spans。现有 legacy `explicit_context_overlap_still_fails_closed` 保持原断言。Full gates通过前不得准备D11；门禁完成后仅开fresh B/S17 D11，最多2 requests/0 tasks。D11必须完成observation accept和composition；失败即停止prompt微调，不开R5。成功仅作为诊断能力门槛，之后才可准备full R5；正式M6质量仍需完整任务评测与ADR-0038 independent review门槛，不能由D11替代。

实现已覆盖严格namespace/index/schema映射后的 body-first 集合归一化，失败仍终止于不可信索引或来源 fence。adapter保留移除计数并写入版本化安全 diagnostic v2；日志/报告不保存 evidence ID、line、原文或模型输出。新增 Eval unit 与 strict MiMo HTTP fake/Memory integration 验证证据并集不变、额外context保留、time/body共享、composition与Card evidence span正确；旧 legacy overlap rejection 测试保持通过。

本轮门禁：`cargo fmt --all --check`、`cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings`、`git diff --check` 均有可见 exit 0。第一轮 `cargo test --offline --locked --workspace --all-features` 中所有 crate unit、integration 与 doc-test suite 输出均显示 PASS/0 failed；该命令终端会话未保留数值 exit code。为补取代码而开始的第二轮 full test 按避免无谓重复的指示中断（exit 130，不是测试失败，日志0600且无状态文件）。M6 candidate/gold pins不变，没有 Provider 调用。

D11使用新 root `/Users/chengyuntian/Library/Application Support/mcp-vault/m6-runs/2026-09-29-m6-agent-v1-d11-b-s17-context-normalization`，candidate/packet/gold pins不变，MiMo Flash、v11/v7、retries0、budget160、B/S17诊断上限2 requests/0 tasks。prepare wrapper经根侧正式提权执行并PASS、Provider requests=0，manifest=`42f54813244d70cd72f8f6c3d1e1962c336e2262db2a8474852b08c02fc65fef`。只读postflight确认seal与配置绑定、claim absent、artifacts空、A/B/C State仅含冻结source/revisions，model/template/pins匹配。固定preflight `653db302b8d2a59fd195abded84c9885c66ebbfcc2b518cb0347e21eb8380da1` 本地PASS，并钉住prepare binary及 diagnostic v2计数实现。Runner `d32f56f441011e45bdff9cb66905c25a20ab5e1fa3aa3fd7b84557423870aedd` 仅允许一次CLI调用；仅nonempty observation+composition完整结束的`diagnostic_success_nonempty`/2 requests计为D11能力门槛通过；empty提前结束返回3，失败返回2。runner尚未由根侧启动；D10 claim/report不复用，不在本地发Provider。


### 2026-09-29 D11 composition response failure 与四阶段 strict wire 修复（D12 待执行）

D11实际终态由根侧报告：单来源 B/S17 诊断在 request 1 observation 成功接受，安全归一化计数 `body_context_overlap_removed_count=10`；request 2 composition 以 `provider_response_invalid` 失败，0 task scoring。D11 claim/artifacts保留，不复用。composition当时仍走 MiMo streaming JSON-object 路径，而 observation 已走 strict nonstream function；该泛化错误无法证明是语义结果还是模型/transport形状问题，但暴露出评测链各阶段协议不一致。没有读取原始响应或secret。

依据 ADR-0043，将隔离 M6 的 MiMo observation、composition、relation、answer 统一为 v12/v8 strict function non-stream。其他Provider、普通MiMo与production extraction保持原调用路径。Relation wire 对六个可选字符串逐字段要求：`card_ref/title/content/item_kind/support_operator/reason`；精确空字符串表示“不适用”，adapter只归一化这六项，再执行原relation schema和既有业务验证。验证序列为strict wire schema → relation六字段精确哨兵映射 → 原relation schema → M2 fence；未声明字段、缺字段、非string和非空非法enum均fail closed。三种其他阶段只执行原schema/业务验证，不做空哨兵变换。Provider nonstream仍要求唯一choice/tool、匹配名称、正确finish、无非空content、受限arguments，并通过原Eval业务fences。

本轮定向结果：Providers lib 42/42、Eval lib 58/58、`two_stage_live_boundary` 15/15通过。四阶段HTTP fake验证所有stage使用M6 strict nonstream wire、思考关闭、单tool/auto/无`response_format`；relation空哨兵在wire验证后逐字段移除并由原本地schema接受。普通MiMo JSON-object和其它Provider既有路径的兼容fake保持覆盖。完整 workspace Clippy/tests、fmt与diff-check仍待本轮代码冻结后执行；没有真实Provider请求、没有创建D12。

D12门槛：完整离线门禁通过后，根侧新建fresh隔离B/S17单源诊断，最多2 requests、0 task scoring，candidate/gold与MiMo/预算/retries pins不变。D12只有 observation 与 composition、relation、answer实际调用所需阶段均严格通过才判能力诊断成功；失败则停止D13/full R5并报告具体安全错误，避免后续阶段逐次试探。D12成功仅允许准备full R5，不代表M6语义质量通过；最终仍需holdout达到ADR-0038和既有阈值。Gold/candidate不变，R1–D11现场保留，未提交/推送/部署或进入M7。

D12 draft `/private/tmp/mcp-vault-m6-agent-v1-d12-b-s17-four-stage-strict-nonstream-drafts.json` SHA-256=`50f218fb3e1598175a72d33f4504a261971a4121fcc9cc93322a561c6ab96115`；它只更换D11 draft中的全新 run_root，其他冻结投影不变。离线 prepare wrapper `/private/tmp/mcp-vault-m6-d12-b-s17-prepare.py` SHA-256=`ad591625eb49c2a7126481195614d12cbbd2923258bf79618d4fa31805ce0228`，`--check-only` PASS：30/60、30/30 split、B=8且含S17、最多2 diagnostic requests、requests started=0、target root absent。candidate/packet/gold pins分别为 `bdc5f4d0…ff2c2`、`8750f576…1119a`、`76011645…7bb2`。根侧正式prepare与prepared seal postflight仍待执行。

之后根侧已完成D12 prepare并seal，manifest=`e82c575befa71508c4fdbc2aaaf1b7a9fb9bff1de227c3f6db61d71cb042dfdd`、Provider requests=0。D12绑定stage description仍写observation的binary，按发现的四阶段误导缺陷保留为sealed/unclaimed、不运行。Description仅修为stage-neutral schema-result措辞并加入对应fake断言；格式检查、相关Provider定向测试、workspace Clippy通过。沙箱中的Providers HTTP fake触发系统 `PermissionDenied`（loopback bind），根侧正式提权复跑 Providers 42/42、Eval boundary 15/15 与两个CLI binary build均exit0。修复后 prepare binary SHA=`76893e8f0eb3504f362aef55b7945505f3f64aa3374744c0403e5c88ddb9f3fd`，diagnostic binary SHA=`4351fefbab64210f495a37d4ddd3b444b25bffdf882bae177b6bc10dc52801a3`；candidate/packet/gold pins不变。

D13使用fresh root，draft SHA=`2b66d1b31e086d4ff0f2df47fb2c1939413f72532c841f8a751f88ff8d7bd1a6`，prepare wrapper SHA=`7b8c49c039f651db83999b35315c01fb084a10a476eb9a122d127d3efaf3046d`，check-only PASS。根侧正式prepare exit0、Provider requests=0、manifest=`192f9b768d304914197158f0129d49ac191c21e7e38e75f21c98b3107870467a`。只读postflight与固定preflight `bee8c8975c63fef58818d60bc608b0204efee960919523ccafc51d1e50e1dd75` PASS：seal与配置绑定、claim absent、artifacts空、权限通过、三State frozen source/revisions数量正确且memory/extraction/tasks/jobs为0，MiMo Flash、v12/v8、budget160/retries0与候选/gold pins匹配。Runner wrapper `aa0b2d19556758c7a57ac3c9d54685c6d9194985146ef5565f25c3051f169fe3` 固定D13 preflight和diagnostic binary SHA，恰好一次 B/S17 CLI调用、最多2 Provider requests、0任务评分；safe result含stage/protocol issue/overlap removal count，区分runner exit与diagnostic status。

D13真实诊断终态由根侧报告：wrapper exit2，`diagnostic_failed`，Provider requests=1，0 task scoring，stage=`provider_observation`，code=`provider_schema_invalid`，schema_issue=`type_mismatch`，schema_path=`$.observations`，overlap_removed=0。D13 claim/artifacts保留且不可复用。该schema错误在Provider nonstream tool arguments JSON解析后由本地 schema validator产生；不属于 transport/tool-response protocol错误。Observation模板明确将 `observations` 声明为array（`crates/eval/src/templates.rs`），MiMo strict schema subset会原样保留`type`而仅移除`minimum/maximum/minItems/maxItems`；Provider响应解析后会再次按该schema严格验证。没有发现本地stage投影/array类型被改写或自动填补。D10 observation曾在v10/v6时代通过 Provider解析并到Memory接受阶段后暴露body/context overlap；D11 observation在v11/v7版本接受、成功移除10个重复角色并进入composition。D13同阶段结构错误表明成功请求不保证模型每次遵守同一结构，不能由D10/D11的先前成功推断可靠性。

按D12/D13门槛，停止后续单源诊断和full R5；当前M6保持未验收，`quality_claim=not_evaluated`。没有足够证据证明是本地实现bug，也没有持久保存原始tool arguments供区分模型偏差与服务端strict实现差异。解除阻碍需要能在真实服务端强制并稳定履行完整JSON Schema的模型/Provider，或用户另行批准更换Provider/模型及对应新评测计划；不得通过放宽本地类型校验、文本提取修复或继续小修prompt宣称达标。所有R1–D13现场保留，不再发Provider请求。

### 2026-09-29 A80 产品级提炼与 item 隔离（本地纵切进行中）

用户随后明确要求保持当前 MiMo，不把完整 Observation schema 遵从压力留给模型。此前“D13 后停止所有 Provider 工作”的诊断边界由此产品级重设计覆盖；R1–D13 的 roots、claims 与证据继续保留，均不复用。依新接受的 [ADR-0044](../../adr/0044-bounded-a80-extraction-and-isolated-evaluation.md)，M6 live observation 改为每请求最多80个逻辑块，模型只产 statement 与 source-local evidence indices；服务按 prepared namespace/来源修订严格验证并确定性构卡，完整 source batches 后原子发布。评估协议/冻结 candidate/gold/MiMo 绑定/质量阈值均不变。预算计算和运行使用同一 `a80_batch_count` 函数及manifest block-count fence，硬预算160。

迁移 `0043_semantic_observation_batches.sql` 记录每个 Vault/source revision 的batch hashes、模板/provider fingerprint、attempt state与验证后 proposal hash。崩溃留下的 in-flight request按已消耗预算记账并且不自动重放；仅同 sealed manifest/config 的同 root 可恢复 validated batch/card/answer artifact。`item-failures.jsonl` 私有记录仅写 safe codes：relation失败不应用但继续 answer；A/B/C pack/answer错误作为 item failure继续其它独立 arm/task；source、权限、Provider/config、预算 fence漂移仍全局终止。任何item failure使最终质量声明保持`not_evaluated`。

本轮 A80 fake 集成结果：State batch revision/attempt fail-closed测试通过；Memory parser和单源原子发布测试通过；Eval A80 Provider batching fake与relation failure isolation通过。最近修复了same-root arm JSON `snake_case`恢复匹配缺陷；`a80_resume_reuses_card_and_consumes_existing_attempt_budget` 通过，确认跳过已有card并单调计入既有started请求；`a80_answer_failure_stays_in_denominator_and_later_tasks_continue` 通过，确认失败答案仍入分母且后续task继续。A/B pack/retrieval的A80错误也已改为本地item failure处理。格式化已运行；本轮完整 workspace fmt/Clippy/test/diff-check 尚待冻结代码后执行。此阶段没有真实 Provider 请求。后续真实运行需待代码审阅、分片预算实测和完整离线门禁通过后再由根侧正式准备fresh run；不复用任何旧诊断root。

### 2026-09-30 A80 source-wide retry、请求reservation恢复与当前门禁状态

新增数据库事务唯一键 `(vault_id, extraction_set_id, token_kind=observation_regen)`，把同一 source-arm 所有 batch 的第二次调用共用为一个regen token。初次batch API现在只允许 `ready/attempt_count=0`；failed batch必须经过source fence复核及regen token事务转成 `dispatching/attempt_count=2`。配合State/Memory测试证明第一batch validated、第二batch明确失败时extraction仍running、没有card提前可见；新建服务实例从同一extraction恢复后复用第一batch、为第二batch消耗唯一token，只有全部batch校验后发布两张card。多batch State测试证明对第二个failed batch再要token会冲突。

A80 live JSON现在严格只接受 `{ "claims": [...] }`；claim核心只有非空statement与本batch允许的source-global 1-based integer evidence indices，kind/scope/status/time advisory缺失或不在白名单时归一化为unknown/unspecified并只计数，未知字段和核心/索引错误拒绝。Provider schema dynamic enum逐批绑定连续索引范围。输入输出都省略namespace回填；Vault/source revision/catalog/input/batch/prompt/schema/provider身份仅保留在prepared调用handle与State spec上，模型响应本身不被描述成自证来源。新的冻结wire标识是v14/v10，legacy与v13/v9契约未改。未知kind的`state`存储sentinel在observation、card projection和组织候选查询均还原为`unknown`；不得把旧存储值解释为Fact。

A80 attempt ledger先写`reserved`及`batch_index`，再reserve State；成功后原子文件替换为`started`。崩溃后`reserved/started/uncertain`记录都算共享预算消耗，validated State row只有proposal hash复核通过才优先跳过；ready但ledger已有attempt的batch视为不确定/已预留而不重发。该计数是保守预算charge，不能说每一条都已到达HTTP。若意外出现dispatching State但ledger无对应attempt，会合成safe uncertain行并停止source。

对冻结candidate逐源校验当前文件SHA后，B=8 selected sources共12批，holdout C=15 sources共19批，合计23 source-arm/31 observation batch。主调用上界为31 observation + 90 answers + 6 expected relations =127；唯一source-arm retry最多23次，所以上限150；关系6次与答案4次作为额外上界余量合计160。Relation/answer局部retry尚无执行循环；不为消耗余量加请求。Prepared config仍须在新fresh A80 manifest上重算并核对block counts后才能运行。

已通过的本地定向门禁：State source-wide token/revision测试1/1；Memory跨服务partial-source重启测试1/1、A80 unknown-marker full read test1/1、flat parser/index test1/1；Eval A80 local HTTP fake双batch到双card 1/1、一次schema错误source级regen与共享预算测试1/1、reserved+ready same-root attempt不重发测试1/1、旧terminal card复用测试1/1、prepared-schema source-local index test1/1。未调用MiMo，也未写App Support root。生产 `semantic.extract` 仍是拒绝Provider的占位worker；现有 `semantic-card-live-diagnostic` 固定两阶段不能跑A80。接下来只跑格式、Clippy与一次正式提权的离线全workspace测试，再由主代理按代码行号审阅；A80真实诊断/full M6须另待审阅和重新授权。

2026-09-30 收尾：补齐已发布source在Eval artifact落盘前崩溃的恢复窗口。新建Memory/Eval adapter实例只有在同 extraction/source revision、auth fence、content/file hash、namespace、batch catalog及prompt/schema/provider fingerprints一致，且全部既存batch均validated、proposal hash可验证时，才从已发布State恢复当前cards或empty-success；不重新发布、不调用Provider、不复用别的extraction的进程内cards。success_nonempty与success_empty均有集成回归。A80 observation/relation/answer对auth、secret、配置/身份漂移、端点/权限/能力、State及budget全局错误统一终止run；新增auth失败单调用回归，原relation与answer局部timeout隔离回归仍通过。完整workspace最终门禁尚待执行；没有真实Provider请求。

### 2026-09-30 暂停检查点：M6 真实运行失败

更新本计划中先前的“未调用 MiMo”状态：正式 A80 run 已在 `target/m6-runs/2026-09-30-m6-agent-v1-a80-mimo` 结束，checkpoint 为 `failed`，`provider_requests=113`、`completed_tasks=30`、`first_error=evaluation_items_failed`。113 是保守 reservation/started 计数，不代表确认的 HTTP dispatch 总数。90 条 answer 与 23 次 observation 的安全错误码均为 `provider_response_invalid`；未发布 Observation/Card。Relations 30 条、packs 60 条。失败 root 与 artifacts 保留，质量状态为 `not_evaluated`。

最终离线 workspace test exit 0，fmt、diff-check 与 workspace Clippy 通过；这些门禁在真实 run 前完成。失败后没有再运行代码测试或 Provider。

只读核实确认 A80 v14/v10 没有命中旧 v12/v8 strict-function stage detection，因此 MiMo `json_object` 使用普通 SSE structured-generation path。失败 root 没有更细的 provider diagnostics，无法确认当时的 status/content-type/chunk/finish reason literal。实现时不要把 A80 改成 tools/function wire。建议新增与 strict-function 分离的 non-stream JSON-object transport、关闭 Auto thinking 但保留显式设置、为静态响应错误提供安全 reason code，并以真实 ProviderService boundary fake 覆盖普通 message-content JSON decode。

代码实施已暂停。160 是单次 run 上限，不是项目累计额度。失败 root 已终态化，不能直接重跑；113 个保守预占和原 ledger 保留。47 只是旧 run 的 `160 - 113` 算术差值，不是可恢复额度或项目剩余额度。用户当前暂停，因此没有新 paid call 授权。用户恢复后，应把旧 113 预占纳入跨 run 记录，再依届时授权和有限预算推进；新代码不能复用旧 sealed root。完整暂停原因、后续步骤和路径范围见 [A80/M6 交接记录](a80-m6-handoff-2026-09-30.md)。本 checkpoint 不授权 Provider 请求、部署或 M7。

最终完整 workspace gate 在真实 run 前通过，覆盖当时未改动的代码。失败后只修改了交接资料和 ignore 规则，没有再次运行完整门禁；前端 lint、test、build 本轮没有重新检查。

为避免 `cargo clean` 删除失败证据，完整失败 root 已复制到 `.private-eval-checkpoints/2026-09-30-m6-a80-failed`。原 root 保持不变；副本目录为 0700，208 个文件均为 0600，逐文件 SHA-256 比对无差异；`.gitignore` 以 `/.private-eval-checkpoints/` 忽略备份。副本只用于只读取证，禁止恢复为 run 或修改 sealed 数据。Sealed manifest hash=`19e35fef44b31c36226840cabe2d8f95a4536e01334d8085acefe3cc575a33dd`；candidate、review package、agent-review pins 见交接记录。

已选择整体 197 路径 WIP checkpoint；路径和分组记在 `/private/tmp/mcp-vault-wip-commit-paths-2026-09-30.json`，清单 SHA-256=`15593f10b5c57a8b8551efea21e46bdada2089cddc96e737d3da84405ecb2a07`。暂停前基线 HEAD=`e046ceb59393db9a4977ba55899eba301ad28005`，不是 WIP checkpoint 的提交号。实际保存提交的 HEAD 与提交记录以本文件最终所在的 HEAD 和 `git log` 为准。没有 push、部署或 M7。

### 2026-10-09 A80 v14/v10 非流式 JSON-object 修复（实现中）

补丁来源容器的历史状态（不代表下方新容器复核结果）：恢复后重新核对实际 checkout，工作树位于 `/workspace/mcp-vault`，当前本地分支名为 `work`，HEAD 为交接指定的 `1861dfd37b611ac7703c3c14021a58c9d014de63`，工作树干净。该容器未继承 `rustc/cargo/rustfmt/clippy`；Node `v24.19.0` 和 pnpm `11.19.0` 可用，Rust 相关门禁尚未运行。未调用真实 Provider、未读取或修改封存评测工件。

根因核对为实现缺陷，不是 A80 低复杂度 claims/schema 契约需要放宽：`semantic_a80_provider_templates` 已使用 v14/v10，但 `ProviderServiceAppBoundary::generate_inner` 只用旧 v12/v8 常量选择 strict-function。A80 因而落入普通 JSON-object SSE aggregator；这与 A80 要求的普通 `choices[].message.content` 非流式响应不一致。将 A80 伪装为 strict-function 会违反 ADR-0044 及本计划已冻结的 wire 边界，因此不采用该替代方案，也不修改本地 schema 验证来掩盖 Provider 响应问题。

当前纵切改动：

- `StructuredGenerationRequest` 增加独立的 `non_stream_json_object` typed 语义；它与 `strict_function_call` 互斥，旧 strict-function wire 保持原路径。
- Eval 仅对 Xiaomi MiMo 的 A80 v14/v10 observation、relation、answer 设置该语义。Provider 强制 `response_format={"type":"json_object"}`、`stream=false`，不发送 `tools`/`tool_choice`，通过 `request_json_once` 读取单一 `choices[].message.content`，随后复用既有 envelope repair、JSON/schema 校验和本地规范化。
- 新路径的 Auto thinking 确定性关闭；显式 enabled/disabled 仍按模型设置发送；MiMo token preset 仍使用 `max_completion_tokens`。旧 strict-function 的既有 thinking/tool 合同不改。
- 为 choices/message/content 形状增加静态安全错误子码，并以边界测试确认私有响应标记不进入诊断；不保存响应正文、headers 或凭据。Provider 错误码清单同步更新到 `docs/interfaces.md`。
- `crates/eval/tests/two_stage_live_boundary.rs` 的 A80 fake 已改为真实 `application/json` 非流式 Chat Completion 响应，断言完整请求形状，并覆盖两批 observation→两张确定性卡片及 relation/answer 三个 A80 阶段。

补丁来源容器的前端历史验证：使用临时 pnpm store 执行 frozen-lockfile 安装，`frontend/admin` 的 lint、test（41 passed，10 skipped）和 build 均通过。Rust 1.94.0 工具链恢复或由主环境提供后，仍需运行适用 rustfmt、Provider/Eval/Memory 定向测试、Clippy 与项目检查；当前容器因缺少 `rustc/cargo/rustfmt/clippy` 无法运行这些检查。此修复不授权新的真实 M6 run；M6 质量结论仍为 `not_evaluated`，M7 不在范围内。

### 2026-10-09 06:17 UTC 新环境补丁恢复与 M6 工程复核（默认门禁通过；全部特性外部阻塞）

当前 checkout 为分支 `work`、HEAD `1861dfd37b611ac7703c3c14021a58c9d014de63`。本节只记录本次新容器证据；上文补丁来源环境及更早环境的通过结果不作为本次验证结果。

- [x] 保全环境设置已有的 `docs/development-and-testing.md` 与 `scripts/setup-dev.sh` 原字节及 SHA256。原哈希分别为 `cfc82046f5f1c178080ee1e42391a38cb4a0f1b77aa8e682c6ac908e8de05489`、`9a6afd6c4098565091fb5f1f525177905ec2812ac9e6506d52b6dbe05b93a16a`；补丁与这两个路径无交集，应用前后哈希一致。
- [x] 原补丁经 Library 原生文本读取及唯一获准的末尾 LF 恢复后为 35369 字节，SHA256 `92d729937bc5a2d4740c23e964c998c7ff7b15c4243b9fccf2a1200262cdebf3` 与原始文件完全相符；基线及 `git apply --check` 均通过，14 文件补丁只应用一次。
- [x] 新环境工具链实测：rustc/cargo 1.94.0、rustfmt 1.8.0-stable、clippy 0.1.94、Node v24.19.0、pnpm 11.19.0。普通 shell 先加载 `/home/agent/.cargo/env`；未重装工具链。
- [x] `source_diagnostic.rs` 的隔离目录改为 canonicalized `std::env::temp_dir()`，保留所有原断言及私有目录要求，避免 Linux 不存在 `/private/tmp` 和平台临时目录符号链接问题。
- [x] 新容器 `cargo fmt --all --check` / `git diff --check` 通过；完成状态修复后再次通过。
- [x] 新容器默认特性 `cargo test --workspace --locked` exit 0：685 passed、0 failed、0 ignored；含 Provider unit 45、service 11、streaming 8、audit 1，Eval boundary 16（含 A80 三阶段/两批卡片/恢复及旧 strict-function），Memory semantic 43 与 source diagnostic 3。
- [ ] 新容器全部特性 workspace tests：`cargo test --workspace --all-features --locked` 在 `ort-sys 2.0.0-rc.13` 构建阶段退出 101，测试尚未执行。既定 `cdn.pyke.io` 预编译包下载明确返回 `CONNECT proxy failed: proxy server responded 403/403`；仅尝试本次标准构建一次，未换二进制来源、未改网络设置。
- [ ] 新容器全部特性 workspace Clippy：受同一 ort-sys 构建先决条件阻塞，未再次触发相同 CDN 下载，保持 pending；不得把下面的默认特性 Clippy 通过视为全部特性通过。
- [x] 新容器 `cargo clippy --workspace --all-targets --offline --locked -- -D warnings` exit 0（同一工作区 Cargo 缓存）。
- [x] 新容器 frontend lint / test / build 全部 exit 0；Vitest 为 41 passed、10 skipped（既有 skip 保留，未增加跳过）。
- [x] 独立静态审查发现并修复一个 P1：合法 JSON 搭配截断/过滤/缺失或异常 finish reason 曾被新非流式路径接受。现在解析前要求 stop，并拒绝未完成状态、非零/非法 choice index 和意外工具调用；保留显式 thinking 与旧 strict-function，补充合法 JSON 下的拒绝和脱敏回归。独立复核确认 P1 已解决且未发现新增实质缺陷；动态结果另计。

本轮仅进行代码及本地 fake 工程验证，不读取或修改封存工件、不调用付费 Provider、不读取或新增 API keys、不 push/PR/部署、不扩展 M7。M6 真实模型质量保持 `not_evaluated`。

验证环境记录：默认 `cargo test --workspace --offline --locked` 因新容器缺少 serde registry 缓存退出 101。去掉 offline 后可访问 crates.io，但默认 `/home/agent/.cargo/registry` 为只读，缓存创建以 `Read-only file system (os error 30)` 失败。已将本次命令的 `CARGO_HOME` 指向工作区 scratch 独立目录；继续使用原 Rust 1.94.0，未运行 rustup 安装、未改 shell 配置或设置脚本。`CARGO_BUILD_JOBS=3 cargo test --workspace --locked` 已在该缓存下完成，exit 0，计数见上方新容器结果。本轮结果仅证明本地工程门禁，不代替真实 M6 模型质量验收。

复核结束时间：2026-10-09 06:38:40 UTC。最终保全核对确认两份环境设置文件与原备份逐字节一致，Cargo.lock 未变，代码保留在未提交工作树。默认 workspace / Clippy、fmt、diff、前端与独立代码复核完成；剩余全部特性测试及 Clippy 需既定 ort-sys 下载通路恢复后再执行。既有封存工件、真实 Provider、API keys、M7、push/PR/部署均未涉及。

本次本地验证日志位于工作区 scratch 的 `m6-recovery-20261009.k65s1i4v/validation/`：`workspace-default-writable-cache.log`、`clippy-default.log`、`workspace-all-features.log`、`frontend-lint.log`、`frontend-test.log`、`frontend-build.log`。复现 Rust 检查需先加载 `/home/agent/.cargo/env`，将 `CARGO_HOME` 指向该 recovery 目录的 `cargo-home`；全部特性另将 `XDG_CACHE_HOME` 指向其 `xdg-cache`。这些均为命令级环境变量，不修改用户设置文件。

### 2026-10-09 M6 端到端验收收敛（离线修复；真实运行待本轮明确授权）

目标是实施计划 §12.4 全部门槛通过，补丁、fake 通过和单次 smoke 均不是退出条件。已有冻结30个 ADR、60 tasks/30 holdout 的数据方案继续沿用，不重造 gold、不读取真实密钥、不新建真实 run。冻结输入已获准只读核验，云端三份 candidate/review 工件与 handoff pins 一致，原来源 commit 与当前 HEAD/工作树30份 ADR 的 blob、SHA256及行数全部匹配。父线程转述 Mac 只读配置为官方 `https://api.xiaomimimo.com/v1/`、`mimo-v2.6-flash`；用户已选择后续编译和评测全程云端，不迁移 Mac 凭据。完整 runner 已存在，暂不新增可选 smoke 或评分系统；运行后必须由独立评审者盲评，producer/implementer 不自评。

- [x] 复核硬门槛：支持精度≥95%、限定保留≥95%、覆盖≥90%；关键条件和无答案困难负例无已知关键失败；重复专项优于B且不损伤限定；C任务点估计不低于A/B较好者，按类别/分母报告；权限、删除、失效、预算、恢复用例全部通过。任何 critical/high 未决错误、泄露、复活、证据不足或 item failure 均不允许验收。
- [x] 为完整 runner 的成功和失败 Provider calls 增加按 arm/stage 的实际耗时；旧恢复 reservation 未测量时保留 partial，明示不覆盖检索/组包/评审，不伪造完整延迟。
- [x] 修正旧 `pending_manual_review`/human-only 报告为 ADR-0038 独立盲评 pending，始终保持 `human_review=false`、`m6_acceptance=not_evaluated`；未自动判定语义质量。
- [x] 旧两阶段 diagnostic 遇到 A80 在打开 State/Auth 前拒绝，避免误用 composition。
- [x] A80 preflight 和 prepare 显式限制完整运行≤160 requests；新准备草稿要求明确 `generation_token_limit`，在隔离模型中冻结，并报告有效单次上限与最大生成 token 数。输出预算不冒充输入或金额上限，原模型配置不变。
- [x] 本轮新增行为定向测试及 Eval 回归均 exit 0：prepare binary 1、live_runner 38、source_diagnostic 4、Provider/Memory HTTP boundary 16、Eval unit 61、app_boundary 4、synthetic fixture 4、shadow 1，合计129通过、0失败。`cargo clippy -p mcp-vault-eval --all-targets --offline --locked -- -D warnings`、fmt check 和 diff check 均通过。日志为 recovery scratch validation 下的 `m6-acceptance-targeted.log`、`m6-acceptance-regression.log`、`m6-acceptance-clippy.log`。没有重跑无关前端或默认 workspace 685项基线。
- [x] 关闭独立审查 P1：runtime 启动完整比较并保留封存 snapshot；本地合成 State 测试覆盖输出限额32768→131072、capabilities及 endpoint 漂移，均在网络连接前拒绝。关闭 P2：发送前失败且 transport attempts=0 的调用保留实际耗时，A80预留预算与旧协议实际 attempts 分开核算；旧未测量恢复仍为 partial。独立只读复核确认两项关闭且未发现新增实质问题。
- [x] 复核修复后针对性 Rust 回归118项通过（live_runner40、Eval unit61、prepare1、HTTP boundary16），14项候选/gold validator Python 测试通过。candidate validator 允许无关代码 HEAD 推进，仍严格检查冻结 commit blob/hash/行数以及当前 HEAD/工作树来源原字节。实际 pre-run review 结构校验通过，`gold_reviewed`、`human_review=false`、30 holdout、0 post-run blind scores；不把结构通过当作本轮语义质量评审。日志为 `m6-review-fixes.log` 和 `m6-review-regression.log`。
- [ ] 全部特性工程门禁仍受既有 ONNX 标准下载403阻断；没有环境变化时不重复下载。
- [ ] 在本轮明确授权后，仅一个 fresh full M6（冻结 A/B/C、30 holdout/90 answers，所有 attempts 合计≤160，显式出站输出 token 上限）；之后独立盲评、逐项证据判分及如实成本/延迟报告。旧失败根不可重用，不自动追加付费重跑。

最短路径是关闭新增离线回归与环境门禁 → 配置仅在云端可用的认证及核对非秘密参数 → 取得一次性数据/费用授权并准备新隔离根 → 全量真实评测 → 独立盲评与全部阈值裁定。冻结来源共114516 UTF-8字节，B/C observation来源计入重复为126304字节，holdout查询2907字节；这些不含提示/schema/JSON framing/生成pack，不是模型输入 token 总上界，不能把 `160×32768` 的输出预算当总价。已有685项默认 workspace通过及此前代码审查继续作为基线证据，不为本轮少量 Eval 变更重造整个验收系统。M7、部署、push 和旧数据清理不在范围内。

云端认证已完成代码/官方文档核验，并依父线程明确指示新增显式 `init-m6-cloud-provider` 入口及离线测试；实际配置尚未执行。用户已反馈在 Codex Personal vault 保存 `MIMO_API_KEY` Network secret，但当前任务仅作布尔检测，结果为不存在，未输出值、片段、长度或哈希。这不证明填写失败；环境必须请求该 key，并允许 `api.xiaomimimo.com` HTTPS:443目标，声明/发布更新后需新任务继承，当前工具未暴露环境声明可供核对。占位符经 `ProviderInput.secret` → State/Auth 加密 → prepare隔离复制 → transport `Authorization: Bearer <placeholder>` 原样传递，程序不需要真实token作本地签名。入口要求显式 flag及全新私有绝对根，固定官方endpoint/model，显式 `structured_output=true`、32768、600s、重试0及并发1；输出 prepare所需非秘密路径，拒绝覆盖既有根，不请求Provider。当前 reqwest0.12.28实际启用 WebPKI roots、保留环境代理，未启用 native roots或显式环境CA加载；代理TLS与替换仍待在获准新任务中验证，不声明认证成功。未创建真实 Provider/持久凭据、未读取 Personal vault值、未改网络或TLS设置。

新增入口离线验证：`cargo test -p mcp-vault-eval --offline --locked --test cloud_provider_init` 4通过，覆盖缺失授权/变量、空值与换行的安全拒绝，既有根和符号链接父目录保护，SQLite URI元字符拒绝，以及合成占位符 State/Auth 加密及原字节恢复、角色绑定、权限和再次初始化不改写。独立审查发现主密钥权限依赖umask，已显式设为0600；全部入口测试在子进程umask022下通过，独立复核确认关闭且没有新增实质问题。测试只注入合成占位符，结果在 `m6-cloud-init.log`；Eval all-targets Clippy及新增入口定向Clippy均通过，见 `m6-review-clippy.log`、`m6-cloud-init-clippy.log`，fmt/diff check通过。不重复已完成的118项回归或685项默认基线。

待配置授权及新任务继承后，初始化命令为（此处仅文档，未执行）：

```bash
cargo run -p mcp-vault-eval --offline --locked --bin init-m6-cloud-provider -- \
  --initialize-authorized-m6-provider /workspace/scratch/m6-cloud-provider-config-NEW
```

父目录须已存在且无符号链接，最终目录必须不存在。普通 shell 先加载 `/home/agent/.cargo/env`，Cargo缓存需使用该任务实际可写且已备好的目录。命令输出的 `source_database_path`、`source_master_key_path`、`source_vault_slug` 直接用于现有 prepare草稿；另填冻结来源/任务、全新run根、`external_request_budget=160`、`generation_token_limit=32768`、`provider_timeout_seconds=600`。初始化不等于 prepare或付费run授权；三步不自动串联。旧补丁文件不包含后续修复/初始化入口，迁移需以当前HEAD为基线单独提取本轮路径及新增文件，在云端独立工作树核验应用；不直接覆盖Mac既有改动，不携带环境设置两文件、State/密钥/评测输出或构建缓存。

### 2026-10-09 08:09 UTC 指定提交的 Cloud 单轮验收：DNS 阻塞，已停止

- 远端 `fix/m6-cloud-evaluation` 已 fetch，完整提交核验为 `e5a47d643583ec1111e2cb46b126866b33b498d8`，相对 `1861dfd37b611ac7703c3c14021a58c9d014de63` 恰为25文件。使用 detached HEAD，未移动 main、未 push/PR/部署。此环境最初工作区干净，没有交接中旧环境的两份未提交设置改动，`scripts/setup-dev.sh` 不存在；没有重建或覆盖它们。
- `MIMO_API_KEY` 只作存在性布尔检查，结果 true；未输出值、片段、长度或哈希，未访问 Mac。复用 Rust/Cargo1.94、Node24.19、pnpm11.19，未重装。默认 workspace 685项、后续定向回归及代码审查沿用上述已验证证据，不冒充本环境重跑。
- 三个入口构建成功；使用显式初始化入口创建私有安全占位配置，随后准备唯一新根 `/workspace/scratch/m6-cloud-e5a47d6-20261009/run-01`。初始化和 prepare 均零 Provider 请求。冻结 candidate/review/pre-run记录三个hash不变，两个 validator PASS；旧 `generate_holdout_review.py --check` 因固定历史HEAD断言失败，未重生成或修改已冻结证据包。
- 离线 preflight PASS：30来源、60任务、30 holdout/90 answers，三臂来源字节匹配、每份State30文件、生成表为空、完整性/外键检查通过、权限和seal通过。主要调用31 observation batches +90 answers +6 relations=127；23个source-arm再生成余量及10个未实现调用余量后上界160。官方MiMo/mimo-v2.6-flash，输出上限32768、并发1、transport retries0，配置均冻结。
- TLS握手诊断（无凭据、无应用HTTP请求）证明当前WebPKI-only为UnknownIssuer，增加环境已有CA后成功。以现有Cargo特性重建：`cargo build --offline --locked -p mcp-vault-eval -p mcp-vault-providers --features reqwest/rustls-tls-native-roots --bins`。`CARGO_HOME` 与 `CARGO_TARGET_DIR` 分别指向同一scratch根下 `cargo-home` 和 `target`，`CARGO_BUILD_JOBS=3`。只选择已有合法信任功能，未改网络权限、系统证书文件、TLS验证或产品源码。必须同时选中providers包；只选eval的reqwest为dev依赖，不足以改变runner。
- 依本轮父任务转述的明确数据/费用授权，仅调用一次完整runner，2026-10-09T08:07:52.713137Z启动。观察到连续 `provider_dns_failed` 后主动SIGTERM停止，08:08:49Z退出143；没有自动第二轮或恢复。系统解析官方域名返回 `EAI_AGAIN (-3): Temporary failure in name resolution`。`validated_socket` 在构建授权header和transport budget reserve之前调用本地DNS；代理TLS握手可达不代表此本地DNS前置条件可用，不绕过SSRF/DNS检查。
- 保留原始checkpoint/attempts/usage，不伪造终态：attempt ledger24条started，23条完成item failure全部DNS失败，已测transport attempts=0；最后1条在中断时未完成，结果/用量未知，仍保守消耗预占。checkpoint仍记录running/23，因此外层停止状态另存 `validation/final-status.json`，整体延迟为partial。23个完成调用耗时合计55156ms、最大5014ms；没有观测到成功响应，observations/cards/answers均0，completed_tasks=0。
- M6仍 `not_evaluated`、`human_review=false`。无可评分输出，独立盲评未开始；保留30题/90答案分母，不给质量分。输入/输出tokens与真实费用无可用返回，标unknown，不填零或把24预占称为24次收费HTTP请求。全部特性门禁仍沿用既有ort-sys官方CDN403阻塞记录，未重复触发下载。

本环境证据位于 `/workspace/scratch/m6-cloud-e5a47d6-20261009/validation/`：`build.log`、`build-native-roots.log`、`tls-preflight.log`、`provider-init.json`、`prepare.json`、`offline-preflight.json`、两个validator输出及 `final-status.json`。私有run/provider-config目录含State及安装密钥，不提交或整体分享。阻塞解除需要在既有安全策略下使官方域名的本地DNS验证可用；本记录不授权更改网络权限、绕过验证或另开付费轮次。

### 2026-10-09 Cloud 恢复轮：明确授权与首次基础错误停止保护

父任务转述用户授权 `Sentinel_d853e715783c81919be0ce313c1a9d1c`：允许把已验证的51文件改动推送原 `fix/m6-cloud-evaluation` 分支，并仅启动一个全新M6 run。原总预算160扣除旧run保守预占24，恢复轮硬上限136（包括任何带认证探测），官方MiMo `mimo-v2.6-flash`、每次输出32768、并发1、transport retries=0。只发送必要ADR/query；gold和评分资料不得进入Provider请求。基础网络/鉴权/证书错误首次即停，不自动追加下一轮。

- [x] 51文件与已验证补丁逐字节一致；仅代码、测试、文档、迁移及公开合成TLS fixture，无真实State/Auth、缓存或原始运行数据。已提交并推送 `a1c1a83b97d2bb02623f1e0c48daf476d380d399`，远端SHA一致；未强推或修改main、PR、部署。
- [x] 一次执行连接短暂断开后先核实状态：环境已恢复、无评测进程、未创建新run、新增带认证请求0；未重复启动。
- [x] 原runner只把鉴权/配置错误作为全局失败，连接/DNS/TLS映射可被当作局部失败而继续预约。启动前补齐共享基础故障分类，含请求/连接/响应超时或中断、流超时及HTTP服务错误；模型schema局部恢复规则保持不变。41项runner回归通过，其中新增14错误码×3阶段的42条合成路径，断言首次错误后无额外调用/预约/attempt记录。独立只读复核通过。
- [x] 无凭据预检：使用现有平台CA完成一次TLS握手；无凭据、无origin应用HTTP请求。保留证书/主机名验证，未改持久信任或网络设置；此结果不冒充真实模型调用成功。
- [x] 新schema/native-roots CLI构建 exit 0；补充停止保护已提交并推送 `a0f4c51334935a570d23cc7c4dc7135a49838ffe`，远端SHA一致。Eval all-targets Clippy、fmt/diff通过；冻结candidate及pre-run gold记录validator通过，原字节/pins不变。
- [ ] fresh prepare被现有预算准入拒绝，未封存或执行：完整协议计算上界160，高于授权136。31 observation batches +90 answers +6 relations为主流程127；另计23个source-arm再生成额度、6个relation retry额度、4个answer retry额度，共160。不能以主流程127冒充完整最坏上界，也不能静默把配置抬至160或放宽准入。
- [ ] 唯一恢复轮尚未启动；新增带认证请求0、成功响应0、无评分输出，M6仍not_evaluated。需父任务协调决定：另加24次恢复轮额度（与旧24预占合计184），或明确修改并验证再生成/准入策略以保留136硬上限；当前授权不作这两项推断。旧run保持原样。

新私有根 `/workspace/scratch/m6-cloud-e5a47d6-20261009/run-02` 保留未封存的离线准备状态；没有live-config、seal、claim、attempt ledger或Provider输出。一次先于创建空0700根的prepare检查因路径不存在退出，此时未访问Provider State；创建同一新根后离线prepare在预算检查退出。两次均不是live runner启动，未发送Provider请求，未创建下一轮。详细安全状态及计数保存在 `validation/recovery-blocked-status.json`，失败原文在 `validation/recovery-prepare-stderr.log`。现有预算136未被消耗；旧run真实用量/费用未知，不将合计费用写为零。

本节离线证据为同一scratch根 `validation/recovery-fail-fast-tests.log`、`recovery-fail-fast-clippy.log`、`recovery-network-preflight.log`。全部特性仍受既有官方ort-sys CDN403阻塞，不改写为通过；代码/合成测试不代替M6质量验收。

### 2026-10-09 Cloud 唯一恢复轮完成：M6 未通过

本节取代上一节的“136额度阻塞、未启动”当前状态，保留旧记录作为时间线。父任务转述新的明确授权 `Sentinel_3e82b57823808191a41c821b39046100`：恢复轮最多160次，旧轮24次保守预占不返还，累计上限184；单次输出32768、并发1、transport retries=0，不自动下一轮。旧未封存136草稿移入独立准备归档，仍使用从未启动的同一 `run-02` 身份重新准备；没有重用旧付费根或修改冻结gold。

- [x] 在提交 `7fae5fd39574efec28dff5388d8f04918ef0ea45` 上封存并通过离线准入：30来源、60任务、30 holdout/90答案、State44、完整性与权限通过；冻结三份证据hash和30份来源字节不变。无凭据TLS检查通过。
- [x] 仅启动一次：09:47:35.429839–10:12:14.700400 UTC，墙钟1479.270561秒。PID70819正常退出0，但结果与checkpoint均为 `failed`，首个汇总错误 `evaluation_items_failed`；进程退出0不能当验收成功。
- [x] 完成30/30题、90/90答案，预约与已测transport calls均131：observation40、relation1、answer90。旧24保守预占加本轮131共155/184；未使用额度不授权新轮。没有基础网络、鉴权或TLS失败，没有传输重试；协议内来源再生成计入上述40和总预算。
- [x] 来源提交14/23份（B5/8、C9/15）；9份以 `semantic_flat_claim_unknown_field` 拒绝。另有 `T-S23-1` 的关系动作以 `semantic_organization_scope_not_different` 拒绝。10个item failures保留，未删除任务或降低分母。模型自报答案 supported 为A0/B5/C11，insufficient为A30/B25/C19；自报状态不是独立正确率。
- [x] 本轮返回输入296558、输出68908 tokens；A11426/2398、B121475/26811、C163657/39699。Provider调用耗时合计1409644ms、最大97425ms；此范围不含检索、组包和独立评审。费用字段unknown，旧轮用量与账单unknown；不填零、不把输出授权上界冒充总费用。
- [x] 独立代码审计确认评测证据缺陷：A只持久化coverage/reasons/events，没有发送payload/hash；B/C adapter添加的 `evidence_excerpts` 在 `project_pack_projection` 反序列化为不含该字段的 `MemoryPack` 时被删除，再被用于实际请求。60份pack、463条目全部无正文excerpt；引用ID和语义断言仍在。删除前计入的estimated_tokens仍保留，不能当实际输入用量。
- [x] A的原运行30条coverage全部零命中且无索引降级；独立只读重建两次完全一致，9张普通索引表摘要不变、30来源hash相符。路径使用Lexical模式，将完整问句逐词AND；索引完整不代表问题能召回。重建只是参考，不冒充发送时完整payload。该弱基线及证据缺陷限制本轮相对收益结论。
- [x] 运行结束后修正审核器自身的schema矛盾：`task_result`须有四个terminal维度与嵌套evidence；旧实现仅接受四维。新增验证同时核对嵌套来源hash/行界，不放宽gold、分母或质量门槛。17项Python测试通过，独立增量代码审查通过；没有改变已运行的二进制、配置或输出。
- [x] 独立盲评已锁定，2026-10-11合并既有引用勘误并通过记录校验；简要结果见下节。固定 `human_review=false`，保留来源缺项及全部90答案；历史范围丢失、引用缺限定和无依据推断仍是失败证据。
- [ ] M6验收未通过。全部特性工程门禁还受既有ort-sys官方CDN403阻塞，默认workspace702测试和前端42通过/10既有skip不替代全部特性门禁。

本轮原始证据保持不变，位置 `/workspace/scratch/m6-cloud-e5a47d6-20261009/run-02/artifacts/`；安全汇总为 `validation/recovery-outcome-summary.json`，终态为 `validation/recovery-live-finished.json`。独立输入审计与只读重建在 `validation/ordinary-reconstruction-20261009T101131Z/`；匿名盲评与锁分文件在 `review-blind/`，私有映射不交给评审员。`observations.jsonl` 的batch输出投影为空对象，盲评所用规范observation由成功集合的只读State查询导出；不据空投影虚构模型未返回内容。

父任务最新指示将“报告提交”限定为用户可读报告，禁止未经额外批准新增Git推送或PR。本节和审核器修复仅保留在云工作区，不推送、不部署；不会因剩余29次累计额度自动调用Provider。后续最小修复顺序是先补齐安全的实际输入留存和正文投影契约，再用独立合成fixture验证问句检索、格式严格拒绝及关系作用域，最后处理历史范围、完整证据和无答案表达；冻结holdout已见，不能用本轮样本迭代后仍称未见验收。


### 2026-10-11 原云工作区离线修复与回放

父任务转述用户“继续吧，今天有重置”，只恢复 Codex 工作额度，不扩大 MiMo 授权。沿用 HEAD `7fae5fd39574efec28dff5388d8f04918ef0ea45` 及原工作区；没有新建环境、付费调用、推送、PR或部署，没有读取或变更持久凭据。恢复后确认无遗留评测进程，原运行产物 hash 在修复后仍全部一致。

- [x] 修复 A80 时间字段契约：新 prompt v15/schema v11 明确嵌套字段白名单，并把时间 evidence indices 限定到当前批次；非法 advisory 枚举仍按既有规则回退，不降低核心或证据检查。保留历史 v14/v10 MiMo wire 行为。新的私有 observation 产物保留脱敏 claims，避免再次丢掉本地拒绝诊断。
- [x] 修复实际输入留存：A 在派发前持久化白名单输入与 hash，命中须绑定冻结来源；B/C 使用评测专用类型保留授权 evidence excerpts，并核对引用、来源修订、路径、span 长度及内容 hash。最终 pack 含正文与 hash 后检查字节和 token 预算，生产 MemoryPack DTO 不变。
- [x] 新 `index-lexical-recall-v2` 复用现有 relaxed lexical retrieval 与 relevance admission，不初始化 Provider、不启用语义命中；旧 `index-frozen-v1` 保持原语义。自然问句、无关查询、来源修订、实际派发 payload/留存一致性和正文保留均有离线回归覆盖。
- [x] 126项定向 Rust 测试通过：Eval lib62、应用边界4、live runner42、本地HTTP假Provider16、A80 normalizer2。Eval all-targets Clippy `-D warnings`、fmt及diff检查通过。既有17项Python测试不重复运行；合并后的盲评记录重新通过 validator。全部特性门禁仍受既有 ort-sys 官方 CDN403 阻塞，未无变化重试；不重跑无关前端。
- [x] 使用原30个冻结查询和原基线 State 作一次只读诊断回放：新配置26题有命中、共31条来源，旧配置30题均零命中；9张普通索引表摘要不变，Provider未初始化、调用0。没有选择 gold 作为输入、没有模型重答；该回放不是原请求复现、答案质量测量或未见 holdout。
- [x] 合并此前11份锁定评审/补充记录，含 task-group-3 的引用类型勘误；保持90答案、118份产物及全部冻结分母，`human_review=false`、`record_status=insufficient_evidence`、`m6_acceptance=not_passed`。最后锁分时间2026-10-09T10:30:47.882879Z，协调者在2026-10-11T00:43:46.319179Z开始映射汇总；独立评审员未取得映射。

| 原运行指标 | A | B | C |
|---|---:|---:|---:|
| 完整限定保留 | 3/49 | 13/49 | 16/49 |
| 完整可用信息覆盖 | 0/23 | 5/23 | 6/23 |
| 无答案题明确缺口且无虚构当前结论 | 6/7 | 6/7 | 7/7 |
| 四维任务结果 met / 总维度 | 66/120（另1项证据不足） | 76/120 | 76/120 |

原 observation/card 的来源领域事实支持率为 B289/379、C486/677；两层复用事实，不视为独立样本。覆盖、限定、关键约束和关系覆盖0/6均不足；A零命中与B/C原文投影缺失又限制相对收益解释，不据分数宣称M2改进或验收通过。9次未知字段拒绝的原始 claims 已丢失，不能断言它们全由同一个嵌套字段触发；旧A实际输入也无法精确恢复。当前离线修复不追溯改变原运行质量结论，不自动第二轮。真正模型质量、关系作用域及历史状态保留仍待后续工作，新的真实验证须另获明确授权。

证据仍在同一 scratch 根：`validation/oct11-offline-fix-summary.json`、`validation/oct11-eval-*.log`、`validation/oct11-a80-normalizer.log`、`validation/oct11-offline-replay/validation-summary.json`、`validation/recovery-post-run-agent-review.json`、`validation/recovery-quality-summary.json`。改动仅留在工作树，完整补丁为 `validation/oct11-offline-fixes.patch`。
