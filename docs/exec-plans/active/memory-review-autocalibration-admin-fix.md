# ExecPlan：修复记忆检索、自动补校准和 Admin 操作闭环

## 1. Title and status｜标题与状态

- 仓库：`cyt-666/mcp-vault`。
- 建议仓库路径：`docs/exec-plans/active/memory-review-autocalibration-admin-fix.md`。
- 编写日期：2026-09-05；最后更新：2026-09-06。
- 执行者：Codex。
- 编写基线：`92d9367949570ccd3c19ece8895cdd8f00cc5e78`，版本 `0.2.1`，提交说明 `feat: simplify memory pipeline and release 0.2.1`。本次重新查询 GitHub main，返回的仍是此提交。
- 状态：**本地工程实现与可执行回归已交付；宿主 all-features／外部互操作及真实模型质量验收 pending，保留 active。**
- 替代关系：本文件是本轮修复的独立执行依据，整合 92d9367 Review、部署抽查、Admin 缺口和自动校准讨论。不要重新执行旧版架构迁移计划，也不要恢复已取消的全局 consolidation 或记忆历史状态机。

执行者无需读取聊天记录。先核对实际 HEAD 与未提交修改：基线仅用于定位，不要求 checkout/reset 到旧提交。已经修复的问题用测试证明后记录为“当前 HEAD 已修复”，不能倒退用户代码。按 M0–M7 实施，不以再写一份计划代替实施。

### 当前部署必须覆盖的起始状态

用户已经部署 0.2.1，配置了 embedding 模型并生成了向量，也重新生成了记忆。当前卡点是没有有效校准记录，而不是尚未安装模型或尚未生成全部向量。

**升级后的首要验收：不重新绑定模型、不重新生成记忆、不重建现有有效向量、不打开 Admin 页面、不手工提交校准 API，服务启动后自动发现缺校准并运行后台校准；通过评测后后续 recall 自动使用语义检索。** 该流程须在既有 Provider 授权和明确的运行预算内完成。

自动执行不等于一定评测通过。模型不适配、调用失败或校准样本无法支持可靠阈值时，报告真实原因并提供重试／诊断；禁止伪造通过指标或只去掉警告。

## 2. Purpose and user-visible result｜目标与产品行为

本期完成以下三个闭环，而不是只补 API 或只改提示词：

1. **查询闭环**：所有候选经过同一请求资格检查；memory 和 related_notes 都执行相关性准入；对象聚合、排序、章节定位和输出预算正确；已删除内容不能通过任何模型读取入口重新出现。
2. **自动准备闭环**：现有部署升级、新模型绑定、关键配置变化都能自动触发有界校准；无需管理员制作标注集、填写阈值、Recall@5 或报告哈希；界面能区分向量覆盖和语义检索可用。
3. **管理闭环**：迁移预检／执行、校准状态／运行、暂停来源恢复、显式记忆编辑，都具备可操作的页面、真实 API 调用和端到端测试。

继续保留已经确定的简单记忆模型：

- 一篇来源文档拥有一组当前派生记忆；成功重生成整组替换，不维护模型可读历史。
- 显式记忆独立保存，更新保留未提交的元数据。
- 删除从当前记忆域退出；来源暂停只能由明确恢复操作解除。
- 一次提取返回 `memories[]`，模型只提炼内容和可选类型／标签，不管理 ID、权限、来源身份或状态变化。
- 有价值技术知识可以提取，但不能冒充用户已经掌握／采用；保留个人进度、环境、时间、实验范围和否定条件。

## 3. Governing requirements｜规范与证据

实施前阅读以下文档的相关完整章节，并把实际适用的标题／锚点记入 M0。业务新决策先补 ADR 与规范，再同步代码；不能让旧文档迫使实现回退。

| 来源 | 本轮关注位置 |
|---|---|
| `AGENTS.md` | Vault 隔离、Markdown 权威、协议与服务边界、Provider 调用、安全写入、测试要求。文件中仍有旧 lifecycle/consolidation 描述，须与 ADR-0026 对齐，而非恢复旧架构。 |
| `PLANS.md` | Required plan sections、Plan quality rules；维护 Progress、Decisions、验证结果和恢复说明。 |
| `docs/adr/0026-current-source-owned-memory-sets.md` | Decision、Migration and recovery、Rejected alternatives；保持单来源集合、真实删除及显式元数据语义。 |
| `docs/memory-system.md` | §3 Source-owned extraction、§4 Explicit remember and update、§5 Delete and pause、§6 Recall、§7 Embedding freshness、§8 Migration。 |
| `docs/adr/0023-preserve-source-language-and-persist-multilingual-retrieval-metadata.md` | 与 ADR-0026 一致的来源语言、派生检索别名原则。 |
| `docs/adr/0024-bound-embedding-inputs-and-rebuild-current-model-vectors.md`；`0025-aggregate-note-vector-chunks-before-ranking.md` | 输入有界、向量新鲜度、先按对象聚合；不得因本轮修排名而无端全库重建。 |
| `docs/product-requirements.md`、`docs/architecture.md`、`docs/interfaces.md`、`docs/data-model.md`、`docs/security.md` | M0 定位实际记忆、Admin、后台任务、Provider 数据发送、预算与迁移条款；更新对应章节。 |
| Review：`code-review-92d9367.md` | R1–R6 和语义校准／生成质量两项缺口；本计划 §4 已完整复述，不强制额外附件。 |

新增一份当前未占用编号的 ADR，记录“有界自动基准校准＋启动补偿＋双通道准入＋Admin 闭环”。仅修订校准运维方式，不建立另一套记忆生命周期。

证据边界：既有 Review 为固定 SHA 的静态审查，附带从源码摘取的 SQL／公式复现，不是 Rust 项目测试。部署抽查发现的语言／遗漏问题也不是提示词根因的完整证明。本计划没有重新运行生产查询、浏览器 E2E、仓库编译或真实模型评测。

## 4. Current repository state｜问题清单和修改导航

### 4.1 六项确定的 Review 问题

| ID | 优先级 | 当前行为与位置 | 本轮修复目标 |
|---|---|---|---|
| R1 | P1 | `crates/memory/src/service.rs::recall()` 给 FTS/context 传 `CurrentMemoryFilter`；语义命中只 `current_memory().get()`。`crates/state/src/current_memory.rs::get()` 只做 current/freshness，不做 `append_filter()` 中的 kind、valid_at、min_importance。 | 所有通道执行同一请求过滤，语义启用后也不能返回过期、错误类型或不满足重要性的记录。 |
| R2 | P1 | `recall()` 将 `IndexService::retrieve_notes(Hybrid)` 结果仅做预算后放入 related_notes。`semantic_note_rank_score()` 接受所有非负 cosine。 | 普通 search 可以返回标明性质的候选，但 recall 的笔记结果必须经过相关性准入；不能只让 memories 为空。 |
| R3 | P1 | `forget()` 的 note-derived 分支要求 `old_set.provider_id/model_id` 存在。合法 Markdown 重建却允许清空已不存在的 Provider/Model。 | 无原模型配置也能本地删除、暂停并恢复发布；不调用模型，不降低修订／来源校验。 |
| R4 | P2 | context-only 候选仅得 `0.08/(rank+1)`，随后被 `score.total < 0.18` 全部丢弃。 | “可返回”与“如何排名”分离；相关上下文候选能补充 FTS 窗口，不能靠降低统一阈值放松所有结果。 |
| R5 | P2 | memory 语义循环先对块 enumerate，后按 memory_id 去重，仍用原块名次计分。 | 有效唯一对象才占名次；前面对象的重复／失效块不影响另一个对象的排名贡献。 |
| R6 | P2 | `estimate_note_tokens()` 只计 snippet 和 `min(heading_count,32)*8`，实际却返回完整 headings/title/path/tags 等。 | 按最终实际返回对象统一估算；超大首条不例外，跳过后继续寻找合格短结果。 |

基线位置约为：memory `recall` 1320–1690、`forget` 1090–1310、评分与预算 3335 附近；state `get` 334–352、eligibility/filter 1640–1728；indexer `retrieve_notes` 1065–1165、语义候选 1380–1610、语义评分 1922 附近。以符号为准，不依赖修复后旧行号。

### 4.2 自动校准和生成质量缺口

| ID | 当前行为 | 修复要求 |
|---|---|---|
| C1 | `semantic_calibration()` 读设置；`set_semantic_calibration()` 验证并保存调用方已算好的报告。没有完整自动评测／发布链路。 | 服务端真正计算并发布，不能把 PUT 包装成“运行”按钮。 |
| C2 | 缺校准就 `break 'semantic_admission`，查询 embedding 都不调用。 | 保留未准备完成时的明确降级，但必须自动补齐，尤其覆盖已有模型和向量的升级环境。 |
| C3 | `quality_eval.rs` 只支持 deterministic；其 recall 关闭 related_notes。 | 抽离可复用评测引擎，真实 Provider 校准由 worker 执行；测试覆盖两通道和真正纯语义命中。 |
| G1 | 原始 Markdown 被完整送入提取，512 KiB 是报错边界而非静默截取；提示词未明确来源语言及进度／环境／下一阶段检查。 | 加内容选择规则及正反例，保留全文和一次提取；不要把已知约 15 KiB 案例误诊为大小截断。 |
| O1 | 分数尺度不同，部分字段名暗示 RRF 却含混合加权；retrieval profile hash 不能准确解释实际校准状态；section 输出缺少可靠匹配章节。 | 版本化评分说明与实际诊断，章节定位独立返回；不能用“分数变大”作为质量证据。 |

### 4.3 五个已存在但没有页面闭环的接口

基线后端路由和 handlers 位于 `crates/admin-api/src/lib.rs`；前端在 `frontend/admin/src/App.tsx::loadPage()` 与 `pages.tsx::MemoryPage`。

| ID | 已有接口 | 实际缺口 |
|---|---|---|
| U1 | `POST /memory/migration/preflight` | 无预检按钮、报告展示。 |
| U2 | `POST /memory/migration/execute` | 无指纹／确认串流程、结果及未解决项展示。 |
| U3 | `GET/PUT /memory/semantic-calibration` | 页面不取状态，也没有操作；PUT 实际是保存外部报告，不是执行校准。 |
| U4 | `POST /memory/extraction/sources/{file_id}/resume` | 删除会暂停来源，但页面没有恢复入口。还需能查询空集合的来源及最新 set_revision。 |
| U5 | `PATCH /memories/{id}` | 页面只有创建／删除，缺显式记忆编辑。 |

`loadPage('memory')` 仅加载 memories、extraction、embeddings、jobs overview。向量覆盖成功提示不能代表校准完成。所有下文新增接口均是待实现设计，不是当前已有功能。

## 5. Scope / invariants｜范围、不变量和授权

### 必须做

完成 R1–R6、C1–C3、U1–U5、G1 和必要 O1；交付自动升级补偿、真正计算的校准、页面、升级与恢复测试。不是只改旧六条 Review，也不是只把截图中的 URL 挂到按钮。

### 不做

不换 embedding 模型，不增加向量数据库、知识图谱、独立校准微服务或必需 reranker；不恢复归档／历史读取／全局合并；不要求用户手工标注自己的 Vault 才能使用；不自动迁移／清理旧知识，不自动恢复用户暂停的来源，不自动全库重新提取。

### 必须保持

- `VaultContext`、权限和来源 current-only 校验应用于内容、ID、计数、诊断、缓存、任务、报告；没有 `vault:read` 时不泄露笔记元数据或是否命中。
- SQL 在 state repository；模型调用经既有 ProviderService；规范文件写入经 Vault Core；Admin 和 MCP 仅做协议转换。
- 规范 Markdown 仍为知识的可重建依据。校准报告、任务和评测向量属于派生／运行状态，不写进用户笔记或当前记忆。
- 所有当前模型读取入口维持删除语义，旧 ID、history 参数、资源 URI、缓存和向量都不能绕回已删除内容。
- 本轮不提高 `MEMORY_CONTRACT_GENERATION` 来触发清理，不改旧已发布 migration。快照兼容、校准策略、评分规则分别版本化。
- 不能把结构化输出合法、source hash 相同或向量存在当成“语义真实／质量已验证”。
- 只添加所需的后台任务状态，复用既有 jobs/outbox，不为每条记忆增加新生命周期。

### 运行自动化与本次开发授权分开

产品在已绑定模型、ProviderMode 允许请求、没有显式禁用自动维护且预算允许时，默认自动做小规模基准校准。继承既有本地／远程地址、凭据、网络和并发策略；不得绕过 ProviderMode=Disabled。新增自动 embedding 消耗在发行说明和 AI 设置中说明，可暂停，但不能新增一个默认关闭、需要旧用户手工开启的隐藏校准开关。

默认校准只向已授权 Provider 发送随版本提供的非私人评测文本；不自动把完整 Vault 外发或调用生成模型。业务向量已经有效时不重算。扩展到私人语料的额外生成／人工评测不属于默认自动流程。

**Codex 在本次实施中不允许自行使用生产凭据、调用收费模型、迁移／重新提取真实 Vault、push 或部署。** 开发验证默认用隔离 Vault 和本地 FakeProvider；真实 Provider 质量验收仅在另行授权的配置中运行。缺真实凭据不妨碍把真实 worker/Provider 执行能力实现完整，不能留下 `todo!` 或仅支持 fake 的生产路径。

## 6. Proposed design A｜修复查询、删除和预算

### 6.1 统一资格检查 → 相关性准入 → 对象排名

把当前混合步骤整理为同一条数据流，不引入无必要的通用框架：

```text
鉴权与 Vault 限制
→ 生成 FTS / context / vector 候选
→ 当前内容、来源、规范修订、输入 profile/hash 校验
→ 请求的 kind / valid_at / min_importance 等过滤
→ 以语义或强词法证据决定 admitted
→ 最佳有效块聚合、对象级排名融合和有界 boost
→ 去重展示、输出预算、最终 current/revision 再验证
```

1. 在 state 增加带 `CurrentMemoryFilter` 的批量读取或等价仓库方法，复用 `append_filter()`。`get()` 的 current-only 语义不应被偷偷改成“永远使用当前时间”，因为详情读取与 recall 的时间查询是不同契约。
2. 候选加分前统一应用请求条件，输出前做有界再次验证。若候选获取与输出间发生修改，放弃旧快照；无变化的合法语义命中必须仍然返回。记录快照式一致性边界，不宣称可以撤回已在途响应。
3. 引入内部 `admitted/reason` 与 score 分离。通过 query evidence 的 context-only 候选不能因缺少某个分数组件再次被 `0.18` 拦截；只有 context 相同但与查询无关的内容仍然拒绝。
4. 先过滤并聚合 `memory_id → best_valid_chunk`，再计算对象名次。重复块、失效块、被权限／请求条件排除的对象不得占对象排名；限定候选池不能静默宣称全库搜索完毕。
5. 对 related_notes 走同等步骤。可以增加内部 `retrieval purpose=search_candidate/recall_context`，或抽出候选收集函数；不擅改公开精确 lexical AND/OR 契约。校准 raw-candidate 功能只可被内部评测服务调用，MCP 参数不能打开 bypass。
6. 两通道分别校准；不同角色即使同模型，正文长度／分块也不同，不能直接套同一个 min_cosine。相同输入的 embedding 缓存可以复用，但阈值与结果池分开。

在已有正常化基础上调整中文与长问句，保留完整代码标识符、缩写和限制条件。不能只靠低覆盖率阈值或词尾启发式去“修”跨语言；有已验证语义能力后必须能命中没有字面重合的表达。常见 bigram、recency、importance 均不能独立放行。

### 6.2 评分与观测

本轮不再为让数字好看而统一乘数或重写融合算法。保留 `score` 为排序分，不当概率；在保持单通道合理基线的前提下只修 R4/R5 等确定缺陷。对现有非传统 RRF 贡献补准确说明，并给新诊断用无歧义的 `*_contribution` 命名；旧字段暂作兼容别名但不可重复计分。

诊断最少包含：原始 cosine（实际未算则省略/null）、原始 BM25、各路贡献、有效对象名次、相关性通过／拒绝原因、实际 embedding profile、校准记录标识、策略版本。响应级分别给权限范围内的 candidate/eligible/admitted/returned counts 和 coverage／budget 原因。

`retrieval_profile_hash` 应反映实际生效的请求策略和两通道配置，不能硬编码为“uncalibrated”。**检索签名改变不等于 embedding 输入改变**：仅修排序、预算或校准，不应让现有全部向量过期。语义活跃状态与向量覆盖是独立字段。

### 6.3 删除不再依赖原 Provider/Model

对 `MemoryNoteSetSnapshotRecord` 增加能兼容旧记录的本地变更种类，或让模型元数据可选并做针对操作的条件校验。最低要求：

- 生成快照保留真实调用元数据；本地删除快照不需要原模型记录，不伪造 ID、不选一个替代 Provider。
- 删除继续准备完整剩余集合，进行 source/File ID、set_revision、规范修订与快照哈希检查，安全发布后暂停来源。
- schema／序列化／Markdown／重建／state publish 全链路兼容已有 prepared/applied 快照；旧生成快照仍可恢复。
- 删除一条不能破坏其他条目的正文、时间和可选元数据；未变项的有效向量应保留，不因删除一条就把整个集合的向量删掉重算。删除最后一条时保留可管理的空集合与暂停信息，不把它变成历史记忆。
- 自动校准和向量任务必须尊重 source resolver；晚到的旧任务不能写回可读的被删知识。
- 无原 Provider/Model 时删除、投影恢复均为零模型调用；程序中断后重放不复活内容。

### 6.4 预算与章节定位

先构造最终的、已经有界的 `MemoryView/RelatedNoteView`，再通过同一估算器计算实际序列化字段的费用。覆盖正文、snippet、source、title、path、tags、entities、heading 文本、score diagnostics 和响应包装；保留明确硬字节上限作为补充，不承诺 byte/4 精确等于模型 token。

两通道共享总预算，未用份额可互借；超大首条不能豁免，跳过后继续扫描后续合格候选直到对象／时间／预算上限。分页和 `available_*` 基于准入后的有界对象池；诊断过大时也须有界，不把 max_tokens=128 当作必须返回知识。若预算连最小合法响应包装都容纳不下，按公开契约返回稳定的预算错误，不声称该请求已满足内容预算。

保留获胜 chunk 的稳定标识、源修订和对应 heading path／源范围。`result_granularity=section` 需要返回可识别的匹配 section，不能只改一个字符串却返回整篇目录；snippet 来自匹配块，而不是优先保留不相关的词法片段。

本轮优先通过派生章节定位投影与响应 DTO 修复，不改变已经生成向量的文本规则。若确实需要修改 embedding 输入，单列依据、受影响块和增量重建方案；不能把解决“缺校准”的前提重新变成全库 embedding。

## 7. Proposed design B｜真正可用的自动校准与现有部署补偿

### 7.1 自动执行的是评测与发布，不是填表

新增内部 `ensure_retrieval_calibration()`（名称可按项目风格调整），服务端负责检查、去重和排队。由现有 worker 执行共享评测引擎：获取真实 embedding、收集合法原始候选、选择阈值、运行留出验证、持久化完整结果、比较配置并发布。

不得用当前要求 active calibration 的 public recall 来完成“首次校准”，否则永远不会产生语义候选。评测必须复用同一份检索／过滤／聚合／打分实现，通过仅内部可构造的 evaluation context 临时指定候选阈值；它只绕过“必须先有已发布校准”这一个前置，不绕过 Vault、权限、current-only、请求过滤、Provider 策略或输入上限。

**具备 embedding 模型即可完成默认校准，不新增生成模型依赖。** 不让在线 LLM 自己标注所有正负例并把自评当真值。

### 7.2 默认样本：随版本交付的带标注基准

本期采用可实现的冷启动方案，而不是要求用户提供真实 Vault 的完整答案标注：

1. 随二进制／发行包内置非私人的 Markdown、短记忆、查询和显式相关性标签，记录来源和基准版本。构造样例可为合成事实，但答案必须由样例原文直接支持；开发时审阅并加入一致性测试，不能声称未发生的人工审阅已完成。
2. 至少 120 个检索问题，校准集和独立验收集各 60 个；每份至少 40 个有答案和 20 个无答案。以不同源主题／来源划分，避免同一问题微小改写跨集合。可以先扩展现有 `tests/fixtures/memory-quality`，运行时按无私密文本的同版本资源打包。
3. 同时覆盖 memory 与 note 的实际输入形态，包含中问中、英问英、中问英、英问中、短缩写、代码标识符、长问题、多事项问题、否定／版本限制、难负例和完全库外问题。纯语义子集不能靠共享标签或词面命中过关。
4. 无答案标签必须针对该评测语料整体核实；不能因为“不是这一条的来源”就把另一条内容自动标为负例。
5. 对 memory/note 分别按当前真实输入规则生成基准向量和查询向量；同一 profile 的完全相同输入缓存复用。评测使用隔离的临时索引／数据库，不在真实 Vault 中创建评测笔记或记忆，也不影响 recall_count。
6. 缓存和评测工作区仍由 Vault＋profile＋基准版本隔离，不通过复制真实密钥或创建可被用户选择的伪 Vault 绕过授权。使用 ProviderService 的原授权上下文执行，仅把合成文本放在内部评测语料中。

**复用现有向量的含义：不重建已经有效的业务向量。** 首次校准可能还需计算有限的测试查询／基准文档向量，不能承诺零新增 embedding 请求。默认不需要重新提取用户记忆，也不把已有业务向量当作有质量标签的验证集。

结果必须注明 `evaluation_scope=builtin_benchmark`、真实 Provider/model/profile、样本数和失败例。基准通过可支持启用该实现的基础语义准入，**不等于用户全库的检索正确率已达到相同数字**。语料变化和长文最大相似度偏差仍需实际回归观察；默认不以抽样未知真实文档构造假负例。

### 7.3 阈值选择与指标

先校验 Provider 返回数量／索引映射、维度、有限值与向量模长。缺失、NaN/Inf、零向量和请求失败都不是“正常空结果”，不能让无答案样本因此得到正确分；不能把失败样本静默移出统计分母，整轮失败或按明确规则记录未完成。

在 calibration split 上选择每个通道的准入参数；阈值候选由有效原始相似度的排序值／相邻分界产生，处理边界包含关系和确定性并列。冻结参数后仅在 holdout 上验证，不反复调整直到 holdout 恰好通过。

评测整条检索链路，包括词法/context 候选、语义候选、R1 请求条件、R2 准入、对象聚合和固定 K=5，而不是只测“查询与答案”的孤立一对向量。合并通道还需整体无答案测试，防止笔记旁路。

指标定义：

- 有答案 `Recall@5`：每个查询前 5 返回对象中的标注相关对象数／该查询相关对象数，再做宏平均，不把 Hit@5 改名 Recall@5。
- `returned_precision@5`：前 5 个实际返回对象中相关的比例；有答案空返回按失败计，不赋值 1。
- `MRR@5` 或 `nDCG@5`：排序质量；报告单独的纯语义与跨语言子集。
- `no_answer_false_return_rate`：无答案查询中返回任何不相关对象的查询数／无答案查询数；分两通道和整体统计。
- 报告原始计数、失败 case、样本量、延迟、embedding 输入量与请求次数；数值不是真实性概率。

本轮激活至少保持基线已经声明的 `Recall@5 >= 0.70`、无答案误返回率 `<= 0.05`，同时要求有答案 returned_precision@5 `>= 0.80`，以及纯语义子集 Recall@5 `>= 0.70`。同一 holdout 上不能低于修复后词法对照的整体 Recall@5；否则定位策略冲突，不用关闭词法隐藏退化。≥0.90 的召回是优化目标，不在没有模型结果时伪装成已达到。

这些是可评测的工程门槛，不保证任意模型都通过；与旧规范不一致的指标在 M0 明确 ADR 和版本，不能实施中偷偷放低。20 个无答案样本的一次错误就是 5%，须显示绝对数量与小样本局限。

不能通过全返回空、关闭语义子集、把基准词写进 production if 分支、人工填写指标或使用 fake 向量报告激活生产。生产校准结果必须由真实 Provider 执行路径产生；测试注入只在测试依赖边界使用并标记，不作为生产“已验证”记录。

### 7.4 启动补偿：这是强制路径

注册 worker、完成必要数据库迁移和 Vault 恢复后，异步分页检查所有当前可用 Vault 的配置。不能放在 Admin 页面 load/GET 内，不能等下一次绑定事件，不能要求先完成旧记忆迁移标记才能处理已经存在的 current 记忆。

```text
for each ready Vault:
  for channel in configured embedding_memory / embedding_note:
    检查 Provider 授权、禁用状态、预算
    计算 effective retrieval signature
    若已有适用且通过的校准 → 无动作、零 Provider 调用
    若该 signature 有活动任务／可恢复 checkpoint → 复用
    若失败仍在退避期／预算耗尽 → 保留明确原因，不重建无限新任务
    否则 → 原子提交一次 calibration job
```

该检查本身只读配置／索引元数据并排队，不能同步调用 Provider 阻塞服务启动。普通文件同步、显式记忆读取和合格词法结果应保持可用。

除启动外，还由以下事件调用同一逻辑：模型角色绑定或关键 Provider/model/input 规则变化、允许 Provider 请求后、Vault 初始化／恢复完成后、必要的向量准备完成后、用户点击重试。低频补偿只扫配置／任务状态，不扫描整个 Vault 正文；利用现有调度机制，避免新增无界轮询。

**已经有全部有效向量＋缺校准时必须直接排队校准。** 缺部分向量时分别显示 coverage 和 calibration 状态；仅在既有索引策略允许时补缺失向量。默认基准不依赖用户库有足够人工标注，不应因“用户没准备训练集”停止自动化。

### 7.5 签名、任务去重、预算与恢复

校准有效性签名至少包括：Vault、channel/role、实际 embedding profile、query/document 输入规则、对象聚合及准入／打分版本、基准版本／标签哈希。检索签名应独立于正文 embedding 输入哈希，避免仅更改校准就让业务向量失效。

存储尽量复用 settings 和现有 jobs：每个 Vault/channel 保留当前启用记录及当前任务／最近结果，必要的派生报告以有界保留策略保存。不要创建持久的“每条记忆校准世代”。

- 用数据库层活动任务唯一性或原子 enqueue 去重，不依赖进程内 mutex。启动、绑定变化、多个页面重试并发时只执行一个有效任务。
- 用户界面传入的重试请求只引用配置／任务身份，不能自报成功指标。当前模型或签名在运行中变化，结果拒绝发布并为最新配置排队；旧任务不能覆盖新配置。
- Provider 响应批次持久化后续跑复用；失败／重启不会重复已经保存的向量计算。无法保证远端已处理但本地未落盘这一窗口零重复计费，应记录可重复批次的上界，不宣传跨网络 exactly-once。
- 显式校准失败不覆盖仍适用的已通过配置；若配置已经不同，旧配置不适用，只能明确降级，不能混用旧模型阈值。
- 建议起始资源上限：每 channel/profile 单轮不超过 512 个不同 embedding 输入、累计 2 MiB 输入、32 次实际 HTTP 请求（含重试）、15 分钟；每 Vault 同时 1 个校准任务，全局至多 2 个。参数属于可配置工程预算，不是质量阈值；M0 根据 Provider 批限制核实，使完整基准在预算内可执行。
- 请求数／字节预算要跨进程重启持久化，包含 Provider 内部重试；不能只数顶层 embed 调用。价格未知时不编造金额，报告计数／字节／已知 tokens。
- 瞬时故障有界退避；质量未通过、鉴权失败和预算耗尽不得无限重试。重试／取消／最后错误复用现有 jobs。
- 禁用自动校准时不能等同关闭仍然有效的语义查询；二者状态分别表达。模型未授权时零网络请求。

### 7.6 发布与兼容

保留现有 `GET /memory/semantic-calibration` 的兼容 memory 状态，并加 channel 信息或可选 channel 参数；具体 schema 在 M0 固定。新增服务端 run/retry 操作，默认自动任务和手动重试必须调用同一个执行器。

现有 `PUT` 可以保留为高级外部结果导入兼容接口，但不得伪装成校准运行接口，也不得成为用户的正常流程。新报告由服务端生成并保存，不让前端重新提交其阈值／分数。旧记录若缺少新签名／验证来源，不伪造缺失证明：保留原数据，标记需要自动复核并排队。

只有评测通过并且发布时签名仍匹配才更新 active 记录；GET、Admin 展示和 recall 使用同一有效性函数。记忆和笔记的状态分别报告，一边通过不掩盖另一边失败。现有 coverage=100% 只表示向量覆盖，不构造语义已就绪假象。

## 8. Proposed design C｜Admin 页面与接口闭环

所有路径均为 Vault 内相对路径，沿用 `/vaults/{slug}` 分发和 `adminApi` 的作用域、Origin/CSRF、认证和错误转换。不能在组件内拼接另一个 Vault 的 ID 或裸 fetch 绕过封装。

### 8.1 接口矩阵（现有与新增区分）

| 业务 | 接口状态 | 页面与服务端要求 |
|---|---|---|
| 迁移预检 | 已有 `POST /memory/migration/preflight` | 展示来源分类、计数、混合／未解决 IDs、preflight_hash、所需确认；预检不改 canonical，审计允许。 |
| 执行迁移 | 已有 `POST /memory/migration/execute` | 提交确认字符串及最新 preflight_hash；处理 409、部分未解决、生成任务和错误；禁止用自动补校准触发迁移。 |
| 校准状态 | 已有 `GET /memory/semantic-calibration` | `loadPage` 加载两通道状态，展示 coverage 与 calibration 分离；读取不能触发收费操作。 |
| 运行／重试校准 | **新增** `POST /memory/semantic-calibration/run`，支持 `channel=memory/note/all` 的有界请求 | 返回 202＋job/复用标识，不阻塞 HTTP 等整轮；worker 自动发布结果。若 schema 更适合 body 则在 M0 固定，但调用两端必须一致。 |
| 旧外部报告导入 | 已有 `PUT /memory/semantic-calibration` | 保留兼容、准确标识来源；不做让用户填写 quality metrics 的默认表单。正常“保存结果”由服务端任务完成。 |
| 来源列表 | **新增** `GET /memory/extraction/sources?paused=true&limit=50&offset=0` | 按 source set 查询，不从可见记忆列表反推；返回 file_id、path、set_revision、paused、当前项数、来源是否可恢复以及 next_offset。空集合也必须可列出。 |
| 恢复来源 | 已有 `POST /memory/extraction/sources/{file_id}/resume` | 用列表返回的最新 expected_set_revision，确认后恢复并排队一次提取；允许恢复最后一条已删的空集合。 |
| 编辑独立记忆 | 已有 `PATCH /memories/{id}` | 只对 explicit 显示编辑；加载并保留原值，提交 expected_revision，只提交有意修改的字段。 |

新增接口名称可按实际路由风格在 M0 收敛，但不是可省略功能。维护一张“用户操作→实际请求→handler→service→UI 测试”的映射；接口列表存在不算完成。

### 8.2 页面行为

**校准面板**：默认是状态／进度／重试／查看结果，不是阈值表单。显示使用哪个模型、哪份基准、何时通过、是否只通过内置基准、失败原因及请求预算。开页只查看；真正初始化由启动补偿负责。一个接口失败不应把整个记忆页面变成无法使用；保留其他已加载功能并显示局部错误。

**迁移面板**：有 legacy 数据时给入口；无迁移需要则清晰说明，不把历史迁移变成每次升级必走步骤。确认后才能执行，preflight 变化要求重新查看；网络超时不能无提示重复整个迁移。成功返回还要区分 unresolved 项，不用绿色“完成”掩盖未解决数据。

**暂停来源面板**：独立于记忆列表存在。删除最后一条后仍能找到路径、集合版本和恢复按钮；来源已删除则禁用恢复并解释，不读旧正文。恢复只能明确操作，重新启动、重生成全部、自动校准不得解除暂停。恢复状态变更与排队须用同一事务／outbox 或持久幂等状态保证，不出现“已恢复但永久没任务”。

**显式记忆编辑**：使用当前后端 patch 的字段名称（创建用 `kind`，patch 基线用 `memory_type`，不能直接复用而丢字段）。测试 omitted/set/clear 的三态反序列化；不把未触碰字段发送为 null。冲突显示刷新／重新确认，不静默覆盖；派生项提示修改原文或明确另存，不开放逐条修改后再隐性继承。

**列表与状态**：至少接通 `next_offset`，不要只显示第一页 50 条却把 `memories.length` 叫全库总数；暂停来源也分页。切换 Vault 时取消／隔离旧请求与操作状态，不能将 A 的结果显示或写入 B。

### 8.3 前端与协议说明

更新 `frontend/admin/src/{App.tsx,pages.tsx,api.ts,view-model.ts,App.test.tsx}` 及必要组件。可以抽出记忆页组件，避免把全部新逻辑堆成更大的 pages.tsx，但不要做无关全站重构。

更新 MCP `tools/list`、资源说明、Admin 文案和错误标签：不再出现 forget 默认归档、remember 需全局整理等旧描述。不要为校准暴露“跳过相关性／来源校验”的 MCP 参数。

## 9. Work breakdown｜按依赖执行的里程碑

### M0 — 固定基线、契约和红色回归

**范围**：`AGENTS.md`、`PLANS.md`、规范／ADR、现有 Review 对应测试和 API 测试。

1. 记录实际 HEAD、工作区、工具版本；读取 §3 文档，核对所有路径／符号、最新 migration、任务注册和 ProviderMode/重试/预算接口。
2. 为 R1–R6 建立能失败的 Rust 回归；提取已有脚本可辅助理解，但不能把脚本结果当 Rust 测试。
3. 建立关键升级夹具：模型已绑定、业务向量有效、无校准记录；以及无原模型配置的派生集合恢复夹具。
4. 固定新增端点、校准资源预算、报告签名、基准来源和按通道的响应 schema。新增 ADR，修正 AGENTS 中仍与 ADR-0026 冲突的旧概念。
5. 保存现有分数和序列化基线；明确行为变更 R4/R5、诊断命名兼容。建立端点到 UI 及测试的覆盖表。

**验收**：准确区分既有失败与新测试红色；有可重复的用例与实际命令，不先调一个全局阈值。

### M1 — 请求过滤、无模型删除与发布安全

**依赖**：M0。**文件**：memory service/model/current_markdown、state current_memory、快照 migration、相关 MCP/Admin handlers。

实现 R1、R3 及 §6.3 的恢复；检查已过期／未生效记忆、所有请求过滤、空集合、模型元数据可选、删除后全入口不可见。更新 schema 时处理既有 prepared 快照，不批量废弃运行中的有效工作。

**验收**：R1/R3、恢复与隔离用例通过；无原 Provider 的删除为零模型调用；新旧快照可读可恢复。该阶段不自动启用新语义策略到生产。

### M2 — 双通道准入、对象名次、预算与候选引擎

**依赖**：M1。**文件**：memory recall、indexer retrieval/chunk metadata、state 查询、Provider vector adapter、MCP DTO。

实现 R2/R4/R5/R6。提取供正常 recall 和评测共同使用的内部候选与准入计算，保持公开检索契约；不存在校准也能在受控内部 evaluation context 取得 raw candidates。实际活跃语义路径继续受有效校准约束，后续 M3/M4 自动完成，不永久词法化。

补齐实际分数诊断、候选范围、完整预算、匹配章节标识；不得因仅修响应／排序使业务 embedding profile 全量失效。

**验收**：R2/R4/R5/R6 与两通道正反例通过；相关笔记但无记忆的对照可返回，不能通过关闭 related_notes 过关。

### M3 — 校准计算引擎与非私人基准

**依赖**：M2。**文件**：建议 `crates/memory/src/calibration.rs`（或现有合适应用模块）、memory/provider/state 边界、`quality_eval.rs`、合成 fixtures。避免 indexer 反向依赖 memory，复用纯策略类型或通过应用层传入校准视图。

1. 完成基准、真实 Provider 调用路径、缓存、阈值扫描、holdout、正确指标定义和机器可读报告。
2. memory/note 分别校准，运行整链路；保持标注与返回对象／section 的映射一致。
3. 实现报告有效性及发布前签名检查；前端和 test fixture 不可自报“服务端真实评测完成”。
4. 扩展现有 deterministic runner 复用引擎并保留 baseline/after 文件；真实执行是 worker 的正式能力，不限于命令行示例。CLI 可复用，不另建一套计算算法。

**验收**：离线确定性验证阈值选择与失败分支；真实 Provider 路径完整、可用本地合同服务器测试，不是 stub。没有真实凭据时只把效果标 pending，不能把整个自动校准实现延期。

### M4 — 启动自动补偿、任务去重与服务端操作 API

**依赖**：M3。**文件**：`crates/server/src/workers.rs`、server 启动／Vault 初始化组合、state jobs/settings/outbox、Provider/model 变更调用点、admin-api。

1. 注册校准 handler，启动后异步检查现有 Vault，向量已完备但缺校准也直接排队。
2. 接入配置变化、Provider 恢复、显式重试及低频补偿；并发触发共用原子去重。
3. 执行有界评测、持久 checkpoint、失效配置拒绝发布和自动激活；GET 不启动收费调用。
4. 接通校准状态/run API 及复用 job 返回；添加独立来源列表与最新 set_revision，修复 resume＋排队的一致性。

**验收**：主场景 A01 在不打开 UI／不重新绑定／不重新生成情况下完成；重复重启零额外校准请求；失败、预算、模型切换、取消可观察可恢复。

### M5 — 五项 Admin 功能与浏览器闭环

**依赖**：M4（组件可并行开发，但验收需真实契约）。**文件**：frontend Admin、admin-api public tests。

实现 §8 全部交互：迁移预检／执行、校准状态／重试、空集合来源恢复、显式编辑、分页和局部错误。旧 PUT 不做指标填写默认界面；新增 run 真正启动服务端计算，结果由服务器保存。

**验收**：每个交互有真实形状的 API mock 组件测试，再有浏览器＋真实 Admin 后端＋FakeProvider 的 E2E；不能只 assert 文案出现。检查 CSRF/Origin、修订冲突、多 Vault、最后一条删除后恢复和满向量未校准状态。

### M6 — 提取覆盖、语言和可定位章节

**依赖**：M1/M2；可与 M3–M5 非冲突部分并行。**文件**：extraction prompt/schema/normalization、indexer section projection、MCP tools/resources 说明、generation fixtures。

1. 提取提示词明确保持来源主要语言，保留技术字面量；混合笔记检查完成状态、环境、下一阶段和条件性实验结论，不把计划／建议变成已完成／已采纳。
2. 使用中文合成 Weekend 学习记录：frontmatter 与正文都有状态，末尾有下一阶段，同时包含大量通用知识。应生成可独立检索的进度、环境和实验信息，不丢有价值知识，也不固化为代码特判。
3. 保留一次调用与 `memories[]`；可选元数据规范化不丢有效正文；缺正文、截断和 API 故障不当成成功空集合。
4. Prompt 版本变更只影响明确排队或后续需要的提取。**本次升级不会自动对用户已重生成的全部记忆再付费重提取。** 提供可见的重新提取入口和受影响提示，真实重生成需用户明确操作。
5. 完成 section 粒度与实际命中路径一致的输出及工具说明同步；尽量保持已有向量文本不变。

**验收**：结构管线测试通过，真实生成内容覆盖／忠实性单列评测；不是仅检查 prompt 包含“progress”。预算输出与 source locator 一致。

### M7 — 升级回归、全链路验收与交付

**依赖**：M0–M6。

运行 §10 命令和验收矩阵，保存实际日志、前后对照、失败／跳过原因。验证“带现有向量的 0.2.1 数据库启动新版本”的恢复，不只在空库测试。更新 docs、接口、README、发行说明和运维指引。

交付实现完整的自动校准，而不是“请用户今后自行制作校准报告”。生产参数效果未跑真实模型时明确未验收，不能因此把代码中的真实执行路径省略。全部交付物与测试状态写入 Outcomes 后再决定计划是否可归档。

## 10. Validation｜验收矩阵与实际命令

### 10.1 R1–R6 对应的最小回归

测试名称为建议新增名称，不声称基线已有或已经通过。

| ID | 建议测试 | 必须证明 |
|---|---|---|
| R1 | `semantic_recall_respects_type_validity_and_importance` | 真正开启语义路径：错误 kind、未来 valid_from、过期 valid_to、低 importance 分别排除；符合条件的纯语义候选仍返回。边界时刻与 SQL 一致。 |
| R2 | `recall_related_notes_rejects_positive_cosine_hard_negative` | 两通道都开启；无关正余弦、偶然 OR 词面命中都不能作为上下文返回；相关笔记且无个人记忆时仍返回笔记。 |
| R3 | `derived_forget_works_after_rebuild_without_original_provider` | 无原模型配置恢复合法集合，删除成功、其余项保真、来源暂停；Provider 调用为零；删除最后一条仍有可管理空集合。 |
| R4 | `context_only_relevant_candidate_survives_admission` | FTS 前 50 外的 context 相关候选可返回；同 context 但无 query evidence 的候选不能返回。 |
| R5 | `semantic_memory_rank_is_invariant_to_duplicate_chunks` | A 从 1 块变 32 块，B 最佳有效相似度与对象名次未变时贡献不变；失效／过滤块不占名次。 |
| R6 | `recall_budget_counts_actual_heading_text_and_metadata` | 60 个各 100 字节标题不按“32 个固定费用”处理；长路径、标签、来源、诊断均计入；超大项跳过后能选短项。 |

### 10.2 自动校准与现有部署（A 系列）

| ID | 场景 | 预期 |
|---|---|---|
| A01 | 0.2.1 数据库已有角色绑定、有效业务向量、无校准；启动新版本，不访问 Admin、不重新绑定、不重生成。 | 自动产生真实执行流程对应的后台校准任务，成功后 pure-semantic recall 有贡献；业务 Markdown/ID/revision/向量内容不变。CI 用合同 FakeProvider，不能把其指标复制到生产。 |
| A02 | 相同环境连启两次／运行中重启。 | 同签名活动任务复用，已完成校准不重复；可恢复批次不重复计算。 |
| A03 | 初次绑定、新 Vault 初始化、Provider 从允许范围内恢复、模型关键配置变化。 | 都进入同一 ensure 逻辑，漏掉事件可由低频补偿补上。 |
| A04 | 已有100%向量但校准缺失。 | UI 不能显示“语义已就绪”；服务不会要求重建原向量来触发校准。 |
| A05 | 校准运行中更换模型或检索策略签名。 | 旧报告不得发布，最新签名只有一个有效任务；不同 profile 不混算。 |
| A06 | 向量覆盖不全／未配置一个通道。 | 覆盖与校准单独报告；只处理已配置角色，缺 note 模型不阻止 memory 的合法准备。 |
| A07 | Provider disabled、未授权、取消维护。 | 零未授权网络请求；原因不是笼统“未校准”，普通本地查询保持可用。 |
| A08 | 超时／限流、预算耗尽、重启、再次自动补偿。 | 持久预算与有界退避生效，重启不能重置消费上限，无无限任务风暴。 |
| A09 | 找不到兼顾有答案与无答案的阈值。 | 报质量未通过，不自动填高分、不把所有结果过滤为空后宣布成功；符合条件时保持既有适用配置。 |
| A10 | 首次无校准执行评测。 | 内部能取得语义候选，不被正常 recall 前置挡住；MCP/未授权请求不能使用内部 bypass。 |
| A11 | memory/note 使用不同模型，或同模型但不同正文输入规则。 | 分别评测、签名和启用；整体无答案不能被另一通道绕过。 |
| A12 | 报告写入／发布间崩溃，之后恢复。 | 复用已保存结果；重新核对签名后发布一次，不造成功记录，不丢当前可用配置。 |
| A13 | 修改 token budget／诊断字段等非 embedding 输入行为。 | 不强制业务向量全量重建；确属检索策略变化才重校准。 |
| A14 | 校准用内置非私人基准。 | 评测数据不进入真实 Vault、记忆列表、向量业务命中、recall_count 或默认正文日志；报告明确 scope。 |
| A15 | 旧 PUT 导入结果／仅 fake 的评测输出。 | 不冒充服务端真实执行；缺新签名时自动复核，不造缺失样本或 report provenance。 |
| A16 | 服务运行环境有 embedding、没有生成模型。 | 默认自动校准仍可完整运行，不要求 memory_extraction/reranker 模型。 |
| A17 | 无答案测试通过但有答案／纯语义子集失败。 | 不能激活；报告分项计数，防止永久 lexical-only 被伪装为校准成功。 |

### 10.3 Admin 用户流程（U 系列）

| ID | 场景 | 必须观察的真实效果 |
|---|---|---|
| U1 | 页面发起预检。 | 调用已有 POST，显示报告，不改知识；错误和指纹可见。 |
| U2 | 预检确认并执行迁移，期间有并发变化。 | 带正确确认串/hash；409 必须重新检查；部分未解决可见，不能自动重试覆盖。 |
| U3 | 向量齐全、校准排队／成功／失败。 | 加载正确 GET，真实展示状态；重试调用 run 而不是提交伪指标；页面打开本身不启动额外任务。 |
| U4 | 删除来源最后一条记忆，刷新／切页，再恢复。 | 在独立来源列表仍能找到空集合；传正确 set_revision；明确恢复后成功排队，并且不要求原 Provider。 |
| U5 | 编辑 explicit 的正文及部分元数据。 | PATCH expected_revision 正确；未触碰字段保留，显式清空生效；派生项不能原地编辑；冲突提示正确。 |
| U6 | 51 条以上记忆／暂停来源、某状态端点失败、切换 Vault。 | 分页完整，局部错误不拖垮管理功能；旧请求不串 Vault；计数不冒充全量。 |

### 10.4 生成、可解释性与数据安全（G/S 系列）

| ID | 场景 | 预期 |
|---|---|---|
| G1 | 中文混合学习笔记，frontmatter completed/week/environment，末尾 next stage，主体有大量教程内容。 | 真实输出评测保留显式完成状态、环境、下一阶段和实验范围，正文主要语言一致；不在代码中硬编码 Weekend。 |
| G2 | 计划、第三人称引用、未采纳建议、否定和条件实验。 | 不变成用户已完成／已采纳；主体与条件保留。 |
| G3 | 正常空集合、输出截断、optional 字段错误、内容超限。 | 合法空才替换为空；截断／错误不清空；可选字段可修正但不丢合法正文。 |
| G4 | 长问句、中英释义、KV Cache 释义、MoE 显存匹配章节。 | 无字面重合的语义命中确实工作，section 标识／snippet／来源一致；不拿 keyword-only 成功冒充跨语言成功。 |
| S1 | 两个 Vault 同名文档、相同 query/profile、并发校准和操作。 | 正文、ID、计数、报告、任务、缓存和写入严格隔离。 |
| S2 | 删除、修改源文档、重新生成后，用旧 ID/history 参数/资源/向量继续查。 | 被删或过期派生内容全入口不可见；暂存报告也不能变成记忆检索来源。 |
| S3 | 新旧集合发布中断、无原模型快照、中途暂停／恢复。 | 只发布完整且仍有资格的集合，重放不覆盖新编辑；恢复操作和排队不永久脱节。 |
| S4 | 0.2.1 数据库升级、已有显式元数据／current集合／有效向量。 | 非破坏性升级；保留知识与数据权限，支持 pending 旧快照；自动补校准与旧记忆迁移相互独立。 |
| S5 | 非法 Origin/CSRF、未授权 run／resume／patch、恶意 note／报告字段。 | 安全边界不因“让校准能跑”而放开；默认日志无秘密及真实正文。 |
| S6 | diagnostics开关、60个超长标题、低预算及候选窗口达上限。 | 分数没有重复加贡献，输出不超公布的预算估算规则／硬字节上限；coverage 与 counts 不作虚假全量断言。 |
| S7 | 单来源普通重生成／删除与自动校准同时发生。 | 校准不修改正文／暂停状态，source resolver 排除旧知识；无自动全库提取或无端重新向量化。 |

### 10.5 真实模型质量与机制测试不能混用

CI 使用 FakeProvider／临时数据库证明调度、计算、过滤、隔离和接口契约。真实 Provider 校准必须执行相同引擎；测试成功不代表真实 embedding 质量通过。

生成质量至少建立 15 个独立合成案例，包含来源、必须保留的事实和禁止的推断。真实输出按支持精度、关键事实覆盖、主体／条件／语言错误计数。目标为支持精度 >=95%、关键事实覆盖 >=90%，关键进度／否定案例不可被平均值掩盖；这些是待验证目标，不是静态 prompt 检查能证明的性质。

真实评测使用单独获准的非生产配置／合成基准；报告固定代码 HEAD、Provider/model/profile、数据集版本、预算及实际请求计数。无凭据时列 `deployment quality evaluation pending`，不能将 fake 标签里的 supported=true 当作实时模型判断。应交付完整运行能力和明确失败信息，不要求用户从头手做校准。

### 10.6 命令与日志

从仓库根目录执行，依实际 workspace 包名核实，不升级依赖来掩盖错误：

```bash
git status --short
git rev-parse HEAD
rustc --version
cargo --version
pnpm --version
cargo metadata --no-deps --format-version 1
```

按里程碑运行相关子集：

```bash
cargo test --locked -p mcp-vault-memory --all-features
cargo test --locked -p mcp-vault-state --all-features
cargo test --locked -p mcp-vault-indexer --all-features
cargo test --locked -p mcp-vault-providers --all-features
cargo test --locked -p mcp-vault-admin-api --all-features
cargo test --locked -p mcp-vault-mcp --all-features
cargo test --locked -p mcp-vault-server --all-features
```

保留现有 deterministic runner 的可执行入口，扩展后用同一语料分别保存 before/after，不覆盖基线：

```bash
cargo run --locked -p mcp-vault-memory --example quality_eval -- \
  --mode deterministic \
  --fixtures tests/fixtures/memory-quality \
  --output target/quality/after.json
```

最终检查：

```bash
pnpm --dir frontend/admin install --frozen-lockfile
pnpm --dir frontend/admin lint
pnpm --dir frontend/admin test
pnpm --dir frontend/admin build
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
bash scripts/check-docs.sh
```

读取并按真实参数运行现有 MCP conformance 与 migration 检查脚本；存在可用浏览器测试框架则复用，没有则新增最小 E2E 入口并记录确切命令。不能把上面组件 test 自动称为真实浏览器 E2E。

必须新增从变更前 schema/snapshot 升级的集成测试，以及不启动浏览器的 A01 服务启动测试。校准实时 Provider 的执行由真实 worker验证或在授权的部署配置中执行，CLI 是否扩展由复用需要决定；不得用一个不存在的 `--mode live` 命令假装已交付。

每项记录命令、退出码、测试数、环境限制、失败名称和日志路径。缺工具、依赖或网络时如实报告并继续其他可执行工作；不删测试、放宽权限、固定返回 query 答案来变绿。

## 11. Rollback and recovery｜升级与回滚

### 11.1 向前升级

1. 用前一版本副本建立回归备份，核对 Vault 与 SQLite 成对的一致性；本次开发不动真实部署。
2. 新 schema 仅对校准派生状态／本地快照必要字段做兼容迁移。按当前最大 migration 编号追加；不重写 `0015`，不自动删 legacy/current 表。
3. 不因修改校准、评分或 UI 就改 embedding 输入 profile；已有有效业务向量保持可用。
4. 升级后注册并启动有界补偿；校准可异步运行，但必须能完成或给出具体终止原因。失败不阻塞文件同步，不无限弹出“未校准”且无动作。
5. 旧记忆迁移与恢复暂停来源仍由明确 Admin 操作触发；校准任务绝不复活旧知识或解除暂停。
6. 后端与 Admin 构建产物同版本发布，避免新后端配旧静态页面；发行说明注明自动校准的小规模 embedding 请求和可关闭的维护设置。

### 11.2 回滚

- 自动维护可停止／取消，当前仍适用的校准结果可继续供查询使用；需禁用语义时明确降级为合格词法结果，不恢复原来“非负 cosine 全放行”。
- 校准报告发布使用版本／签名预条件。失败候选不覆盖当前合法结果，旧模型阈值不能跨配置复用。
- 本次不自动改写业务正文，正常回滚不应要求恢复所有记忆；修复中人工测试的副本用副本快照恢复即可。
- 若 snapshot schema 改动使旧二进制无法读取，不能只回退 binary。用经过验证的前向兼容修正，或在停写状态恢复成对的 DB＋Vault／历史备份；严禁只恢复一个介质。
- 遗留业务向量若实际失效，只能按当前内容重新生成缺失部分，不能为维持表面可用而返回过期数据。

### 11.3 故障注入点

覆盖：校准 enqueue 后／Provider 批响应落盘前后／报告保存后发布前／模型切换／预算耗尽重启；删除规范文件写入后投影提交前；空集合暂停发布后；resume 修改成功而排队失败；旧提案恢复和用户并发写入。

期望：可恢复／明确冲突；无跨 Vault、无已删除正文返回、无无限计费、无前端虚报成功。无需为这些故障再加一套全局记忆代际系统。

## 12. Progress｜执行进度

仅在实际验证后勾选，不把部分完成算完成：

- [x] 2026-09-05：编写本执行计划并读取 Review；仅代表文档完成。
- [x] M0：实际基线、规范／ADR、真实路由、资源预算和红色回归已记录。
- [x] M1：请求过滤、无原模型删除和快照恢复通过。
- [x] M2：双通道准入、context-only、对象排名、预算与候选引擎通过。
- [x] 2026-09-06 M3：真实 Provider adapter 校准执行器、基准、指标／构建来源报告、独立及联合质量门槛和签名发布完整；离线合同通过。
- [x] 2026-09-06 M4：A01 启动补偿、去重、持久预算、重试／取消、状态与运行 API 通过；已保存报告恢复发布和 resume 意图重放通过。
- [x] 2026-09-06 M5：五项管理功能、空集合恢复、分页和真实 Chromium E2E 通过；35 项前端测试。
- [x] 2026-09-06 M6：生成全文／语言传输约束、章节定位、协议说明及离线回归完成；真实输出效果见下方 pending。
- [x] 2026-09-06 M7 可执行工程门槛：314 项默认 workspace 测试、fmt、all-features Clippy、前端、迁移、docs、四个官方工具支持的 MCP 版本通过。
- [ ] M7 环境／外部工程门槛：all-features 测试因宿主 ONNX 链接失败；Litmus 未安装；官方工具不支持 2024-11-05；真实 Obsidian 客户端互操作未执行。
- [ ] M7 实际质量：获准的真实 Provider 校准／生成评测已记录；未执行则明确 pending。
- [x] 2026-09-06 交付：代码、前端、兼容迁移、测试、验收报告、前后合同报告、升级／回滚说明及剩余项在本地工作树；未 git commit/push/deploy。

## 13. Decisions｜决定记录

- **D1（2026-09-05）**：维持 ADR-0026 当前集合架构，不恢复 lifecycle、全局合并和历史读取。正确性修复不等于再次重构产品。
- **D2（2026-09-05）**：自动校准必须包含启动补偿；只监听新绑定会漏掉用户当前部署。Admin GET 不是后台任务触发器。
- **D3（2026-09-05）**：默认内置带标注基准＋真实 embedding，不依赖新增生成模型、用户标注或自报质量指标。基准通过不冒充全库人工验证。
- **D4（2026-09-05）**：保留业务有效向量，校准新增样本有界；检索签名与 embedding 输入版本分开。拒绝“重建全部向量才能校准”。
- **D5（2026-09-05）**：资格、相关性、排名三个层次分离，修复 R1/R4；不同通道各自校准，不把最终分数当相似度。
- **D6（2026-09-05）**：五项 Admin 缺口按用户流程交付；校准 PUT 保留高级兼容而非伪运行按钮，最后一条删除后的来源必须仍能管理。
- **D7（2026-09-05）**：自动校准可以自动开始，旧数据迁移和恢复用户暂停来源不能自动开始。开发中无生产／付费授权不执行真实操作，但代码必须具备真实运行能力。

实现中改变决定需记录日期、证据、理由和被拒绝方案，同时更新关联验收，不得悄悄放宽。

## 14. Surprises and discoveries｜已知发现与待核实项

已知：

- `semantic_profile_uncalibrated` 不代表没有向量；基线在 embed query 前就退出。
- GET/PUT calibration 只是读／保存，不能等同运行校准。
- 后端五个功能已有 handlers，前端遗漏并非 UI 按钮暂时不可见；需要真实接线和 E2E。
- source set 可恢复但原模型信息缺失是合法状态，删除不应把它当冲突。
- 原始 Markdown 在基线提取路径没有删除 frontmatter；不要用未证实的输入截断解释已知进度遗漏。
- `AGENTS.md` 中的旧记忆叙述与已经 Accepted 的 ADR-0026 部分不一致，需要同步文档。

待 M0／真实评测确认：当前 HEAD 是否新增修复、ProviderMode 的精确授权范围、模型能力／批处理限制、模型同名静默更新能否被识别、既有 job 去重与预算恢复接口、现有 E2E 可用性。无法从模型名称推断多语言质量；报告中保持这些边界。

## 15. Outcomes｜最终交付模板与完成定义

实际填写结果见 [验收报告](../reports/memory-review-autocalibration-admin-fix.md)。以下模板与原验收定义保留作对照。

最终报告至少填写：

```text
实际起始 HEAD / 最终代码状态：
R1–R6 每项修改文件、关键行为与测试：
A01 现有模型＋向量＋无校准启动场景的证据：
默认校准样本、模型、签名、真实/模拟来源与结果：
向量／记忆是否重新生成，哪些必须保持不变：
U1–U5 实际页面操作→请求→服务调用→E2E 证据：
删除最后一条后的暂停来源恢复证据：
生成覆盖、来源语言、章节定位和分数解释：
迁移／故障恢复、权限／Vault 隔离：
实际运行命令、退出码、日志与未运行项目：
是否触及生产数据／凭据／付费 Provider（开发阶段默认否）：
自动维护预算、失败原因与重试／取消方法：
发行升级与回滚说明：
仍未通过的工程或真实质量门槛：
```

**完成不是：API 注册了、按钮出现了、分数变大了、向量覆盖 100% 了、degraded 字段不见了。**

**完成是：当前这种已配置模型和向量的旧部署能自动补做实际校准；语义路径真实参与且遵守所有过滤；无关结果不能从笔记通道绕过；删除／恢复／编辑／迁移在页面上可完成；已有知识不被无意重写；所有结果能被测试和报告复核。**

## 16. Source anchors｜复核入口

本计划的源码事实固定于 §1 SHA。以下相对路径和符号可直接定位；不是对尚未实现的新接口的存在性断言。

- `crates/memory/src/service.rs`：`recall`、`forget`、`semantic_calibration`、`set_semantic_calibration`、`calibrated_semantic_rank_score`、`estimate_note_tokens`、`current_extraction_system_prompt`、`extract_note_with_options`。
- `crates/state/src/current_memory.rs`：`get`、`current_eligibility_sql`、`append_filter`、`MemoryNoteSetSnapshotRecord`、来源集合读取／发布。
- `crates/indexer/src/lib.rs`：`retrieve_notes`、`add_semantic_note_hits`、`semantic_note_rank_score`、`note_embedding_chunks`。
- `crates/admin-api/src/lib.rs`：`vault_admin_routes`、校准 GET/PUT、migration preflight/execute、resume、memory patch handlers。
- `frontend/admin/src/App.tsx`：`loadPage('memory')`；`pages.tsx`：`MemoryPage`、`MemoryEmbeddingPanel`、`MemoryExtractionPanel`。
- `crates/memory/examples/quality_eval.rs`：仅 deterministic 的参数约束和 related_notes=false 的原评测调用。
- 附件 `code-review-92d9367.md`：R1–R6 原始审查和复现边界。本文件已纳入其全部修复目标，无附件也可执行。

## 17. 给 Codex 的启动指令

```text
请执行 docs/exec-plans/active/memory-review-autocalibration-admin-fix.md。
先核对实际 HEAD、AGENTS.md、PLANS.md 与 ADR-0026，不覆盖未提交修改，
不回退到旧提交。按 M0–M7 实施，不要只再输出分析或计划。

范围包含六项 Review 修复、真实服务端自动校准、现有部署启动补校准、
五项 Admin 用户流程、生成覆盖／语言与必要的章节和诊断修复。
最重要的验收是 A01：已有 embedding 模型和有效向量但无校准记录的数据库，
升级启动后无需重新绑定、重新生成、打开页面或手动调 API，自动校准并在通过后启用。
复用已有有效业务向量，不伪造评测数据，不只清除警告，不新增必需生成模型。

保留单来源当前集合与真实删除，不恢复历史状态机或全局 consolidation。
校准可自动运行，迁移旧数据和恢复用户暂停来源仍需明确确认。
每阶段更新计划进度、决定、实际命令和测试结果。
不自行调用收费模型、使用生产凭据、迁移/重生成真实 Vault、push 或部署。
没有真实模型验收条件时，继续完成真实执行能力及离线测试，
把真实效果明确标为 pending，不能用 fake 测试或缺凭据为由省略产品闭环。

最终交付代码、测试、前端、兼容迁移、运行报告、升级和回滚说明。
```


### Execution record — 2026-09-05 M0

- HEAD `92d9367949570ccd3c19ece8895cdd8f00cc5e78`; initial untracked files were this plan and its `:Zone.Identifier` companion only. No existing code edits were overwritten.
- Read AGENTS/PLANS, ADR-0026 and ADR-0023–25, memory-system §§1–10, product requirements §§3.3/3.5/3.6/3.9, architecture §§6/9/11/12, interfaces §§6.7–9/10.3 current memory, data-model §§13/19, security §§12–14. Latest schema is 0015 in root `migrations/` (not crates/state/migrations).
- Tools: rustc/cargo 1.94.0, pnpm 11.19.0; `cargo metadata --no-deps --format-version 1` exit 0, saved in target/memory-review/metadata.json.
- Baseline `cargo test --locked -p mcp-vault-memory --all-features`: exit 101; 4 integration tests failed only at loopback bind with `Operation not permitted` (target/memory-review/m0-memory.log). Re-running with approved local-network sandbox escalation; no external Provider access.
- Baseline deterministic runner command from §10.6 completed, report target/quality/before.json, log target/memory-review/m0-quality.log. Fake quality remains mechanism-only.
- R1–R6 confirmed in current code. Additional R3 issue: full-set apply unconditionally deletes all member vectors. Preserve unchanged vectors during local deletion.
- New regression `derived_forget_works_after_rebuild_without_original_provider` tests the legitimate model-less restored projection, deletion to empty set, pause and zero generation calls.
- D8: keep old migrations immutable; append 0016 to permit missing model references in local-deletion snapshots, with generation snapshots still requiring model identity. Existing snapshot rows and bytes must survive upgrade.
- M0 remaining: expand red R1/R2/R4/R5/R6 evidence alongside their implementation; scheduler budgets/contracts finalized after transport inspection. No milestone falsely marked complete.

### Execution record — 2026-09-05 M1–M4 implementation

- M1: `get_filtered` reuses repository request predicates for vector candidates and final ranking reads. `semantic_recall_respects_type_validity_and_importance` now executes the real calibration engine against a local synthetic embedding server before exercising the semantic branch (no imported fake metrics).
- R3 red command exit 101 with `Conflict`: target/memory-review/m0-r3-red.log. After fix, `cargo test --locked -p mcp-vault-memory --all-features` passed 7 unit + 8 integration tests (m1-memory.log). Later M2 passed 7 unit + 10 integration (m2-memory.log).
- M1 migration: `cargo test --locked -p mcp-vault-state migration_0016` exit 0, one targeted upgrade test. It upgrades a schema-0015 prepared snapshot and compares stored items/hash/status/model identity before and after. Initial test fixture compilation/column naming mistakes are preserved in earlier command output; fixed to actual `capability_json` and numeric FK-violation count.
- R4 integration establishes the target is outside FTS's 50-object window, then verifies a context-only relevant candidate survives and unrelated context does not. R5 object ordinal now advances after freshness/request filters and duplicate collapse. R2 adds a recall-specific note admission service and leaves public search intact. R6 estimates serialized views and final response JSON, including diagnostics; legacy 128-token happy-path expectation was invalid for the complete object and is now paired with a hard byte assertion at 320 tokens.
- M3: appended schema 0017 for Vault/channel/signature checkpoint state and singleton jobs, without touching business vectors or canonical data. ProviderTransport charges persisted attempts immediately before HTTP dispatch, including internal retries. Default limits: 512 unique inputs, 2 MiB serialized request bytes, 32 HTTP attempts, 15 minutes per automatically admitted round, batches of 16, two worker slots globally and one active durable job per Vault.
- D9: explicitly requested Admin retries may allocate another bounded round while retaining cumulative spent requests/bytes and completed embedding cache. Restarts and automatic compensation never allocate that extra budget.
- Built-in corpus `tests/fixtures/memory-quality/calibration.json`: 40 synthetic sources, 120 questions, disjoint source-topic splits with 40 answered + 20 no-answer questions each. Pure-semantic labels are checked against lexical admission. No claim of human review or real-model quality.
- M3 unit checks cover invalid vectors, multi-answer Recall vs Hit distinction, empty-answer precision, split counts and pure-semantic labels. Real adapter path passed the local contract calculation in R1; deployment quality evaluation pending.
- M4 code: registered `retrieval.calibrate`; immediate startup reconciliation tick after recovery/worker registration and later 300-second compensation use the same ensure operation. GET remains read-only. New run, maintenance and source-list API routes compile. Resume state update and extraction job insertion are now one repository transaction following the canonical revision write.
- `cargo check --locked -p mcp-vault-server` exit 0 (m4-api-check.log). M4 acceptance remains open until startup, cancellation, model-switch and recovery tests run.
- M5 implementation underway: UI now calls migration preflight/execute, calibration GET/run/maintenance, independent paused-source list/resume, and explicit PATCH. Per-endpoint load failures and pagination are being verified. No browser E2E claimed yet.


### Execution record — 2026-09-05 M3–M6 verification

- A01 actual production startup compensation loop + durable worker passed against a reopened disk database, preserving valid business vectors and canonical identity. A full-suite attempt exceeded the original 20-second wait under concurrent load; the isolated test passed in 9.06 seconds, and the diagnostic wait now permits 60 seconds without changing a quality gate (`target/memory-review/a01-startup.log`).
- `cargo test --locked -p mcp-vault-memory --all-features`: 10 unit + 14 integration tests passed (`memory-final.log`). Added R5 one-to-32 chunk invariance, R6 full heading/metadata budget and note no-answer, cancellation/retry/pause/model change, independent note calibration, and 15 full-source language/coverage transport cases.
- M3/M4: reports are published under complete configuration signatures; terminal publication requires the saved passed report to match. Engineering budgets are typed and configurable for new signatures, frozen on admission. Actual dispatch checks maintenance and durable counters. Terminal retry preserves consumption/cache; old terminal signatures are bounded to eight plus current per channel.
- M5: 33 frontend unit tests passed. Real Chromium found an actual migration response mismatch (`migration.completed`, not top-level `completed`), now fixed. Real browser workflows remain in verification; initial fixture/background DB contention produced redacted authentication_unavailable errors and is being investigated, not marked passed.
- M6: new 15-source bilingual fixture has required facts and forbidden claims; offline tests prove full source transfer and no repeated extraction for unchanged sources. Real generation quality remains pending, not inferred from contract outputs.
- `scripts/release/check-migrations.sh` passed, including 0015 snapshot preservation through 0016 and 0017 accounting/isolation checks (`migrations.log`).
- Official MCP default invocation failed with npm GitFetcher/Arborist packaging error (`mcp-conformance.log`). Same pinned upstream source at `/tmp/mcp-vault-conformance-src.CKYcxp/conformance-74edef34d674f563537be8c6587cebaa58e830ca` passed all six 2026-07-28 scenarios using the existing expected-failure baseline (`mcp-conformance-local.log`). No conformance exceptions were added.
- Decisions: signature-specific publication avoids an obsolete task overwriting a newer report; quality thresholds remain fixed, while transport limits are configurable engineering budgets. Canonical deletion requires no original Provider. Full behavior and upgrade/rollback instructions now live in `docs/memory-autocalibration-operations.md` and are linked by the governing specifications.

### Execution record — 2026-09-06 M3–M7 final verification

- A01 final production-loop test passed in the full workspace suite. Investigation beyond the earlier timeout found actual valid-vector reuse was missing for previously queued embedding jobs, and deferred SQLite publication could report passed without active settings. Reuse now validates stored bytes/profile/input hash and preserves IDs/timestamps; publication is one conditional SQL statement. The fixture also now registers the real embedding handler so old jobs drain normally. These are the resolved causes; increasing a timeout was not the complete fix.
- `cargo test --locked --workspace` with temporary localhost permission: exit 0, 314 passed, 0 failed/ignored (`workspace-verified.log`). R2 now runs both calibrated channels against actual vectors with positive-cosine/OR hard negatives. Added final response revalidation during a concurrent delete and durable `memory.source_resume` recovery after canonical commit.
- D10: resume first persists an idempotent intent, then commits canonical bytes and the transactional source-state/extraction-job pair. `source_resume_replays_canonical_commit_and_queues_extraction_exactly_once` proves recovery, exact bytes and one extraction job. The API reports `resume_accepted`, not premature completion.
- D11: calibration and production use the same exact cosine implementation as well as input/lexical/object-score helpers. Real high-dimensional cache support is bounded at 32 MiB; a 160×3072-dimensional repository round trip and database reopen prove persistence. Build reports include source commit and clean/dirty/unknown state.
- D12: two individually passing 1/20 no-answer failures can be disjoint. Combined holdout error IDs are unioned for query eligibility; 2/20 blocks both semantic channels, preserves reports and does not create automatic retry storms. Initial `joint-gate-local.log` caught a missing ensure guard; fixed result is `joint-gate-verified.log`. Manual retry remains available.
- `pnpm --dir frontend/admin lint`, `test`, `build`: all exit 0, 35 tests. Real Chromium `scripts/e2e/memory-admin.py` passed U1–U5 and 53-memory pagination (`browser-verified.log`, `browser-e2e/result.json`). Component tests additionally cover 51 paused sources, polling-tail preservation and late responses after changing Vault.
- `cargo fmt --all --check`, `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`, `bash scripts/release/check-migrations.sh`, `bash scripts/check-docs.sh`: exit 0. Latest report/save-before-publication replay and DB reopen budget checks have separate `publication-recovery-verified.log` and `budget-restart-verified.log` evidence.
- `cargo test --locked --workspace --all-features`: exit 101 (`all-features-verified.log`), host ORT C/C++ binary needs `__isoc23_strtol/strtoll/strtoull` and `basic_string::_M_replace_cold`, unavailable in host libraries. No test/feature removed. Earlier sandbox localhost failures remain in `workspace-final.log`; authorized local rerun passed.
- Official upstream SHA `74edef34d674f563537be8c6587cebaa58e830ca`, local package override: 2026-07-28, 2025-11-25, 2025-06-18, 2025-03-26 passed existing baseline, no added exceptions. Tool rejects 2024-11-05 as unknown; Litmus command reports executable missing. Neither is counted as passed.
- Deterministic runner now executes the same dual-channel calibration engine through a local synthetic HTTP adapter and emits explicit `local_synthetic_contract` / `semantic_quality_proven=false`. Before remains `target/quality/before.json`; after is `target/quality/after.json` (`quality-verified.log`). Real generation/semantic quality remains pending. The 15-source coverage fixture now includes long mixed tutorial text, frontmatter, middle progress and final next-stage facts; no unchanged-source forced extraction.
- Final deliverables: [acceptance report](../reports/memory-review-autocalibration-admin-fix.md), [operations / upgrade / rollback](../../memory-autocalibration-operations.md), ADR-0027, specs and release-readiness notes. Plan stays active solely to retain unexecuted release/environment/real-quality gates visibly; no production credentials, paid requests, real Vault writes, push or deployment occurred.

### Follow-up — 2026-09-06 container build input correction

- User's image build exposed a missed packaging dependency: production `calibration.rs` uses `include_str!` for `tests/fixtures/memory-quality/calibration.json`, but the Docker builder copied only workspace manifests, crates, migrations and the frontend output. Local workspace tests had the file and did not catch the omission.
- Fixed Dockerfile with one explicit COPY for that benchmark before `cargo build`; no corpus bytes, signature, runtime behavior, business data or migration changed. The runtime needs only the embedded binary, not a new fixture mount. Deployment documentation now records this build input.
- Reproduced the original Docker COPY file set under `/tmp/mcp-vault-docker-inputs-t2ls0e6u`, without `.git` or the tests directory. `CARGO_TARGET_DIR=/home/cheng/code/mcp-vault/target cargo check --offline --locked -p mcp-vault-server --manifest-path /tmp/mcp-vault-docker-inputs-t2ls0e6u/Cargo.toml` exited 101 with the exact missing `calibration.json` error (`target/memory-review/docker-inputs-before.log`). After copying only the specified benchmark, the same command exited 0 (`docker-inputs-after.log`).
- Full Docker build remains unverified here: `docker info --format '{{.ServerVersion}}'` exited 1 with `permission denied while trying to connect to the docker API at unix:///var/run/docker.sock`, including outside the sandbox. This is host socket access, not an automatic-approval rejection. No deployment was attempted.

### Follow-up — 2026-09-06 version 0.2.2

- Updated workspace version, all 13 local Cargo.lock package entries, Admin package version, current README date/version, and deployment image/archive examples (including `.env.example`) to 0.2.2. External dependency versions and historical 0.2.1 baseline/upgrade evidence remain unchanged.
- `cargo metadata --offline --locked --no-deps --format-version 1` and `cargo check --offline --locked -p mcp-vault-server`: exit 0; metadata consistency verified all 13 members at 0.2.2 (`target/memory-review/version-0.2.2-metadata.json`, `version-0.2.2-check.log`). Formatting, docs and diff checks passed.
- `pnpm --dir frontend/admin build` exited 1 before compiling because its automatic install could not open the host store SQLite database: `[ERR_SQLITE_ERROR] unable to open database file` (`version-0.2.2-frontend-build.log`). Executed the package's unchanged build commands directly using installed dependencies, `./node_modules/.bin/tsc --noEmit && ./node_modules/.bin/vite build`, from `frontend/admin`: exit 0. No dependency/configuration change, image publication or deployment was performed.

### Follow-up — 2026-09-06 diagnose the reported quality failure

- User supplied the note-channel 0.2.2 report: 160 inputs / 10 requests, no passing threshold, fallback 0.999999, and calibration/holdout metrics identical to lexical control. This proves the fallback report is uninformative, not that the deployment model has zero semantic ability.
- [x] Read-only inspection: runtime discards failed threshold trials. Provider batch decoding also ignores response `index`; out-of-order replies can therefore attach vectors to the wrong inputs. Whether this happened in the reported deployment is unproven.
- [x] Fix batch response mapping with offline parser regression tests; preserve adapters that explicitly omit all indices by retaining their positional response contract.
- [x] Add a diagnostic command using a read-only, no-migration state connection, exact Vault/channel/signature lookup, and the existing corpus/input/candidate/evaluation functions. Export only recomputed synthetic case scores, raw answer ranks, threshold trials and failed gates; no credentials, canonical content, raw cache or publication.
- [x] Verify read-only access, no missing-database creation, failed/passing cache diagnosis, invalid/incomplete cache rejection, unchanged requests/reports/settings, and Vault isolation. Document an exact deployment-side command in `docs/memory-autocalibration-operations.md`.
- [ ] Determine the real model failure from a saved real run. The user has now requested a separate local service and will configure its model in Admin; production access remains outside this task.
- Decision: diagnosis reuses cached vectors and never invokes ProviderService, migrations, workers, threshold publication or full extraction. No Admin endpoint or schema change is needed. The legacy failure report remains intact for comparison.
- Verification example: construct orthogonal document vectors with correct answer cosine 0.8 and a no-answer query's wrong candidate cosine 0.99. The same production scorer gives raw answer Recall@5 = 1, but no threshold satisfies all gates. This tests the diagnostic distinction only; it does not measure the user's model or explain that deployment's failure.
- `cargo test --offline --locked --workspace`: exit 0, 317 passed, 0 failed (`target/memory-review/calibration-diagnostics-workspace.log`). The later strengthened A01 test also exercises the command handler against a passing persisted cache, without extra HTTP requests or report/cache changes: `cargo test --offline --locked -p mcp-vault-server a01` exit 0 (`calibration-diagnostics-a01.log`). Localhost contract tests used the approved sandbox exception; no external model was called.
- `cargo fmt --all --check`, `cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings`, `cargo build --offline --locked -p mcp-vault-server --bin mcp-vault`, `bash scripts/check-docs.sh`, and `git diff --check`: exit 0. Targeted diagnostic, Provider index, and read-only connection tests passed (`calibration-diagnostics-test.log`, `calibration-index-test.log`, `calibration-readonly-test.log`).
- `cargo test --offline --locked --workspace --all-features`: exit 101 (`target/memory-review/calibration-diagnostics-all-features.log`). The same host ORT binary cannot link `__isoc23_strtol/strtoll/strtoull` and `std::__cxx11::basic_string::_M_replace_cold`. This environment gate remains pending; no test was removed. No frontend or protocol behavior changed in this follow-up.
- Actual compiled command against a missing SQLite path exited 1, emitted only `calibration diagnosis failed: cannot open the existing database read-only`, left stdout empty, and created no database (`target/memory-review/calibration-diagnostics-cli.stderr`). This is a passing negative check, not a successful diagnosis.

### Follow-up — 2026-09-06 local real-model investigation

- User authorized starting a local service and will enter model configuration in its Admin page. Started the current diagnostic build from HEAD `6500214` plus the above uncommitted fixes, with a clean process environment and a new private test data directory: `target/memory-review/local-model.09VRlm/data`. No existing Vault, database, environment credentials, or model configuration was imported.
- Actual command: `env -i PATH=/usr/bin:/bin LANG=C.UTF-8 RUST_LOG=info MCP_VAULT_DATA_DIR=/home/cheng/code/mcp-vault/target/memory-review/local-model.09VRlm/data MCP_VAULT_DATA_BIND=127.0.0.1:18080 MCP_VAULT_ADMIN_BIND=127.0.0.1:18081 MCP_VAULT_ADMIN_ORIGINS=http://127.0.0.1:18081,http://localhost:18081 MCP_VAULT_LOG_FORMAT=json /home/cheng/code/mcp-vault/target/debug/mcp-vault`. Initial sandbox binding returned `Operation not permitted`; the approved local-listener retry started successfully. Both attempts are retained in `target/memory-review/local-model.09VRlm/server.log`.
- [x] HTTP GET verification: data `/health/ready` returned 200/ready; Admin `/api/v1/setup` returned 200 with `setup_available=true`; Admin `/` and both referenced JavaScript/CSS assets returned 200. The first Admin will create the separate default test Vault under this instance's data directory.
- The user's browser accesses this service through `http://localhost:54893`, so its first setup POST correctly rejected the original 18081-only Origin list. Added that one exact browser Origin to the isolated launcher `target/memory-review/local-model.09VRlm/start.sh` and gracefully restarted the same instance, preserving its database. A real POST to `/api/v1/session` with a deliberately nonexistent account now passes the 54893 Origin check and returns 401 `admin_session_invalid`; the same request from unconfigured port 54894 still returns 403 `origin_rejected`. No global Origin policy or product security code changed. The effective Origin list is `http://127.0.0.1:18081,http://localhost:18081,http://localhost:54893`; use the saved launcher for restarts.
- [x] User configured the GLM Provider and bound both embedding roles. Both local jobs executed, but stopped before dispatch with `provider_endpoint_denied`; no real-model quality result exists yet.
- Local investigation after configuration: read-only lookup of `default` Vault `01a07467-df23-7580-bb89-a43dd468e432` found memory signature `sha256:5b0f41d02facc2621be7cba62bab70178baaa9b7a615918a0215291f5fe89df4` and note signature `sha256:812b26d3745ac0580411f5f11770079c087d3918bfa4bcb6dce80a45e73b5dff`, both `failed`, 0 requests, 0 request bytes. Provider endpoint is HTTPS `open.bigmodel.cn`, mode `remote_allowed`, `allow_private_networks=false`. Only non-secret configuration and scoped run metadata were read.
- Actual host `socket.getaddrinfo('open.bigmodel.cn',443,type=socket.SOCK_STREAM)` returned `198.18.0.167` and `fdfe:dcba:9876::94`. `validated_socket` rejects the entire resolution when any IP fails policy; the IPv6 address is private and `endpoint_ip_allowed` therefore rejects it under the configured mode/settings. This is a confirmed pre-dispatch network-policy failure, distinct from the original deployment's completed quality failure. Proxy DNS interception is a possible explanation for those virtual addresses, not independently verified.
- [x] User explicitly permitted private-network resolution for this test Provider through its existing Admin advanced setting and retried. The normal worker subsequently completed both real embedding evaluations. No endpoint security code was weakened.
- [x] Read-only real-run diagnosis completed. Memory signature `sha256:6be8c0eb7db2a996456a70dd120aa4bd3b7f7caba9edc61a75fbf08f7dcd424c`; note signature `sha256:3d6d83481a370be90914428b9d6347998181cbd234599416f7707b971f93d1a0`. Each completed run saved 160 inputs, 2048-dimensional vectors and 10 charged requests. An earlier memory signature was interrupted by a configuration conflict after 6 requests; periodic compensation admitted the current signatures. That obsolete cache remains marked running although its job completed after skipping obsolete signatures; this is a separately observed operational-status defect, not an active model request or the cause of the completed quality failures.
- Actual commands: `MCP_VAULT_DATA_DIR=/home/cheng/code/mcp-vault/target/memory-review/local-model.09VRlm/data target/debug/mcp-vault diagnose-calibration --vault default --channel memory --signature sha256:6be8c0eb7db2a996456a70dd120aa4bd3b7f7caba9edc61a75fbf08f7dcd424c` and the same command with `--channel note --signature sha256:3d6d83481a370be90914428b9d6347998181cbd234599416f7707b971f93d1a0`: both exit 0. Outputs retained as `target/memory-review/local-model.09VRlm/memory-diagnostics.json` and `note-diagnostics.json`; both exactly reproduce their original reports and make zero new Provider requests.
- Real evidence: both channels have raw Recall@5 = 1.0 on both splits, including pure-semantic/cross-language questions. All 40 answerable holdout queries rank the correct document first in each channel. The selected memory floor 0.5099286437 passes calibration but holdout returns false results on 4/20 no-answer queries (20%, limit 5%). The selected note floor 0.4427765161 passes calibration but holdout retains only 27/40 answers (67.5%) and 7/20 pure-semantic answers (35%); its no-answer error is 1/20. These are measured real local model results, not synthetic-vector contract evidence.
- Concrete note failure: `holdout-q03` asks when the Elm compiler can reuse a cached syntax tree. Its correct document ranks first at cosine 0.40490973 but falls below the selected 0.44277652 floor and is rejected. Concrete memory false return: no-answer `holdout-n05` (Pine scheduler marketing revenue) scores 0.53324121 and passes the 0.50992864 floor. This confirms the current fixed admission thresholds fail to generalize to this holdout despite good raw answer ranks. It does not prove every possible threshold/policy must fail, or establish the cause of the earlier production report's 0.999999 fallback.
- Outcome: local network and real execution are verified; real quality remains **failed**, not accepted. No threshold was loosened or force-published. Original production failure remains unresolved because its saved vectors have not been inspected; both completed local runs differ from it by finding calibration-split passing thresholds (4 memory candidates, 1 note candidate). Follow-up policy changes need regression evidence and independent evaluation, rather than tuning to the inspected holdout and calling it new quality validation.
- [x] Diagnose that exact local run from its saved cache, compare unfiltered answer ranks with threshold trials and rejected gates, and record the real cause with evidence. Read-only recomputation will not spend another model request. Further model calls require the user-configured local workflow; the earlier restrictions on production credentials, actual production Vaults, push and deployment remain in effect.
- Recovery: the isolated test data and encrypted model settings persist in the above ignored directory across process restarts. Restart with the same command; do not substitute the repository's ordinary `data/` directory or remove this instance to retry a completed calibration.

### System audit — 2026-09-06 both memory plans

- Scope requested by the user: verify this plan's R1–R6, A01–A17, U1–U6, G1–G4/S1–S7 alongside T01–T28 in `completed/memory-simplification-and-retrieval-v2.1.md`. Current baseline is HEAD `6500214` plus the uncommitted diagnostic/index-mapping follow-up. Preserve those changes and all user-configured local model settings.
- [x] Re-read both acceptance matrices and existing test evidence. Default-feature workspace baseline passed again; frontend package commands hit the previously observed pnpm store SQLite problem, while the exact installed ESLint/Vitest/TypeScript/Vite commands all passed. Logs are under `target/memory-review/system-audit/`; final counts will be recorded in the audit report.
- [x] Audit remaining failure boundaries, add red regressions for newly confirmed defects, then repair and rerun affected tests. First observed defect: an obsolete calibration checkpoint can remain `running` after its job skips outdated signatures and completes.
- [x] Re-run real browser Admin workflows, migration preservation/recovery checks and public HTTP/MCP memory-read checks against disposable local fixtures.
- [x] Exercise real generation through the normal extraction/Provider/Vault Core path using the 15 existing `generation-coverage.json` sources, inspect actual outputs against their labeled facts, and report quality separately from deterministic fixtures.
- Authorization: user explicitly approved at most **15 actual real generation requests** for these synthetic sources and will bind `memory_extraction` in the local Admin page. This extends only the local evaluation scope; no production credentials/data, push or deployment. The evaluator must bound HTTP retries as well as top-level calls, read source configuration without changing it, and persist outputs after each case to avoid spending again after interruption.
- [x] Publish a consolidated audit report mapping plan requirements to actual tests, found/fixed problems, real-model results, and environment/quality gates that remain failed or pending. Do not equate a green unit suite with real semantic quality.
- New red regression `renaming_provider_should_preserve_valid_embedding_identity` failed on a display-only Provider rename: general configuration revisions were part of the embedding fingerprint. Fixed by additive migration 0018 and distinct Provider/model embedding revisions. Admin CAS still advances and rejects stale writes; endpoint/dimension changes still invalidate fingerprints; disabled checks are not bypassed. Existing stored settings/capabilities remain conservative fingerprint inputs.
- Migration 0018 copies current revision values to preserve existing hashes, never rewrites business vectors, and retires orphan calibration rows only when matching Vault/signature jobs are all terminal. The migration regression compares actual vector bytes/timestamps, calibration report/cache/budget, nontrivial legacy revisions 7/11, active-job exclusion and other-Vault isolation. Previous migrations remain unchanged. ADR-0024 and operational/data-model docs now describe the compatibility decision.

### System audit outcomes — 2026-09-06

- 321 default-feature workspace tests and 35 frontend tests pass; Clippy, migrations through 0018, public HTTP and four official MCP revisions pass. Native all-features test linkage remains blocked by the recorded ORT ABI mismatch; Litmus is absent. Exact commands and environment failures are in the [consolidated regression report](../reports/memory-system-regression-20260906.md).
- Three new defects were reproduced before repair: obsolete calibration status, display-only Provider edits invalidating embedding identity, and expanded report horizontal overflow. Regression logs retain red/green evidence. UI report uses wrapping preformatted text and a shrinkable grid, without changing report values.
- Authorized real generation completed: exactly 15 HTTP requests with mimo-v2.5, 15 synthetic sources, 132 items, 91/91 labeled facts observed, primary language preserved 15/15, unchanged-source retries skipped 15/15. Codex evidence review flags two temporal inferences in source 13; independent human review remains pending. No further paid request was made.
- Local existing deployment upgraded from schema 17 to 18 after consistent backup: requests 26 before/after, cached vectors and completed reports unchanged, obsolete run cancelled, user model configuration retained. No business notes were created by the upgrade.
- Real embedding holdout still fails the unchanged gates despite all answerable questions ranking correctly before thresholding. The exact original production fallback is not diagnosed without its saved vectors. Keep release/real-quality gates open; do not force publication or train against the inspected holdout.
- Decisions: request budget attaches at generation HTTP dispatch, including clones/retries; evaluator copies only explicitly authorized local configuration into disposable state, persists outputs after each source, and rejects reusing its output directory. Separate identity revisions retain existing fingerprints during migration while Admin concurrency revisions continue advancing.

### Follow-up — finish unresolved quality and environment work

- User challenged leaving semantic quality and environment gates unresolved. Continue investigation and repairs, preserving prior local changes. No additional paid model calls authorized.
- [ ] Re-evaluate admission limitations from saved real vectors; freeze any candidate policy using calibration data only, retain inspected holdout as regression evidence rather than new independent quality proof. Do not promise a passing model or force activation.
- [ ] Obtain isolated Litmus tooling and execute against a disposable fixture; investigate a compatible ORT runtime without replacing host system libraries.
- [ ] Record actual changes, regressions and remaining independently verifiable quality requirements.

### User-approved supersession — 2026-09-06

The user rejected built-in quality scores as a production gate and approved optional
model-guided grouping with hard default size bounds. Automatic-calibration acceptance
requirements are superseded by ADR-0028 and [the new ExecPlan](../completed/model-guided-note-chunking.md).
Preserve this plan's other R/Admin/source-ownership fixes and historical evidence.
