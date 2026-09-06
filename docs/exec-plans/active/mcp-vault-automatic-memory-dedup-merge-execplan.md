# ExecPlan：记忆自动去冗余、跨文档同义合并与现有版本无操作升级

## 1. 状态、基线与执行方式

- 仓库：`cyt-666/mcp-vault`。
- 建议存放：`docs/exec-plans/active/automatic-memory-dedup-merge.md`。
- 编写日期：2026-09-06。
- 状态：**M0–M6 已实施并通过功能回归；M7 工程检查、旧库自动启动黄金场景与官方 MCP 验收通过。Litmus props/locks 存在已在原始 HEAD 复现的既有失败，M7-A 不冒充全绿；M7-B 真实模型质量及生产部署 pending。2026-09-06 最终记录见第 19 节。**
- 本计划自包含；Codex 只需要本文件和仓库，不需要此前聊天、执行包或人工整理的重复清单。
- 兼容目标：用户正在使用的 current-only、单文档来源集合、一次 `memories[]` 提取、精简 MCP 返回值的版本。此前对话最后确认的提交标识为 `be30f701195e363aae6e85fa16f85868192f1f1b`；它仅是定位参考，不是本次确认的远端 HEAD。
- 证据边界：起草时未取得远端最新源码；本次执行已按用户要求复核本地 HEAD `c82a633ef7244c8983d1756246d3e0baf9678f94`，未切换提交。下文设计经实际代码落实，差异和验证证据见第 19 节；不以远端版本覆盖本地工作。
- 执行前读取 `AGENTS.md`、`PLANS.md`、当前 memory-system/interfaces/data-model 文档，以及涉及 current-source-owned-memory-sets、MCP presentation、检索诊断的最新 ADR。此计划有意扩展“单来源提取结果不能共同支持正式记忆”的旧限制，必须新增 ADR 写明取代范围；保留其他安全约束。

**执行要求：从 M0 开始实现并测试，不再只输出另一份计划。当前工作树已有修复，用回归测试证明后复用；不要覆盖用户未提交修改。**

## 2. 用户目标与不可妥协的升级场景

### 2.1 自动、无操作前置

系统必须自动完成：

1. 一条记忆正文内部的重复句式精简，以及同一文档提取集合内的重复/被完整覆盖条目清理。
2. 不同文档中语义等价事实的合并，包括中文与英文的等价表达。
3. 新建、修改、重新生成、删除来源之后的增量维护。
4. **已有重复数据的升级后补处理，而不仅仅对新生成数据生效。**

用户正常升级并启动服务即可。不要求点击迁移、开启去重、选择重复项、批准合并、重新绑定模型、重新生成记忆、重建全库向量、上传标注集或调用手工 API。不要求每次打开 Admin 页面后才触发。

管理页面只能提供状态、诊断及可选重试，不是正常运行前置。数据库迁移和本任务的记忆兼容转换由系统自动完成，不能复用一个必须用户提交 `preflight_hash` 和确认字符串的手工迁移流程作为必经入口。

### 2.2 黄金兼容场景

```text
旧版本数据库 + 原 Vault + 当前文档来源集合
已有重复记忆 + 已有有效向量
已有可调用的记忆提取模型及原有 Provider 授权
        ↓
升级二进制/镜像并启动
        ↓
本地格式兼容接管（无需生成模型）
        ↓
后台自动枚举既有集合，去除同篇冗余
        ↓
自动查找跨文档候选，验证同义关系，合并正式记忆
        ↓
list/get/recall/资源/Admin 使用同一个正式记忆视图
        ↓
重复事实只作为一个正式记忆出现，来源仍可追溯
```

“无感”表示无需人为推进、正常查询和笔记使用不被整库长任务阻塞，不表示任意规模的数据在启动瞬间已完成所有语义判断。后台处理期间允许尚未处理的重复暂时存在；每个已提交的合并必须一致可见。不能把“任务已创建”当作“去重已完成”。

有配置、权限和可用模型时必须自行完成。服务故障、模型关闭或额度不足时明确显示可恢复的阻塞原因；状态恢复/预算补充后自动继续，不丢内容、不伪造完成，也不要求用户重新配置本来正确的环境。

### 2.3 本期不是再做强制校准

沿用当前版本的检索行为及最新 ADR：生产检索不新增“必须先校准”的门槛；合并也不依赖一个用户手工填写的 calibration 报告。向量仅用于发现候选，语义合并由受约束判断确认。不得把旧自动校准执行计划整套搬回本任务。

## 3. 已观察问题与测试锚点

本次只读复查：`list_memories(source_path=深度学习/损失函数.md, limit=100, include_details=true)` 返回完整 7 条，无后续分页；其中“Neural network training flow…”和“One-sentence summary: Loss measures error…”重复覆盖同一流程。该集合使用 `ownership=note_derived`、`note_set_id`、来源修订，以及 `_mcp-vault/memory/current/sources/<file_id>.md`。

本次 `recall("StandardScaler 为什么只能在训练集上 fit？", max_results=4, max_related_notes=0)` 的前两条分别来自《自动微分、拟合与回归评估》和 Weekend 2，中文/英文都表达仅在训练集拟合、对验证测试集使用训练统计量、防止数据泄漏，`degraded=[]`。

此前完整抽查还发现：总览与专题重复提取、表格与一句话总结被拼成重复正文，以及跨文档的 GPU/训练流程/上下文压缩知识重复。此前统计的 306 条/25 篇只属于当时快照，本次未重新全量统计，**不得在程序或测试中硬编码这些数量或真实 UUID**。

这些样本证明存在内容重复，不证明所有相近条目都该合并，也不证明整个知识库可以减少到某个预设数量。回归测试使用合成等价样例，不提交真实 Vault 正文、账号信息和稳定 ID。

## 4. 范围与非目标

### 本期必须交付

自动同篇去冗余、跨篇等价合并、正式记忆统一读取、当前来源贡献维护、现有版本自动接管和回填、模型/任务恢复、规范 Markdown 可重建、MCP 精简协议兼容，以及相关前后端状态和回归测试。

### 本期不做

跨 Vault 合并；自动合并显式用户记忆；知识冲突自动裁判；互补事实合成为新推论；全库每次重摘要；复杂历史生命周期；通用知识图谱；运行时必须新增的 reranker/数据库/外部服务；每次 recall 调用生成模型；按数量指标强行压缩记忆。

**只做展示层隐藏重复不算完成。** 原文和当前来源证据允许保留相同文字，但模型/API 的正式记忆对象、正式向量索引、列表计数和管理操作不能仍将同义条目作为数个独立对象。

## 5. 核心设计：按文档保留贡献，按事实提供正式记忆

```text
来源 A 的当前提取集合                    来源 B 的当前提取集合
  A1：训练集拟合标准化参数                 B1：fit on train only
              \                           /
               \                         /
                正式派生记忆 M：同一个事实
                支持：A1、B1；来源：A、B
                一个 ID、一个正文、一个修订
```

### 5.1 三类数据与唯一责任

| 数据 | 责任 | 正常 MCP 是否作为记忆返回 |
|---|---|---|
| 原笔记 | 原始内容与明确语境 | 通过笔记工具读取，不混成正式记忆 |
| 当前文档贡献集合 | 当前文档提供的去冗余事实、限定条件、来源身份及版本 | 否；仅作为受控来源证据/管理详情 |
| 正式记忆 | 一个可独立理解的事实及其当前支持贡献 | 是；所有记忆入口一致 |

显式记忆保持原身份、正文、元数据、修订和编辑规则，不参与本期自动合并；即使与派生内容相似也不能被删除或重写。派生合并保持 `ownership=note_derived` 的业务含义（文档支持），不是继续保证“只有一个来源”。

### 5.2 逻辑实体（不是固定 SQL 契约）

优先复用现有 source-set 和规范解析器，只增加必要映射：

- `SourceSet`：Vault、稳定来源 File ID、当前提取输入哈希、来源修订、set revision、paused、当前贡献。
- `Contribution`：稳定 ID、所属 source set、正文及其 hash、显式范围/有效期/类型等兼容元数据、规则版本。每条只属于一个文档。
- `FormalMemory`：稳定 ID、正文、revision、content hash、选择的代表贡献、兼容元数据。
- `Support`：formal memory ↔ contribution，指向被验证的确切贡献 hash/来源版本；同一当前贡献至多归入一个正式记忆。
- `MergeDecisionCache`：基于输入 hash、必要语境、模型配置及规则版本的派生判断缓存，不能成为事实的唯一来源。

同一文档多个重复贡献先缩减，不能令同一文档以三条表述变成“三份证据”。`source_count` 按当前唯一来源 File ID 计算，不等于 contribution 数，不当作事实置信度或排名奖励。

### 5.3 规范持久化与可重建

建议继续用现有 `current/sources/<file_id>.md` 保存该来源**当前去冗余贡献**，新增受保护的 `current/facts/<formal_id>.md` 保存正式正文及支持映射。实际路径遵守项目已有保留目录规则；显式记忆路径不变。

正式文件含模式版本、ID、revision、代表贡献以及精确支持引用；引用至少包含来源/贡献标识及用于验证的输入 hash。不得只把聚类关系保存在 SQLite 中，导致从 Markdown 重建时重新冒出所有重复或必须重新调用模型。

重建规则：已发布规范文件 + 当前来源身份是依据；贡献集合不自动以第二种入口投影成正式记忆。没有支持的正式文件不可见并自动清理；坏引用记录诊断，不凭路径相似猜测来源。文件移动沿用 File ID；同路径删后新建不是同一来源。

所有规范写入走 Vault Core，投影与任务走 state repositories。跨文件写入和 SQLite 不是天然一个事务，复用现有 prepared operation/操作日志/发布恢复机制，不声称按顺序写几个文件就实现了原子提交。

## 6. 合并规则：等价、包含与相关必须分开

### 6.1 同篇去冗余

同一来源的一次提取结果中处理：

1. 严格重复：正文和语义元数据相同，只保留一个稳定条目。
2. 同义重述：正文详解、总结、表格描述等表达同一个完整命题，保留一条完整且不重复的表达。
3. 完全覆盖：同来源 A 完整蕴含 B，B 没有独立新增信息，可移除 B；A 不能因“更长”而天然获胜。
4. 单条正文内部同义句重复：允许有界等价改写，删除复述，不删除主体、条件、数字、公式或例外。

保留公式、例子、不同机制等独立信息；教学例子不是仅因看起来基础就删掉。导航和修订信息可在未来提取 prompt 中减少，但本期存量去重不能借机把所有“低价值”独有内容无依据清空。

先复用生成时同一 `memories[]` 请求内部的去冗余要求，再做轻量本地规范化及必要的批次判断。存量集合直接处理已有结果，不重新运行全部源笔记提取。新请求失败或输出截断不当作成功空集合。

同源包含消除后，把旧条目映射到存活贡献以完成受影响支持的重新验证；**不能把原短贡献跨文档的其他来源直接升格为支持更长正文**。必要时保留分开的正式记忆，不牺牲来源真实性。

### 6.2 跨篇只合并完整等价命题

允许：语言、措辞、格式不同，但各自支持相同完整信息，含必要主体、项目、版本、时间、否定、数值、单位、作用范围与限定条件。

不允许直接合并：

- 同主题但不同命题，例如 KV Cache 的用途与它不属于长期记忆。
- 一般规则与特定项目实例，仅前者能覆盖后者的一部分。
- Weekend 1 的 lr=0.1 与 Weekend 2 的 lr=0.001 等不同实验结果。
- 计划完成与已经完成、建议采用与已采纳、用户与被引用第三人。
- 否定/更正/矛盾事实，以及“默认/可能/仅在条件 C/总是”等不同强度。
- 不同有效期、项目版本或不能确认属于同一语境的同字面描述。
- 一条多事实大摘要与只覆盖其中一项的短句。

可缺省的分类标签不是模型推断事实的证据。null 与 fact 不必机械判冲突，但明确不兼容的语义类别不得合并。未知语境不能被视为任意语境；从可用的源标题/明确元数据读取必要背景，禁止猜测。

### 6.3 精确匹配也必须有语境

可对空白/编码做用于候选查找的规范化，但精确自动合并的安全键应包含正文与可确认范围。不要对代码标识符盲目 lowercase，不删除否定、单位、标点中的数值含义，也不把一般 Unicode 正常化结果等同于事实相同。

“默认端口是 8080”出现在两个不同项目中，不能因字符串完全一致自动合并。只有明确范围一致的等价记录才走无需模型的快路径。

### 6.4 不以向量阈值或连通分量决定合并

向量相似只选候选，不代表等价；不采用“cosine > 0.9 就删掉一条”。也不采用 A≈B、B≈C 的传递闭包直接合并 A/B/C。

新成员加入一个正式记忆时必须支持**该正式正文的完整命题**，并与成员已知范围一致。代表内容变化后重新验证受影响成员，不能沿用旧 pair 结论。必要时分组拆开；不把相似度聚类替代来源证据判断。

第一版代表正文优先选择一个现有、语言合适、限定清楚、被全部有效支持共同认可的贡献；不拼接所有成员的独特细节，不每增加一个来源就重写一遍。中文来源的新增提取默认中文；存量英文条目不因本次迁移被全库翻译。

## 7. 自动判断内核与候选搜索

### 7.1 使用已有模型，不增加用户配置任务

默认复用 Vault 已绑定、已允许使用的记忆提取模型进行必要的等价判断；不要求再配置“合并专用模型”。沿用原 Provider 的数据发送范围、密钥、网络策略、输入上限与并发限制，不把内容发送到新的服务。

已有 embedding 用于候选发现，正文与 profile 没变即可复用。查询/文档 task 和维度按现有输入规则处理；缺向量时可退回词法候选或后台生成必要输入，不能假装所有跨语言重复已检查。模型停用期间继续精确安全去重并保留其他记录，恢复后自动续跑语义步骤。

### 7.2 候选生成

同源集合：先分解明显重复桶，随后有界批次覆盖语义候选；集合超过单次模型输入时分页，不截断后说整篇已去重。

跨源：合并严格键候选、词法/实体/显式范围候选及有效向量近邻。内部候选搜索不得复用最终 recall 的去重/准入门槛，避免尚未合并的候选被筛没；仍保留 Vault、当前来源、授权与输入有效性校验。

以 Top-K 和批次限制控制成本，但要持久记录尚未处理的候选游标。不能把“每条只检查 5 个邻居”称为全库语义无重复。对小型固定验收库需要覆盖标注重复组；大型库报告候选覆盖和待处理范围，不做无界全库 O(n²) LLM 请求。

每个新增/变更贡献需要与未合并单例和已有正式记忆双方比较；不能只比较旧 formal 向量而漏掉同一批新贡献之间的重复。来源集合 hash 变化与决策规则版本变化应失效相应缓存，不在每次重启重算。

### 7.3 判断输出是提案，不是写命令

拟定小型输出：`equivalent / left_covers_right / right_covers_left / related / different / uncertain`，配简短差异/保留条件；跨源只有 equivalent 可自动合并，同源 covers 必须验证被覆盖条目无独有信息。

使用请求局部引用映射到已有对象，检查重复、缺失、越界与不认识的引用。模型不能指定数据库 ID、文件路径、来源版本、删除命令、权限或任意存储操作。无效输出只能有界修复或标记暂未判定，不能使原记忆消失。

不能仅信任模型给的 confidence。正反例和保留条件有测试支撑；不确定保持分开并缓存当前判定，不要求用户审核。真正的 transient failure 自动退避重试；语义 uncertain 不每小时重复付费，输入或判断规则改变后再评估。

候选 hash、必要语境 hash、模型配置、prompt/schema 版本构成判断缓存键。已持久化结果重用；外部调用成功但进程未保存即崩溃的窗口，若 Provider 不支持幂等则不能承诺费用 exactly-once，使用稳定请求标识、及时检查点和预算限制重复。

## 8. 发布、更新和删除的完整语义

### 8.1 正式 ID 与修订

迁移单例优先沿用其旧记忆 ID。首次合并多个正式对象，按确定规则选择一个存活 ID（如最早已发布 ID，平局按字典序），不是按每次模型输出顺序随机选择。正文或支持/元数据改变时增加正式 revision；检查 `expected_revision` 的写入不能接受旧语义范围。

被吸收的旧正式 ID 从公共记忆库移除，`get/update/forget` 返回既有 not_found/冲突契约并要求重新查询。**不默默把旧 ID 的删除请求重定向到一个支持范围更大的合并对象**。内部旧 ID→贡献的迁移映射不作为模型可用的历史别名。

模型重复提交、任务重试和崩溃恢复都复用准备时保存的 ID/字节；同一操作不能再创建第二份正式记忆。拆分误合并或来源条件变化时，存活子组按确定规则保留原 ID，其余子组新建稳定 ID；模型输入不负责这些决定。

### 8.2 原子发布与并发

本期优先复用已有每 Vault 写入序列/锁与修订前提，采用**局部受影响集合**的发布操作，不为每次更新重建全 Vault。生成/判断在锁外进行，发布时锁内重新检查所有依赖的 source set、contribution、formal revision、内容 hash、暂停状态和作用域。

两个 worker 同时把同一新事实创建为不同单例，精确安全键要有数据库约束/冲突重试；语义冲突由串行局部发布及提交前重新读取候选处理，不依赖只在进程内有效的锁。

文件准备、提交、投影和清理有持久操作日志。一次合并必须让公开读路径看到“旧一致视图”或“新一致视图”，不能同时看到 M 和已被吸收的 A/B，也不能在清理 A/B 后还没发布 M 时丢失知识。可用已有读写门/发布指针；数据库事务和规范文件恢复协议要配套。

来源变更与删除优先于待发布的合并。revision/hash 变化令旧提案失败并自动重排，不将已经过时的判断直接应用到新正文。

### 8.3 来源更新行为表

| 事件 | 必须行为 |
|---|---|
| A 内容没变，手动重新生成 | 新输出成功后替换 A 的贡献；等价贡献可继续指向 M，无必要不改 M 正文/向量 |
| A 内容改变，重新提取未完成 | A 的旧贡献立即失去当前支持资格；M 若仍有 B 的有效完整支持则保留，否则对模型不可见 |
| A 不再包含事实 | 移除 A 对 M 的支持，不影响 B；不要把旧来源合并进新集合 |
| A 出现新事实/不同条件 | 新贡献重新匹配；不能直接覆盖仍由 B 支持的 M |
| A 返回合法空集合 | 只撤掉 A 的贡献；不能清空其他来源支持的记忆 |
| 提取超时/输出损坏 | 不当作空集合；按当前原文有效性决定旧支持是否可用，自动重试 |
| A 移动/改名但 File ID 和输入不变 | 更新当前路径展示；无需重新提取和重做语义判断 |
| A 被删除 | 立刻撤销其支持；后台实际清理相关贡献、空正式记忆与索引 |
| 最后有效来源消失 | 正式记忆立即不可读，并最终物理移除，不只换成 archived |

若原代表来自 A，但只剩 B 有效，必须验证 B 支持当前全文，否则从 B 重新选代表或暂时不返回。不能只删除来源数组中的 A，却继续返回只有 A 支持的细节。

最终读取再次校验当前支持，不等待后台清理才停用失效来源。若重选代表需要模型判断，读取路径不即时调用模型；应使用已经验证的安全代表或暂时不返回该对象，并排队修复。

### 8.4 正式记忆删除

`forget_memory(M)` 删除整个当前正式对象及它的所有已归并来源贡献关联，不是仅移除当前展示的第一个来源。已知支持贡献从当前规范来源集合移除，相关来源的自动提取按既有删除策略一并暂停；不触发模型、不要求原 Provider 仍存在。

这是一次用户删除操作的服务器语义，不要求逐篇确认。界面和工具描述应提前说明可能影响多个来源；正常自动去重/合并**不触发暂停**，必须与用户 forget 区分。

旧生成、旧合并、启动回填、向量重建、Markdown 投影重建都不能复活被删贡献。删除与新支持并发时必须检查最新正式 revision；不能漏掉删除快照之后加入的来源而仍报告已完整删除。

只保留必要、有限且模型不可读的操作日志/删除防重信息；不把完整旧记忆正文作为默认可搜索“历史”。原笔记的正常读取不等于记忆回收，不能承诺删除记忆会删掉源笔记里的同一事实。

不引入永久语义禁词/禁止事实表。因此本期保证已知贡献、历史任务和后台重建不会复活；**未来一篇全新文档再次写同一事实，或用户明确恢复原来源，是新的输入事件**。不能声称在不保留任何排除规则的情况下永远识别并禁止所有未来改写。

### 8.5 元数据与引用

独立显式记忆所有非默认 importance/confidence、tags/entities、有效期及修订保持不变。派生合并不把来源数转为置信度，不自动取最高 importance/confidence；按稳定代表及明确兼容规则派生，未知保持未知。

有效时间、项目、否定等属于命题资格，不是简单取所有来源日期的并集。互不兼容者分开。`created_at` 和整理时间不成为事实成立时间。

来源展示去重到当前唯一文档；每条 path/revision 来自验证过的真实来源。各来源都必须支持所展示的完整正文，不能把“讨论过相同主题”也写进 sources。source_path 过滤通过 support join 查询，A 或 B 都能找到同一个 M，但只能返回一次。

## 9. 自动维护任务与成本边界

### 9.1 同一个 ensure 入口

新增/复用应用层入口，例如 `ensure_memory_dedup_scheduled(vault, reason)`，由以下事件自动调用：

- 服务启动并加载已有 Vault 后；包括没有发生过任何新文件事件的旧数据。
- 某 Vault 从初始化/维护状态恢复可用。
- 新建/修改/重新生成来源集合成功。
- 来源贡献被删除/失效，需要撤销支持或重选代表。
- embedding/记忆提取 Provider 恢复、缺失向量补齐。
- 去重规则版本升级，或自动任务发现未覆盖的当前数据。

首次升级的来源枚举必须翻完分页，不只读前 50/100 条；启动任务可按当前高水位扫描，再处理扫描中发生的增量事件。记录集合 hash/贡献 hash/规则版本，重新启动不重复处理未改变数据。

### 9.2 默认启用与公平调度

对原本已允许记忆提取的 Vault，默认启用自动去重，复用原模型和授权；不增加必需开关。沿用安装级限额与 Provider 上限；可提供高级预算配置，但正常用户不必先填表。

建议工程初始值：每 Vault 一个整理 worker，全局小并发；一次模型判断有界条目/输入字节；每片处理有限请求后保存游标并让出资源。若现有系统没有预算，本期默认每 Vault 同时 1 个任务、安装级最多 2 个并发整理任务；每任务片最多 16 个模型请求或运行 5 分钟即 checkpoint；每 Vault 滚动 24 小时最多 256 次判断请求及 4 MiB 输入（先达到者生效），每次仍受 Provider 更小的限制。预算窗口结束自动继续。这些是工程初值，不是质量参数；M0 可按现有更严格策略缩小，修改需记录原因和默认行为，不得要求用户先手动配置。

达到窗口预算时保存检查点，下一窗口**自动续跑**；不能永久卡住等管理员点击。同步、普通读取、用户显式写入优先于存量整理。查询期间不等待一次整库模型请求。

任务状态是维护操作状态，不给正式记忆新增 stale/archived 生命周期。输出清楚区分：等待模型/额度、处理中、已覆盖当前高水位、失败待自动重试、有保守未合并项。模型 uncertain 是正常“保持分开”，不是数据错误或必须用户审批。

### 9.3 去重与恢复

持久任务键至少含 Vault、处理范围、数据/算法版本；复用现有 durable jobs/checkpoint，不能用每次新的 UUID 绕过去重。进程重启保留尝试次数、退避时间、已完成判断和未完成游标。

瞬时网络故障有限重试并自动调度下次；权限未允许或模型已停用不反复打 API，状态改变后续跑。质量不确定不无限重复问同一模型。长期无法处理要显示具体原因，但不得删内容或谎报无重复。

## 10. 现有版本的自动兼容与回填

### 10.1 M0 必须核实的旧格式

读取实际线上版本相符的源集合 Markdown、显式记忆格式、SQLite migration、当前 item 的 ID/revision、向量输入与元数据、paused 标记及 pending jobs。构造对应的旧版本测试快照；不能只从一份已经新格式化的测试库验证升级。

旧路径和返回字段以前存在不代表实际 HEAD 完全一致；执行者必须记录适配映射。尤其核对旧 `note_set_id` 单值、`canonical_path`、source revision、`ownership` 和精简 presentation。

### 10.2 自动接管分为本地与语义两步

**A. 本地兼容接管：不需要 LLM。**

1. 增加必要 schema 与规范版本兼容解析，保留原显式记录与暂停状态。
2. 从当前有效源集合创建贡献映射和对应单例正式记忆；保留可复用的旧 ID/revision。旧集合中的内部记录转为贡献角色，不再同时成为第二套正式对象。
3. 在一个 Vault 的本地接管完整可验证后切换公共查询到正式视图；准备过程不向模型暴露临时文件。数据大时允许分页准备，但不能在同一响应里混用新旧权威集合。
4. 用现有事件/版本前提处理接管时的用户编辑和删除；若实际代码需要短暂提交门，限于本地切换，不在门内运行模型。没切换前保留旧一致只读/写入路径；新事件自动补齐。

**B. 后台自动整理：使用已有提取结果。**

1. 同源去重复/内部复述，保留独有事实。
2. 建立跨源候选并验证等价，按局部事务合并正式对象。
3. 更新规范支持映射和检索投影，清理被吸收的正式记录。
4. 按保存游标持续处理，覆盖所有可读的旧集合；没有新文档事件也会执行。

两步均无需手工 migration API、UI 预检批准或用户逐条选择。可做内部 preflight 自检和自动操作日志，但不能把报告交给用户等待确认才开始。

### 10.3 向量和索引迁移

保留现有可复用向量：相同正文/块输入、模型配置、维度与 task 时可以改映射或复制绑定到存活 formal ID，不重新请求 embedding。改变代表正文、分块字节或 profile 时只更新受影响对象。

迁移/整理可以保留专用的贡献候选向量索引，但它必须与正式检索命名空间分开；**正常 recall 只按正式对象评分，不能因 M 有 5 个来源就读出 5 次或加 5 次分**。贡献证据不能作为普通笔记重新索引进 related_notes。

正式对象多块按最佳有效块聚合后计算对象名次，继承以前的请求过滤和预算修复；不混用旧 profile、不把原块序号当对象 rank。精简内容时输入 hash 变了就不能沿用旧向量假装有效。

### 10.4 升级完成与未完成

使用内部版本/水位记录：本地接管完成、已检查来源/贡献、候选已评估/尚待评估、formal 数、合并数、保守保留数、失败和重试。数量变化仅用于诊断，不设“必须删掉 50%”的目标。

不能只设一个 `migration_done=true` 就不再处理后续增量；也不能每次重启都重扫并重新向模型发送全库。

升级初期可暂时显示旧重复，后台提交后逐步减少；不承诺在模型不可用时自动识别所有跨语言同义句。设计必须交付真正可调用既有模型的路径，不能把 FakeProvider 演示称为产品已实现。

## 11. MCP、Admin 与写入兼容

### 11.1 所有正式入口共用一个仓库

检查并改造 `recall`、`list_memories`、`get_memory`、`forget_memory`、显式 update、MCP resources/动态上下文、Admin 列表/详情/计数、后台摘要/索引导出。不能只改 recall；不能用旧 ID、`include_details=true` 或旧规范目录读取到被吸收的正式记忆作为当前记录。

保留精简默认输出：ID、content、revision、ownership、必要限定和去重来源路径即可。`include_details` 才给支持映射/来源版本/整理诊断；显式请求评分时保留评分，但不默认倾倒贡献正文和所有 pair 判断。

### 11.2 字段、分页与修改

- `ownership=note_derived` 可继续表示文档派生；在工具描述中明确可有多个来源。
- `sources[]` 按当前来源 File ID 去重，默认可导航；大量来源采用明确上限、计数和可继续获取的详情，不能把响应无限放大。
- 不把单个 `note_set_id` 伪装成全体所有者。兼容单来源时可保留单值，多来源时置空/省略并在 details 增加复数引用；更新 schema、前端类型、说明及测试。
- `canonical_path` 指向正式文件而非原笔记；来源仍通过 `sources[].path` 读取，工具描述不能误导模型去读系统内部文件。
- `source_path=A` 与 `source_path=B` 查询同一 M，结果均只一条；结果计数基于 formal ID，不能是 JOIN 后支持行数。
- 分页使用稳定 formal ID/cursor；并发合并使游标视图过期时按已定义契约重启或返回可识别提示，不能静默重复/漏掉未变化对象并称完整。
- `revision` 仍是写操作前提；自动来源增减/合并变更支持范围时旧 revision 失效，避免一次删除意外影响新增来源。
- `update_memory` 继续只编辑 explicit，派生内容通过原笔记更新，不让自动合并改写用户显式声明。

### 11.3 读取资格与隔离

所有候选/成员/正式记录按 Vault 隔离。来源适用范围、有效期、类型、重要性过滤和 current-only 必须在合并后仍生效；不能仅因某个来源 current 就豁免另一条请求资格。

若存在更细来源权限，任何返回的正文/来源计数必须仅由本次授权范围内可用支持导出；代表来自不可见来源时不能泄漏它的专有细节。没有 `vault:read` 的调用保留既有对 ordinary notes 的限制，不因聚合增加信息外泄。

来源失效后，即使旧正式向量和缓存暂未清理，也不能恢复旧正文或被删来源；缓存键包括公开对象修订和有效支持版本。

### 11.4 管理界面只观察，不审批

显示一个去重后的正式记忆及其多来源；管理统计不把内部贡献当额外记忆。可查看整理进度、失败原因和保守保留项，但不能要求用户确认每一组或手工启动升级。

暂停来源管理保持独立于正式列表。某篇来源贡献已删光、某条正式记忆全部删除后，原有暂停状态仍能查询和按现有显式恢复操作处理。自动整理不能擅自恢复来源。

模型与界面工具描述更新后实际 tools/list、schema、处理函数及前端数据加载一致；不恢复以前的默认大响应。

## 12. 实施里程碑（M0–M7）

### M0 — 基线、兼容格式和失败测试

读取本地实际 HEAD、工作区及项目规范。按本计划而非旧全局 consolidation 设计建立 ADR；特别核对此前关于“正式记忆只有一个来源”的约束如何被本次决策替代。

导航优先检查：

- `crates/memory/src/service.rs`、`model.rs`、`current_markdown.rs`：提取、发布、删除、当前结果与规范格式。
- `crates/state/src/current_memory.rs`、`background.rs`：投影、过滤、快照和任务。
- `crates/indexer/src/lib.rs`、`crates/providers/src/`：候选索引、向量身份、对象聚合及 Provider 调用。
- `crates/mcp/src/lib.rs`、`presentation.rs`：所有读写入口与精简输出。
- `crates/admin-api/src/`、`frontend/admin/src/`：实际列表/详情/来源/状态调用。
- `crates/server/src/workers.rs` 及当前启动 composition root、`migrations/`、既有恢复与迁移测试。

这些是已有审查的搜索起点；如果结构变了，记录真实符号，不创建另一个平行 service。确认 SQL 留在 state、模型调用走共享 ProviderService，避免 memory/indexer 循环依赖。

创建旧版本快照与合成重复/非重复样本，先证明当前实现同篇和跨篇重复；建立公开 list/get/recall 的 baseline。记录哪些检查受环境限制，不能以 fake 结果冒充真实模型行为。

**验收：** 新 ADR、实际接口/格式映射、旧升级 fixture、可重放红色测试齐全；不硬编码生产 UUID、25 篇或 306 条。

### M1 — 贡献/正式记忆模型与无模型接管

实现规范格式、SQLite 支持关联、stable ID/revision、old parser 兼容以及自动本地接管。先以单例正式记录跑通全部读写，保持旧语义等价。

切换前后 source pause、显式元数据、来源 hash、有效期和权限不得丢失。系统保留目录不参与普通笔记索引，避免新 formal/贡献文件触发自我提取循环。

**验收：** 旧库在没有模型调用时可自动升级至可用一致视图；当前可见事实数量/内容不凭空减少；Markdown 重建无需 LLM 恢复单例与引用；实际接口只认一个正式存储入口。

### M2 — 同篇/正文内部去冗余

实现第 6.1 节的精确去重、等价重述、受验证包含消除和正文复述精简。存量直接读取既有集合，新增生成 prompt 不要求多余账本；当前 valid source 支持的内容才可处理。

保持 unique information，不为减少条目强制每篇固定数量。新内容 hash 变化只失效必要向量；仅支持成员变更不把全文无理由改写。

**验收：** “Loss 流程＋一句话总结”和“模块表格＋同义句”被自动消除冗余，独立公式/例子/限定仍在；处理失败不导致合法旧内容变成空集合。

### M3 — 跨文档等价合并内核

实现候选 union、必要背景、批次判断、结果缓存、完整命题验证和稳定代表选取。合并中英文同义事实，保守保留 related/covers/uncertain 跨源记录；不靠高 cosine/字符串相同就改写归属。

支持 group-to-member 验证；scope 冲突和链式相似不能扩大组。保留显式记忆排除、去重来源计数、不增加来源数 boost。

**验收：** 标注为同义的跨源条目形成一个正式 ID，多来源真实支持；“KV Cache 用途/非长期记忆”、不同实验与否定状态均不误合并。

### M4 — 更新、删除、竞争与崩溃恢复

实现局部发布、来源 delta、合法空集合、代表失效、最后来源清理、合并后 forget，以及旧 pending task 的版本防护。修改来源 A 不能覆盖 B 的事实；删除 M 不能仅删 A。

拆分无效组和异常回滚使用现有贡献/操作日志自动恢复，不需要用户手动再提取全部文档。暂停只约束来源重新提取，不妨碍当前有效剩余贡献做去重；此前已删除的条目绝不可凭历史重建。

**验收：** 故障注入下公开读视图一致、来源不被错误认定、已删除内容不复活、无 Provider 仍可本地删除、重复任务不重复写入。

### M5 — 后台自动补处理与增量调度

接入服务启动、Vault ready、来源集合发布、Provider 恢复等事件。读取所有旧集合分页、保存高水位/工作游标、持久去重和预算续跑，不依赖 UI 或新文件事件。

实现真实可调用既有提取模型的判断路径；没有凭据时测试使用注入 fake，但生产实现不能只有占位函数。已有有效业务向量复用，新增候选/正式向量按必要输入生成。

**验收：** 第 13.1 节黄金场景在旧快照上完成同篇与跨篇自动处理；重启后不重跑成功工作、额度恢复后无需人工继续、暂停标记不被撤销。

### M6 — 全入口兼容、Admin 状态、性能

让 MCP 各读入口、Admin 列表/详情/统计、resources 和相关导出统一正式视图；来源筛选和多来源引用正确，旧 ID 明确失效，默认精简保持不变。

删除/编辑的修订校验与批量来源范围在工具描述和界面体现。来源列表不因为没剩记忆而消失；维护面板只观察和可选重试。

验证分页、计数、结果预算、缓存、新旧索引隔离和普通相关笔记检索不泄露内部贡献；读取路径不调用生成模型，单文档改动不触发全库模型整理。

**验收：** 所有入口不重复展示已合并组；计数一致、跨 Vault 不泄漏、重复来源不影响分数；默认工具结构没有重新膨胀。

### M7 — 全面验证与交付

运行下列矩阵与工程检查；更新架构、数据模型、接口、MCP 描述、升级说明、运维状态和本 ExecPlan。提交实际测试结果及未验证项目，清楚区分机制测试、真实模型质量和生产部署验证。

**验收：** 自动升级与增量处理都完整，不需要用户额外操作。不是“把 API 做完了等待后续 UI/后台”，也不是“只在 recall 临时去重”。不得把尚未执行的真实模型测试标成通过。

## 13. 验证：黄金场景、回归矩阵与命令

### 13.1 黄金自动升级集成测试

输入：从正在使用的旧格式构造数据库与 Vault，含同源重复、正文内部重复、中英跨源同义、独立非重复/矛盾/不同时间事实、显式记忆、暂停来源，以及已生成的有效向量；没有新格式标记和去重任务。

启动修复后的完整应用/worker，不打开 Admin、不调用迁移/运行 API、不改模型绑定、不改笔记、不重新提取全库。使用可记录调用的 FakeProvider 运行机制测试，另行提供同一真实 Provider 执行路径。

必须断言：

1. 自动本地接管后可正常查询，旧唯一信息、显式 metadata、paused 和原笔记字节不变。
2. 后台自动完成既有数据的同篇、句内和跨篇处理，不只枚举数据或写 queued 状态。
3. 标注 StandardScaler 等价组在 list 和 recall 只占一条，sources 同时包含 A/B；get 是同一正式对象。
4. 标注 Loss 流程/摘要冗余只剩一个表达，独有公式/例子不丢。
5. 显式记忆未参与合并，protected 非等价事实分开；没有新增编造事实。
6. 已吸收 ID 不再通过旧 API/资源返回成另一条正式记忆；内部贡献不混入普通 notes 搜索。
7. 除实际改动的输入外，原有效向量未重新请求；没有全库 `memory.extract` 任务。
8. 重启后相同规则/内容不新增模型请求和正式记录；新增另一篇同义笔记会自动加入现有 M。
9. 测试全程不需要手工填写阈值、报告、确认哈希、重复选择或模型专用配置。

建议测试名：`upgrade_existing_current_source_sets_auto_dedups_without_user_actions`（拟新增）。

### 13.2 必须覆盖的测试矩阵

| ID | 场景与断言 |
|---|---|
| D01 | 同源完全重复去成一条，保留稳定贡献和必要 metadata |
| D02 | 同源详解/一句话总结相同信息被精简，额外公式/例子独立保留 |
| D03 | 同一条正文前后重复复述被精简，条件/数字/符号不被删 |
| D04 | 同源包含消除后不把其他文档的短证据升格为支持更长命题 |
| D05 | 中文/英文完整等价跨源形成一个正式 ID，来源各自可核验 |
| D06 | 同主题不同事实（KV Cache 用途与非长期记忆）不合并 |
| D07 | 不同项目/版本/时间/设备/实验设置即使字面相同也不误并 |
| D08 | “计划/完成”“建议/采纳”“可能/总是”“用户/第三人”区分 |
| D09 | 否定、矛盾、单位和关键数值差异不被正常化抹掉 |
| D10 | A≈B、B≈C 而 A 与 C 不等价时不传递聚类成一个事实 |
| D11 | 模型响应引用越界/漏项/无效枚举/截断不导致原知识消失 |
| D12 | unknown/uncertain 保持分开，不要求用户审核也不无限收费重问 |
| L01 | A/B 支持 M；只删除 A，M 与 B 保留，A 不在 sources |
| L02 | 删除/失效最后来源，M 即刻不可读并自动清理索引/规范记录 |
| L03 | 修改 A 成另一事实，不覆盖 B 的原 M，形成正确 delta |
| L04 | 原代表来源失效，剩余来源不支持全部正文时不泄漏独有细节 |
| L05 | 合法空提取只移除该来源，失败/截断不当作空结果 |
| L06 | rename/move 沿用 File ID 不重提取；同路径删除重建不继承 |
| L07 | 新增、修改、删除过程中源 hash/revision 变化令旧判断拒绝提交 |
| L08 | source_path=A/B 均返回同一 M 一次，计数不按 support 行数 |
| F01 | forget 合并 M 删除全部已知贡献且暂停相应来源，无需模型在线 |
| F02 | 自动去重移除重复贡献不暂停来源，不能误调用用户 forget |
| F03 | pending 提取/合并/升级回填与向量/Markdown 重建不复活删除项 |
| F04 | 合并与删除并发、组支持变化导致旧 expected_revision 拒绝 |
| F05 | 旧被吸收 ID 不重定向执行更大范围删除；explicit 更新不变 |
| U01 | 黄金旧版本快照无需任何 UI/API/重绑/新文件事件自动处理 |
| U02 | 源集合超过 100 条、查询分页/候选窗口有界时仍自动续页 |
| U03 | Vault 启动时未 ready、稍后可用时会补处理；不是仅启动一次机会 |
| U04 | 重启/并发触发仅一次同版本工作，已持久判断和向量复用 |
| U05 | Provider 断开/预算耗尽后检查点保留，恢复/下窗口自动继续 |
| U06 | 已暂停来源标记保留；可整理当前有效剩余贡献但不重新提取 |
| U07 | 旧 snapshot/pending job 自动安全收口，不能先删掉后说迁移完成 |
| U08 | 接管中用户编辑/删除，切换到新视图后最新变更不丢不复活 |
| U09 | schema 已升级而后台未完成时正常查询可用，不混用两个权威入口 |
| P01 | 各 memory 工具、Admin、资源、导出共用正式对象，不仅 recall 去重 |
| P02 | include_details/旧 canonical 引用/旧 ID 不能绕过可见性恢复重复 |
| P03 | 代表正文、来源、revision、ownership、单/多 note_set 字段语义一致 |
| P04 | 默认输出仍精简，来源很多时有界且可继续查看，真实字段参与预算 |
| P05 | 显式非默认 confidence/importance/tags/entities/有效时间往返不变 |
| P06 | 两 Vault 同文本/ID/路径不能共享私有贡献、判断、缓存、计数或任务 |
| P07 | 权限/有效期/类型/重要性和来源健康在合并后仍有效 |
| P08 | 来源数量/重复向量块不叠加 score，最佳有效块按对象排名 |
| P09 | 普通 notes 索引不包含 reserved facts/contributions，不自我提取循环 |
| P10 | source 删除后旧 vector/cache 不能泄漏正文或路径，读前资格复核 |
| R01 | 多文件提交/投影中断后恢复是旧一致视图或新一致视图 |
| R02 | 从规范 Markdown 重建支持关系无需模型、不重新生成每个成员正式项 |
| R03 | 并发等价单例创建最终收敛一个正式对象，ID 和 revision 稳定 |
| R04 | 误合并/不再成立的组可基于当前贡献自动拆开，不需要重提取全库 |
| R05 | 小增量只处理受影响来源/候选，recall 不调用生成模型 |

上述为拟新增或应复用的断言，不代表现有测试名或已执行结果。可拆成更多单测，但不能省略负例、旧版本快照或恢复测试。

### 13.3 质量数据与指标

最少准备 60 个合成判断案例：同源重复/包含/句内重复、跨源等价、跨语言、相似但不可并、否定/数值/版本/状态差异，并包含链式陷阱。每例标注可保留命题、允许合并组、必须分开的组与支持范围。数据可公开，不提交真实用户库。

至少另有 20 个查询案例，分别检查重复是否占结果名额及独有知识能否召回；同时包含无答案与相邻主题，避免把无关问题伪装成去重成功。

报告：

- merge precision：被实际合并的可比较成员关系中，标注同义的比例，列绝对错误数。
- duplicate-group coverage：标注重复组被正确归一的比例，不用“模型调用完成率”替代。
- unique-information retention：升级前独有受支持信息仍能从正式内容/来源读出，不含为去重而创造的新结论。
- provenance precision：每个展示来源是否支持展示全文，不能只检查文件存在。
- retrieval duplicate occupancy：同一查询结果中等价事实额外占用的槽位，配独有命题 Recall@K，防止全空结果通过。
- 规范文件/正式记忆数量、复用和新增向量量、模型调用/输入量、维护耗时及查询延迟。

CI 的关键保留/隔离/删除/修订断言须全通过；受保护非等价案例不能有误合并。真实模型目标：merge precision 至少 99%，重复组覆盖至少 90%，关键独有事实保留和正确来源断言全部通过；同时提供分子分母和小样本局限。门槛是目标不是已测结果，也不是承诺任意私有语料绝不出错。

FakeProvider 只证明机制；必须实现并交付调用真实既有模型的判断和回填代码。没有授权运行真实模型时记录 pending，不能把真实路径留成 TODO 后归咎于缺凭据。质量不达标保留数据、失败案例并修复，不能降低门槛或删负例凑成功。

### 13.4 工程命令

从仓库根目录先记录：

```bash
git status --short
git rev-parse HEAD
rustc --version
cargo --version
pnpm --version
cargo metadata --no-deps --format-version 1
```

根据实际 workspace 名称，优先使用现有 crate manifest 分别运行：

```bash
cargo test --locked --manifest-path crates/memory/Cargo.toml --all-features
cargo test --locked --manifest-path crates/state/Cargo.toml --all-features
cargo test --locked --manifest-path crates/indexer/Cargo.toml --all-features
cargo test --locked --manifest-path crates/providers/Cargo.toml --all-features
cargo test --locked --manifest-path crates/mcp/Cargo.toml --all-features
cargo test --locked --manifest-path crates/admin-api/Cargo.toml --all-features
cargo test --locked --manifest-path crates/server/Cargo.toml --all-features
```

最终执行：

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

路径/脚本若在实际版本调整，先读取并记录真实替代命令；不要声称不存在的入口运行成功。迁移、MCP conformance 和 release 脚本先读用法再运行。无需为本期额外引入另一个 Python 服务。

复用现有质量评测 harness，增加 `dedup/merge/upgrade` 场景和 case-level JSON 报告；如果新建 runner，在 M0 写出可执行入口，并在交付时记录真实命令。正常产品后台工作不依赖用户安装 cargo 或运行 example。

保存 baseline/after、HEAD、fixture hash、模型/规则版本、命令退出码、失败名及环境限制。工程测试不能通过删除断言、忽略测试、关闭语义/权限、固定返回测试 query 达成。

## 14. 升级安全、恢复与回滚

### 14.1 自动保护不变成手工审批

迁移前内部检查旧 schema、规范文件可读性、源引用与磁盘空间。正常数据通过后自行继续；异常记录隔离报告并让其他可处理来源继续，不以“请先下载预检报告并批准”阻塞全库。

保护被本次规范转换/整理触及的数据，优先复用现有操作日志和版本恢复能力；必要的 before-image/manifest 放在受权限保护、模型与普通笔记索引不可访问的运行时位置，并有清理期限。不是建立一套给模型搜索的旧记忆历史，也不把用户所有原笔记复制成新的长期数据源。

不能为一次兼容升级擅自 reset pipeline generation 或执行破坏性旧迁移。数据库变更使用前向新增 migration；若需重建表，验证主键、外键、索引、旧行和未完成任务完整。

### 14.2 回滚层次

- 某次合并不再成立：利用仍有效的当前贡献和受控操作信息重建正确分组，分配稳定正式对象，并更新索引；不要求全库重新提取。
- 模型服务不可用：暂停语义工作，精确安全路径及查询继续；不回滚已经验证并发布的有效组，不清空数据。
- 新后台任务有性能问题：可以停止调度，已发布正式格式和读取仍工作；不是让所有用户必须执行一个回滚向导。
- 程序版本回退：验证旧 binary 能否读新 schema/规范，不能假定 additive SQL 就完全兼容。若不兼容，在获授权维护环境恢复成对数据库和 Vault/运行状态；只回滚一个文件或只还原 SQLite 不保证一致。

自动迁移与清理已经授权为产品预期，但 Codex 开发执行阶段**不得访问生产凭据、付费调用真实模型、实际修改用户 Vault、push 或部署**，除非用户另行明确授权。该开发限制不允许被误做成产品每次合并都要用户确认。

### 14.3 故障注入点

LLM 已返回但结果未保存；判断保存后规范文件未发布；部分 formal 文件已写但投影尚未提交；已吸收记录清理一半；代表来源同时被编辑；forget 与新来源合并同时发生；升级水位扫描中途新增笔记；Provider 恢复时多个触发一起排队。

每个位置都检查：正式读视图一致、独有事实不丢、删除不复活、原来源正文不变、重复任务不创造第二个正式对象、旧判断不覆盖新内容，且其他 Vault 不受影响。

## 15. Progress｜实际执行进度

- [x] 2026-09-06：编写本独立计划；只读复查同源 Loss 冗余及跨源 StandardScaler 重复。最新 GitHub 源码读取失败，未冒充完成新代码审查。
- [x] 2026-09-06 M0：本地 HEAD、规范/ADR、当前格式和红色测试已核实。
- [x] 2026-09-06 M1：贡献/正式模型、规范恢复和无模型自动接管已完成。
- [x] 2026-09-06 M2：同篇/句内去冗余、包含关系保真测试通过。
- [x] 2026-09-06 M3：跨源跨语言等价合并、负例与来源完整性通过。
- [x] 2026-09-06 M4：变更、删除、并发、崩溃恢复和重建通过。
- [x] 2026-09-06 M5：现有部署启动补处理、持续增量、预算自动续跑通过。
- [x] 2026-09-06 M6：所有 MCP/Admin/资源/计数统一，精简与权限兼容通过。
- [x] 2026-09-06 M7-A 功能/工程：格式、Clippy、全工作区 all-features、前端、文档、HTTP smoke、官方 MCP 与旧版本黄金升级通过。
- [ ] M7-A 全部协议检查无失败：Litmus props/locks 尚有原始 HEAD 同样存在的失败；官方套件不覆盖 2025-03-26 / 2024-11-05 默认 server 场景。详见第 19 节，没有把 skipped 当通过。
- [ ] M7-B 真实模型质量/部署：pending；本次没有授权生产凭据、付费调用或部署，已交付真实 ProviderService 调用路径。

每里程碑补具体代码、测试和日期。只写了 prompt、后端 API、一个状态面板或 FakeProvider 测试，不得标记整个功能完成。

## 16. Decisions 与待核实事项

### 已定决策

- D1：单文档提取集合保留为当前贡献；正式记忆允许多来源，同义合并不改变原文。
- D2：存量升级和日后增量全部自动，不把手工预检/批准/重新生成作为前置。
- D3：跨源仅做完整等价合并；同源允许受验证的冗余/包含消除，不能跨源抹掉独有细节。
- D4：不引入全局记忆历史状态机、跨主题大摘要或查询时生成式整理。
- D5：所有正式读入口共用统一对象；内部贡献保存证据，不当作重复的正式记忆返回。
- D6：不自动合并/删除显式记忆，不用来源数量提升置信度；当前语义和有效期条件保留。
- D7：删除正式 M 处理所有已知支持，按已有策略暂停来源；普通自动整理不暂停。
- D8：旧被吸收 ID 失效，不隐式转发写操作；稳定 ID/revision 和本地安全发布优先。
- D9：已有有效向量和提取结果复用；不恢复强制校准门槛，不要求专用合并模型。
- D10：不确定保持分开，保留比错误归并更重要；有限候选搜索不能被描述为证明全库不存在语义重复。

### M0 必须填实的事项

当前 schema/规范解析器实际名称，startup/Vault ready 入口，source-set 发布与恢复机制，旧 pending job 的兼容方式，Provider 授权与限额框架，精简 MCP 对 details/单来源字段的约束，Admin 分页及状态展示的真实组件位置。

若本地实际决策与本计划冲突，记录冲突并按用户本次明确目标新增 ADR，不无声恢复旧方案。只有涉及不可逆丢数据、权限扩张或生产操作时才需要用户额外决定；普通实现细节自行选择并用测试支撑，不把自动化需求再变成一串人工确认。

## 17. 最终交付与 Codex 启动指令

### 17.1 交付内容

```text
实际起始/最终 HEAD：
本地旧格式兼容映射与规范版本：
M0–M7 完成情况和对应代码位置：
黄金升级场景实际运行方式（无 UI/API/重绑/重新提取）：
同篇、句内、跨篇、中英重复组的前后对照：
独有事实和每个来源完整支持的保真结果：
正式对象 ID/revision/旧 ID 行为：
来源更新、删除、暂停与自动恢复维护的测试结果：
已有向量复用量、新增调用、后台资源和查询开销：
Markdown 重建、迁移与故障注入证据：
所有工程命令、退出码、失败与未运行项：
真实模型/生产部署是否已验证：
生产数据是否被操作（默认应为否）：
发布说明、自动后台状态、回滚限制：
```

**完成标准：从用户当前含重复的旧版本正常升级并启动，系统自行接管并自动整理同篇/跨篇数据；新增或变化内容继续自动处理；正式列表/查询只呈现统一事实，来源不造假，独有信息不丢，旧数据不复活，原笔记不被改写。**

后台的语义能力必须完整实现，而不仅是排了一个不会执行的 job。真实模型效果未获验证时可以区分工程完成与部署待验收，但不能把“需要真实模型验收”变成用户日常运行必须手工参与。

### 17.2 直接给 Codex 的启动指令

```text
执行 docs/exec-plans/active/automatic-memory-dedup-merge.md。
先读 AGENTS.md、PLANS.md 和最新 ADR，核对实际 HEAD 与工作树，从 M0 开始实施，
不要只输出另一份计划。不要强制切回旧提交，不覆盖我的现有修改。

保留按文档管理的当前贡献集合，新增多来源正式记忆；
完成同篇条目去重、正文内部复述精简，以及跨文档/中英文等价事实合并。
不是只在 recall 隐藏重复，list/get/recall/资源/Admin 和正式索引必须统一。

最重要：兼容我现在已经有重复记忆的版本。
升级启动后，无需手工迁移、确认、重新生成记忆、重建全库向量、重新绑定模型、
打开管理界面或调用 API，自动接管并在后台处理全部现有可用数据。
日后文档新建/修改/重生成/删除要增量自动维护；重启、额度恢复和 Provider 恢复后自行续跑。

复用已有提取模型、授权和有效向量；不增加必须配置的专用模型或强制校准步骤。
不恢复历史状态机/全局大摘要，不合并互补、矛盾、不同条件或用户显式记忆。
保留全部独有信息、真实来源、权限/有效期过滤、修订前提、真实删除及暂停语义。

按 M0–M7 更新实际代码、测试和文档；尤其运行旧版本黄金升级、同义正例、
非等价负例、并发删除/更新及规范 Markdown 重建测试。
不得硬编码生产数据、假装所有高相似都等价、通过清空内容降低重复率、
删测试或只接按钮/只写 TODO 的后台内核。

未经另行授权，不使用生产凭据/真实付费 API，不修改真实 Vault，不 push/部署。
产品自动工作不需要每次用户确认；开发测试权限限制不能变成产品手工前置。
最终交付代码、兼容迁移、完整自动流程、实际验收和发布/回滚说明。
```

### 17.3 本计划的证据范围

依据用户已接受的 current-only 架构、此前重复抽查、2026-09-06 的 Obsidian Notes 只读返回和本轮自动化/升级要求。源码路径为此前审查导航，本次未成功读取 GitHub 最新内容；所有结构调整须由执行者从本地实际代码验证。测试例是设计要求，文档中没有宣称已经运行仓库测试或修好了线上版本。

## 18. 2026-09-06 前次中断记录（历史，不代表最终状态）

实际基线 `c82a633ef7244c8983d1756246d3e0baf9678f94`。用户给出的短文件名不存在，本文件是实际执行计划，开始时仅本文件未跟踪；未覆盖其他用户修改、未切换 HEAD。Rust/Cargo 1.94.0，pnpm 11.19.0。

M0 映射：`current_markdown::{parse_note_set,render_note_set}` 使用 `mcp-vault-memory-set/v2.1`；`memory_current_items` 直接属于 `memory_note_sets`，`current_eligibility_sql` 检查 canonical revision 和 source hash。`apply_prepared_note_set`/`memory_note_set_snapshots` 处理发布与恢复。server composition root 注册 worker，周期 ready-Vault 遍历负责补偿。`judge_memory_equivalence` 复用 extraction_runtime/ProviderService 的现有绑定、权限与网络边界。ADR-0030 已记录本次目标的限定取代范围；没有声称正式对象已发布。

已实现的子项：

- 提取时精确去重保留大小写、兼容字符、kind/tag 差异，避免检索归一化误删独有事实。
- `deduplicate_source_exact` 复用 prepared snapshot，保留旧 ID/metadata/暂停状态，去除既有集合内严格重复；修复中间删除后 ordinal 不连续导致 Markdown 无法重建的问题。
- `memory.deduplicate_source` 在启动和周期 ready-Vault reconciliation 自动入队并由真实 worker 执行；分页枚举现有集合。
- `judge_memory_equivalence` 实现真实已有 Provider 的结构化调用、严格局部引用校验和 explicit 排除；0020 保存 Vault 隔离的判断缓存与实际 dispatch 的滚动预算。该函数只返回提案，尚无正式多来源发布，不能把缓存 equivalent 当作合并完成。

未完成：M0 完整黄金旧数据库快照和 60/20 语义质量集；M1 多来源正式对象/规范/无模型接管；M2 句内及同义/包含精简；M3 候选 union、组成员验证与发布；M4 多来源删除/拆分/并发/崩溃恢复；M5 语义检查点/Provider 恢复与预算自动续跑；M6 正式视图 MCP/Admin/资源统一；M7 全矩阵和真实质量。所有原里程碑复选框保持未完成。

机制回归 `existing_v21_exact_duplicates_compact_without_extraction_and_rebuild_stays_compact`：测试夹具构造 v2.1 三条含重复的集合（含暂停）；精确整理后两条，独有事实保留，移除 ID 不可读，重建后仍两条，第二次整理为零，不新增提取调用、不改原笔记。它直接调用 source handler 的应用方法，不是完整应用启动黄金测试，也不证明跨文档或中英合并。

### 最终验证记录（工程子集，不能据此勾选 M7）

起始/结束 HEAD 均为 `c82a633ef7244c8983d1756246d3e0baf9678f94`，未 commit/push/部署。未访问生产凭据、未付费调用真实模型、未修改生产 Vault。

| 命令 | 退出码/结果 |
|---|---|
| `git status --short`、`git rev-parse HEAD`、`rustc --version`、`cargo --version`、`pnpm --version`、`cargo metadata --no-deps --format-version 1` | 0，已核实本地基线 |
| `cargo test --locked --manifest-path crates/memory/Cargo.toml --all-features` | 0，36 tests（含 21 项集成测试） |
| `cargo test --locked --manifest-path crates/state/Cargo.toml --all-features` | 修复 migration version 19→20 断言后 0，52 tests |
| `cargo test --locked --manifest-path crates/indexer/Cargo.toml --all-features` | 0，14 tests |
| `cargo test --locked --manifest-path crates/providers/Cargo.toml --all-features` | 101，fastembed/ort 原生链接环境失败，未把关闭 feature 当通过 |
| `cargo test --locked --manifest-path crates/mcp/Cargo.toml --all-features` | 0，25 tests |
| `cargo test --locked --manifest-path crates/admin-api/Cargo.toml --all-features` | 0，25 tests |
| `cargo test --locked --manifest-path crates/server/Cargo.toml --all-features` | 0，45 tests |
| `pnpm --dir frontend/admin install --frozen-lockfile` | 沙箱首次 1；获准沙箱外重跑 0 |
| `pnpm --dir frontend/admin lint` | 0 |
| `pnpm --dir frontend/admin test` | 0，35 tests |
| `pnpm --dir frontend/admin build` | 0 |
| `cargo fmt --all --check` | 0 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | 0 |
| `cargo test --locked --workspace --all-features` | 101，原生链接符号缺失 |
| `cargo test --locked --workspace` | 0，默认功能完整工作区补充回归，不替代 all-features |
| `bash scripts/check-docs.sh` | 0 |
| `bash scripts/release/check-migrations.sh` | 修复迁移版本断言后 0，12 项 migration gates |
| `bash scripts/conformance/mcp.sh` | 沙箱首次 1；获准重跑仍 1，npm GitFetcher 打包错误 |
| `git diff --check` | 0 |

失败输出原文摘录（完整命令日志保留在 `target/automatic-memory-dedup-validation/`）：

```text
# 本地 HTTP FakeProvider 测试首次被沙箱阻止，获准重跑后通过
Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }

# pnpm 首次安装失败，获准重跑后通过
[ERR_SQLITE_ERROR] unable to open database file
[ERR_PNPM_META_FETCH_FAIL] GET https://registry.npmmirror.com/pnpm: fetch failed

# 全功能 workspace/provider 原生链接仍失败
rust-lld: error: undefined symbol: __isoc23_strtol
rust-lld: error: undefined symbol: __isoc23_strtoll
rust-lld: error: undefined symbol: __isoc23_strtoull
rust-lld: error: undefined symbol: std::__cxx11::basic_string<char, std::char_traits<char>, std::allocator<char>>::_M_replace_cold(char*, unsigned long, char const*, unsigned long, unsigned long)
collect2: error: ld returned 1 exit status

# 官方 MCP conformance 尚未进入有效场景执行
npm error GitFetcher requires an Arborist constructor to pack a tarball
```

未运行/未验证：真实模型质量、生产部署、完整旧版本黄金启动、多来源 canonical 重建、跨文档并发 forget/merge、60 个语义判断和 20 查询的 case-level 质量报告。Litmus/Obsidian 实际客户端与 release image/SBOM 流程未运行：此次没有更改 WebDAV 接口，也没有构建或获准部署发布镜像；不能据此声明发布就绪。已有向量复用量、新增向量数、全库后台成本和查询延迟未量测。

本地旧格式测试文件 SHA-256：`c503ba4e8d14cb29c199b55856bc7a6f14cfabdcc177bbd8232f237b6c4c8a89`（`crates/memory/tests/memory_v2_1.rs`，不是独立旧数据库快照）。迁移 0020 SHA-256：`d2c332b810e30adae8f008cc2f327a0e8df40b73e8667c0e45c7f7f37712b23e`。判断规则 `memory-equivalence-v1`；规范仍为 v2.1，没有发布新 formal 格式。

默认功能工作区实际通过测试总数：334；没有把默认功能结果折算为 all-features 成功。

### 2026-09-06 继续实施

已新增迁移 0021、正式事实 Markdown v2.2、贡献支持映射和唯一公开视图；多文件操作日志支持恢复及删除所有已知支持并暂停来源。已实现 bounded 候选队列、既有向量复用、句内精简及同篇包含处理。正式对象接管/合并/过滤/旧 ID 不转发/多来源删除/Markdown 重建回归通过。当前不勾选完整里程碑：真正启动升级 fixture、负例和故障矩阵仍在补充，旧“未实现”条目记录的是前次中断时状态。


## 19. 2026-09-06 实际交付与验收

### 基线、格式和实现映射

实际起始/最终 HEAD 均为 `c82a633ef7244c8983d1756246d3e0baf9678f94`。未 checkout/reset、提交、push 或部署；保留原计划和用户现有修改（包括执行期间出现的 `lefthook.yml`）。用户启动指令中的短文件名未存在，实际维护本文件，没有另建替代计划。

| 里程碑 | 实际实现与证据 |
|---|---|
| M0 | 已读项目规范和 PLANS；新增 ADR-0030 明确扩展 ADR-0026 的单来源正式对象限制。`state/tests/support/pre_dedup_snapshot.rs` 生成仅到 migration 0019 的磁盘快照；server 黄金测试先断言旧版 9 条当前对象。 |
| M1 | migrations 0020/0021；`state/src/current_memory/formal.rs`、`memory/src/current_markdown.rs`。旧贡献集合继续 v2.1，新正式事实为保留目录 facts 下 v2.2 Markdown；公开读统一 `memory_public_items`。分页接管无生成请求，覆盖后原子切换。旧 YAML 空 tags/entities 的 null 写法也兼容。 |
| M2 | `memory/src/service/formal.rs`：同篇等价/包含去冗余；句内精简提案再做完整等价校验，保留数字和代码。Loss 公式与跨来源短证据不被错误扩展；变更正文才失效其向量。 |
| M3 | `memory/src/dedup.rs` 与 service：六种严格关系、精确输入/规则/模型依赖缓存、既有授权提取模型真实调用，源上下文经 VaultCore 限量读取。向量/词项/同篇候选 union；每个成员对完整代表判断，不做传递聚类。 |
| M4 | 持久多文件操作 before/after、预期修订和来源 hash 校验、即时来源资格过滤、删除所有已知贡献并暂停、崩溃恢复、Markdown 重建。被吸收正式 ID 保留不可复用的纯 ID 预留，不保存模型可读历史。 |
| M5 | `server::admit_memory_maintenance`、真实 `memory.deduplicate` handler、周期 ready-Vault 调度、发布后增量 admission。持久候选/扫描游标、限额、重试时间；Provider 恢复和预算下窗口自动继续。旧 job handler 保留兼容。 |
| M6 | state 统一正式列表/get/FTS/recall/count；MCP、资源和 Admin 共用服务。来源精简最多 8 条并提供真实 source_count，详情完整；Vault 绑定 ID 游标避免删除导致 offset 漏项。Admin 只读维护状态，不引入批准按钮。正式模式下隐藏贡献不进入 FTS 语料，冷重建也清理旧贡献 FTS。 |
| M7 | 以下命令、黄金 fixture、60 对标注机制用例/20 查询、原始 HEAD Litmus 对照、相关规范/ADR/运维更新；真实模型质量与生产验收独立 pending。 |

### 黄金自动升级与恢复证据

入口为 `server/src/memory_dedup_startup_tests.rs` 的 `upgrade_existing_current_source_sets_auto_dedups_without_user_actions` 和 `upgraded_worker_resumes_after_provider_recovers_without_user_actions`。测试使用合成临时 Vault、真实磁盘 SQLite、真实 ProviderService HTTP 传输接本地 fake、生产共享 admission、WorkerSupervisor 和周期后台循环。冻结旧快照前已有提取结果、绑定和有效向量；冻结后仅正常 connect/migrate/start，没有 UI/API 调用、重新绑定、重新提取或重建全库向量。fixture 的无变化 `vault.reconcile` 是空 handler；记忆维护与 embedding handler 为生产实现，因此这是启动业务链集成测试，不冒充生产二进制部署。

| 输入 | 自动处理结果 |
|---|---|
| 同篇重复 StandardScaler，加另一篇英文完整等价描述 | 一个正式事实、两个可核验来源 |
| Loss 详解含 `w = w - lr * grad`，同篇短总结 | 保留完整详解和公式，删除冗余总结 |
| `Use batch size 32.` 在同一正文重复两次 | 精简成一次，32 保留 |
| KV Cache 用途 / 非长期记忆 | 两条均保留 |
| 用户显式记忆，confidence=0.7 | 内容和元数据不变，不参与自动合并 |
| 已暂停来源 | 暂停标记不变，当前有效贡献仍可整理 |

总计 9→6；原笔记字节不变。没有新增 memory.extract job/提取请求；黄金测试实际旧向量 8 个、保留复用 4 个、新增 embedding 输入 1 个（其余为被吸收或正文已变更的旧对象），新增输入只能是发生精简的正文；正常/故障恢复运行分别记录 19/18 次判断请求，执行顺序会影响候选请求数。重启实际完成新 admission 的 job 后检查不增加判断调用；清空派生集合/正式投影/FTS 后仅从 Markdown 恢复相同 ID、revision、正文和来源数，无 LLM 请求，FTS 条目数与公开对象数一致。

补充回归覆盖：同源包含不能升级另一来源短证据、非传递链、模型等待期间编辑使旧判断不能发布、合并/forget 的 MetadataCommitted 注入恢复、最后来源立即失效、无模型本地清理、旧 ID 不转发和重新生成不复用、模型 profile 改变自动拆分重验、畸形模型响应保持原数据且缓存 uncertain 不重复收费。预算实际预留覆盖 HTTP 重试；state 测试推进旧窗口后证明再次获额度且判断缓存仍在。

`tests/fixtures/memory-quality/dedup-merge.json` 有 60 对标注（30 对含正反方向）、20 查询；120 来源验证超过 100 的分页接管。机制报告 `target/automatic-memory-dedup-validation/labeled-mechanism.json`：20/20 等价关系合并、40/40 受保护非等价关系保持分开、20/20 查询目标召回且正式 ID 不重复。此 fake 按标注响应，仅证明发布/保留机制，不是 99% precision / 90% coverage 的真实模型达标证据；真实语言判定、私有语料 Recall@K、生产延迟仍 pending。

### 成本、失败与验收环境

每 Vault 持久滚动 24 小时预算为 256 次实际请求及 4 MiB 输入，重试计费；单 slice 最多 16 次请求、8 个候选对、300 秒，共享并发上限 2。贡献页 128、变化 seed 每 slice 16、词项/向量各 Top 32，超过 64 贡献不全量两两判断；扫描游标持久保存，后续周期重新检查有变更的贡献。有效旧向量先复用，向量候选两次流式扫描、内存有界；此实现没有声称大库已穷尽所有等价关系，也没有生产级负载测量。正常 recall 无生成式判断。

发现并解决：旧显式 Markdown 空列表被 YAML 解析成 null 导致冷重建隔离；测试 worker 缺少实际排入的新 handler 导致未完成/反复 claim；跨阶段读资格曾使用过宽 pending-operation 条件；旧正式 ID 可能被再提取复用；隐藏原始贡献可能污染 FTS 排名；新持久游标误把既有空字符串解析成 UUID（全量回归捕获，已修正空值读写约定）；上述均已修复并增加断言。

主机 glibc 2.35/GCC 11 无法链接 all-features ONNX 的 `__isoc23_strtol` 等符号（原失败退出 101）。使用隔离 `rust:1.94-trixie` 镜像完成相同 all-features 检查，未关闭 feature。镜像 digest `sha256:652612f07bfbbdfa3af34761c1e435094c00dde4a98036132fca28c7bb2b165c`，工作目录挂载，registry 只读，独立 `target/dedup-trixie`。

官方 MCP 原脚本的 npx Git 打包报 `GitFetcher requires an Arborist constructor to pack a tarball`；脚本现按固定 commit `74edef34d674f563537be8c6587cebaa58e830ca` 拉取并按锁文件安装，默认入口已通过。先构建 fixture 后启动计时，修复冷构建超过 30 秒造成假超时；原有 prompts caching-hints expected failure 未修改。补跑旧版：2025-11-25 的三项场景通过；2025-06-18 仅 tools/resources 适用。固定官方套件不接受 2024-11-05，2025-03-26 的这些 server 场景全被跳过；脚本现对无适用场景返回 2，并拒绝把 upstream SKIPPED 打印成通过。这两个版本的官方验证属于工具覆盖限制，未宣称通过，也没有用 --force 冒充旧规范认证。

Litmus 原始 HEAD 对照使用 `git archive HEAD` 到 `/tmp/mcp-vault-head-litmus`，没有改工作树。当前与原始 HEAD 的 props 均 10/14、4 失败（PROPPATCH 501 及依赖断言）；locks 均 36/41、5 失败（3 个 PROPPATCH 501、2 个 complex_cond_put 连接关闭）。这部分未通过，不能记作本功能全绿；没有修改无关 WebDAV 生产实现或删断言。

### 发布、限制与回滚

更新了 ADR-0030、memory-system、architecture、data-model、interfaces、security、deployment-and-operations。升级自动添加 schema 并后台接管，无需人工迁移或日常重新绑定；状态可观察 adopted、候选数、错误代码和 retry_at。不确定/模型不可用时保留数据并自动续跑，本地来源删除/恢复不依赖模型。

没有操作真实 Vault、使用生产凭据、付费调用、push 或部署。真实模型质量门槛和真实部署验收 pending，不能用 fake 结果替代。程序降级不能只还原 SQLite 或单个规范文件：旧版本不认识正式事实格式，须在获授权维护环境恢复配套数据库及 Vault/运行状态备份。单次中断由操作日志自动恢复；模型/规则改变可由当前贡献拆分重新判断，无需重提取全库。计划留在 active，保留协议既有失败和 M7-B 的可见跟踪。


### 最终命令清单与保留输出

日志统一保留于 `target/automatic-memory-dedup-validation/`；本节覆盖第 18 节的旧中断结果，失败原日志仍保留。标注 fixture SHA-256：`2e53b330f42dcfcc53693883060cceb81da32830661e616613ed795f5c84cb42`；`mechanism-summary.json` 记录 HEAD、规则版本和真实/假模型边界。

| 实际命令 | 最终退出码/结果 | 日志 |
|---|---|---|
| `pnpm --dir frontend/admin install --frozen-lockfile` | 0 | formal-ui-install-final.log |
| `pnpm --dir frontend/admin lint` | 0 | formal-ui-lint.log |
| `pnpm --dir frontend/admin test` | 0，35 tests | formal-ui-test.log |
| `pnpm --dir frontend/admin build` | 0 | formal-ui-build.log |
| `cargo fmt --all --check` | 0 | formal-final-fmt.log |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | 0 | formal-final-clippy.log |
| `cargo test --locked --offline --workspace --all-features`（以下 Docker 环境） | 0，348 tests | formal-final-locked-suite.log |
| `cargo test --locked --offline --manifest-path crates/{memory,state,indexer,providers,mcp,admin-api,server}/Cargo.toml --all-features`（逐个执行，以下 Docker 环境） | 全部 0；memory 46、state 53、indexer 14、providers 25、mcp 26、admin-api 25、server 47 tests | formal-final-locked-suite.log |
| `cargo test --locked -p mcp-vault-server memory_dedup_startup_tests -- --nocapture` | 0，两项黄金升级，30.78 秒 | formal-golden-evidence.log |
| `bash scripts/check-docs.sh` | 0 | formal-final-docs.log |
| `bash scripts/release/check-migrations.sh` | 0，12 项 migration gates | formal-final-migrations.log |
| `bash scripts/interop/http-smoke.sh` | 0，真实 HTTP/MCP/OAuth/DAV/控制面边界 | formal-http-smoke.log |
| `bash scripts/conformance/mcp.sh`（设持久输出目录，默认固定源码入口） | 0，2026-07-28 的六项官方场景，保留既有 caching baseline | formal-conformance-final.log |
| 同脚本，`MCP_VAULT_CONFORMANCE_PACKAGE=/tmp/mcp-vault-official-conformance` | 0，修复 skipped 误报后重跑当前版 | formal-conformance-accepted.log |
| 同脚本，`MCP_VAULT_CONFORMANCE_SPEC_VERSION=2025-11-25` | 0，三项适用官方场景 | formal-conformance-2025-11-25.log |
| 同脚本，`MCP_VAULT_CONFORMANCE_SPEC_VERSION=2025-06-18` | 0，两项适用官方场景 | formal-conformance-2025-06-18-accepted.log |
| 同脚本，`2025-03-26` / `2024-11-05` | 2，官方套件没有适用默认 server 场景/不接受版本；明确 blocked | formal-conformance-2025-03-26-blocked.log / formal-conformance-2024-blocked.log |
| `python3 /tmp/run-vault-litmus.py 'basic copymove props'` | 1；basic 16/16、copymove 13/13，props 10/14 | formal-litmus.log |
| `python3 /tmp/run-vault-litmus.py 'locks http'` | 1；locks 36/41，后续 http 因严格失败退出未运行 | formal-litmus-locks-http.log |
| `python3 /tmp/run-vault-litmus.py http` | 0，http 4/4 | formal-litmus-http.log |
| 原始 HEAD 临时 archive 上分别运行相同 props / locks | 均 1，与当前完全相同的失败 | formal-litmus-head-props.log / formal-litmus-head-locks.log |
| `git diff --check`、`bash -n scripts/conformance/mcp.sh` | 0 | 最终工作树检查 |

Docker 最终实际执行形式（不关闭原生 feature）：

```bash
docker run --rm --user 1000:1000 \
  -v /home/cheng/code/mcp-vault:/workspace \
  -v /home/cheng/.cargo/registry:/usr/local/cargo/registry:ro \
  -w /workspace -e RUSTUP_TOOLCHAIN=stable \
  -e CARGO_TARGET_DIR=/workspace/target/dedup-trixie \
  rust:1.94-trixie sh -c '
    cargo test --locked --offline --workspace --all-features &&
    for crate in memory state indexer providers mcp admin-api server; do
      cargo test --locked --offline --manifest-path crates/$crate/Cargo.toml --all-features || exit
    done'
```

Litmus 临时 harness 从生产 fixture 获取测试 URL/凭据，再调用仓库 `scripts/interop/webdav-litmus.sh`；只使用合成实例。原始 HEAD 对照增加 `MCP_VAULT_TEST_REPO=/tmp/mcp-vault-head-litmus CARGO_TARGET_DIR=/home/cheng/code/mcp-vault/target`。harness 副本保存在验收目录，不能对生产 URL 不加区分运行。


## 20. 2026-09-06 运行反馈：反复回队、优先级与隐藏错误

用户反馈：索引完成后只剩 memory.deduplicate，queued/0 attempts 的更新时间持续变化；记忆页显示等待接管、候选 0/0、模糊的“等待后台重试或下一轮检查”。这不能证明语义整理在前进。代码确认：Deferred 退回队列会恢复 attempt；此前 handler 丢弃具体错误代码、不写 progress，UI 又隐藏未识别的维护状态。尚未读取用户运行实例的实际错误，不能把提高优先级宣称为已解决该实例的根因。

用户明确要求提高优先级、不为其他任务让出队列。实现：memory.deduplicate 从 -1 升为 100；每次 admission 通过 Vault-scoped repository 自动提升旧活跃任务，不改变 ID、租约、检查点或 retry deadline。正常候选批次及单批额度/时限到达后在同一 worker 租约内连续执行，保留 Tokio 取消响应；只有实际错误需要回队重试，持久日预算/模型不可用仍按已有机制等待。

每批写入无正文的 progress：slices_completed、checked_pairs、pending_pairs、adopted、maintenance_status、wait_reason、resume_at。短暂错误保留原 error code，任务页显示实际批次数/候选数量/原因/下次时间；记忆页未知状态显示真实代码，不再用统一等待文案覆盖。没有新增生产操作或用户手工迁移步骤。

验证：新增 state 回归证明旧任务提升后先于普通索引领取、不同 Vault 不变、重试时间和检查点保留；强化黄金升级检查多批次与持久进度；新增前端回归验证 queued/0 attempts 仍可显示进度及未知维护错误。定向 state 后台测试 12 项、黄金启动 2 项、前端测试 36 项、前端 lint/build、Clippy、fmt、check-docs 均通过。新增前端测试初次 build 发现多传 notify prop（TS2322），已修正并重跑通过；原失败日志与通过日志保留在 `target/automatic-memory-dedup-validation/dedup-priority-*.log`。全工作区 all-features 结果在本节末补录。

只读查看本机 docker ps，仅发现本轮临时 Rust 验收容器，没有用户正在使用的服务；未访问生产凭据、修改生产 Vault 或部署。本次修复确认了调度与可观察性缺陷，不声称已定位截图对应实例的具体接管错误。

本次全量首次回归暴露旧测试 `full_vault_extraction_distinguishes_source_and_generated_output_failures` 依赖低优先级调度顺序，将现在先领取的 dedup job 错交给 extraction handler，得到 memory_extract_path_invalid。测试改为明确按目标 ID 选择提取任务，并新增断言 dedup 确实先被领取；原有提取成功/失败/来源断言全部保留。定向重跑通过，首次失败日志保留为 dedup-priority-workspace.log。

本次最终全工作区 `cargo test --locked --offline --workspace --all-features`（第 19 节同一隔离 Docker 环境）退出 0，349 项通过；Clippy/fmt/check-docs/git diff --check 退出 0，前端 lint/test/build 退出 0（36 项测试）。日志已保存为 `target/automatic-memory-dedup-validation/dedup-priority-workspace-final.log` 等。修改停留在本地工作树，未部署到用户实例。


## 21. 2026-09-06 真实运行反馈：前置条目检查长期挤占配对处理

用户先观察多轮 0/0，后出现 758 个待比较组合，证明运行实例并非完全死锁：前置阶段最终结束，但配对长时间得不到运行。不能把优先级修改或测试总数当作这条路径已可靠的证据。复查确认两个缺陷：句内条目检查放在候选排队之前；“每批 4 条”只计算成功改写，未变化条目不计入上限。大量无需改写的记忆可反复耗尽 16 请求/300 秒整轮限制，候选仍为 0。旧外层时限和请求数共用同一错误码，使超时被当作普通批次续跑，误导了诊断。

修复：先持久化候选并检查最多 2 个组合，再检查最多 2 个正式记忆条目（是否改写都受限）。新增前向 migration 0022，持久保存条目游标和当前阶段；外部调用前推进游标，取消后下次从后续条目继续，未完成条目在下一轮扫描仍可重试。累计条目检查计数仅在检查返回成功后增加，包含无需改动/缓存检查，不冒充独立记忆条数或成功改写数。

整轮超时独立为 memory_equivalence_slice_timeout，保留真实阶段并退避；请求数上限仍用原码继续。worker 每 2 秒发布当前阶段、候选组合数和累计条目检查次数。UI 移除“已运行 N 批”，明确“待比较组合”可能超过记忆条数。这里的正文去冗余仅影响系统管理的记忆条目及其规范文件，原笔记不变。

复现：新增 `unchanged_memory_bodies_cannot_starve_pairs_and_interrupted_checks_resume_after_cursor`，20 条合成记忆均无需改写。修复后第一轮 checked_pairs>0、pending_pairs>0、sentence_checked=2；随后故意阻塞条目调用并取消，重新创建服务后游标前进、检查数量增长，逐篇原笔记字节不变。

失败对照：临时恢复旧执行顺序及无界不改写扫描（不是声称切换到了某个已提交旧版本），同一测试以 memory_equivalence_slice_exhausted 失败，候选阶段没有机会运行。修复代码通过 finally 恢复；日志 dedup-stall-negative-control.log。正常修复回归通过，日志 dedup-stall-regression.log。新增 0021→0022 迁移/跨 Vault 检查点隔离回归，保留原候选 cursor、错误/重试时间。

之前遗漏的是大批无需改写输入、慢响应与取消组合下的前进性验证；fake 黄金样本只能证明它实际覆盖的机制，不能据此声称生产语料完整可靠。本节最终验收结果随后补录；没有访问生产凭据或替用户部署。

本轮首次全量还发现新进度采样的并发缺陷：在 select 的 timer 分支内部 await 数据库，维护 future 会停止轮询；单连接内存库可能由维护占用事务、采样等待连接而互相阻塞。旧生产 worker 清空测试在原 5 秒期限内失败。修正为将整个采样 future 与维护 future 并发 select，未放宽原断言；定向回归结果与全量重跑日志分别为 dedup-stall-reporting-fix.log、dedup-stall-workspace-final.log。首次失败日志 dedup-stall-workspace.log 保留。

用户要求暂停并提交，后续在公司继续。最终全工作区 all-features 通过 351 项测试；前端 36 项及 lint/build、Clippy/fmt、13 项迁移 gates、文档检查通过。真实模型质量及用户实例实际错误/部署仍待验证；未进行生产操作。保留用户 lefthook.yml，不纳入本次提交。


## 22. 暂停交接：明天继续

暂停原因：用户要求先提交代码休息，后续在公司继续。代码、迁移、测试和本进度文档一起提交；未 push、未替用户部署。原起始 HEAD 为 c82a633ef7244c8983d1756246d3e0baf9678f94，无需切回该提交。工作树剩余 lefthook.yml 未纳入本次功能提交。

已完成：M0–M6 实现、旧库自动接管/同篇去冗余/跨文档等价合并、优先级 100 和保持 worker 连续处理；修复正文无改动不计入上限导致配对迟迟不能运行，加入 0022 条目扫描检查点；区分请求上限和整轮超时，实时显示实际阶段及比较组合数，移除误导性的批次数；原笔记字节不变。旧行为负对照确实失败，新前进性/取消恢复回归通过。进度采样的单连接数据库阻塞也已在交付前修复，原 5 秒测试 0.12 秒通过。

最终验证：全工作区 locked/offline/all-features 在前述隔离 Docker 环境退出 0，共 351 项；前端 36 项及 lint/build、Clippy、fmt、文档、13 项 migration gates 全部通过。详细命令沿用第 19–21 节；本机最终日志为 target/automatic-memory-dedup-validation/dedup-stall-workspace-final.log 等，target 中日志未提交，仓库中的 fixture、测试和本摘要足够重跑。没有把真实模型质量或生产实例验证标成通过。

继续时按以下顺序：

1. 以当前提交检查工作树，保留本地修改；不要重复实现已完成模块。
2. 核对用户实例实际运行的版本及阶段。用户最后截图由长期 0/0 变为已检查 0、待比较 758；这证明旧流程最终到达候选阶段，不是完全死锁。758 是组合数，不是记忆条数或合并次数。
3. 本次新修复尚未由本 Agent 部署。下一步需在用户授权的运行环境验证新版本的阶段、已检查组合和条目检查计数持续前进，重启/超时后从检查点继续。不能因本地假模型测试通过就宣称用户实例已修复，也不要求重新生成/重新绑定或手工迁移。
4. M7-B 真实模型质量和生产验收仍 pending；未经授权不访问生产凭据、付费调用或修改真实 Vault。M7-A 的既有 Litmus props/locks 失败及官方套件不覆盖部分旧协议的限制仍按第 19 节保留，不冒充全绿。
5. 只有出现新的失败证据才继续扩大修改；正常 recall/list/get 与来源资格、删除不复活、原笔记不变仍为回归重点。
