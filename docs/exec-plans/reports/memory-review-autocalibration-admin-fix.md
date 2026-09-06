# Memory review / automatic calibration / Admin 验收报告

后续综合回归及获授权的真实模型结果见[2026-09-06 回归报告](memory-system-regression-20260906.md)；以下保留初次交付时的证据和限制。

- 日期：2026-09-06；执行依据：[ExecPlan](../active/memory-review-autocalibration-admin-fix.md)。
- 起始及当前 HEAD：`92d9367949570ccd3c19ece8895cdd8f00cc5e78`。成果是本地未提交工作树，不把 HEAD 当作包含本轮代码的新提交。
- 起始未提交项只有 ExecPlan 及其 `:Zone.Identifier`；二者保留。未 reset、覆盖用户代码、push 或部署。
- 工程实现已交付；部署模型语义／多语言／生成效果为 **deployment quality evaluation pending**。所有模型测试使用临时 localhost 合成服务；没有使用生产凭据、收费模型或真实 Vault。
- 规范：ADR-0026 的当前来源集合与真实删除保持不变；新增 [ADR-0027](../../adr/0027-bounded-automatic-retrieval-calibration.md)。

## A01：已有模型和有效向量、唯独缺校准

`crates/server/src/calibration_startup_tests.rs` 的
`a01_existing_binding_and_vectors_calibrate_on_start_without_admin_or_regeneration`
在磁盘 SQLite 中建立既有角色绑定、规范显式记忆及有效向量，关闭／重新连接数据库后，直接启动生产使用的
`run_reconciliation_loop` 和 `WorkerSupervisor`。没有调用 Admin、重新绑定、生成或手工校准 API。

启动首个 reconciliation tick 发现缺失记录，提交 Vault 单例任务，经真实 ProviderService HTTP adapter、持久请求计数、
内置基准计算、holdout 门槛、当前签名检查和原子发布后，纯语义 recall 返回预期对象。测试比较规范内容、ID、revision
及有效向量记录，证明没有为触发校准而重新生成业务向量；再次启动不增加校准请求。

这里的“真实路径”指生产 worker、repository、Provider HTTP transport 和评测代码，模型响应是明确标识的本地合成合同数据。
测试不证明任何部署模型已通过质量验收。测试初始 DB 使用当前迁移创建；0015→0016/0017 的既有快照兼容性另由迁移测试验证，
不声称运行过旧版生产二进制或真实升级部署。

实现还修复了两个实际阻碍：已有 embedding job 按 input/profile 查询并复用合法向量；通过报告以单条 SQL 条件发布，
避免 SQLite 延迟事务升级锁冲突造成“评测通过但不能启用”。空队列 claim 不持有写锁，非空队列用有资格条件的原子争抢。

## R1–R6 和安全回归

| 项目 | 最终行为与主要位置 | 可复核测试 |
|---|---|---|
| R1 | `state/current_memory.rs::get_filtered` 共用请求谓词；`memory/service.rs` 语义候选、最终返回前检查类型、时间、重要性和当前来源资格 | `semantic_recall_respects_type_validity_and_importance`；`recall_revalidates_current_objects_after_related_note_provider_wait` |
| R2 | `indexer::retrieve_notes_for_recall` 与记忆都执行强词面或校准余弦准入；普通 search 的候选语义保留 | `positive_cosine_hard_negative_is_rejected_while_lexical_answer_survives`：两通道实际启用，拒绝无关正余弦及偶然 OR 命中，仍返回真正相关的 note-only 答案 |
| R3 | 删除已恢复的来源集合不要求原 Provider/model；保留未删项元数据和有效向量，删除末项留下可管理空暂停集合 | `derived_forget_works_after_rebuild_without_original_provider`，含零生成调用与恢复发布 |
| R4 | 准入与排序分开，不再用统一 0.18 截断合格 context-only 对象 | `context_only_relevant_candidate_survives_admission`，覆盖 FTS 前 50 之外及无 query evidence 的负例 |
| R5 | 有效且通过过滤的唯一 memory 对象才占语义名次 | `semantic_memory_rank_is_invariant_to_duplicate_chunks`：1→32 块不改变另一对象贡献 |
| R6 | 按完整序列化响应 UTF-8 字节数除 4 向上取整计预算；包括来源、URI、长路径、完整标题、诊断和 JSON 外壳；超大项跳过后继续选短项 | `recall_budget_counts_actual_heading_text_and_metadata`，60 个长标题及 note no-answer |

最终输出前再次核对对象 revision／来源，等待 related-note Provider 期间删除的记忆不会进入响应。
`mark_recalled` 只作用于最终保留的对象。旧 ID、历史参数、资源、向量不能恢复已删除或失效的当前记忆；既有 MCP/current-set 隔离测试继续通过。

## 自动校准机制与证据范围

| 验收 | 实现／证据 |
|---|---|
| A02、A08、A12 | 同签名任务去重、批次缓存、跨数据库重开不返还已用预算；保存通过报告但丢失发布记录后，可无额外请求恢复发布。状态测试 `budget_checkpoint_stop_retry_and_vault_isolation` 和服务测试 `calibration_controls_cancel_retry_pause_and_signature_changes_are_real` |
| A03、A04、A06、A16 | 启动立即补偿，之后 300 秒；绑定、Provider mode/edit、初始化完成、embedding job 完成进入同一 ensure；64 个 Vault 分页；角色独立，不需要生成／reranker。浏览器起始 100% 向量但无校准，正确显示未启用 |
| A05、A07、A13、A15 | 完整 Vault/channel/model/input/policy/corpus 签名；发布前重检；disabled/maintenance/cancel 限制实际 dispatch；旧 PUT 保留兼容数据但不能启用；排名／预算更改不改变业务 embedding 输入身份 |
| A09、A10、A17 | 独立合成候选后端共享生产输入准备、FTS5 形状、精确余弦、相关性准入和对象贡献函数；无需已激活语义即可评测；MCP 没有 bypass 参数。无答案全空但有答案／纯语义失败不能通过 |
| A11 | memory/note 单独校准并报告；两者可用时再计算 holdout 无答案失败 ID 的并集。单通道各 1/20、联合 2/20 时阻止语义启用并保留报告，不自动无限重试。`individually_passing_channels_cannot_bypass_joint_no_answer_gate` |
| A14、S1、S7 | 基准、向量缓存和报告不进入业务 Markdown／记忆列表／业务向量／recall_count；repository 均按 Vault/channel/signature 限定。校准不迁移旧知识、不解除暂停、不提取笔记 |

内置 `calibration.json`：40 个非私人合成来源、120 个查询；校准／holdout 来源分离，每份 40 个有答案和 20 个无答案查询，
包含 20 个纯语义跨语言有答案查询。阈值仅从校准 split 选择，冻结后计算 holdout。
Recall@5 计算相关对象比例；返回精确率的有答案空结果计零；另报 MRR、纯语义、跨语言、无答案绝对计数和失败 ID。
门槛为 Recall≥0.70、Precision≥0.80、无答案误返回≤0.05、纯语义 Recall≥0.70，holdout 不低于词面控制组。

报告包含实际 profile、数据集哈希、构建 commit／dirty 状态、输入数、HTTP 尝试数／字节和耗时。
默认每通道每轮最多 32 次请求、2 MiB 请求字节、512 个输入、每批 16、900 秒；内部 HTTP 重试也先记账再发送。
32 MiB 检查点容纳真实高维输入，160×3072 向量的 DB 往返已有回归。每通道保留当前及最多 8 个其他终态签名。
允许手动申请新的有界重试额度，自动重试／重启不重置累计消费。维护开关与查询有效性独立。

基准使用独立、无私人数据的 candidate backend；不是对真实 Vault 调用完整公开 recall。
请求过滤、当前来源资格、删除竞态与完整响应预算由真实业务路径的 R/S 回归验证，不把合成文档全部有资格当作隔离证明。

## 五项 Admin 流程及分页

真实 Chromium 脚本：`scripts/e2e/memory-admin.py`；真实临时 Admin HTTP／SQLite／Vault Core，只有模型端点为本地合成服务。
结果在 `target/memory-review/browser-e2e/result.json`，截图在同目录 `memory-admin.png`，请求未使用前端网络 mock。

| 流程 | 页面 → 实际端点 → 可观察结果 |
|---|---|
| U1 | 迁移预检 → POST `/memory/migration/preflight` → 显示服务端报告、指纹和确认串 |
| U2 | 确认迁移 → POST `/memory/migration/execute` → `migration.completed` 实际结果；HTTP 安全测试和前端测试另外证明并发 409 必须重新预检 |
| U3 | GET `/memory/semantic-calibration`；运行／重试 → POST `/run`；暂停／恢复维护 → PUT `/maintenance` → 后台作业真实计算并显示按通道结果；打开页面本身不发起校准 |
| U4 | 删除末条后刷新 → GET `/memory/extraction/sources?paused=true` 仍看到空集合；POST `/{id}/resume` 携带 set_revision → 返回已接受与 job_id，后台恢复并提取 |
| U5 | 显式记忆编辑 → PATCH `/memories/{id}` 携带 expected_revision 和实际改变字段 → 内容／元数据更新；未触碰字段保留，空字段明确清空；派生项不可原地编辑 |
| U6 | 浏览器加载 53 条记忆；35 项前端测试还覆盖 51 个暂停来源、轮询保留尾页、局部端点错误和切 Vault 后迟到响应隔离；计数明确是已加载数量 |

所有端点都在 control plane，沿用 Admin session、CSRF、Origin 和 Vault 解析，不在 handler 中直接读写文件、SQL 或调用 Provider。
恢复来源先保存 `memory.source_resume` 意图，再进行 canonical revision 写入；暂停状态改变与 extraction job 插入同一事务。
`source_resume_replays_canonical_commit_and_queues_extraction_exactly_once` 注入规范提交后失败，证明重放采用已提交字节、不覆盖新暂停，且只排队一次。

## 生成语言、覆盖与章节诊断

`memory-current-set-v2-language-coverage` 保留全文主语言、技术标识，以及 frontmatter／正文中的完成状态、环境、时间、
条件实验和未来计划；不把引用、否定或未采纳建议当成用户已完成。运行时加载移到 unchanged-source/recovery 检查之后，
提示词升级不强制重提取或丢弃已准备快照。

新增 `generation-coverage.json` 的 15 个双语来源包含 required facts／forbidden claims，首个长中文来源覆盖 frontmatter、
大量教程、正文中段进度和尾部下一阶段。离线测试证明完整原文和新约束进入实际请求、未改变来源不再次提取；
既有空集合／截断／非法可选字段／来源变化测试通过。**这些不是实际生成质量评分，G1/G2 的真实输出质量仍 pending。**

章节结果保留当前 revision、真正获胜 chunk 的 heading_path、projection 字节范围、snippet 和 URI；词面／语义合并时
不把其他章节冒充命中章节。MCP 标明 section 与 note fallback，偏移属于纯文本投影而非 Markdown 原始字节。
诊断给出各阶段有界候选数、过滤后对象数、返回数、语义对象名次和实际贡献；不把候选窗口当全库总量。

## 实际检查与产物

日志均保存在本地 `target/memory-review/`（不提交含运行日志的 target）；以下路径是证据位置，不是生产结果。

| 命令／检查 | 结果 | 日志／产物 |
|---|---|---|
| `cargo fmt --all --check` | 退出 0 | `fmt.log` |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | 退出 0 | `clippy-verified.log` |
| `cargo test --locked --workspace`，允许临时 localhost 监听 | 退出 0；314 tests，0 failed，0 ignored | `workspace-verified.log` |
| `cargo test --locked --workspace --all-features` | 退出 101；宿主 ORT 链接不兼容，未宣称通过 | `all-features-verified.log` |
| `pnpm --dir frontend/admin lint` / `test` / `build` | 均退出 0；35 tests | `frontend-lint.log`、`frontend-test.log`、`frontend-build.log` |
| `bash scripts/release/check-migrations.sh` | 退出 0；旧 fixture／0015 prepared snapshot／校准预算隔离 | `migrations-verified.log` |
| `bash scripts/check-docs.sh` | 退出 0 | `docs-verified.log` |
| `PLAYWRIGHT_BROWSERS_PATH=.../target/memory-review/playwright LD_LIBRARY_PATH=/home/cheng/anaconda3/lib python scripts/e2e/memory-admin.py` | 退出 0；真实 Chromium 五流程与记忆分页 | `browser-verified.log`、`browser-e2e/result.json` |
| `MCP_VAULT_GIT_HEAD=92d936... cargo run --locked -p mcp-vault-memory --example quality_eval -- --mode deterministic --fixtures tests/fixtures/memory-quality --output target/quality/after.json` | 退出 0；同一校准引擎双通道合同评测，`semantic_quality_proven=false` | `quality-verified.log`、`target/quality/before.json`、`after.json` |
| 官方 MCP 固定 SHA `74edef34d674f563537be8c6587cebaa58e830ca`，`MCP_VAULT_CONFORMANCE_PACKAGE=file:$PWD/target/memory-review/conformance-src/conformance-74edef34d674f563537be8c6587cebaa58e830ca MCP_VAULT_CONFORMANCE_SPEC_VERSION=<version> MCP_VAULT_CONFORMANCE_OUTPUT_DIR=target/memory-review/mcp-<version> bash scripts/conformance/mcp.sh` | 2026-07-28、2025-11-25、2025-06-18、2025-03-26 通过既有 baseline；没有新增豁免 | 对应 `mcp-<version>.log` 及目录 |

前后报告中的旧确定性 generation/retrieval 数值只是固定合同结果；新版增加了调用同一生产校准引擎的明确标识的合成合同报告。
不能据此给出“真实模型提高 X%”或宣称部署语义可用。实际通过与否仍由部署中的 Provider 对内置基准计算决定。

已保存的失败证据和限制：

- `joint-gate.log`、`workspace-final.log`：沙箱禁止 localhost bind，`Operation not permitted`；允许本地测试监听后重跑，不修改产品权限。
- `joint-gate-local.log`、`workspace-final-local.log`：新增联合门槛遗漏自动 ensure 退避，已修正并由 `joint-gate-verified.log`／完整 workspace 证明。
- `all-features-verified.log`：`rust-lld: undefined symbol: __isoc23_strtol`、`__isoc23_strtoll`、`__isoc23_strtoull` 和 `basic_string::_M_replace_cold`。当前 glibc/libstdc++ 无法链接可选 ONNX 二进制；未删除 feature 或降级依赖掩盖失败。
- `mcp-conformance.log`：默认 npm GitFetcher 缺 Arborist；下载同一固定 upstream SHA 并本地构建后运行。`mcp-2024-11-05.log`：官方工具 `Unknown spec version: 2024-11-05`；该版本官方 conformance 仍未验收，不能当作跳过后成功。
- `webdav-litmus.log`：`WebDAV Litmus is not installed; interoperability gate is blocked`。workspace 内 WebDAV HTTP／并发／预条件回归通过；真实 Obsidian 客户端和 Litmus 外部互操作验收未运行。
- 真实 embedding、跨语言效果和 15 个来源的真实生成输出评测均 pending；没有单独获准的非生产模型条件，不使用生产凭据代替。

## 升级、回滚及剩余验收

镜像构建补修（2026-09-06）：用户构建发现 Docker builder 缺少生产 `include_str!` 所需的
`tests/fixtures/memory-quality/calibration.json`。已在 Dockerfile 增加该文件的明确 COPY，
不改变基准内容／签名，也不要求运行时挂载。按 Docker COPY 集合构造的隔离目录中，
`cargo check --offline --locked -p mcp-vault-server` 修复前退出 101、复现同一缺文件错误，
补入该文件后退出 0；日志为 `docker-inputs-before.log` / `docker-inputs-after.log`。
整镜像构建在本机仍未验证：`docker info` 退出 1，报
`permission denied while trying to connect to the docker API at unix:///var/run/docker.sock`。

完整操作步骤见 [自动校准运维契约](../../memory-autocalibration-operations.md#upgrade-and-rollback)。
升级前协调备份 DB、canonical Vault、revision history 和独立保护的 master key；新版本只向前应用 0016/0017。
0016 保留旧快照并允许本地删除时模型身份为空，0017 增加校准运行状态和活动作业唯一约束，不自动迁移旧记忆。
启动自动校准不解除暂停、不全库提取、不重写合法业务向量。需要停止自动网络维护时使用维护开关；已有合法报告保留。

不提供破坏性 down migration。回滚二进制前停服务、另外备份升级后写入，再恢复配套的升级前 DB/Vault/history/key；
不让旧 SQLx 二进制直接使用新 schema。正常恢复重放当前集合快照、来源恢复意图和校准缓存，保持来源 revision 预条件。

工程成果不因缺真实凭据而省略任何产品流程。当前未通过的发布门槛是上述平台 all-features 链接、外部互操作及部署模型效果，
并非“只实现 FakeProvider”的占位项。ExecPlan 留在 active，以免把未执行的发布／真实效果验收标作完成；本地代码与报告可直接审查。
