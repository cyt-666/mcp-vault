# ExecPlan：完整记忆单元与生产清零

创建／更新：2026-09-09。执行者：Codex。状态：实施中。

## 目标与用户可见结果

按照用户已确认的完整计划重建记忆系统。明确记忆直接保存；自动记忆选择完整原文单元；召回按任务选择完整内容；概览仅作有来源的导航。取消模型重写、事实合并、旧记忆兼容读取和旧数据转换。交付管理界面、离线清零命令、真实模型验收与生产重建。

用户已授权最终抛弃全部旧记忆，包括显式记忆、自动记忆、合并结果、旧排除／暂停记录及派生数据；保留笔记、附件、文件历史、账号、Vault 和 Provider 配置。生产不在当前主机且无法访问，用户已确认由生产端执行切换；本轮交付经过本地验证的离线命令、中文手册和验收步骤，不宣称已完成生产清零。已有真实模型测试授权继续适用于隔离副本和计划要求的验收。

## 约束与依据

- [产品要求](../../product-requirements.md)：便携知识、任务上下文、权限、离线可用与自动处理。
- [架构](../../architecture.md)：Vault 隔离、Vault Core 统一写入、仓储 SQL、双监听模块化单体。
- [接口](../../interfaces.md)、[安全](../../security.md)、[ADR-0033](../../adr/0033-source-preserving-memory-units.md)。
- [真实合并审查](../reports/lossless-consolidation-validation-20260908.md)：默认条件与操作顺序实际误放行，不能以同模型覆盖复核保证生成正文无损。

## 当前代码状态

工作树已有用户前序记忆实现的大量未提交变更，保留无关修改，不重置或清空工作树。MemoryService 包含旧生成、合并、校准和升级流程；state/current_memory 兼有来源集合与正式合并投影；最新迁移为 0027。MCP/Admin/worker 均直接消费这些契约。普通 Markdown 的 comrak 解析、Vault Core 的原子写入及来源集合的快照恢复可复用。生产不在本地实验目录；旧实验服务保持暂停。公共 MCP、Admin、后台任务和 UI 已切换 v3；旧格式读取与合并／校准运行模块已退役。

## 范围

本轮包含新模型、规范文件、查询仓储、原文选择、来源更新与删除、任务召回、自动概览、Admin/MCP、后台任务、一次性清零工具、容器与真实服务验证。无新项目配置、全局事实图、模型生命周期接口或日常人工候选审核。一般教程只作为知识资料，自动记忆聚焦约定、决策、状态和实际经验。

## 不变量与风险

原文、生成的检索说明、概览有独立身份；生成说明不得成为正文证据。每个自动单元的完整正文和必要上下文均由当前来源提取，保留源语言、步骤顺序、否定和限定。原文引用正确不代表来源内容本身真实，也不代表模型选择覆盖所有有用内容。

所有对象、缓存、队列和操作 Vault-scoped。新自动正文直接暴露原文，读取要求 memory:read 与 vault:read。模型与笔记均是不可信输入，不能指定文件身份、路径、版本或越过应用写边界。

新 schema 独立，旧 Markdown 和旧 ID 不自动接管。普通笔记、凭据、Provider、文件历史不属于清零对象。清零必须在离线独占下按持久化阶段执行，完成后重跑无操作。

## 设计

### 正文与自动生成

MemoryUnit 分明确／自动所有权，保留 ID、revision、完整正文、可选分类／有效期、来源与范围。明确记忆原样保存授权内容；自动记忆模型只选择服务生成的原文候选编号，并提供分类和检索说明。Markdown 分析形成完整段落／列表／表格／引用／代码块与完整章节；标题链、上级引言随候选保留。分批覆盖长笔记，超限完整单元保留知识导航并报告，禁止摘要代替。

每批结果在仓储按来源哈希、模型输入及生成规则保存检查点，全部批次完成后使用预期修订原子发布。整篇来源哈希变动立即使旧单元不可读；同身份／同哈希移动只更新导航。删除自动单元同时暂停该来源生成，其他有效单元保留；明确恢复后再处理。

### 召回与概览

召回先权限／当前资格过滤，再混合召回与相关性、多样性排序。上下文是排序信号，明确路径／主题过滤才可排除结果。默认 4096 Token、12 个单元、4 个资料线索，上限 32000 Token；所有正文、范围、来源和导航计入预算。超长单元提供读取入口，继续选择其他候选。只折叠正文与范围完全相同的展示，原始身份独立；相似度不授权删除。

正常召回无生成模型调用；Embedding／rerank 故障退回关键词。get_memory_overview 提供目录／主题概览与单元入口，生成文本单独标识，依赖来源与修订；失效则退回确定性导航。memory_overview 角色可覆盖生成模型，未配置时使用 memory_extraction。概览不参与自动提取或成为事实来源。

### 接口与 UI

保留 recall/get_memory/list_memories/remember/update_memory/forget_memory 能力名称，更新新契约，增加 get_memory_overview。memory/context 提供有界导航。移除合并／校准／迁移旧接口和 UI。管理端提供正文与来源、明确记忆编辑、自动项另存为明确记忆、删除／来源恢复、生成与概览进度、暂停续跑及故障诊断。

### 清零与生产

新增前向迁移建立新记忆表／初始化状态，保留已应用 SQLx 迁移文件，绝不转换旧记录。initialize-memory --discard-legacy-memory 在服务停止后列出无正文清理清单、清理旧登记命名空间／表／任务／向量并持久化完成状态。旧受管内容永不作为普通来源扫描。

生产：停止写入和旧服务 → SQLite/Vault/历史成对备份 → 离线初始化 → 新服务启动 → 保留数据、登录、WebDAV/MCP、新读写验收 → 开放服务 → 自动全量重新生成与概览构建。旧暂停与排除不继承。维护窗口内可成对回滚；开放新写入后不可直接用旧整库覆盖。

## 工作步骤与进度

- [x] 2026-09-09：用户确认范围、无项目配置、来源删除暂停、验收后自动全量生成及旧记忆全部抛弃。
- [x] 2026-09-09：核对现有服务、仓储、Markdown 解析、接口和运维边界，建立计划／ADR。
- [x] 新单元与规范格式、仓储和发布恢复。
- [x] 2026-09-09：完整原文章节／上级引言解析的 4 项回归通过；独立新仓储编译通过；明确记忆精确字节和 35 单元／3 批次原子发布、实例续跑、删除暂停及冷重建两项集成回归通过。
- [x] 原文候选选择、分批检查点及来源控制。
- [x] 2026-09-10：更新自动选择 v3 契约，固定合成边界 fixture，补 fake Provider 的未完成／已发布 profile 缓存隔离回归，并完成 11 来源受控选择验收；运行链通过，语义选择质量未通过，后续改进待定。
- [ ] 新召回、完整预算、来源权限与导航。
- [x] 自动概览、MCP、Admin、worker 纵向整合。
- [x] 删除旧运行链与兼容契约，交付离线清零及中文切换手册。
- [ ] 全工作区、UI、协议、容器和清零恢复工程门禁。
- [ ] 30 来源／60 任务冻结语料及真实模型对照验收。
- [ ] 生产端由用户执行清零切换、保留数据核对与真实重建验收；当前开发环境无法代为完成。

## 决策

用户批准的计划优先于旧 ADR 的模型合并和兼容约束；已落地迁移 checksum 仍需保留以安全打开共享操作库。可复用通用原子发布／来源资格代码，但最终运行不能读取旧记忆或依赖旧合并状态。不得为了缩小改动保留假成功兼容分支。

## 发现与调整

公共运行链已切换到 v3；旧模型重写／合并／校准模块和对应路由已删除。前序未提交实现的退役源码保存在忽略的开发归档中，历史 SQL 迁移文件没有修改。新的明确记忆复制操作保留当前来源定位，UI 支持本机 Obsidian 来源链接和有界目录浏览。

2026-09-09 工程验证：明确字节、70 单元／3 批次、权限、预算、来源变更、概览依赖和冷重建回归通过；v27 离线清零中断恢复、多 Vault 隔离、普通文件／账号／Provider 保留及完成幂等通过。Admin UI lint／33 项测试／构建及真实浏览器编辑、复制、删除恢复、分页和概览流程通过。官方 MCP 2026-07-28 使用固定源码版本通过当前 expected-failure 基线；存在基线内 prompts/list caching-hint 限制，不能说所有规范项零失败。Linux aarch64 容器构建完成，最终源码构建与启动验证继续执行。

真实验收冻结 30 个来源、60 个任务（27 篇原始笔记与 3 篇受控困难来源），清单 SHA-256 为 `1925bd6f74cd2b85439ca4ba12e2bf90568ce449614432077e465a8452db4efd`。初始真实 MiMo 输出误选整篇 Codex 架构总览，保留失败证据后调整选择规则，同语料重跑。修订规则尚非盲测集，报告必须说明开发语料属性，不把模型自述答案可用性当成分数。

真实长任务发现 UTF-8 末字无换行导致行号计算切到字符内部并 panic；后台续租监控被分离，形成永久 running。修复以字节行索引计算位置，覆盖 LF／CRLF／CR 和多字节尾部；任务同步／异步 panic 转为明确失败并停止续租。生产 panic 日志只记录可信代码位置，省略可能包含正文的 payload。相关 6 项解析和 2 项 panic 收尾／脱敏回归通过。SIGTERM 纳入正常退出路径。原始运行失败记录保留在隔离验收目录中。

首轮明确记忆测试在手动移除投影及其幂等映射后仍期望旧幂等键拒绝，测试前提不成立。将精确请求冲突断言移到映射仍存在时验证，通过；没有修改实现来维持错误测试前提。失败及复测保留在 target/memory-v3-development/vertical-tests*.log。

所有尚未解决的限制保留在本节和验收报告，不降低断言来掩盖失败。

### 2026-09-10 自动选择质量边界收紧（实现与受控验收完成，语义选择质量未通过，后续改进待定）

本轮针对真实审查中一次性 PPT 制作／演讲规格与旧参考架构混入长期记忆的问题，只调整选择调用契约，不改变召回排序、项目配置、冲突解决或来源硬上限。`crates/memory/src/v3/selection.rs` 现在明确：一次交付细节不会因真实发生或出现“已决定”而升级；混合来源逐单元保留有明确采纳关系、理由和范围的项目决策／经验；参考架构事实不能自动成为当前约定，未注明日期不能推断当前；有意义的持续偏好、接续所需状态及带前提／例外的最小完整操作单元仍可选。来源路径和文件类型只作理解线索。

选择契约版本升级为 `memory-source-unit-selection-v3`，评价 profile 版本为 2。二者以及完整 `SYSTEM` 都进入 `extraction_profile_hash`，因此已发布 set 或已验证 batch 不会误命中旧选择边界；变更后由正常 generation/backfill 或 `include_evaluated` 重评并原子替换，未擅自对现有 Vault 运行重评。新增冻结合成质量用例覆盖混合 PPT／真实决策、非 PPT 一次交付、旧参考架构、持续偏好、带前提例外的操作、当前阻塞状态和假设方案；fake Provider 回归只证明缓存隔离与机制，不证明语义质量。本轮已按用户恢复授权完成 11 来源选择运行和逐项人工审阅，结果记录在 `target/memory-selection-quality-20260910/review-v3-01/final-report.md`，选择语义质量未通过。

后续有界验收工具增加 selection-only 模式：11 个冻结来源、无真实 Provider 的 manifest 校验和 3 个 example 测试通过，结果只证明选择输入／输出契约、完整正文落地和错误边界，不证明真实语义质量。该模式不启动现有服务、不调用 embedding 或 overview，也不把 11 源开发语料当作盲测 gold。前轮 recall 对照曾有 76/89 embedding 覆盖，live-01 的误填 2030 已按授权修正为 2048；未统一 embedding 配置、覆盖率与预算前，不能把新旧 recall 差异全部归因于 selection prompt。11 来源选择验收已完成但未通过；更大 30 来源／60 任务对照、embedding／recall 和生产验收仍待后续。

本轮最终工程门禁记录在 `target/memory-selection-quality-20260910/`：最终 fmt、workspace clippy（all-targets/all-features）和 workspace test（all-features）退出码均为 0；`live_memory_v3` example 三项测试退出码为 0。早先无权限本地监听失败日志保留，后续授权重跑通过。

本轮最终收尾增加 `live_memory_v3` 的 selection-only 工具回归，3 项全部通过，验证错误参数在触碰输入前失败、来源清单必须唯一匹配哈希，以及工具只使用 selection schema 并写入完整原文单元。真实语义冻结集 11 个来源已运行并完成人工审阅，选择质量未通过；该冻结集、selection-only 工具和 2048 配置修正仅记录实验边界，2048 的修正不触发重新嵌入。工作区全量测试曾在主密钥解析缺陷修复前出现 `fresh_install_provisions_only_the_managed_master_key` 的 `Authentication(MasterKeyUnavailable)`，该历史失败由本轮修复并复测通过；更大语义对照、embedding／recall 和生产切换仍 pending。

## 验证

命令：cargo fmt --all --check；cargo clippy --workspace --all-targets --all-features -- -D warnings；cargo test --workspace --all-features；pnpm --dir frontend/admin lint/test/build；项目官方 MCP conformance；文档、容器构建和真实双监听检查。

回归覆盖：原文逐字一致、编号伪造／越界、完整结构、TUI 默认与 GPU 顺序、否定／例外／作用域、来源变化／移动／删除、删除暂停与恢复、并发修订、多 Vault、最小权限、崩溃恢复、冷重建、输出预算、超大首项、概览失效、Provider 降级、长笔记尾部、提示注入、清零保持非记忆数据及初始化幂等。

真实验收冻结至少 30 个来源和 60 个任务（至少 20 个无答案／冲突／困难负例）。使用真实原始笔记的隔离副本，不导入旧 325 条生成记忆。固定任务与预算对比笔记混合检索，记录覆盖、错误、任务结果、Token、延迟和费用。关键命题关系全部通过，整体任务结果不得低于基线。人工审阅真实输出；不以 fake 或模型自评冒充质量证明。

2026-09-10 工程收尾检查：`cargo fmt --all --check`、workspace clippy 和 `cargo test --workspace --all-features` 均通过；迁移测试现比较数据库成功应用版本序列与嵌入式 `MIGRATOR` 全版本序列，保留历史 `LEGACY` checksum 校验、重复运行和 schema-drift fail-closed 测试。此前 fresh-install 的 `MasterKeyUnavailable` 是主密钥解析长度缺陷，历史失败证据保留在 `target/memory-selection-quality-20260910/final-workspace-retry-fixed.log`，修复后最终工作区证据在 `final-workspace-auth-fix.log`。auth 最终 29 项通过，其中 secret focused 子集 7 项通过；`live_memory_v3` selection-only 3 项通过。真实 11 来源语义冻结已完成但选择质量未通过；更大真实 Provider 对照和生产端切换仍未完成。

同日修复安装主密钥解析的长度识别缺陷：旧逻辑先无条件移除末尾 LF/CRLF，导致合法的 32 字节原始密钥本身以 LF 或 CRLF 结尾时被截短；此外，原始密钥最后一个字节为 CR 且文件另附单个 LF 时，也会误删密钥中的 CR，fresh-install 因此偶发 `MasterKeyUnavailable`；单独 CR 不会被旧逻辑移除。现按既有 raw32／hex64 契约先识别完整长度，再仅接受一个 LF 或 CRLF 分隔符；不接受额外空白、双换行或非法长度。合成回归覆盖 raw 尾 LF／CR／CRLF 与文件后缀空／LF／CRLF 的 9 种组合、hex64 后缀和拒绝边界；旧实现失败证据与修复后 auth 7 项通过结果保存在 `target/memory-selection-quality-20260910/`。最终 `cargo test --workspace --all-features`、workspace clippy 和 fmt 均通过；11 来源真实选择已完成但质量不通过，更大对照与生产验收仍待完成。

## 回滚与恢复

开发期间保留无关工作树变更；隔离测试独立目录。生产只在前置门禁通过后切换，清零阶段日志可恢复。源文件写入始终经过 Vault Core 及其修订／历史／outbox 机制。备份只作运维回退，不进入新模型可读路径。

## 结果

### 2026-09-10 selection prompt 对照暂停

用户下班前要求暂停真实测试，撤销此前 launch 授权。对照 runner 从未启动，`run-01` 不存在，Provider/transport 请求 0 次；没有在途请求，因此无 usage 可报告，也没有需要终止的实验进程。冻结输入、expected、anchor、v3/v4 SYSTEM 和 44 条 paired request plan 均保留在 `target/memory-selection-prompt-comparison-20260910/frozen/`。后续不得自动恢复、补跑或重放；继续必须等待用户明确授权。

### 2026-09-11 selection prompt 对照首错停止

用户明确继续后，已用冻结输入启动 `comparison/run-01`。共 37 次 transport attempt，36 次成功；第 37 次在 F05 v3 首 batch 发生 Provider schema 错误后停止，无重试或补跑。S19、S20、S02、F01、F02、F03、F04 共 7 个来源的 14 个 source-arm 完整；F05 v3 已尝试但失败，F05 v4 与 F06—F08 未执行，8 个 source-arm 保持 `partial`。结果、机械覆盖、逐单元分类和审查报告均已保存；后续等待用户明确，不自动补跑。

### 2026-09-11 恢复旧 selection prompt

按用户要求，`crates/memory/src/v3/selection.rs` 的 `SYSTEM` 精确恢复为冻结的 selection-v3 文本，`EXTRACTION_PROMPT_VERSION` 同步恢复为 `memory-source-unit-selection-v3`。`extraction_profile_hash` 同时包含 prompt version 和完整 SYSTEM，因此 v4 的 profile/batch 不会被复用；恢复只改变后续 profile 身份，不自动重新提取、不写数据库，也不改变 batching。真实对照产物和其他工作区改动保留。

实施中。工程、真实模型与生产验收分开记录，三者未完成前不归档为全部完成。
